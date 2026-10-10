use std::{
    collections::VecDeque,
    fmt,
    sync::{Arc, Mutex, PoisonError},
};

use rootcause::{
    Report, ReportRef, markers::ReportOwnershipMarker, report_attachment::ReportAttachmentRef,
};

/// The recent output of one startup attempt, recorded by its readiness waiters.
///
/// The waiters push each line before checking it, so a snapshot taken after a wait ended always
/// includes the line that decided it (for example a bind failure). Don't fill these from the log
/// forwarding consumers instead. As separate subscribers they lag behind the waiters, so that line
/// could be missing.
#[derive(Debug, Clone)]
pub(super) struct StartupLogs {
    pub(super) stdout: StartupLogBuffer,
    pub(super) stderr: StartupLogBuffer,
}

impl StartupLogs {
    pub(super) fn new(max_lines: usize) -> Self {
        Self {
            stdout: StartupLogBuffer::new(max_lines),
            stderr: StartupLogBuffer::new(max_lines),
        }
    }

    /// Copies the lines captured so far.
    pub(super) fn snapshot(&self) -> CapturedOutput {
        CapturedOutput {
            stdout: self.stdout.render(),
            stderr: self.stderr.render(),
        }
    }
}

/// The most recent stdout and stderr lines of the `cargo leptos` process, captured when startup
/// failed.
///
/// Attached to the report of every startup failure that happens once `cargo leptos` is running.
/// Failures before that, such as an unresolvable app directory or `cargo leptos` failing to spawn,
/// have no output and no attachment, so [`find_in`](Self::find_in) returns `None` for them. Each
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
    ///
    /// Returns `None` for failures that happened before `cargo leptos` was running, since those
    /// have no output.
    ///
    /// Accepts any report, whatever its ownership and thread-safety markers. For a
    /// [`ReportRef`], for example one obtained while walking nested reports, use
    /// [`find_in_ref`](Self::find_in_ref).
    #[must_use]
    pub fn find_in<C: ?Sized, O: ReportOwnershipMarker, T>(
        report: &Report<C, O, T>,
    ) -> Option<&Self> {
        Self::find_in_ref(report.as_ref())
    }

    /// Like [`find_in`](Self::find_in), but for a [`ReportRef`].
    #[must_use]
    pub fn find_in_ref<C: ?Sized, O, T>(report: ReportRef<'_, C, O, T>) -> Option<&Self> {
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
pub(super) struct StartupLogBuffer {
    lines: Arc<Mutex<VecDeque<String>>>,
    max_lines: usize,
}

impl StartupLogBuffer {
    fn new(max_lines: usize) -> Self {
        Self {
            lines: Arc::new(Mutex::new(VecDeque::with_capacity(max_lines))),
            max_lines,
        }
    }

    pub(super) fn push(&self, line: &str) {
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

    #[test]
    fn find_in_accepts_cloneable_and_local_reports() {
        let logs = StartupLogs::new(10);
        logs.stderr.push("err-line");
        let cloneable = report!("startup failed")
            .attach(logs.snapshot())
            .into_cloneable();
        let local = report!("startup failed")
            .attach(logs.snapshot())
            .into_local();

        assert_that!(CapturedOutput::find_in(&cloneable).map(CapturedOutput::stderr))
            .is_equal_to(Some("err-line"));
        assert_that!(CapturedOutput::find_in(&local).map(CapturedOutput::stderr))
            .is_equal_to(Some("err-line"));
    }

    #[test]
    fn find_in_ref_searches_below_the_given_report() {
        let logs = StartupLogs::new(10);
        logs.stderr.push("err-line");
        let report: Report = report!("startup failed")
            .attach(logs.snapshot())
            .context("while running browser tests")
            .into_dynamic();

        let found = report
            .iter_sub_reports()
            .find_map(CapturedOutput::find_in_ref)
            .map(CapturedOutput::stderr);

        assert_that!(found).is_equal_to(Some("err-line"));
    }

    #[test]
    fn find_in_returns_none_without_attachment() {
        let report: Report = report!("unrelated failure");

        assert_that!(CapturedOutput::find_in(&report)).is_equal_to(None);
    }
}
