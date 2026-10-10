# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.4.0] - 2026-10-10

### Added

- `LeptosTestAppConfig::with_build_profile(BuildProfile::Release)` builds the app with `cargo leptos serve --release`
  (or `watch --release`). Defaults to `BuildProfile::Dev`. See the README's "Build profile".
- `LeptosTestApp` now implements `Debug`, showing its base URL, site address, reload port, and app directory.
- `CapturedOutput`, the recent stdout/stderr of `cargo leptos`, attached to every report of a startup failure that
  happened once `cargo leptos` was running. Find it with `CapturedOutput::find_in(&report)` (any report type) or
  `CapturedOutput::find_in_ref(report_ref)`, even after further context was added. Printing the whole report still
  shows it.
- `LeptosBrowserTestError::PortInUse`. When a port collision can't be retried (the port was pinned, or it happened on
  the final attempt), startup now fails with this error. Previously it fell through to a generic error, and in `watch`
  mode it waited for the full startup timeout. It covers any address the app failed to bind (the site address or an
  auxiliary listener) as well as cargo-leptos's reload port, since the output doesn't reliably say which one failed.
  The report carries the output line that reported the collision, which shows the actual bind failure.

### Changed

- **Breaking:** `LeptosBrowserTestError::StartupStdoutClosed` is renamed to `StartupOutputClosed`, since it means
  that both stdout and stderr closed.
- **Breaking:** `LeptosBrowserTestError` is `#[non_exhaustive]`. Matches on it need a wildcard arm.
- **Breaking:** Startup-failure errors no longer embed the captured output. `StartupOutputClosed`, `StartupTimedOut`
  and `StreamRead` are now struct variants with only `app_name` and `expected_line` (plus `timeout` and `reason` for
  `StartupTimedOut`), and their messages are a single line. The captured output moved to the `CapturedOutput` report
  attachment. `StartupFailureContext` is removed. Code that read `ctx.stdout_tail` / `ctx.stderr_tail` should use
  `CapturedOutput::find_in(&report)` and its `stdout()` / `stderr()` instead.

### Fixed

- Sub-second `with_graceful_shutdown_timeout` values are rounded up to the next whole second instead of truncated, so
  e.g. 500ms no longer becomes a 0s grace period.
- Port-collision retries are detected from the live output instead of the captured log tail, so they are still
  detected with `with_startup_log_tail_lines(0)`. A collision only fails the attempt if the startup line doesn't
  follow, so an app logging a harmless bind failure (for example of a metrics listener) still starts. In `serve` mode
  the attempt ends once cargo-leptos exits. In `watch` mode, where cargo-leptos keeps running, it ends 5 seconds after
  the collision instead of after the full startup timeout. Reload-port collisions reported by cargo-leptos
  (`Reload TCP port … already in use`) are now retried too.
- Only collisions on an auto-allocated port are retried. A pinned `site_addr` is no longer retried just because
  `reload_port` was left unpinned (and vice versa).
- Startup now makes up to three retries after the first attempt, as documented, instead of three attempts in total.
- The output captured for a failed startup is now recorded by the readiness check itself, so it always includes the
  line that decided the outcome (for example the bind failure). It could previously be missing because the log
  capture lagged behind the readiness check.

## [0.3.0] - 2026-06-17

### Fixed

- Startup readiness detection now watches both stdout and stderr for the configured `with_startup_line` fragment. With
  this, leptos-browser-test now also supports leptos-applications who write their readiness lines to stderr.

### Changed

- **Breaking:** Updated `rootcause` to v0.13

## [0.2.0] - 2026-05-15

