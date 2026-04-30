use std::{
    ffi::{OsStr, OsString},
    path::PathBuf,
    time::Duration,
};

use rootcause::Report;

use crate::{CargoLeptosMode, LeptosBrowserTestError, SiteScheme, app::LeptosTestApp};

const DEFAULT_STARTUP_LOG_TAIL_LINES: usize = 200;
const DEFAULT_STARTUP_TIMEOUT: Duration = Duration::from_secs(60 * 10);
const DEFAULT_STARTUP_TIMEOUT_REASON: &str =
    "default — generous bound for a cold cargo-leptos compile of server + wasm";
const DEFAULT_INTERRUPT_TIMEOUT: Duration = Duration::from_secs(3);
const DEFAULT_TERMINATE_TIMEOUT: Duration = Duration::from_secs(8);
const DEFAULT_TERMINATION_TIMEOUTS_REASON: &str =
    "default — graceful cargo-leptos shutdown before SIGKILL";

/// Configuration for a Leptos test app process.
#[derive(Debug, Clone)]
pub struct LeptosTestAppConfig {
    pub(crate) app_dir: PathBuf,
    pub(crate) app_name: String,
    pub(crate) mode: CargoLeptosMode,
    pub(crate) cargo_bin: Option<OsString>,
    pub(crate) site_scheme: SiteScheme,
    pub(crate) site_addr: Option<String>,
    pub(crate) reload_port: Option<u16>,
    pub(crate) startup_line: Option<String>,
    pub(crate) startup_timeout: Duration,
    pub(crate) startup_timeout_reason: String,
    pub(crate) startup_log_tail_lines: usize,
    pub(crate) interrupt_timeout: Duration,
    pub(crate) terminate_timeout: Duration,
    pub(crate) termination_timeouts_reason: String,
    pub(crate) forward_logs: bool,
    pub(crate) extra_env: Vec<(OsString, OsString)>,
}

impl LeptosTestAppConfig {
    /// Create a config for a test app directory.
    #[must_use]
    pub fn new(app_dir: impl Into<PathBuf>) -> Self {
        Self {
            app_dir: app_dir.into(),
            app_name: "Leptos test app".to_owned(),
            mode: CargoLeptosMode::Serve,
            cargo_bin: None,
            site_scheme: SiteScheme::Http,
            site_addr: None,
            reload_port: None,
            startup_line: None,
            startup_timeout: DEFAULT_STARTUP_TIMEOUT,
            startup_timeout_reason: DEFAULT_STARTUP_TIMEOUT_REASON.to_owned(),
            startup_log_tail_lines: DEFAULT_STARTUP_LOG_TAIL_LINES,
            interrupt_timeout: DEFAULT_INTERRUPT_TIMEOUT,
            terminate_timeout: DEFAULT_TERMINATE_TIMEOUT,
            termination_timeouts_reason: DEFAULT_TERMINATION_TIMEOUTS_REASON.to_owned(),
            forward_logs: true,
            extra_env: Vec::new(),
        }
    }

    /// Set a descriptive app name used in logs and errors.
    #[must_use]
    pub fn with_app_name(mut self, app_name: impl Into<String>) -> Self {
        self.app_name = app_name.into();
        self
    }

    /// Select `cargo leptos serve` or `cargo leptos watch`.
    #[must_use]
    pub const fn with_mode(mut self, mode: CargoLeptosMode) -> Self {
        self.mode = mode;
        self
    }

    /// Override the cargo binary used to invoke `cargo leptos`.
    ///
    /// Useful for selecting a vendored toolchain or a `cargo +channel` proxy. If unset, the
    /// `CARGO` environment variable is honored when present; otherwise the default `cargo` on
    /// `PATH` is used.
    #[must_use]
    pub fn with_cargo(mut self, cargo_bin: impl Into<OsString>) -> Self {
        self.cargo_bin = Some(cargo_bin.into());
        self
    }

    /// Set the browser-facing URL scheme used by [`LeptosTestApp::base_url`].
    ///
    /// This does not configure TLS for the Leptos process; it only controls the URL returned to
    /// browser tests and the default startup line expected in stdout.
    #[must_use]
    pub const fn with_site_scheme(mut self, site_scheme: SiteScheme) -> Self {
        self.site_scheme = site_scheme;
        self
    }

