use std::{
    collections::VecDeque,
    sync::{Arc, Mutex, PoisonError},
};

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

    pub(crate) fn stdout_tail(&self) -> String {
        self.stdout.render()
    }

    pub(crate) fn stderr_tail(&self) -> String {
        self.stderr.render()
    }
}

#[derive(Debug, Clone)]
pub(crate) struct StartupLogBuffer {
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

    pub(crate) fn push(&self, line: &str) {
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
        if lines.is_empty() {
            "<no output captured>".to_owned()
        } else {
            lines.iter().cloned().collect::<Vec<_>>().join("\n")
        }
    }
}

#[cfg(test)]
mod tests {
    use assertr::prelude::*;

    use super::StartupLogBuffer;

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

        assert_that!(buffer.render()).is_equal_to("<no output captured>");
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
}