> **Upgrade note**: 0.2.0 requires cargo-leptos with
> [PR #648](https://github.com/leptos-rs/cargo-leptos/pull/648) landed to work as expected. Until it ships upstream,
> install cargo-leptos from here:
> ```sh
> cargo install --locked --git https://github.com/lpotthast/cargo-leptos --branch graceful-shutdown-v2 cargo-leptos
> ```

### Added

- `LeptosTestAppConfig::with_graceful_shutdown_timeout` and `LeptosTestAppConfig::with_graceful_shutdown_unix_signal`
  configure the budget and Unix signal the managed Leptos app gets to shut down gracefully when no longer needed (on
  drop). Values are forwarded to `cargo leptos` via `LEPTOS_GRACEFUL_SHUTDOWN_TIMEOUT_SECS` and
  `LEPTOS_GRACEFUL_SHUTDOWN_UNIX_SIGNAL`. Default timeout is 10 seconds. Default Unix signal is `SIGINT`.
- Re-export `tokio_process_tools::UnixGracefulSignal` so callers can pick the Unix signal without taking a direct
  dependency on `tokio-process-tools`.

### Changed

- **Breaking**: `LeptosTestAppConfig::with_termination_timeouts(interrupt, terminate, reason)` is replaced by
  `with_graceful_shutdown_timeout(timeout)` and `with_graceful_shutdown_unix_signal(signal)`. Termination is now
  delegated to `tokio_process_tools::GracefulShutdown`; the previous split between an interrupt timeout and a terminate
  timeout is gone, and the configured values are recorded as structured tracing fields on the spawn log rather than via
  a required `reason` string.
- Async stdout/stderr forwarding through `tokio::io::stdout()` / `tokio::io::stderr()` (replaces `println!` /
  `eprintln!`). When the managed app emits output faster than the parent stdio can drain, the async handles back off
  internally instead of panicking with a "Resource temporarily unavailable" error that std's print macros raise once
  tokio has put the parent stdio into non-blocking mode.
- `tokio` now requires the `io-std` feature (used by the new async stdio forwarding).
- Migrated to `tokio-process-tools` v0.11.2, bringing proper graceful termination on both Windows and Unix.
- Migrated to `assertr` v0.6.0 (dev-dependencies).
- CI installs forked cargo-leptos until upstream ships (see PR #648).

### Removed

- **Breaking**: `LeptosTestAppConfig::with_termination_timeouts` (see *Changed*).

## [0.1.1] - 2026-04-30

### Fixed

- Readme examples.

## [0.1.0] - 2026-04-30

### Added

- Public `LeptosTestAppConfig` builder, `LeptosTestApp` handle, and `LeptosBrowserTestError` enum for launching a
  `cargo leptos serve|watch` subprocess and waiting for it to listen.
- `StartupFailureContext` carries the per-failure diagnostics (`app_name`, `expected_line`, `stdout_tail`,
  `stderr_tail`) shared by every startup-failure variant of `LeptosBrowserTestError`.
- `with_startup_timeout` and `with_termination_timeouts` require a `reason` argument so the rationale for the chosen
  timeout values lives next to them. These reasons are logged when timeouts are reached unexpectedly.
- Automatic retry on transient port-bind collisions: `LeptosTestAppConfig::start` re-spawns `cargo leptos` up to three
  times when the failure stderr looks like `Address already in use` (or the Windows equivalent), provided the caller
  did not pin both `site_addr` and `reload_port`.
- `StartupLogBuffer` recovers from a poisoned mutex instead of panicking, so a panic during error rendering cannot mask
  the original startup failure.
- Integration smoke tests under `tests/smoke.rs` driving the full flow against a real Leptos SSR fixture at
  `tests/fixtures/leptos-ssr-app`.
- GitHub CI covering format, check, clippy, test, build, doc, and an MSRV gate at the declared `rust-version` (1.89.0).

[Unreleased]: https://github.com/lpotthast/leptos-browser-test/compare/v0.4.0...HEAD

[0.4.0]: https://github.com/lpotthast/leptos-browser-test/compare/v0.3.0...v0.4.0

[0.3.0]: https://github.com/lpotthast/leptos-browser-test/compare/v0.2.0...v0.3.0

[0.2.0]: https://github.com/lpotthast/leptos-browser-test/compare/v0.1.1...v0.2.0

[0.1.1]: https://github.com/lpotthast/leptos-browser-test/compare/v0.1.0...v0.1.1

[0.1.0]: https://github.com/lpotthast/leptos-browser-test/tree/f18da041b8eb4c27606cbe2fcd59d8534693f707
