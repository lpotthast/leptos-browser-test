use crate::{
    LeptosBrowserTestError, LeptosTestAppConfig, cargo_leptos,
    error::StartupFailureContext,
    ports,
    site::{format_base_url, parse_socket_addr},
    startup::StartupLogs,
};
use rootcause::{IntoReport, Report, bail, prelude::ResultExt};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use tokio::io::AsyncWrite;
use tokio_process_tools::{
    AutoName, BroadcastOutputStream, Consumable, Consumer, DEFAULT_MAX_BUFFERED_CHUNKS,
    DEFAULT_READ_CHUNK_SIZE, GracefulShutdown, LineParsingOptions, Next, NumBytesExt, ParseLines,
    Process, ReliableWithBackpressure, ReplayEnabled, StreamReadError, TerminateOnDrop,
    WaitForLineResult,
};
use unwrap_infallible::UnwrapInfallible;

/// A running Leptos test app process.
///
/// The app process is terminated automatically when this value is dropped. This relies on
/// [`TerminateOnDrop`], which requires an active multithreaded Tokio runtime.
/// Browser tests should use `#[tokio::test(flavor = "multi_thread")]`.
pub struct LeptosTestApp {
    _process: TerminateOnDrop<BroadcastOutputStream<ReliableWithBackpressure, ReplayEnabled>>,
    _stdout_replay: Consumer<()>,
    _stderr_replay: Consumer<()>,
    base_url: String,
    site_addr: String,
    reload_port: u16,
    app_dir: PathBuf,
}

impl LeptosTestApp {
    /// Start a test app with the default config.
    ///
    /// # Errors
    ///
    /// Returns an error if startup fails.
    pub async fn serve(
        app_dir: impl Into<PathBuf>,
    ) -> Result<Self, Report<LeptosBrowserTestError>> {
        LeptosTestAppConfig::new(app_dir).start().await
    }

    /// The base URL, for example `http://127.0.0.1:3000` or `https://127.0.0.1:3000`.
    #[must_use]
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// The bound site address, for example `127.0.0.1:3000`.
    #[must_use]
    pub fn site_addr(&self) -> &str {
        &self.site_addr
    }

    /// The reload port passed through `LEPTOS_RELOAD_PORT`.
    #[must_use]
    pub const fn reload_port(&self) -> u16 {
        self.reload_port
    }

    /// The canonical app directory.
    #[must_use]
    pub fn app_dir(&self) -> &Path {
        &self.app_dir
    }
}

impl std::fmt::Debug for LeptosTestApp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LeptosTestApp")
            .field("base_url", &self.base_url)
            .field("site_addr", &self.site_addr)
            .field("reload_port", &self.reload_port)
            .field("app_dir", &self.app_dir)
            .finish_non_exhaustive()
    }
}

/// Resolved view of a `LeptosTestAppConfig` after path canonicalization, port allocation,
/// and startup-line/base-URL derivation. Recomputed for each spawn attempt.
struct RuntimeConfig {
    app_dir: PathBuf,
    site_addr: String,
    reload_port: u16,
    base_url: String,
    startup_line: String,
}

/// Live process plus the log buffers and replay handles tied to it.
struct SpawnedProcess {
    process: TerminateOnDrop<BroadcastOutputStream<ReliableWithBackpressure, ReplayEnabled>>,
    stdout_replay: Consumer<()>,
    stderr_replay: Consumer<()>,
    logs: StartupLogs,
}

/// Maximum number of times we restart cargo-leptos on a port-bind collision before giving up.
///
/// Only collisions on an auto-allocated port are retried. If the user pinned the colliding port,
/// that's a configuration error, not a race, and we don't retry it.
const MAX_PORT_COLLISION_RETRIES: u32 = 3;

/// Which port a startup attempt failed to bind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PortCollision {
    /// The app could not bind its site address.
    Site,
    /// cargo-leptos (watch mode) could not bind the reload port.
    Reload,
}

impl PortCollision {
    /// Classifies a single output line as a port-bind collision, if it is one.
    fn detect(line: &str) -> Option<Self> {
        let lowered = line.to_ascii_lowercase();
        // cargo-leptos probes the reload port before binding it and reports
        // "Reload TCP port 127.0.0.1:3001 already in use. ..." when it is taken.
        if lowered.contains("reload tcp port") && lowered.contains("already in use") {
            return Some(Self::Reload);
        }
        // Linux/macOS: "Address already in use" / "address already in use (os error 48)"
        // Windows:     "Only one usage of each socket address (protocol/network address/port)"
        if lowered.contains("address already in use")
            || lowered.contains("only one usage of each socket address")
        {
            return Some(Self::Site);
        }
        None
    }

    fn port_name(self) -> &'static str {
        match self {
            Self::Site => "site",
            Self::Reload => "reload",
        }
    }
}

