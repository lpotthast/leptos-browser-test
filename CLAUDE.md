# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Crate purpose

Library crate (`leptos-browser-test`) that owns `cargo leptos serve` / `cargo leptos watch` process management for
crate-local Leptos test apps used in browser-driven integration tests. It returns a `LeptosTestApp` whose `base_url()`
is fed into a separate browser harness (browser-test, Playwright, Selenium, thirtyfour, etc.) — this crate intentionally
does **not** drive WebDriver itself.

## Commands

Routine work goes through Cargo directly; `Justfile` has the maintenance tooling.

- `cargo check` — fast typecheck.
- `cargo test --lib` — in-module unit tests only, no fixture build. Single test:
  `cargo test --lib <name>` (e.g. `cargo test --lib startup_log_buffer_keeps_recent_tail`); single
  module: `cargo test --lib config::tests`.
- `cargo test --doc` — doctests. `src/lib.rs` uses `#![doc = include_str!("../README.md")]`, so every
  `rust`/`rust,no_run` block in the README is compiled as a doctest (run by `just test` and CI's
  `test-unit` job); use `rust,ignore` for snippets that need crates outside this crate's dependencies.
- `cargo test --test smoke -- --test-threads=1` — full integration smoke against the SSR fixture
  (see *Smoke tests* below). Plain `cargo test` runs lib, smoke, *and* doctests.
- `cargo fmt` / `cargo fmt --check`.
- `cargo clippy --all-targets --all-features -- -D warnings` — must pass clean. `Cargo.toml` sets
  `clippy::pedantic = warn` and `rust::missing_docs = warn` (and CI sets `RUSTFLAGS=-D warnings`), so new public items
  need `///` docs and pedantic-clean code.
- `just verify` — non-mutating pre-PR suite: `fmt-check check lint test build doc check-msrv`, where `test` =
  `test-unit` + `test-doc` + `test-smoke` (needs cargo-leptos and the wasm target), `doc` treats rustdoc warnings as
  errors, and `check-msrv` builds on the `1.89.0` toolchain. All cargo invocations use `--locked`.
- `just tidy` — mutating housekeeping: `cargo update --workspace`, `cargo sort --workspace`, `cargo fmt --all`.
- `just msrv` / `just minimal-versions` — MSRV discovery (currently `1.89.0`, also checked in CI and by
  `just check-msrv`) and minimum-dep-version checks; require `just install-tools` first.

### Smoke tests

`tests/smoke.rs` drives `LeptosTestAppConfig` end-to-end against the vendored Leptos SSR fixture
in `tests/fixtures/leptos-ssr-app/` (a copy of the official `leptos-rs/start-axum` template). To
run them locally:

- Install `cargo-leptos` on `PATH`: `cargo install cargo-leptos --locked`.
- Add the WASM target: `rustup target add wasm32-unknown-unknown`.
- cargo-leptos uses *any* `wasm-bindgen` binary on `PATH` (e.g. one from `just install-tools`) and only downloads the
  version matching the fixture's `Cargo.lock` when none is found. A version mismatch makes startup fail with "different
  bindgen format"; fix it by bumping the whole family in the fixture:
  `cargo update wasm-bindgen wasm-bindgen-futures wasm-bindgen-macro wasm-bindgen-macro-support wasm-bindgen-shared js-sys web-sys`
  (run inside `tests/fixtures/leptos-ssr-app`).
- Cold runs compile the fixture (server + wasm) and can take **several minutes**; subsequent runs
  reuse `tests/fixtures/leptos-ssr-app/target/`. Tests use a 600s startup timeout to accommodate
  this — keep new smoke tests similarly generous. `with_startup_timeout` also takes a required reason string,
  which is logged and included in `StartupTimedOut` errors.
- Run with `--test-threads=1` so concurrent fixture builds don't fight over the same `target/`
  (this is what CI does in `.github/workflows/ci.yml`).

CI (`.github/workflows/ci.yml`) roughly mirrors `just verify` as separate jobs (no standalone `check` job, since
clippy covers it; `build` and `doc` run with `--release`): `test-unit` runs `cargo test --lib` and
`--doc`; `test-smoke` installs a prebuilt cargo-leptos via `taiki-e/install-action` + the wasm target and runs
`cargo test --test smoke`. The fixture's `target/` is included in the `Swatinem/rust-cache` workspaces list. An
`advisories` job runs `cargo deny check advisories`, so a new RustSec advisory against a dependency fails CI.

## Architecture

The public surface is small and re-exported from `src/lib.rs`: `LeptosTestApp`, `LeptosTestAppConfig` (builder),
`CargoLeptosMode`, `BuildProfile`, `SiteScheme`, `LeptosBrowserTestError`, `tokio_process_tools::UnixGracefulSignal`,
plus `rootcause`'s `Report`/`bail`/`report`/`ResultExt`. Errors are `rootcause` reports wrapping a `thiserror` enum
(`src/error.rs`); startup-failure variants carry a `StartupFailureContext` with the captured stdout/stderr tails.

**Startup orchestration** (`src/app.rs::start_configured_app`) is the load-bearing path:

1. `resolve_runtime_config`: canonicalize `app_dir`; pick a free site port via `ports::find_free_port` if `site_addr`
   was not pinned, then a free reload port that excludes the site port. Default startup line is
   `"listening on {base_url}"` (so it includes the scheme).
