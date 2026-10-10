use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use rootcause::{Report, bail, prelude::ResultExt};
use tokio::io::AsyncWrite;
use tokio_process_tools::{
    AutoName, Consumable, Consumer, DEFAULT_MAX_BUFFERED_CHUNKS, DEFAULT_READ_CHUNK_SIZE,
    GracefulShutdown, LineParsingOptions, Next, NumBytesExt, ParseLines, Process, TerminateOnDrop,
};
use unwrap_infallible::UnwrapInfallible;

use crate::{
    LeptosBrowserTestError, LeptosTestAppConfig, cargo_leptos, ports,
    site::{format_base_url, parse_socket_addr},
    startup::{OutputStream, RetryPolicy, StartupExpectation, StartupOutcome, wait_for_ready},
};

/// A running Leptos test app process.
///
/// The app process is terminated automatically when this value is dropped. This relies on
/// [`TerminateOnDrop`], which requires an active multithreaded Tokio runtime.
/// Browser tests should use `#[tokio::test(flavor = "multi_thread")]`.
pub struct LeptosTestApp {
    _process: TerminateOnDrop<OutputStream>,
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

/// Live process plus the replay handles tied to it.
struct SpawnedProcess {
    process: TerminateOnDrop<OutputStream>,
    stdout_replay: Consumer<()>,
    stderr_replay: Consumer<()>,
}

pub(crate) async fn start_configured_app(
    config: LeptosTestAppConfig,
) -> Result<LeptosTestApp, Report<LeptosBrowserTestError>> {
    let mut attempt = 1;

    // Only ever exits via `return`. The final attempt's `RetryPolicy` retries nothing, so
    // `wait_for_ready` turns any collision on it into an error instead of a retry.
    loop {
        let retry_policy = RetryPolicy::for_attempt(&config, attempt);
        let runtime = resolve_runtime_config(&config)?;
        let spawned = spawn_with_log_capture(&runtime, &config)?;
        let expected = StartupExpectation {
            app_name: &config.app_name,
            startup_line: &runtime.startup_line,
            timeout: config.startup_timeout,
            timeout_reason: &config.startup_timeout_reason,
            site_addr: &runtime.site_addr,
            reload_port: runtime.reload_port,
            mode: config.mode,
            log_tail_lines: config.startup_log_tail_lines,
        };
        let outcome = wait_for_ready(&spawned.process, &expected, retry_policy).await?;
        match outcome {
            StartupOutcome::Ready => {
                tracing::info!("{} started at {}", config.app_name, runtime.base_url);
                return Ok(build_app(spawned, runtime));
            }
            StartupOutcome::PortCollision(collision) => {
                tracing::warn!(
                    "{} {} port collision on attempt {attempt}/{}, retrying with fresh ports",
                    config.app_name,
                    collision.port_name(),
                    RetryPolicy::MAX_ATTEMPTS,
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
    let stdout_replay = process
        .stdout()
        .consume_async(ParseLines::inspect_async(
            LineParsingOptions::default(),
            move |line| {
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

    let stderr_replay = process
        .stderr()
        .consume_async(ParseLines::inspect_async(
            LineParsingOptions::default(),
            move |line| {
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
    })
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
