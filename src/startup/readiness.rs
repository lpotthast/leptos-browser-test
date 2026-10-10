use std::{
    sync::{Arc, OnceLock},
    time::Duration,
};

use rootcause::{IntoReport, Report};
use tokio::{sync::Notify, time::Instant};
use tokio_process_tools::{
    LineParsingOptions, StreamReadError, TerminateOnDrop, WaitForLineResult,
};

use super::{
    OutputStream,
    collision::{DetectedCollision, PortCollision},
    logs::{StartupLogBuffer, StartupLogs},
    retry::RetryPolicy,
};
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
    PortCollision(DetectedCollision),
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
    /// The site address, reported if a port is in use.
    pub(crate) site_addr: &'a str,
    /// The reload port, reported if a port is in use.
    pub(crate) reload_port: u16,
    /// Decides how long a port collision may precede the startup line.
    pub(crate) mode: CargoLeptosMode,
    /// How many recent lines per stream a startup failure reports.
    pub(crate) log_tail_lines: usize,
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
/// Each error report carries the output seen while waiting as a
/// [`CapturedOutput`](super::CapturedOutput) attachment.
pub(crate) async fn wait_for_ready(
    process: &TerminateOnDrop<OutputStream>,
    expected: &StartupExpectation<'_>,
    retry_policy: RetryPolicy,
) -> Result<StartupOutcome, Report<LeptosBrowserTestError>> {
    tracing::info!(
        "Waiting {:?} ({}) for {} to start...",
        expected.timeout,
        expected.timeout_reason,
        expected.app_name,
    );

    let logs = StartupLogs::new(expected.log_tail_lines);
    let startup_waiter = wait_for_startup_line(
        process,
        expected.startup_line,
        expected.timeout,
        expected.collision_grace_period(),
        &logs,
    );
    process.seal_output_replay();

    let app_name = || expected.app_name.to_owned();
    let expected_line = || expected.startup_line.to_owned();
    let outcome = match startup_waiter.await {
        Ok(StartupWait::Ready) => Ok(StartupOutcome::Ready),
        Ok(StartupWait::PortCollision(collision)) if retry_policy.retries(collision.kind) => {
            Ok(StartupOutcome::PortCollision(collision))
        }
        Ok(StartupWait::PortCollision(collision)) => {
            let report: Report<LeptosBrowserTestError> =
                Report::new(LeptosBrowserTestError::PortInUse {
                    app_name: app_name(),
                    site_addr: expected.site_addr.to_owned(),
                    reload_port: expected.reload_port,
                });
            Err(report.attach(collision))
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
    outcome.map_err(|report| report.attach(logs.snapshot()))
}

/// The first port collision seen on either output stream.
#[derive(Default)]
struct CollisionWatch {
    first: OnceLock<DetectedCollision>,
    /// Notified once, when `first` is set.
    detected: Notify,
}

impl CollisionWatch {
    fn record(&self, collision: DetectedCollision) {
        if self.first.set(collision).is_ok() {
            self.detected.notify_one();
        }
    }

    /// The collision, if one was seen, or `otherwise`.
    fn or(&self, otherwise: StartupWait) -> StartupWait {
        self.first.get().map_or(otherwise, |collision| {
            StartupWait::PortCollision(collision.clone())
        })
    }
}

/// How waiting on stdout and stderr ended.
#[derive(Debug, PartialEq, Eq)]
enum StartupWait {
    Ready,
    PortCollision(DetectedCollision),
    StreamClosed,
    Timeout,
}

/// Subscribes to stdout and stderr immediately and returns a future racing both for the startup
/// line, recording every line it sees in `logs`.
///
/// Output only counts as closed once both streams closed, so that `logs` holds all of it. A port
/// collision seen before the startup line is reported instead of `StreamClosed` or `Timeout`.
/// With a `collision_grace_period`, it is also reported once that period passed after the
/// collision without the startup line showing up.
///
/// Deliberately not an `async fn`: the subscriptions must exist before the caller seals the
/// output replay, but an `async fn` body would only run (and subscribe) on first poll.
fn wait_for_startup_line(
    process: &TerminateOnDrop<OutputStream>,
    startup_line: &str,
    timeout: Duration,
    collision_grace_period: Option<Duration>,
    logs: &StartupLogs,
) -> impl Future<Output = Result<StartupWait, StreamReadError>> {
    let collisions = Arc::new(CollisionWatch::default());
    let stdout_waiter = wait_for_line_recording_collisions(
        process.stdout(),
        startup_line,
        timeout,
        logs.stdout.clone(),
        Arc::clone(&collisions),
    );
    let stderr_waiter = wait_for_line_recording_collisions(
        process.stderr(),
        startup_line,
        timeout,
        logs.stderr.clone(),
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
                (Some(WaitForLineResult::Timeout), _) | (_, Some(WaitForLineResult::Timeout)) => {
                    return Ok(collisions.or(StartupWait::Timeout));
                }
                (Some(WaitForLineResult::StreamClosed), Some(WaitForLineResult::StreamClosed)) => {
                    return Ok(collisions.or(StartupWait::StreamClosed));
                }
                _ => {}
            }
        }
    }
}

/// Waits on one output stream for the startup line, recording every line in `log` and port
/// collisions in `collisions`.
fn wait_for_line_recording_collisions(
    stream: &OutputStream,
    startup_line: &str,
    timeout: Duration,
    log: StartupLogBuffer,
    collisions: Arc<CollisionWatch>,
) -> impl Future<Output = Result<WaitForLineResult, StreamReadError>> {
    let expected_line = startup_line.to_owned();
    stream.wait_for_line(
        timeout,
        move |line| {
            log.push(&line);
            if line.contains(&expected_line) {
                return true;
            }
            if let Some(kind) = PortCollision::detect(&line)
                && collisions.first.get().is_none()
            {
                collisions.record(DetectedCollision {
                    kind,
                    line: line.into_owned(),
                });
            }
            false
        },
        LineParsingOptions::default(),
    )
}