    /// Bind the Leptos app to a fixed site address such as `127.0.0.1:3000`.
    ///
    /// If not set, a free localhost port is selected.
    #[must_use]
    pub fn with_site_addr(mut self, site_addr: impl Into<String>) -> Self {
        self.site_addr = Some(site_addr.into());
        self
    }

    /// Use a fixed reload port.
    ///
    /// If not set, a free localhost port is selected.
    #[must_use]
    pub const fn with_reload_port(mut self, reload_port: u16) -> Self {
        self.reload_port = Some(reload_port);
        self
    }

    /// Override the stdout line fragment that marks the app as ready.
    #[must_use]
    pub fn with_startup_line(mut self, startup_line: impl Into<String>) -> Self {
        self.startup_line = Some(startup_line.into());
        self
    }

    /// Set the startup timeout, with a `reason` describing *why* this value was chosen.
    ///
    /// The reason is logged at startup and embedded in
    /// [`LeptosBrowserTestError::StartupTimedOut`](crate::LeptosBrowserTestError::StartupTimedOut)
    /// so a future debugger sees the rationale alongside the elapsed duration. Forcing the
    /// argument prevents a stale source comment from being the only record of why a number
    /// was tuned.
    #[must_use]
    pub fn with_startup_timeout(mut self, timeout: Duration, reason: impl Into<String>) -> Self {
        self.startup_timeout = timeout;
        self.startup_timeout_reason = reason.into();
        self
    }

    /// Set how many recent stdout/stderr lines are retained for failure diagnostics.
    #[must_use]
    pub const fn with_startup_log_tail_lines(mut self, lines: usize) -> Self {
        self.startup_log_tail_lines = lines;
        self
    }

    /// Set graceful process termination timeouts, with a `reason` describing *why* these
    /// values were chosen.
    ///
    /// The reason is logged when the child is dropped. Forcing the argument keeps the
    /// rationale next to the numbers instead of in a soon-to-rot source comment.
    #[must_use]
    pub fn with_termination_timeouts(
        mut self,
        interrupt_timeout: Duration,
        terminate_timeout: Duration,
        reason: impl Into<String>,
    ) -> Self {
        self.interrupt_timeout = interrupt_timeout;
        self.terminate_timeout = terminate_timeout;
        self.termination_timeouts_reason = reason.into();
        self
    }

    /// Add an environment variable for the `cargo leptos` process.
    ///
    /// Calls are last-write-wins: repeated `with_env` invocations for the same key override
    /// earlier values when the child is spawned. `with_env` is also applied *after* the
    /// framework env (`LEPTOS_SITE_ADDR`, `LEPTOS_RELOAD_PORT`, `RUST_BACKTRACE`), so it can
    /// be used as an escape hatch to override those.
    #[must_use]
    pub fn with_env(mut self, key: impl AsRef<OsStr>, value: impl AsRef<OsStr>) -> Self {
        self.extra_env
            .push((key.as_ref().to_owned(), value.as_ref().to_owned()));
        self
    }

    /// Forward each captured stdout/stderr line to the parent process's stdout/stderr.
    ///
    /// Defaults to `true` to keep the historical behavior. Set to `false` to silence the
    /// child's logs while still capturing the startup tail used in failure diagnostics.
    #[must_use]
    pub const fn with_forward_logs(mut self, forward_logs: bool) -> Self {
        self.forward_logs = forward_logs;
        self
    }

