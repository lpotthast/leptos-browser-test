use crate::{LeptosBrowserTestError, error::StartupFailureContext};
use rootcause::{IntoReport, Report, bail};
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use tokio_process_tools::{
    LineParsingOptions, StreamReadError, TerminateOnDrop, WaitForLineResult,
};

use super::{OutputStream, collision::PortCollision, logs::StartupLogs, retry::RetryPolicy};

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
}

/// Waits until `process` prints the expected startup line on stdout or stderr, or reports a port
/// collision that `retry_policy` allows retrying.
///
/// # Errors
///
/// Returns an error if both output streams close first, the startup timeout elapses, or reading
/// the output fails. Each error embeds the recent output captured in `logs`.
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

    let startup_waiter = wait_for_startup_line(
        process,
        expected.startup_line,
        expected.timeout,
        retry_policy,
    );
    process.seal_output_replay();

    match startup_waiter.await {
        Ok(StartupWait::Ready) => Ok(StartupOutcome::Ready),
        Ok(StartupWait::PortCollision(collision)) => Ok(StartupOutcome::PortCollision(collision)),
        Ok(StartupWait::StreamClosed) => {
            bail!(LeptosBrowserTestError::StartupStdoutClosed(
                startup_failure_context(expected, logs),
            ));
        }
        Ok(StartupWait::Timeout) => {
            bail!(LeptosBrowserTestError::StartupTimedOut {
                ctx: startup_failure_context(expected, logs),
                timeout: expected.timeout,
                reason: expected.timeout_reason.to_owned(),
            });
        }
        Err(err) => {
            let err: Report<StreamReadError> = err.into_report();
            Err(
                err.context(LeptosBrowserTestError::StreamRead(startup_failure_context(
                    expected, logs,
                ))),
            )
        }
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
/// line, or for a port collision that `retry_policy` allows retrying.
///
/// Deliberately not an `async fn`: the subscriptions must exist before the caller seals the
/// output replay, but an `async fn` body would only run (and subscribe) on first poll.
fn wait_for_startup_line(
    process: &TerminateOnDrop<OutputStream>,
    startup_line: &str,
    timeout: Duration,
    retry_policy: RetryPolicy,
) -> impl Future<Output = Result<StartupWait, StreamReadError>> {
    let (stdout_waiter, stdout_collision) =
        wait_for_line_or_collision(process.stdout(), startup_line, timeout, retry_policy);
    let (stderr_waiter, stderr_collision) =
        wait_for_line_or_collision(process.stderr(), startup_line, timeout, retry_policy);

    // A waiter stops on either the startup line or a retryable collision. Its collision slot
    // tells the two apart.
    let matched = |collision: &OnceLock<PortCollision>| {
        collision.get().map_or(StartupWait::Ready, |collision| {
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

            if stdout_result == Some(WaitForLineResult::Matched) {
                return Ok(matched(&stdout_collision));
            }
            if stderr_result == Some(WaitForLineResult::Matched) {
                return Ok(matched(&stderr_collision));
            }

            match (stdout_result, stderr_result) {
                (Some(WaitForLineResult::StreamClosed), Some(WaitForLineResult::StreamClosed)) => {
                    return Ok(StartupWait::StreamClosed);
                }
                (Some(stdout), Some(stderr)) => {
                    debug_assert!(
                        matches!(stdout, WaitForLineResult::Timeout)
                            || matches!(stderr, WaitForLineResult::Timeout)
                    );
                    return Ok(StartupWait::Timeout);
                }
                _ => {}
            }
        }
    }
}

/// Waits on one output stream for the startup line or a collision that `retry_policy` allows
/// retrying. The returned slot is set when the wait stopped because of such a collision.
fn wait_for_line_or_collision(
    stream: &OutputStream,
    startup_line: &str,
    timeout: Duration,
    retry_policy: RetryPolicy,
) -> (
    impl Future<Output = Result<WaitForLineResult, StreamReadError>>,
    Arc<OnceLock<PortCollision>>,
) {
    let expected_line = startup_line.to_owned();
    let collision = Arc::new(OnceLock::new());
    let collision_slot = Arc::clone(&collision);
    let waiter = stream.wait_for_line(
        timeout,
        move |line| {
            if line.contains(&expected_line) {
                return true;
            }
            match PortCollision::detect(&line) {
                Some(detected) if retry_policy.retries(detected) => {
                    let _ = collision_slot.set(detected);
                    true
                }
                _ => false,
            }
        },
        LineParsingOptions::default(),
    );
    (waiter, collision)
}

fn startup_failure_context(
    expected: &StartupExpectation<'_>,
    logs: &StartupLogs,
) -> StartupFailureContext {
    StartupFailureContext {
        app_name: expected.app_name.to_owned(),
        expected_line: expected.startup_line.to_owned(),
        stdout_tail: logs.stdout_tail(),
        stderr_tail: logs.stderr_tail(),
    }
}
