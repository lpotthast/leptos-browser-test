use std::{
    sync::{
        Arc, OnceLock,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use rootcause::{IntoReport, Report};
use tokio_process_tools::{
    LineParsingOptions, StreamReadError, TerminateOnDrop, WaitForLineResult,
};

use super::{OutputStream, collision::PortCollision, logs::StartupLogs, retry::RetryPolicy};
use crate::LeptosBrowserTestError;

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
}

/// Waits until `process` prints the expected startup line on stdout or stderr, or reports a port
/// collision that `retry_policy` allows retrying.
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
        Ok(StartupWait::PortCollision(PortCollision::Site)) => {
            Err(Report::new(LeptosBrowserTestError::SitePortInUse {
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
    /// Set when the waiter stopped on a port collision rather than the startup line.
    collision: OnceLock<PortCollision>,
}

impl LineWatch {
    fn seen(&self) -> usize {
        self.seen.load(Ordering::Acquire)
    }
}

/// How waiting on stdout and stderr ended.
enum StartupWait {
    Ready,
    PortCollision(PortCollision),
    StreamClosed,
    Timeout,
}

/// Subscribes to stdout and stderr immediately and returns a future racing both for the startup
/// line or a port collision.
///
/// Deliberately not an `async fn`: the subscriptions must exist before the caller seals the
/// output replay, but an `async fn` body would only run (and subscribe) on first poll.
fn wait_for_startup_line(
    process: &TerminateOnDrop<OutputStream>,
    startup_line: &str,
    timeout: Duration,
    stdout_watch: &Arc<LineWatch>,
    stderr_watch: &Arc<LineWatch>,
) -> impl Future<Output = Result<StartupWait, StreamReadError>> {
    let stdout_waiter =
        wait_for_line_or_collision(process.stdout(), startup_line, timeout, stdout_watch);
    let stderr_waiter =
        wait_for_line_or_collision(process.stderr(), startup_line, timeout, stderr_watch);
    let stdout_watch = Arc::clone(stdout_watch);
    let stderr_watch = Arc::clone(stderr_watch);

    // A waiter stops on either the startup line or a port collision. Its watch tells the two
    // apart.
    let matched = |watch: &LineWatch| {
        watch
            .collision
            .get()
            .map_or(StartupWait::Ready, |collision| {
                StartupWait::PortCollision(*collision)
            })
    };

    async move {
        tokio::pin!(stdout_waiter);
        tokio::pin!(stderr_waiter);

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
            }

            match (stdout_result, stderr_result) {
                (Some(WaitForLineResult::Matched), _) => {
                    return Ok(matched(&stdout_watch));
                }
                (_, Some(WaitForLineResult::Matched)) => {
                    return Ok(matched(&stderr_watch));
                }
                (Some(WaitForLineResult::StreamClosed), _) => {
                    return Ok(StartupWait::StreamClosed);
                }
                (_, Some(WaitForLineResult::StreamClosed)) => {
                    return Ok(StartupWait::StreamClosed);
                }
                (Some(WaitForLineResult::Timeout), _) => {
                    return Ok(StartupWait::Timeout);
                }
                (_, Some(WaitForLineResult::Timeout)) => {
                    return Ok(StartupWait::Timeout);
                }
                _ => {}
            }
        }
    }
}

/// Waits on one output stream for the startup line or a port collision, recording what it saw in
/// `watch`.
fn wait_for_line_or_collision(
    stream: &OutputStream,
    startup_line: &str,
    timeout: Duration,
    watch: &Arc<LineWatch>,
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
            match PortCollision::detect(&line) {
                Some(detected) => {
                    let _ = watch.collision.set(detected);
                    true
                }
                None => false,
            }
        },
        LineParsingOptions::default(),
    )
}
