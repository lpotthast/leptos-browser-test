use crate::{
    LeptosBrowserTestError, LeptosTestAppConfig, cargo_leptos,
    error::StartupFailureContext,
    ports,
    site::{format_base_url, parse_socket_addr},
    startup::StartupLogs,
};
use rootcause::{IntoReport, Report, bail, prelude::ResultExt};
use std::path::{Path, PathBuf};
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
/// Only applies when the caller did not pin both `site_addr` and `reload_port`; if the user
/// pinned a port and the spawn fails because the port is taken, that's a configuration error,
/// not a race, and we surface it on the first attempt.
const MAX_PORT_COLLISION_RETRIES: u32 = 3;

pub(crate) async fn start_configured_app(
    config: LeptosTestAppConfig,
) -> Result<LeptosTestApp, Report<LeptosBrowserTestError>> {
    let auto_allocated = config.site_addr.is_none() || config.reload_port.is_none();
    let max_attempts = if auto_allocated {
        MAX_PORT_COLLISION_RETRIES
    } else {
        1
    };

    for attempt in 1..=max_attempts {
        let runtime = resolve_runtime_config(&config)?;
        let spawned = spawn_with_log_capture(&runtime, &config)?;
        match wait_for_ready(&spawned, &runtime, &config).await {
            Ok(()) => {
                tracing::info!("{} started at {}", config.app_name, runtime.base_url);
                return Ok(build_app(spawned, runtime));
            }
            Err(err) if attempt < max_attempts && is_port_collision(&err) => {
                tracing::warn!(
                    "{} port collision on attempt {attempt}/{max_attempts}; retrying with fresh ports",
                    config.app_name,
                );
                drop(spawned);
            }
            Err(err) => return Err(err),
        }
    }

    // The retry loop always exits via `return`; this branch is unreachable but keeps the type
    // checker happy without a panic.
    unreachable!("start_configured_app retry loop must exit via return")
}

fn is_port_collision(err: &Report<LeptosBrowserTestError>) -> bool {
    let (LeptosBrowserTestError::StartupTimedOut { ctx, .. }
    | LeptosBrowserTestError::StartupStdoutClosed(ctx)
    | LeptosBrowserTestError::StreamRead(ctx)) = err.current_context()
    else {
        return false;
    };
    stderr_indicates_port_collision(&ctx.stderr_tail)
        || stderr_indicates_port_collision(&ctx.stdout_tail)
}

fn stderr_indicates_port_collision(text: &str) -> bool {
    let lowered = text.to_ascii_lowercase();
    // Linux/macOS: "Address already in use" / "address already in use (os error 48)"
    // Windows:     "Only one usage of each socket address (protocol/network address/port)"
    lowered.contains("address already in use")
        || lowered.contains("only one usage of each socket address")
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
) -> Result<(), Report<LeptosBrowserTestError>> {
    tracing::info!(
        "Waiting {:?} ({}) for {} to start...",
        config.startup_timeout,
        config.startup_timeout_reason,
        config.app_name,
    );

    let startup_waiter = wait_for_startup_line(spawned, runtime, config);
    spawned.process.seal_output_replay();

    match startup_waiter.await {
        Ok(WaitForLineResult::Matched) => Ok(()),
        Ok(WaitForLineResult::StreamClosed) => {
            bail!(LeptosBrowserTestError::StartupStdoutClosed(
                startup_failure_context(&config.app_name, &runtime.startup_line, &spawned.logs),
            ));
        }
        Ok(WaitForLineResult::Timeout) => {
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

async fn wait_for_startup_line(
    spawned: &SpawnedProcess,
    runtime: &RuntimeConfig,
    config: &LeptosTestAppConfig,
) -> Result<WaitForLineResult, StreamReadError> {
    let stdout_expected_line = runtime.startup_line.clone();
    let stdout_waiter = spawned.process.stdout().wait_for_line(
        config.startup_timeout,
        move |line| line.contains(&stdout_expected_line),
        LineParsingOptions::default(),
    );

    let stderr_expected_line = runtime.startup_line.clone();
    let stderr_waiter = spawned.process.stderr().wait_for_line(
        config.startup_timeout,
        move |line| line.contains(&stderr_expected_line),
        LineParsingOptions::default(),
    );

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

        if stdout_result == Some(WaitForLineResult::Matched)
            || stderr_result == Some(WaitForLineResult::Matched)
        {
            return Ok(WaitForLineResult::Matched);
        }

        match (stdout_result, stderr_result) {
            (Some(WaitForLineResult::StreamClosed), Some(WaitForLineResult::StreamClosed)) => {
                return Ok(WaitForLineResult::StreamClosed);
            }
            (Some(stdout), Some(stderr)) => {
                debug_assert!(
                    matches!(stdout, WaitForLineResult::Timeout)
                        || matches!(stderr, WaitForLineResult::Timeout)
                );
                return Ok(WaitForLineResult::Timeout);
            }
            _ => {}
        }
    }
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
    use std::time::Duration;

    use assertr::prelude::*;
    use rootcause::Report;

    use super::{
        LeptosBrowserTestError, StartupFailureContext, is_port_collision,
        stderr_indicates_port_collision,
    };

    fn ctx_with_stderr(stderr: &str) -> StartupFailureContext {
        StartupFailureContext {
            app_name: "demo".to_owned(),
            expected_line: "listening on".to_owned(),
            stdout_tail: String::new(),
            stderr_tail: stderr.to_owned(),
        }
    }

    #[test]
    fn detects_unix_address_already_in_use() {
        assert_that!(stderr_indicates_port_collision(
            "Error: Address already in use (os error 48)"
        ))
        .is_true();
    }

    #[test]
    fn detects_lowercase_address_already_in_use() {
        assert_that!(stderr_indicates_port_collision(
            "thread 'main' panicked: address already in use"
        ))
        .is_true();
    }

    #[test]
    fn detects_windows_phrasing() {
        assert_that!(stderr_indicates_port_collision(
            "Only one usage of each socket address (protocol/network address/port) is normally permitted"
        ))
        .is_true();
    }

    #[test]
    fn rejects_unrelated_errors() {
        assert_that!(stderr_indicates_port_collision(
            "error: linking with `cc` failed: exit status: 1"
        ))
        .is_false();
        assert_that!(stderr_indicates_port_collision("")).is_false();
    }

    #[test]
    fn is_port_collision_recognizes_startup_timed_out() {
        let report = Report::new(LeptosBrowserTestError::StartupTimedOut {
            ctx: ctx_with_stderr("Address already in use"),
            timeout: Duration::from_secs(5),
            reason: "test".to_owned(),
        });
        assert_that!(is_port_collision(&report)).is_true();
    }

    #[test]
    fn is_port_collision_recognizes_stdout_closed() {
        let report = Report::new(LeptosBrowserTestError::StartupStdoutClosed(
            ctx_with_stderr("address already in use"),
        ));
        assert_that!(is_port_collision(&report)).is_true();
    }

    #[test]
    fn is_port_collision_ignores_unrelated_variants() {
        let report = Report::new(LeptosBrowserTestError::FindFreeSitePort {
            app_name: "demo".to_owned(),
        });
        assert_that!(is_port_collision(&report)).is_false();
    }

    #[test]
    fn is_port_collision_ignores_startup_with_clean_stderr() {
        let report = Report::new(LeptosBrowserTestError::StartupTimedOut {
            ctx: ctx_with_stderr("compilation error: ..."),
            timeout: Duration::from_secs(5),
            reason: "test".to_owned(),
        });
        assert_that!(is_port_collision(&report)).is_false();
    }
}