#[cfg(all(test, unix))]
mod tests {
    use std::time::{Duration, Instant};

    use assertr::prelude::*;
    use tokio::process::Command;
    use tokio_process_tools::{
        AutoName, DEFAULT_MAX_BUFFERED_CHUNKS, DEFAULT_READ_CHUNK_SIZE, GracefulShutdown,
        NumBytesExt, Process, TerminateOnDrop,
    };

    use super::{
        DetectedCollision, OutputStream, PortCollision, RetryPolicy, StartupExpectation,
        StartupLogs, StartupWait, wait_for_ready, wait_for_startup_line,
    };
    use crate::{CapturedOutput, CargoLeptosMode, LeptosBrowserTestError, LeptosTestAppConfig};

    const STARTUP_LINE: &str = "listening on http://127.0.0.1:3000";
    const BIND_PANIC: &str = "thread main panicked: Address already in use (os error 98)";
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
    ) -> (StartupWait, CapturedOutput) {
        let process = spawn_sh(script);
        let logs = StartupLogs::new(10);
        let waiter = wait_for_startup_line(
            &process,
            STARTUP_LINE,
            timeout,
            collision_grace_period,
            &logs,
        );
        process.seal_output_replay();
        let outcome = waiter.await.expect("read output");
        (outcome, logs.snapshot())
    }

    fn app_collision(line: &str) -> StartupWait {
        StartupWait::PortCollision(DetectedCollision {
            kind: PortCollision::App,
            line: line.to_owned(),
        })
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn harmless_collision_before_startup_line_is_ready() {
        let script =
            format!("echo '{HARMLESS_COLLISION}' >&2; echo '{STARTUP_LINE}'; exec sleep 30");

        let (outcome, _) = wait(&script, Duration::from_secs(30), None).await;

        assert_that!(outcome).is_equal_to(StartupWait::Ready);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn startup_line_within_grace_period_is_ready() {
        let script =
            format!("echo '{HARMLESS_COLLISION}'; sleep 0.2; echo '{STARTUP_LINE}'; exec sleep 30");

        let (outcome, _) = wait(
            &script,
            Duration::from_secs(30),
            Some(Duration::from_secs(10)),
        )
        .await;

        assert_that!(outcome).is_equal_to(StartupWait::Ready);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn collision_followed_by_exit_is_a_collision() {
        let script = format!("echo '{BIND_PANIC}' >&2; exit 1");

        let (outcome, _) = wait(&script, Duration::from_secs(30), None).await;

        assert_that!(outcome).is_equal_to(app_collision(BIND_PANIC));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn collision_without_startup_line_ends_after_grace_period() {
        let script = format!("echo '{BIND_PANIC}' >&2; exec sleep 30");

        let started = Instant::now();
        let (outcome, _) = wait(
            &script,
            Duration::from_secs(30),
            Some(Duration::from_millis(200)),
        )
        .await;

        assert_that!(outcome).is_equal_to(app_collision(BIND_PANIC));
        assert_that!(started.elapsed()).is_less_than(Duration::from_secs(10));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn collision_without_grace_period_is_reported_on_timeout() {
        let script = format!("echo '{BIND_PANIC}' >&2; exec sleep 30");

        let (outcome, _) = wait(&script, Duration::from_millis(300), None).await;

        assert_that!(outcome).is_equal_to(app_collision(BIND_PANIC));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn exit_without_collision_is_stream_closed() {
        let (outcome, _) = wait(
            "echo 'error: build failed' >&2; exit 1",
            Duration::from_secs(30),
            None,
        )
        .await;

        assert_that!(outcome).is_equal_to(StartupWait::StreamClosed);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn waits_for_both_streams_to_close_and_captures_all_output() {
        // stdout closes first. The bind failure only shows up on stderr afterwards.
        let script =
            format!("echo 'compiling'; exec 1>&-; sleep 0.2; echo '{BIND_PANIC}' >&2; exit 1");

        let (outcome, output) = wait(&script, Duration::from_secs(30), None).await;

        assert_that!(outcome).is_equal_to(app_collision(BIND_PANIC));
        assert_that!(output.stdout()).is_equal_to("compiling");
        assert_that!(output.stderr()).is_equal_to(BIND_PANIC);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn port_in_use_report_names_the_reported_line_even_without_captured_output() {
        let process = spawn_sh(&format!("echo '{BIND_PANIC}' >&2; exit 1"));
        let expected = StartupExpectation {
            app_name: "demo",
            startup_line: STARTUP_LINE,
            timeout: Duration::from_secs(30),
            timeout_reason: "test",
            site_addr: "127.0.0.1:3000",
            reload_port: 3001,
            mode: CargoLeptosMode::Serve,
            log_tail_lines: 0,
        };
        let final_attempt = RetryPolicy::for_attempt(
            &LeptosTestAppConfig::new("./test-app"),
            RetryPolicy::MAX_ATTEMPTS,
        );

        let Err(report) = wait_for_ready(&process, &expected, final_attempt).await else {
            panic!("startup should fail with PortInUse");
        };

        assert_that!(matches!(
            report.current_context(),
            LeptosBrowserTestError::PortInUse { .. }
        ))
        .is_true();
        assert_that!(report.to_string()).contains(format!(
            "The app reported an address already in use: {BIND_PANIC}"
        ));
    }
}
