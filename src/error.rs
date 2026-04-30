use std::{path::PathBuf, time::Duration};

use thiserror::Error;

use crate::CargoLeptosMode;

/// Diagnostic context attached to startup-failure variants.
///
/// Carries the descriptive app name, the stdout fragment we were waiting for, and the most
/// recent stdout/stderr lines captured before the failure was reported. The tails are bounded
/// by [`LeptosTestAppConfig::with_startup_log_tail_lines`](crate::LeptosTestAppConfig::with_startup_log_tail_lines).
#[derive(Debug, PartialEq, Eq)]
pub struct StartupFailureContext {
    /// The descriptive app name.
    pub app_name: String,
    /// The expected startup line fragment.
    pub expected_line: String,
    /// Recent stdout lines.
    pub stdout_tail: String,
    /// Recent stderr lines.
    pub stderr_tail: String,
}

/// Error contexts reported by Leptos test app startup operations.
#[derive(Debug, PartialEq, Eq, Error)]
pub enum LeptosBrowserTestError {
    /// The configured test app directory could not be resolved.
    #[error("failed to resolve {app_name} directory {app_dir:?}")]
    ResolveAppDir {
        /// The descriptive app name.
        app_name: String,
        /// The configured app directory.
        app_dir: PathBuf,
    },

    /// The `cargo leptos` process could not be spawned.
    #[error("failed to spawn `cargo leptos {mode_arg}` for {app_name}", mode_arg = mode.as_arg())]
    SpawnCargoLeptos {
        /// The descriptive app name.
        app_name: String,
        /// The selected `cargo leptos` mode.
        mode: CargoLeptosMode,
    },

    /// A free site port could not be selected.
    #[error("failed to find a free site port for {app_name}")]
    FindFreeSitePort {
        /// The descriptive app name.
        app_name: String,
    },

    /// A free reload port could not be selected.
    #[error("failed to find a free reload port for {app_name}")]
    FindFreeReloadPort {
        /// The descriptive app name.
        app_name: String,
    },

    /// The frontend stdout stream closed before startup completed.
    #[error(
        "{app_name} stdout closed before startup completed. Expected stdout to contain {expected_line:?}.\n\nRecent stdout:\n{stdout_tail}\n\nRecent stderr:\n{stderr_tail}",
        app_name = .0.app_name,
        expected_line = .0.expected_line,
        stdout_tail = .0.stdout_tail,
        stderr_tail = .0.stderr_tail,
    )]
    StartupStdoutClosed(StartupFailureContext),

    /// The frontend did not produce the expected startup line before the timeout.
    #[error(
        "{app_name} did not start within {timeout:?} ({reason}); expected stdout to contain {expected_line:?}.\n\nRecent stdout:\n{stdout_tail}\n\nRecent stderr:\n{stderr_tail}",
        app_name = ctx.app_name,
        expected_line = ctx.expected_line,
        stdout_tail = ctx.stdout_tail,
        stderr_tail = ctx.stderr_tail,
    )]
    StartupTimedOut {
        /// Diagnostic context (app name, expected line, captured tails).
        ctx: StartupFailureContext,
        /// The configured startup timeout.
        timeout: Duration,
        /// The caller-supplied reason that justified this particular timeout, recorded by
        /// [`with_startup_timeout`](crate::LeptosTestAppConfig::with_startup_timeout).
        reason: String,
    },

    /// Reading the frontend stdout stream failed before the startup line was observed.
    #[error(
        "{app_name} failed to read stdout while waiting for {expected_line:?}.\n\nRecent stdout:\n{stdout_tail}\n\nRecent stderr:\n{stderr_tail}",
        app_name = .0.app_name,
        expected_line = .0.expected_line,
        stdout_tail = .0.stdout_tail,
        stderr_tail = .0.stderr_tail,
    )]
    StreamRead(StartupFailureContext),

    /// The configured site address could not be parsed as a `host:port` socket address.
    #[error("invalid site_addr {site_addr:?} for {app_name}: expected `host:port`")]
    InvalidSiteAddr {
        /// The descriptive app name.
        app_name: String,
        /// The supplied site address that failed to parse.
        site_addr: String,
    },
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use assertr::prelude::*;

    use super::{LeptosBrowserTestError, StartupFailureContext};

    fn ctx() -> StartupFailureContext {
        StartupFailureContext {
            app_name: "demo".to_owned(),
            expected_line: "listening on".to_owned(),
            stdout_tail: "out-line".to_owned(),
            stderr_tail: "err-line".to_owned(),
        }
    }

    #[test]
    fn startup_stdout_closed_display_matches_documented_format() {
        let err = LeptosBrowserTestError::StartupStdoutClosed(ctx());
        assert_that!(err.to_string()).is_equal_to(
            "demo stdout closed before startup completed. Expected stdout to contain \"listening on\".\n\nRecent stdout:\nout-line\n\nRecent stderr:\nerr-line"
                .to_owned(),
        );
    }

    #[test]
    fn startup_timed_out_display_matches_documented_format() {
        let err = LeptosBrowserTestError::StartupTimedOut {
            ctx: ctx(),
            timeout: Duration::from_secs(7),
            reason: "tight bound for unit-style smoke test".to_owned(),
        };
        assert_that!(err.to_string()).is_equal_to(
            "demo did not start within 7s (tight bound for unit-style smoke test); expected stdout to contain \"listening on\".\n\nRecent stdout:\nout-line\n\nRecent stderr:\nerr-line"
                .to_owned(),
        );
    }

    #[test]
    fn stream_read_display_matches_documented_format() {
        let err = LeptosBrowserTestError::StreamRead(ctx());
        assert_that!(err.to_string()).is_equal_to(
            "demo failed to read stdout while waiting for \"listening on\".\n\nRecent stdout:\nout-line\n\nRecent stderr:\nerr-line"
                .to_owned(),
        );
    }
}
