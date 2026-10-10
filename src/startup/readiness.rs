use std::{
    sync::{
        Arc, OnceLock,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use rootcause::{IntoReport, Report};
use tokio::{sync::Notify, time::Instant};
use tokio_process_tools::{
    LineParsingOptions, StreamReadError, TerminateOnDrop, WaitForLineResult,
};

use super::{OutputStream, collision::PortCollision, logs::StartupLogs, retry::RetryPolicy};
use crate::{CargoLeptosMode, LeptosBrowserTestError};

/// How long to keep waiting for the startup line after a port collision showed up in `watch` mode.
///
/// In `watch` mode, cargo-leptos keeps running after the app failed to bind, so its output never
/// closes. A collision line that is followed by the startup line within this period was harmless,
/// for example an auxiliary listener giving up on a taken port.
const WATCH_MODE_COLLISION_GRACE_PERIOD: Duration = Duration::from_secs(5);

/// How a startup attempt that did not fail ended.
pub(crate) enum StartupOutcome {
    Ready,
    PortCollision(PortCollision),
}

/// What `wait_for_ready` waits for, plus the details a startup failure reports.
pub(crate) struct StartupExpectation<'a> {
    /// The descriptive app name used in logs and errors.
    pub(crate) app_name: &'a str,
    /// The stdout/stderr line fragment that marks the app as ready.
    pub(crate) startup_line: &'a str,
    /// How long to wait for the startup line.
    pub(crate) timeout: Duration,
    /// Why `timeout` was chosen, reported if it elapses.
    pub(crate) timeout_reason: &'a str,
    /// The site address, reported if the app can't bind it.
    pub(crate) site_addr: &'a str,
    /// The reload port, reported if cargo-leptos can't bind it.
    pub(crate) reload_port: u16,
    /// Decides how long a port collision may precede the startup line.
    pub(crate) mode: CargoLeptosMode,
}

impl StartupExpectation<'_> {
    /// How long to keep waiting for the startup line after a port collision showed up. `None`
    /// waits until the output closes or the startup timeout elapses.
    ///
    /// In `serve` mode, cargo-leptos exits when the app fails to bind, so a real collision closes
    /// the output quickly and no grace period is needed.
    fn collision_grace_period(&self) -> Option<Duration> {
        match self.mode {
            CargoLeptosMode::Serve => None,
            CargoLeptosMode::Watch => Some(WATCH_MODE_COLLISION_GRACE_PERIOD),
        }
    }
}

/// Waits until `process` prints the expected startup line on stdout or stderr, or reports a port
/// collision that `retry_policy` allows retrying.
///
/// A collision line does not end the wait on its own. It only counts once the output closed, the
/// startup timeout elapsed, or the collision grace period of `expected` passed without the startup
/// line showing up.
///
/// # Errors
///
/// Returns an error if both output streams close first, the startup timeout elapses, reading the
/// output fails, or a port collision is reported that `retry_policy` does not allow retrying.
/// Each error report carries the output captured in `logs` as a
/// [`CapturedOutput`](super::CapturedOutput) attachment.
pub(crate) async fn wait_for_ready(
    process: &TerminateOnDrop<OutputStream>,
    logs: &StartupLogs,
    expected: &StartupExpectation<'_>,
    retry_policy: RetryPolicy,
) -> Result<StartupOutcome, Report<LeptosBrowserTestError>> {
    tracing::info!(
        "Waiting {:?} ({}) for {} to start...",
        expected.timeout,
        expected.timeout_reason,
        expected.app_name,
    );

    let stdout_watch = Arc::new(LineWatch::default());
    let stderr_watch = Arc::new(LineWatch::default());
    let startup_waiter = wait_for_startup_line(
        process,
        expected.startup_line,
        expected.timeout,
        expected.collision_grace_period(),
        &stdout_watch,
        &stderr_watch,
    );
    process.seal_output_replay();

    let app_name = || expected.app_name.to_owned();
    let expected_line = || expected.startup_line.to_owned();
    let outcome = match startup_waiter.await {
        Ok(StartupWait::Ready) => Ok(StartupOutcome::Ready),
        Ok(StartupWait::PortCollision(collision)) if retry_policy.retries(collision) => {
            Ok(StartupOutcome::PortCollision(collision))
        }
        Ok(StartupWait::PortCollision(PortCollision::App)) => {
            Err(Report::new(LeptosBrowserTestError::PortInUse {
                app_name: app_name(),
                site_addr: expected.site_addr.to_owned(),
            }))
        }
        Ok(StartupWait::PortCollision(PortCollision::Reload)) => {
            Err(Report::new(LeptosBrowserTestError::ReloadPortInUse {
                app_name: app_name(),
                reload_port: expected.reload_port,
            }))
        }
        Ok(StartupWait::StreamClosed) => {
            Err(Report::new(LeptosBrowserTestError::StartupOutputClosed {
                app_name: app_name(),
                expected_line: expected_line(),
            }))
        }
        Ok(StartupWait::Timeout) => Err(Report::new(LeptosBrowserTestError::StartupTimedOut {
            app_name: app_name(),
            expected_line: expected_line(),
            timeout: expected.timeout,
            reason: expected.timeout_reason.to_owned(),
        })),
        Err(err) => {
            let err: Report<StreamReadError> = err.into_report();
            Err(err.context(LeptosBrowserTestError::StreamRead {
                app_name: app_name(),
                expected_line: expected_line(),
            }))
        }
    };
    match outcome {
        Ok(outcome) => Ok(outcome),
        Err(report) => {
            let output = logs
                .snapshot_after(stdout_watch.seen(), stderr_watch.seen())
                .await;
            Err(report.attach(output))
        }
    }
}

