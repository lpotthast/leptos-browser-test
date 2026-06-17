# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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

[Unreleased]: https://github.com/lpotthast/leptos-browser-test/compare/v0.3.0...HEAD

[0.3.0]: https://github.com/lpotthast/leptos-browser-test/compare/v0.2.0...v0.3.3

[0.2.0]: https://github.com/lpotthast/leptos-browser-test/compare/v0.1.1...v0.2.0

[0.1.1]: https://github.com/lpotthast/leptos-browser-test/compare/v0.1.0...v0.1.1

[0.1.0]: https://github.com/lpotthast/leptos-browser-test/tree/f18da041b8eb4c27606cbe2fcd59d8534693f707