/// Port collisions that abort the current startup attempt so it can be retried with fresh ports.
#[derive(Debug, Clone, Copy)]
struct RetryPolicy {
    site: bool,
    reload: bool,
}

impl RetryPolicy {
    fn for_attempt(config: &LeptosTestAppConfig, attempt: u32, max_attempts: u32) -> Self {
        let can_retry = attempt < max_attempts;
        Self {
            site: can_retry && config.site_addr.is_none(),
            reload: can_retry && config.reload_port.is_none(),
        }
    }

    fn retries(self, collision: PortCollision) -> bool {
        match collision {
            PortCollision::Site => self.site,
            PortCollision::Reload => self.reload,
        }
    }
}

/// How a startup attempt that did not fail ended.
enum StartupOutcome {
    Ready,
    PortCollision(PortCollision),
}

pub(crate) async fn start_configured_app(
    config: LeptosTestAppConfig,
) -> Result<LeptosTestApp, Report<LeptosBrowserTestError>> {
    let max_attempts = MAX_PORT_COLLISION_RETRIES + 1;
    let mut attempt = 1;

    // Only ever exits via `return`. The final attempt's `RetryPolicy` retries nothing, so
    // `wait_for_ready` can't report a collision for it.
    loop {
        let retry_policy = RetryPolicy::for_attempt(&config, attempt, max_attempts);
        let runtime = resolve_runtime_config(&config)?;
        let spawned = spawn_with_log_capture(&runtime, &config)?;
        match wait_for_ready(&spawned, &runtime, &config, retry_policy).await? {
            StartupOutcome::Ready => {
                tracing::info!("{} started at {}", config.app_name, runtime.base_url);
                return Ok(build_app(spawned, runtime));
            }
            StartupOutcome::PortCollision(collision) => {
                tracing::warn!(
                    "{} {} port collision on attempt {attempt}/{max_attempts}, retrying with fresh ports",
                    config.app_name,
                    collision.port_name(),
                );
                drop(spawned);
                attempt += 1;
            }
        }
    }
}

fn resolve_runtime_config(
    config: &LeptosTestAppConfig,
) -> Result<RuntimeConfig, Report<LeptosBrowserTestError>> {
    let app_dir =
        config
            .app_dir
            .canonicalize()
            .context_with(|| LeptosBrowserTestError::ResolveAppDir {
                app_name: config.app_name.clone(),
                app_dir: config.app_dir.clone(),
            })?;

    let site_addr = if let Some(addr) = config.site_addr.as_deref() {
        if parse_socket_addr(addr).is_none() {
            bail!(LeptosBrowserTestError::InvalidSiteAddr {
                app_name: config.app_name.clone(),
                site_addr: addr.to_owned(),
            });
        }
        addr.to_owned()
    } else {
        let port =
            ports::find_free_port().context_with(|| LeptosBrowserTestError::FindFreeSitePort {
                app_name: config.app_name.clone(),
            })?;
        format!("127.0.0.1:{port}")
    };
    let site_port = parse_socket_addr(&site_addr).map(|sa| sa.port());
    let reload_port = match config.reload_port {
        Some(reload_port) => reload_port,
        None => ports::find_free_port_excluding(site_port).context_with(|| {
            LeptosBrowserTestError::FindFreeReloadPort {
                app_name: config.app_name.clone(),
            }
        })?,
    };
    let base_url = format_base_url(config.site_scheme, &site_addr);
    let startup_line = config
        .startup_line
        .clone()
        .unwrap_or_else(|| format!("listening on {base_url}"));

    Ok(RuntimeConfig {
        app_dir,
        site_addr,
        reload_port,
        base_url,
        startup_line,
    })
}