/// What the waiter on one output stream observed.
#[derive(Default)]
struct LineWatch {
    /// How many lines the waiter has seen, including the one it stopped on.
    seen: AtomicUsize,
}

impl LineWatch {
    fn seen(&self) -> usize {
        self.seen.load(Ordering::Acquire)
    }
}

/// The first port collision seen on either output stream.
#[derive(Default)]
struct CollisionWatch {
    first: OnceLock<PortCollision>,
    /// Notified once, when `first` is set.
    detected: Notify,
}

impl CollisionWatch {
    fn record(&self, collision: PortCollision) {
        if self.first.set(collision).is_ok() {
            self.detected.notify_one();
        }
    }

    /// The collision, if one was seen, or `otherwise`.
    fn or(&self, otherwise: StartupWait) -> StartupWait {
        self.first.get().map_or(otherwise, |collision| {
            StartupWait::PortCollision(*collision)
        })
    }
}

/// How waiting on stdout and stderr ended.
#[derive(Debug, PartialEq, Eq)]
enum StartupWait {
    Ready,
    PortCollision(PortCollision),
    StreamClosed,
    Timeout,
}

/// Subscribes to stdout and stderr immediately and returns a future racing both for the startup
/// line.
///
/// A port collision seen before the startup line is reported instead of `StreamClosed` or
/// `Timeout`. With a `collision_grace_period`, it is also reported once that period passed after
/// the collision without the startup line showing up.
///
/// Deliberately not an `async fn`: the subscriptions must exist before the caller seals the
/// output replay, but an `async fn` body would only run (and subscribe) on first poll.
fn wait_for_startup_line(
    process: &TerminateOnDrop<OutputStream>,
    startup_line: &str,
    timeout: Duration,
    collision_grace_period: Option<Duration>,
    stdout_watch: &Arc<LineWatch>,
    stderr_watch: &Arc<LineWatch>,
) -> impl Future<Output = Result<StartupWait, StreamReadError>> {
    let collisions = Arc::new(CollisionWatch::default());
    let stdout_waiter = wait_for_line_recording_collisions(
        process.stdout(),
        startup_line,
        timeout,
        stdout_watch,
        Arc::clone(&collisions),
    );
    let stderr_waiter = wait_for_line_recording_collisions(
        process.stderr(),
        startup_line,
        timeout,
        stderr_watch,
        Arc::clone(&collisions),
    );

    async move {
        tokio::pin!(stdout_waiter);
        tokio::pin!(stderr_waiter);

        // Reset to the end of the grace period once a collision shows up.
        let grace = tokio::time::sleep(Duration::ZERO);
        tokio::pin!(grace);
        let mut grace_running = false;

        let mut stdout_result = None;
        let mut stderr_result = None;

        loop {
            tokio::select! {
                result = &mut stdout_waiter, if stdout_result.is_none() => {
                    stdout_result = Some(result?);
                }
                result = &mut stderr_waiter, if stderr_result.is_none() => {
                    stderr_result = Some(result?);
                }
                () = collisions.detected.notified(),
                    if collision_grace_period.is_some() && !grace_running =>
                {
                    if let Some(grace_period) = collision_grace_period {
                        grace.as_mut().reset(Instant::now() + grace_period);
                        grace_running = true;
                    }
                }
                () = &mut grace, if grace_running => {
                    return Ok(collisions.or(StartupWait::Timeout));
                }
            }

            match (stdout_result, stderr_result) {
                (Some(WaitForLineResult::Matched), _) | (_, Some(WaitForLineResult::Matched)) => {
                    return Ok(StartupWait::Ready);
                }
                (Some(WaitForLineResult::StreamClosed), _)
                | (_, Some(WaitForLineResult::StreamClosed)) => {
                    return Ok(collisions.or(StartupWait::StreamClosed));
                }
                (Some(WaitForLineResult::Timeout), _) | (_, Some(WaitForLineResult::Timeout)) => {
                    return Ok(collisions.or(StartupWait::Timeout));
                }
                _ => {}
            }
        }
    }
}

