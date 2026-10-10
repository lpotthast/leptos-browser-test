# AGENTS.md

Keep this file short and stable. Explain how a mechanism works in code comments next to it, not here.

## Commands

- `just verify` before finishing a change. It runs everything CI checks, including the MSRV build.
- `just install-tools` sets up what the recipes need, including cargo-leptos and the wasm target.

## Smoke tests

- Run them via `just test-smoke`. They share the fixture's `target/` and must not run in parallel.

## Guardrails

- This crate only manages the cargo-leptos process. It intentionally does not drive a browser.
- Tests using this crate need `#[tokio::test(flavor = "multi_thread")]`. Dropping `LeptosTestApp` stops the process
  only on a multi-threaded runtime. A hang or leaked process usually means this is missing.
- Never write forwarded process output with `print!` / `eprintln!`. Tokio makes stdio non-blocking and `print!`
  panics on `EAGAIN`. Use `tokio::io::stdout()` / `stderr()`.
- Error variants in `src/error.rs` describe the failure only. Diagnostics such as captured output go into `rootcause`
  report attachments, not into variant fields.

## Conventions

- A new config option needs a default in `LeptosTestAppConfig::new` and assertions in the `config` tests.
- User-facing changes get a `CHANGELOG.md` entry (Keep a Changelog) and, if relevant, a README update.
- Unit tests live in-module and use `assertr`. Only public-API integration tests go in `tests/`.
- The crate uses an `include` allowlist in `Cargo.toml`. New files that must ship (e.g. `build.rs`) must be added there.
- The fixture is not a workspace member and has its own `Cargo.lock` and `target/`. Its `Cargo.toml` deliberately
  deviates from the upstream template (see its header). Keep those deviations when updating it.