fn spawn_with_log_capture(
    runtime: &RuntimeConfig,
    config: &LeptosTestAppConfig,
) -> Result<SpawnedProcess, Report<LeptosBrowserTestError>> {
    tracing::info!(
        graceful_shutdown_timeout = ?config.graceful_shutdown_timeout,
        graceful_shutdown_unix_signal = ?config.graceful_shutdown_unix_signal,
        "Starting {} in {:?} on {} (reload port {}).",
        config.app_name,
        runtime.app_dir,
        runtime.site_addr,
        runtime.reload_port,
    );

    let cmd = cargo_leptos::command(
        config.mode,
        config.build_profile,
        config.cargo_bin.as_deref(),
        &runtime.app_dir,
        &runtime.site_addr,
        runtime.reload_port,
        config.graceful_shutdown_timeout,
        config.graceful_shutdown_unix_signal,
        &config.extra_env,
    );

    // The graceful shutdown timeout for cargo-leptos itself must be greater (or at least equal to)
    // the timeout that the user specified for his application (enforced by cargo-leptos).
    let timeout = config.graceful_shutdown_timeout + Duration::from_secs(10);
    // We know that cargo-leptos listens for these signals.
    let graceful_shutdown = GracefulShutdown::builder()
        .unix_sigint(timeout)
        .windows_ctrl_break(timeout)
        .build();

    let process = Process::new(cmd)
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
        .context_with(|| LeptosBrowserTestError::SpawnCargoLeptos {
            app_name: config.app_name.clone(),
            mode: config.mode,
        })?
        .terminate_on_drop(graceful_shutdown);

    let logs = StartupLogs::new(config.startup_log_tail_lines);
    let forward_logs = config.forward_logs;

    #[allow(clippy::items_after_statements)]
    async fn write_to<W: AsyncWrite + Unpin>(mut to: W, data: &str) -> tokio::io::Result<()> {
        use tokio::io::AsyncWriteExt;
        to.write_all(data.as_bytes()).await?;
        to.write_all(b"\n").await?;
        to.flush().await?;
        Ok(())
    }

    // Let's forward captured stdout/stderr lines to the output of our process. We do this
    // asynchronously using the tokio::io::std{out|err}() handles, as writing to
    // stdout/stderr directly using print!() could result in unhandled "failed printing to
    // stdout: Resource temporarily unavailable" errors should the cargo-leptos output be
    // consumed too slowly. This can happen because tokio puts the stdio fds into
    // non-blocking mode (once touched) and std print! has no support for that, they just
    // panic when an EAGAIN error is observed. Tokio's stdio handles instead asynchronously
    // wait internally, handling the slow drainage and preventing a blocked runtime.
    let stdout_buffer = logs.stdout.clone();
    let stdout_replay = process
        .stdout()
        .consume_async(ParseLines::inspect_async(
            LineParsingOptions::default(),
            move |line| {
                stdout_buffer.push(&line);
                let line = line.to_string();
                async move {
                    if forward_logs && let Err(err) = write_to(tokio::io::stdout(), &line).await {
                        tracing::error!("Could not forward server process output to stdout: {err}");
                    }
                    Next::Continue
                }
            },
        ))
        .unwrap_infallible();

    let stderr_buffer = logs.stderr.clone();
    let stderr_replay = process
        .stderr()
        .consume_async(ParseLines::inspect_async(
            LineParsingOptions::default(),
            move |line| {
                stderr_buffer.push(&line);
                let line = line.to_string();
                async move {
                    if forward_logs && let Err(err) = write_to(tokio::io::stderr(), &line).await {
                        tracing::error!("Could not forward server process output to stderr: {err}");
                    }
                    Next::Continue
                }
            },
        ))
        .unwrap_infallible();

    Ok(SpawnedProcess {
        process,
        stdout_replay,
        stderr_replay,
        logs,
    })
}

