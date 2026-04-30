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
- `cargo test --lib` — in-module unit tests + doctests, no fixture build. Single test:
  `cargo test --lib <name>` (e.g. `cargo test --lib startup_log_buffer_keeps_recent_tail`); single
  module: `cargo test --lib config::tests`.
- `cargo test --test smoke -- --test-threads=1` — full integration smoke against the SSR fixture
  (see *Smoke tests* below). Plain `cargo test` runs both targets.
- `cargo fmt` / `cargo fmt --check`.
- `cargo clippy --all-targets --all-features -- -D warnings` — must pass clean. `Cargo.toml` sets
  `clippy::pedantic = warn` and `rust::missing_docs = warn`, so new public items need `///` docs and pedantic-clean
  code.
- `just verify` — full pre-PR suite, currently `fmt-check lint test build doc` (note: it does
  *not* run `cargo check` or `cargo clippy` twice — the recipe in `Justfile` is the source of
  truth if you suspect drift).
- `just msrv` / `just minimal-versions` — MSRV (currently `1.89.0`) and minimum-dep-version checks; require
  `just install-tools` first.

### Smoke tests

`tests/smoke.rs` drives `LeptosTestAppConfig` end-to-end against the vendored Leptos SSR fixture
in `tests/fixtures/leptos-ssr-app/` (a copy of the official `leptos-rs/start-axum` template). To
run them locally:

- Install `cargo-leptos` on `PATH`: `cargo install cargo-leptos --locked`.
- Add the WASM target: `rustup target add wasm32-unknown-unknown`.
- Cold runs compile the fixture (server + wasm) and can take **several minutes**; subsequent runs
  reuse `tests/fixtures/leptos-ssr-app/target/`. Tests use a 600s startup timeout to accommodate
  this — keep new smoke tests similarly generous.
- Run with `--test-threads=1` so concurrent fixture builds don't fight over the same `target/`
  (this is what CI does in `.github/workflows/ci.yml`).

CI splits unit and smoke tests into separate jobs (`test-unit` runs `cargo test --lib`,
`test-smoke` installs `cargo-leptos` + the wasm target and runs `cargo test --test smoke`). The
fixture's `target/` is included in the `Swatinem/rust-cache` workspaces list.

## Architecture

The public surface is small and re-exported from `src/lib.rs`: `LeptosTestApp`, `LeptosTestAppConfig`,
`CargoLeptosMode`, `SiteScheme`, `LeptosBrowserTestError`, plus `rootcause`'s `Report`/`bail`/`report`/`ResultExt`.
Errors are `rootcause` reports wrapping a `thiserror` enum.

**Startup orchestration** (`src/app.rs::start_configured_app`) is the load-bearing path:

1. Canonicalize `app_dir`; pick free site port via `ports::find_free_port` if `site_addr` was not pinned, then a free
   reload port that excludes the site port.
2. Build the command via `cargo_leptos::command`. Framework env (`LEPTOS_SITE_ADDR`, `LEPTOS_RELOAD_PORT`,
   `RUST_BACKTRACE=1`) is set first, then `extra_env` is applied — so `with_env` is a deliberate **escape hatch** that
   overrides framework env (last-write-wins on `tokio::process::Command::env`). Don't reorder this.
3. Spawn under `tokio_process_tools::Process` with a `BroadcastOutputStream` configured for reliable delivery + 1 MB
   replay buffer. Two `inspect_lines` consumers fan out: one tees lines into a bounded `StartupLogs` ring buffer (
   `src/startup.rs`), and (when `forward_logs` is true) prints to parent stdout/stderr.
4. Wait on `process.stdout().wait_for_line(...)` for the configured `startup_line` (default
   `"listening on {site_addr}"`). `seal_output_replay()` is called *before* awaiting so late subscribers don't keep
   buffering. Three failure modes — timeout / stdout-closed / stream-read-error — each surface the captured
   stdout+stderr tail in the error message.
5. The returned `LeptosTestApp` holds a `TerminateOnDrop` handle plus the two replay consumers — keeping them alive
   matters; dropping the consumers would lose log forwarding. Cleanup uses `interrupt_timeout` then `terminate_timeout`.

**Multi-thread runtime is mandatory.** `TerminateOnDrop` requires an active multi-threaded Tokio runtime to terminate
the child on drop. Tests using this library must use `#[tokio::test(flavor = "multi_thread")]` — if you see a hang or
leaked process, this is the first thing to check.

**`SiteScheme` is browser-facing only.** It selects `http`/`https` for the `base_url()` returned to tests and feeds the
default startup line. It does **not** configure TLS on the Leptos process — the app itself must serve HTTPS
independently.

**Startup line matching is substring-based** (`line.contains(&expected_line)`), not exact. Custom startup lines via
`with_startup_line` should be unique enough not to false-match earlier log output.

## Repo conventions

- Rust 2024 edition. `rustfmt` defaults; snake_case / PascalCase / SCREAMING_SNAKE_CASE.
- Public fallible functions get a `# Errors` doc section (enforced by reading style, not lints).
- Unit tests live in-module (`#[cfg(test)] mod tests`) next to the code under test. Genuine
  integration tests of the public API go under `tests/` — currently just `tests/smoke.rs`
  driving the SSR fixture.
- The fixture under `tests/fixtures/leptos-ssr-app/` is a vendored copy of `leptos-rs/start-axum`
  and intentionally has its own `Cargo.lock` / `target/`. It is *not* a workspace member and is
  excluded from the published crate via the `exclude` list in `Cargo.toml` (alongside
  `CLAUDE.md`, `Justfile`, `.idea`, etc.). When upgrading the fixture, regenerate it from the
  upstream template rather than hand-editing.
