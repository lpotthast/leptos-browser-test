use std::{path::PathBuf, time::Duration};

use thiserror::Error;

use crate::CargoLeptosMode;

/// Error contexts reported by `LeptosTestApp` startup operations.
#[derive(Debug, PartialEq, Eq, Error)]
#[non_exhaustive]
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

    /// The frontend stdout or stderr stream closed before startup completed.
    ///
    /// The report carries the recent output of `cargo leptos` as a
    /// [`CapturedOutput`](crate::CapturedOutput) attachment.
    #[error(
        "{app_name} output closed before startup completed. Expected stdout or stderr to contain {expected_line:?}."
    )]
    StartupOutputClosed {
        /// The descriptive app name.
        app_name: String,
        /// The expected startup line fragment.
        expected_line: String,
    },

    /// The frontend did not produce the expected startup line before the timeout.
    ///
    /// The report carries the recent output of `cargo leptos` as a
    /// [`CapturedOutput`](crate::CapturedOutput) attachment.
    #[error(
        "{app_name} did not start within {timeout:?} ({reason}). Expected stdout or stderr to contain {expected_line:?}."
    )]
    StartupTimedOut {
        /// The descriptive app name.
        app_name: String,
        /// The expected startup line fragment.
        expected_line: String,
        /// The configured startup timeout.
        timeout: Duration,
        /// The caller-supplied reason that justified this particular timeout, recorded by
        /// [`with_startup_timeout`](crate::LeptosTestAppConfig::with_startup_timeout).
        reason: String,
    },

    /// Reading the frontend output streams failed before the startup line was observed.
    ///
    /// The report carries the recent output of `cargo leptos` as a
    /// [`CapturedOutput`](crate::CapturedOutput) attachment.
    #[error("{app_name} failed to read output while waiting for {expected_line:?}.")]
    StreamRead {
        /// The descriptive app name.
        app_name: String,
        /// The expected startup line fragment.
        expected_line: String,
    },

    /// The app reported that an address it tried to bind is already in use.
    ///
    /// This is usually the site address. The output doesn't reliably say which address failed,
    /// though, so a taken port of an auxiliary listener (gRPC, metrics) ends up here as well. The
    /// captured output shows the actual bind failure.
    ///
    /// A collision only counts if the startup line doesn't follow it. In
    /// [`Serve`](CargoLeptosMode::Serve) mode, that is known once cargo-leptos exits. In
    /// [`Watch`](CargoLeptosMode::Watch) mode, cargo-leptos keeps running after the app
    /// failed, so startup gives the startup line a few seconds before reporting the collision,
    /// without waiting for the startup timeout. Startup only gives up like this when it can't
    /// retry with a fresh port: the site address was pinned with
    /// [`with_site_addr`](crate::LeptosTestAppConfig::with_site_addr), or the collision happened
    /// on the final attempt.
    ///
    /// The report carries the recent output of `cargo leptos` as a
    /// [`CapturedOutput`](crate::CapturedOutput) attachment.
    #[error(
        "{app_name} reported an address already in use while starting at site address {site_addr}. See the captured output for which address failed."
    )]
    PortInUse {
        /// The descriptive app name.
        app_name: String,
        /// The site address the app was started with.
        site_addr: String,
    },

    /// cargo-leptos could not bind the reload port because it is already in use.
    ///
    /// Only happens in [`Watch`](CargoLeptosMode::Watch) mode, which serves the reload
    /// port. Like [`PortInUse`](Self::PortInUse), it is reported without waiting for the
    /// startup timeout, once the reload port was pinned with
    /// [`with_reload_port`](crate::LeptosTestAppConfig::with_reload_port) or the collision
    /// happened on the final attempt.
    ///
    /// The report carries the recent output of `cargo leptos` as a
    /// [`CapturedOutput`](crate::CapturedOutput) attachment.
    #[error(
        "cargo-leptos could not bind reload port {reload_port} for {app_name} because it is already in use."
    )]
    ReloadPortInUse {
        /// The descriptive app name.
        app_name: String,
        /// The reload port cargo-leptos tried to bind.
        reload_port: u16,
    },

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

    use super::LeptosBrowserTestError;

    #[test]
    fn startup_output_closed_display_matches_documented_format() {
        let err = LeptosBrowserTestError::StartupOutputClosed {
            app_name: "demo".to_owned(),
            expected_line: "listening on".to_owned(),
        };
        assert_that!(err.to_string()).is_equal_to(
            "demo output closed before startup completed. Expected stdout or stderr to contain \"listening on\"."
                .to_owned(),
        );
    }

    #[test]
    fn startup_timed_out_display_matches_documented_format() {
        let err = LeptosBrowserTestError::StartupTimedOut {
            app_name: "demo".to_owned(),
            expected_line: "listening on".to_owned(),
            timeout: Duration::from_secs(7),
            reason: "tight bound for unit-style smoke test".to_owned(),
        };
        assert_that!(err.to_string()).is_equal_to(
            "demo did not start within 7s (tight bound for unit-style smoke test). Expected stdout or stderr to contain \"listening on\"."
                .to_owned(),
        );
    }

    #[test]
    fn stream_read_display_matches_documented_format() {
        let err = LeptosBrowserTestError::StreamRead {
            app_name: "demo".to_owned(),
            expected_line: "listening on".to_owned(),
        };
        assert_that!(err.to_string()).is_equal_to(
            "demo failed to read output while waiting for \"listening on\".".to_owned(),
        );
    }

    #[test]
    fn port_in_use_display_matches_documented_format() {
        let err = LeptosBrowserTestError::PortInUse {
            app_name: "demo".to_owned(),
            site_addr: "127.0.0.1:3000".to_owned(),
        };
        assert_that!(err.to_string()).is_equal_to(
            "demo reported an address already in use while starting at site address 127.0.0.1:3000. See the captured output for which address failed."
                .to_owned(),
        );
    }

    #[test]
    fn reload_port_in_use_display_matches_documented_format() {
        let err = LeptosBrowserTestError::ReloadPortInUse {
            app_name: "demo".to_owned(),
            reload_port: 3001,
        };
        assert_that!(err.to_string()).is_equal_to(
            "cargo-leptos could not bind reload port 3001 for demo because it is already in use."
                .to_owned(),
        );
    }
}