async fn wait_for_ready(
    spawned: &SpawnedProcess,
    runtime: &RuntimeConfig,
    config: &LeptosTestAppConfig,
    retry_policy: RetryPolicy,
) -> Result<StartupOutcome, Report<LeptosBrowserTestError>> {
    tracing::info!(
        "Waiting {:?} ({}) for {} to start...",
        config.startup_timeout,
        config.startup_timeout_reason,
        config.app_name,
    );

    let startup_waiter = wait_for_startup_line(spawned, runtime, config, retry_policy);
    spawned.process.seal_output_replay();

    match startup_waiter.await {
        Ok(StartupWait::Ready) => Ok(StartupOutcome::Ready),
        Ok(StartupWait::PortCollision(collision)) => Ok(StartupOutcome::PortCollision(collision)),
        Ok(StartupWait::StreamClosed) => {
            bail!(LeptosBrowserTestError::StartupStdoutClosed(
                startup_failure_context(&config.app_name, &runtime.startup_line, &spawned.logs),
            ));
        }
        Ok(StartupWait::Timeout) => {
            bail!(LeptosBrowserTestError::StartupTimedOut {
                ctx: startup_failure_context(
                    &config.app_name,
                    &runtime.startup_line,
                    &spawned.logs
                ),
                timeout: config.startup_timeout,
                reason: config.startup_timeout_reason.clone(),
            });
        }
        Err(err) => {
            let err: Report<StreamReadError> = err.into_report();
            Err(
                err.context(LeptosBrowserTestError::StreamRead(startup_failure_context(
                    &config.app_name,
                    &runtime.startup_line,
                    &spawned.logs,
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
/// line, or for a port collision that `retry` allows retrying.
///
/// Deliberately not an `async fn`: the subscriptions must exist before the caller seals the
/// output replay, but an `async fn` body would only run (and subscribe) on first poll.
fn wait_for_startup_line(
    spawned: &SpawnedProcess,
    runtime: &RuntimeConfig,
    config: &LeptosTestAppConfig,
    retry_policy: RetryPolicy,
) -> impl Future<Output = Result<StartupWait, StreamReadError>> {
    let (stdout_waiter, stdout_collision) =
        wait_for_line_or_collision(spawned.process.stdout(), runtime, config, retry_policy);
    let (stderr_waiter, stderr_collision) =
        wait_for_line_or_collision(spawned.process.stderr(), runtime, config, retry_policy);

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

/// Waits on one output stream for the startup line or a collision that `retry` allows retrying.
/// The returned slot is set when the wait stopped because of such a collision.
fn wait_for_line_or_collision(
    stream: &BroadcastOutputStream<ReliableWithBackpressure, ReplayEnabled>,
    runtime: &RuntimeConfig,
    config: &LeptosTestAppConfig,
    retry_policy: RetryPolicy,
) -> (
    impl Future<Output = Result<WaitForLineResult, StreamReadError>>,
    Arc<OnceLock<PortCollision>>,
) {
    let expected_line = runtime.startup_line.clone();
    let collision = Arc::new(OnceLock::new());
    let collision_slot = Arc::clone(&collision);
    let waiter = stream.wait_for_line(
        config.startup_timeout,
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

fn build_app(spawned: SpawnedProcess, runtime: RuntimeConfig) -> LeptosTestApp {
    LeptosTestApp {
        _process: spawned.process,
        _stdout_replay: spawned.stdout_replay,
        _stderr_replay: spawned.stderr_replay,
        base_url: runtime.base_url,
        site_addr: runtime.site_addr,
        reload_port: runtime.reload_port,
        app_dir: runtime.app_dir,
    }
}

fn startup_failure_context(
    app_name: &str,
    expected_line: &str,
    logs: &StartupLogs,
) -> StartupFailureContext {
    StartupFailureContext {
        app_name: app_name.to_owned(),
        expected_line: expected_line.to_owned(),
        stdout_tail: logs.stdout_tail(),
        stderr_tail: logs.stderr_tail(),
    }
}

#[cfg(test)]
mod tests {
    use assertr::prelude::*;

    use super::{PortCollision, RetryPolicy};
    use crate::LeptosTestAppConfig;

    #[test]
    fn detects_unix_address_already_in_use() {
        assert_that!(PortCollision::detect(
            "Error: Address already in use (os error 48)"
        ))
        .is_equal_to(Some(PortCollision::Site));
    }

    #[test]
    fn detects_lowercase_address_already_in_use() {
        assert_that!(PortCollision::detect(
            "thread 'main' panicked: address already in use"
        ))
        .is_equal_to(Some(PortCollision::Site));
    }

    #[test]
    fn detects_windows_phrasing() {
        assert_that!(PortCollision::detect(
            "Only one usage of each socket address (protocol/network address/port) is normally permitted"
        ))
        .is_equal_to(Some(PortCollision::Site));
    }

    #[test]
    fn detects_cargo_leptos_reload_port_collision() {
        assert_that!(PortCollision::detect(
            "Reload TCP port 127.0.0.1:3001 already in use. You can set the port in the server integration's RenderOptions reload_port"
        ))
        .is_equal_to(Some(PortCollision::Reload));
    }

    #[test]
    fn rejects_unrelated_errors() {
        assert_that!(PortCollision::detect(
            "error: linking with `cc` failed: exit status: 1"
        ))
        .is_equal_to(None);
        assert_that!(PortCollision::detect("")).is_equal_to(None);
    }

    #[test]
    fn retries_collisions_on_auto_allocated_ports() {
        let config = LeptosTestAppConfig::new("./test-app");
        let retry = RetryPolicy::for_attempt(&config, 1, 4);

        assert_that!(retry.retries(PortCollision::Site)).is_true();
        assert_that!(retry.retries(PortCollision::Reload)).is_true();
    }

    #[test]
    fn does_not_retry_collisions_on_pinned_ports() {
        let site_pinned = LeptosTestAppConfig::new("./test-app").with_site_addr("127.0.0.1:3000");
        let retry = RetryPolicy::for_attempt(&site_pinned, 1, 4);
        assert_that!(retry.retries(PortCollision::Site)).is_false();
        assert_that!(retry.retries(PortCollision::Reload)).is_true();

        let reload_pinned = LeptosTestAppConfig::new("./test-app").with_reload_port(3001);
        let retry = RetryPolicy::for_attempt(&reload_pinned, 1, 4);
        assert_that!(retry.retries(PortCollision::Site)).is_true();
        assert_that!(retry.retries(PortCollision::Reload)).is_false();
    }

    #[test]
    fn does_not_retry_on_the_final_attempt() {
        let config = LeptosTestAppConfig::new("./test-app");
        let retry = RetryPolicy::for_attempt(&config, 4, 4);

        assert_that!(retry.retries(PortCollision::Site)).is_false();
        assert_that!(retry.retries(PortCollision::Reload)).is_false();
    }
}