2. `cargo_leptos::command` builds `cargo leptos <serve|watch> [--release]`. Cargo binary resolution:
   `with_cargo` > `$CARGO` > `cargo`. Framework env (`LEPTOS_SITE_ADDR`, `LEPTOS_RELOAD_PORT`,
   `LEPTOS_GRACEFUL_SHUTDOWN_TIMEOUT_SECS`, `LEPTOS_GRACEFUL_SHUTDOWN_UNIX_SIGNAL`, and `RUST_BACKTRACE=1` only if the
   parent hasn't set it) is set first, then `extra_env` — so `with_env` is a deliberate **escape hatch** that
   overrides framework env (last-write-wins on `tokio::process::Command::env`). Don't reorder this. The
   `UnixGracefulSignal` → `"SIGINT"`/`"SIGTERM"` mapping has an `unreachable!` arm for the non-exhaustive enum; extend
   it when bumping `tokio-process-tools` adds variants.
3. Spawn under `tokio_process_tools::Process` with stdout+stderr as `BroadcastOutputStream`s (reliable with
   backpressure + 1 MB replay). Two `ParseLines::inspect_async` consumers tee lines into the bounded `StartupLogs` ring
   buffers (`src/startup.rs`) and, when `forward_logs` is true, write to parent stdout/stderr via
   `tokio::io::stdout()/stderr()` — **not** `print!`, which panics on `EAGAIN` once tokio has put the stdio fds into
   non-blocking mode.
4. `wait_for_ready` races `wait_for_line` on **both** stdout and stderr; whichever matches first wins.
   `seal_output_replay()` is called after creating the waiters but *before* awaiting, so late subscribers don't keep
   buffering. That is why `wait_for_startup_line` is deliberately a plain `fn` returning a future, not an `async fn`:
   an `async fn` would only subscribe on first poll, i.e. after the seal. Outcomes map to `StartupTimedOut` / `StartupStdoutClosed` (only when both streams closed) / `StreamRead`,
   each embedding the captured log tails.
5. **Port-collision retry:** if ports were auto-allocated (either `site_addr` or `reload_port` unpinned) and the failure
   tails contain "address already in use" (or the Windows equivalent), the process is dropped and startup retries with
   fresh ports, up to `MAX_PORT_COLLISION_RETRIES` (3). Fully pinned ports fail on the first attempt.
6. The returned `LeptosTestApp` holds a `TerminateOnDrop` handle plus the two replay consumers — keeping them alive
   matters; dropping the consumers would lose log forwarding.

**Shutdown is two-level.** On drop, `TerminateOnDrop` sends SIGINT (Unix) / CTRL_BREAK (Windows) to *cargo-leptos*
with a budget of `graceful_shutdown_timeout + 10s`; cargo-leptos in turn forwards the configured signal to the *app*
with `graceful_shutdown_timeout` (passed via the `LEPTOS_GRACEFUL_SHUTDOWN_*` env vars). The extra slack exists because
cargo-leptos requires its own timeout ≥ the app's.

**Multi-thread runtime is mandatory.** `TerminateOnDrop` requires an active multi-threaded Tokio runtime to terminate
the child on drop. Tests using this library must use `#[tokio::test(flavor = "multi_thread")]` — if you see a hang or
leaked process, this is the first thing to check.

**`SiteScheme` is browser-facing only.** It selects `http`/`https` for the `base_url()` returned to tests and feeds the
default startup line. It does **not** configure TLS on the Leptos process — the app itself must serve HTTPS
independently.

**`BuildProfile::Release` only adds `--release`.** What that means for the server vs. WASM build is decided by the app's
`[package.metadata.leptos]` (`bin-profile-release` / `lib-profile-release`); see the README's "Build profile".

**Startup line matching is substring-based** (`line.contains(&expected_line)`), not exact. Custom startup lines via
`with_startup_line` should be unique enough not to false-match earlier log output.

## Repo conventions

- Rust 2024 edition. `rustfmt` defaults; snake_case / PascalCase / SCREAMING_SNAKE_CASE.
- Public fallible functions get a `# Errors` doc section (enforced by reading style, not lints).
- Config setters are `#[must_use]` builder methods (`const fn` where possible); new options also need a default in
  `LeptosTestAppConfig::new` and assertions in `config::tests::{new_uses_documented_defaults, setters_override_defaults}`.
- User-facing changes get a `CHANGELOG.md` entry (Keep a Changelog format) and, if relevant, a README section.
- Unit tests live in-module (`#[cfg(test)] mod tests`, using `assertr` assertions) next to the code under test.
  Genuine integration tests of the public API go under `tests/` — currently just `tests/smoke.rs` driving the SSR
  fixture.
- The fixture under `tests/fixtures/leptos-ssr-app/` is a vendored copy of `leptos-rs/start-axum`
  and intentionally has its own `Cargo.lock` / `target/`. It is *not* a workspace member.
- The published crate is defined by an `include` allowlist in `Cargo.toml` (`src/`, `README.md`, `CHANGELOG.md`,
  `LICENSE-*`), so `tests/`, `CLAUDE.md`, `Justfile`, `.github`, etc. never ship. New files that must be published
  (e.g. a `build.rs` or files read via `include_str!`) have to be added there; check with `cargo package --list`.
- The fixture's `Cargo.toml` deliberately deviates from the template (documented in its header): every dependency uses
  `default-features = false` with only the features the app needs (leptos_axum needs its `default` feature for
  `file_and_error_handler`), and its `wasm-release` / `server-release` profiles must stay in sync with the README's
  "Build profile" recommendation. Keep both when upgrading from the upstream template.