    /// Start the configured Leptos test app.
    ///
    /// The returned [`LeptosTestApp`] terminates the `cargo leptos` process when dropped. Drop-based
    /// termination uses `tokio_process_tools::TerminateOnDrop`, so tests must run inside a
    /// multi-threaded Tokio runtime. Use `#[tokio::test(flavor = "multi_thread")]` for Tokio browser
    /// tests.
    ///
    /// # Errors
    ///
    /// Returns an error if the app directory cannot be resolved, the process cannot be spawned, or
    /// the expected startup line is not observed before the timeout.
    pub async fn start(self) -> Result<LeptosTestApp, Report<LeptosBrowserTestError>> {
        crate::app::start_configured_app(self).await
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use assertr::prelude::*;

    use super::{
        DEFAULT_INTERRUPT_TIMEOUT, DEFAULT_STARTUP_LOG_TAIL_LINES, DEFAULT_STARTUP_TIMEOUT,
        DEFAULT_STARTUP_TIMEOUT_REASON, DEFAULT_TERMINATE_TIMEOUT,
        DEFAULT_TERMINATION_TIMEOUTS_REASON, LeptosTestAppConfig,
    };
    use crate::{CargoLeptosMode, SiteScheme};

    #[test]
    fn new_uses_documented_defaults() {
        let config = LeptosTestAppConfig::new("./test-app");

        assert_that!(config.app_name).is_equal_to("Leptos test app");
        assert_that!(config.mode).is_equal_to(CargoLeptosMode::Serve);
        assert_that!(config.cargo_bin).is_equal_to(None);
        assert_that!(config.site_scheme).is_equal_to(SiteScheme::Http);
        assert_that!(config.site_addr).is_equal_to(None);
        assert_that!(config.reload_port).is_equal_to(None);
        assert_that!(config.startup_line).is_equal_to(None);
        assert_that!(config.startup_timeout).is_equal_to(DEFAULT_STARTUP_TIMEOUT);
        assert_that!(config.startup_timeout_reason).is_equal_to(DEFAULT_STARTUP_TIMEOUT_REASON);
        assert_that!(config.startup_log_tail_lines).is_equal_to(DEFAULT_STARTUP_LOG_TAIL_LINES);
        assert_that!(config.interrupt_timeout).is_equal_to(DEFAULT_INTERRUPT_TIMEOUT);
        assert_that!(config.terminate_timeout).is_equal_to(DEFAULT_TERMINATE_TIMEOUT);
        assert_that!(config.termination_timeouts_reason)
            .is_equal_to(DEFAULT_TERMINATION_TIMEOUTS_REASON);
        assert_that!(config.forward_logs).is_true();
        assert_that!(config.extra_env.is_empty()).is_true();
    }

    #[test]
    fn default_constants_match_documented_values() {
        assert_that!(DEFAULT_STARTUP_LOG_TAIL_LINES).is_equal_to(200);
        assert_that!(DEFAULT_STARTUP_TIMEOUT).is_equal_to(Duration::from_secs(60 * 10));
        assert_that!(DEFAULT_INTERRUPT_TIMEOUT).is_equal_to(Duration::from_secs(3));
        assert_that!(DEFAULT_TERMINATE_TIMEOUT).is_equal_to(Duration::from_secs(8));
    }

    #[test]
    fn setters_override_defaults() {
        let config = LeptosTestAppConfig::new("./test-app")
            .with_app_name("custom")
            .with_mode(CargoLeptosMode::Watch)
            .with_cargo("/opt/cargo")
            .with_site_scheme(SiteScheme::Https)
            .with_site_addr("127.0.0.1:4000")
            .with_reload_port(4001)
            .with_startup_line("ready")
            .with_startup_timeout(
                Duration::from_secs(5),
                "tight bound for unit-style smoke test",
            )
            .with_startup_log_tail_lines(10)
            .with_termination_timeouts(
                Duration::from_millis(100),
                Duration::from_millis(500),
                "test fixture exits immediately on SIGTERM",
            )
            .with_forward_logs(false)
            .with_env("FOO", "bar");

        assert_that!(config.app_name).is_equal_to("custom");
        assert_that!(config.mode).is_equal_to(CargoLeptosMode::Watch);
        assert_that!(config.cargo_bin).is_equal_to(Some(std::ffi::OsString::from("/opt/cargo")));
        assert_that!(config.site_scheme).is_equal_to(SiteScheme::Https);
        assert_that!(config.site_addr).is_equal_to(Some("127.0.0.1:4000".to_owned()));
        assert_that!(config.reload_port).is_equal_to(Some(4001));
        assert_that!(config.startup_line).is_equal_to(Some("ready".to_owned()));
        assert_that!(config.startup_timeout).is_equal_to(Duration::from_secs(5));
        assert_that!(config.startup_timeout_reason)
            .is_equal_to("tight bound for unit-style smoke test");
        assert_that!(config.startup_log_tail_lines).is_equal_to(10);
        assert_that!(config.interrupt_timeout).is_equal_to(Duration::from_millis(100));
        assert_that!(config.terminate_timeout).is_equal_to(Duration::from_millis(500));
        assert_that!(config.termination_timeouts_reason)
            .is_equal_to("test fixture exits immediately on SIGTERM");
        assert_that!(config.forward_logs).is_false();
        assert_that!(config.extra_env.len()).is_equal_to(1);
    }
}
