use std::{
    collections::VecDeque,
    fmt,
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use rootcause::{Report, report_attachment::ReportAttachmentRef};

/// How long a failed startup waits for the log buffers to record the lines its readiness waiters
/// already saw.
const CATCH_UP_TIMEOUT: Duration = Duration::from_secs(1);

#[derive(Debug, Clone)]
pub(crate) struct StartupLogs {
    pub(crate) stdout: StartupLogBuffer,
    pub(crate) stderr: StartupLogBuffer,
}

impl StartupLogs {
    pub(crate) fn new(max_lines: usize) -> Self {
        Self {
            stdout: StartupLogBuffer::new(max_lines),
            stderr: StartupLogBuffer::new(max_lines),
        }
    }

    /// Copies the lines captured so far.
    pub(crate) fn snapshot(&self) -> CapturedOutput {
        CapturedOutput {
            stdout: self.stdout.render(),
            stderr: self.stderr.render(),
        }
    }

    /// Copies the captured lines once the buffers have received at least `stdout_lines` and
    /// `stderr_lines` lines, or after [`CATCH_UP_TIMEOUT`].
    ///
    /// The readiness waiters and the consumers filling these buffers read the output
    /// independently. Without waiting, the line that ended a wait (for example the bind failure)
    /// could still be on its way into the buffer and be missing from the snapshot.
    pub(crate) async fn snapshot_after(
        &self,
        stdout_lines: usize,
        stderr_lines: usize,
    ) -> CapturedOutput {
        let caught_up = async {
            while self.stdout.received() < stdout_lines || self.stderr.received() < stderr_lines {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        };
        if tokio::time::timeout(CATCH_UP_TIMEOUT, caught_up)
            .await
            .is_err()
        {
            tracing::debug!("Log buffers did not catch up with the readiness waiters in time.");
        }
        self.snapshot()
    }
}

/// The most recent stdout and stderr lines of the `cargo leptos` process, captured when startup
/// failed.
///
/// Attached to the report of every startup failure. Each
/// [`LeptosBrowserTestError`](crate::LeptosBrowserTestError) variant that gets it says so in its
/// docs. How many lines are kept is set by
/// [`LeptosTestAppConfig::with_startup_log_tail_lines`](crate::LeptosTestAppConfig::with_startup_log_tail_lines).
///
/// ```rust,no_run
/// use leptos_browser_test::{CapturedOutput, LeptosTestAppConfig};
///
/// # async fn example() {
/// if let Err(report) = LeptosTestAppConfig::new("testing/test-app").start().await {
///     if let Some(output) = CapturedOutput::find_in(&report) {
///         eprintln!("cargo leptos stderr:\n{}", output.stderr());
///     }
/// }
/// # }
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapturedOutput {
    stdout: String,
    stderr: String,
}

impl CapturedOutput {
    /// The captured stdout lines, joined with `\n`. Empty if nothing was captured.
    #[must_use]
    pub fn stdout(&self) -> &str {
        &self.stdout
    }

    /// The captured stderr lines, joined with `\n`. Empty if nothing was captured.
    #[must_use]
    pub fn stderr(&self) -> &str {
        &self.stderr
    }

    /// Finds the captured output attached anywhere in `report`, including reports wrapped in
    /// further context.
    #[must_use]
    pub fn find_in<C: ?Sized>(report: &Report<C>) -> Option<&Self> {
        report.iter_reports().find_map(|node| {
            node.attachments()
                .iter()
                .find_map(ReportAttachmentRef::downcast_inner::<Self>)
        })
    }
}

impl fmt::Display for CapturedOutput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fn or_placeholder(lines: &str) -> &str {
            if lines.is_empty() {
                "<no output captured>"
            } else {
                lines
            }
        }

        write!(
            f,
            "Recent stdout:\n{}\n\nRecent stderr:\n{}",
            or_placeholder(&self.stdout),
            or_placeholder(&self.stderr),
        )
    }
}

#[derive(Debug, Clone)]
pub(crate) struct StartupLogBuffer {
    lines: Arc<Mutex<VecDeque<String>>>,
    max_lines: usize,
    /// Every line ever pushed, including the ones not kept.
    received: Arc<AtomicUsize>,
}