/// Waits on one output stream for the startup line, counting lines in `watch` and recording port
/// collisions in `collisions`.
fn wait_for_line_recording_collisions(
    stream: &OutputStream,
    startup_line: &str,
    timeout: Duration,
    watch: &Arc<LineWatch>,
    collisions: Arc<CollisionWatch>,
) -> impl Future<Output = Result<WaitForLineResult, StreamReadError>> {
    let expected_line = startup_line.to_owned();
    let watch = Arc::clone(watch);
    stream.wait_for_line(
        timeout,
        move |line| {
            watch.seen.fetch_add(1, Ordering::Release);
            if line.contains(&expected_line) {
                return true;
            }
            if let Some(collision) = PortCollision::detect(&line) {
                collisions.record(collision);
            }
            false
        },
        LineParsingOptions::default(),
    )
}

#[cfg(all(test, unix))]
mod tests {
    use std::{
        sync::Arc,
        time::{Duration, Instant},
    };

    use assertr::prelude::*;
    use tokio::process::Command;
    use tokio_process_tools::{
        AutoName, DEFAULT_MAX_BUFFERED_CHUNKS, DEFAULT_READ_CHUNK_SIZE, GracefulShutdown,
        NumBytesExt, Process, TerminateOnDrop,
    };

    use super::{LineWatch, OutputStream, PortCollision, StartupWait, wait_for_startup_line};

    const STARTUP_LINE: &str = "listening on http://127.0.0.1:3000";
    const HARMLESS_COLLISION: &str =
        "metrics listener 0.0.0.0:9090: Address already in use, disabling metrics";

    fn spawn_sh(script: &str) -> TerminateOnDrop<OutputStream> {
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg(script);
        Process::new(cmd)
            .name(AutoName::program_only())
            .stdout_and_stderr(|stream| {
                stream
                    .broadcast()
                    .reliable_with_backpressure()
                    .replay_last_bytes(1.megabytes())
                    .read_chunk_size(DEFAULT_READ_CHUNK_SIZE)
                    .max_buffered_chunks(DEFAULT_MAX_BUFFERED_CHUNKS)
            })
            .spawn()
            .expect("spawn sh")
            .terminate_on_drop(
                GracefulShutdown::builder()
                    .unix_sigint(Duration::from_secs(1))
                    .windows_ctrl_break(Duration::from_secs(1))
                    .build(),
            )
    }

    async fn wait(
        script: &str,
        timeout: Duration,
        collision_grace_period: Option<Duration>,
    ) -> StartupWait {
        let process = spawn_sh(script);
        let stdout_watch = Arc::new(LineWatch::default());
        let stderr_watch = Arc::new(LineWatch::default());
        let waiter = wait_for_startup_line(
            &process,
            STARTUP_LINE,
            timeout,
            collision_grace_period,
            &stdout_watch,
            &stderr_watch,
        );
        process.seal_output_replay();
        waiter.await.expect("read output")
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn harmless_collision_before_startup_line_is_ready() {
        let script =
            format!("echo '{HARMLESS_COLLISION}' >&2; echo '{STARTUP_LINE}'; exec sleep 30");

        let outcome = wait(&script, Duration::from_secs(30), None).await;

        assert_that!(outcome).is_equal_to(StartupWait::Ready);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn startup_line_within_grace_period_is_ready() {
        let script =
            format!("echo '{HARMLESS_COLLISION}'; sleep 0.2; echo '{STARTUP_LINE}'; exec sleep 30");

        let outcome = wait(
            &script,
            Duration::from_secs(30),
            Some(Duration::from_secs(10)),
        )
        .await;

        assert_that!(outcome).is_equal_to(StartupWait::Ready);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn collision_followed_by_exit_is_a_collision() {
        let script =
            "echo 'thread main panicked: Address already in use (os error 98)' >&2; exit 1";

        let outcome = wait(script, Duration::from_secs(30), None).await;

        assert_that!(outcome).is_equal_to(StartupWait::PortCollision(PortCollision::App));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn collision_without_startup_line_ends_after_grace_period() {
        let script = "echo 'Address already in use (os error 98)' >&2; exec sleep 30";

        let started = Instant::now();
        let outcome = wait(
            script,
            Duration::from_secs(30),
            Some(Duration::from_millis(200)),
        )
        .await;

        assert_that!(outcome).is_equal_to(StartupWait::PortCollision(PortCollision::App));
        assert_that!(started.elapsed()).is_less_than(Duration::from_secs(10));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn collision_without_grace_period_is_reported_on_timeout() {
        let script = "echo 'Address already in use (os error 98)' >&2; exec sleep 30";

        let outcome = wait(script, Duration::from_millis(300), None).await;

        assert_that!(outcome).is_equal_to(StartupWait::PortCollision(PortCollision::App));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn exit_without_collision_is_stream_closed() {
        let outcome = wait(
            "echo 'error: build failed' >&2; exit 1",
            Duration::from_secs(30),
            None,
        )
        .await;

        assert_that!(outcome).is_equal_to(StartupWait::StreamClosed);
    }
}