impl StartupLogBuffer {
    fn new(max_lines: usize) -> Self {
        Self {
            lines: Arc::new(Mutex::new(VecDeque::with_capacity(max_lines))),
            max_lines,
            received: Arc::new(AtomicUsize::new(0)),
        }
    }

    fn received(&self) -> usize {
        self.received.load(Ordering::Acquire)
    }

    pub(crate) fn push(&self, line: &str) {
        self.push_line(line);
        self.received.fetch_add(1, Ordering::Release);
    }

    fn push_line(&self, line: &str) {
        if self.max_lines == 0 {
            return;
        }

        let mut lines = self.lines.lock().unwrap_or_else(PoisonError::into_inner);
        if lines.len() == self.max_lines {
            lines.pop_front();
        }
        lines.push_back(line.to_owned());
    }

    fn render(&self) -> String {
        let lines = self.lines.lock().unwrap_or_else(PoisonError::into_inner);
        lines.iter().cloned().collect::<Vec<_>>().join("\n")
    }
}

#[cfg(test)]
mod tests {
    use assertr::prelude::*;
    use rootcause::{Report, report};

    use super::{CapturedOutput, StartupLogBuffer, StartupLogs};

    #[test]
    fn startup_log_buffer_keeps_recent_tail() {
        let buffer = StartupLogBuffer::new(2);

        buffer.push("one");
        buffer.push("two");
        buffer.push("three");

        assert_that!(buffer.render()).is_equal_to("two\nthree");
    }

    #[test]
    fn startup_log_buffer_can_be_disabled() {
        let buffer = StartupLogBuffer::new(0);

        buffer.push("ignored");

        assert_that!(buffer.render()).is_equal_to("");
    }

    #[test]
    fn recovers_from_poisoned_mutex() {
        let buffer = StartupLogBuffer::new(2);
        buffer.push("before-poison");

        // Poison the mutex by panicking while holding the guard in another thread.
        let buffer_clone = buffer.clone();
        let _ = std::thread::spawn(move || {
            let _guard = buffer_clone.lines.lock().expect("first lock");
            panic!("intentional poison");
        })
        .join();

        // Rendering must not panic and must surface the previously captured line.
        assert_that!(buffer.render()).is_equal_to("before-poison");

        // Pushing also recovers and the new line is observable.
        buffer.push("after-poison");
        assert_that!(buffer.render()).is_equal_to("before-poison\nafter-poison");
    }

    #[test]
    fn captured_output_display_shows_both_streams() {
        let logs = StartupLogs::new(10);
        logs.stdout.push("out-line");
        logs.stderr.push("err-line");

        assert_that!(logs.snapshot().to_string())
            .is_equal_to("Recent stdout:\nout-line\n\nRecent stderr:\nerr-line");
    }

    #[test]
    fn captured_output_display_marks_empty_streams() {
        let output = StartupLogs::new(10).snapshot();

        assert_that!(output.stdout()).is_equal_to("");
        assert_that!(output.to_string()).is_equal_to(
            "Recent stdout:\n<no output captured>\n\nRecent stderr:\n<no output captured>",
        );
    }

    #[test]
    fn find_in_locates_attachment_below_added_context() {
        let logs = StartupLogs::new(10);
        logs.stderr.push("err-line");
        let report: Report = report!("startup failed")
            .attach(logs.snapshot())
            .context("while running browser tests")
            .into_dynamic();

        let found = CapturedOutput::find_in(&report).map(CapturedOutput::stderr);
        assert_that!(found).is_equal_to(Some("err-line"));
    }

    #[tokio::test]
    async fn snapshot_after_waits_for_lines_still_in_flight() {
        let logs = StartupLogs::new(10);
        let late_logs = logs.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            late_logs.stderr.push("address already in use");
        });

        let output = logs.snapshot_after(0, 1).await;

        assert_that!(output.stderr()).is_equal_to("address already in use");
    }

    #[test]
    fn startup_log_buffer_counts_lines_it_does_not_keep() {
        let buffer = StartupLogBuffer::new(0);

        buffer.push("ignored");

        assert_that!(buffer.received()).is_equal_to(1);
    }

    #[test]
    fn find_in_returns_none_without_attachment() {
        let report: Report = report!("unrelated failure");

        assert_that!(CapturedOutput::find_in(&report)).is_equal_to(None);
    }
}
