# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.0] - 2026-04-30

- Public `LeptosTestAppConfig` builder, `LeptosTestApp` handle, and `LeptosBrowserTestError`
  enum for launching a `cargo leptos serve|watch` subprocess and waiting for it to listen.
- `StartupFailureContext` carries the per-failure diagnostics (`app_name`, `expected_line`,
  `stdout_tail`, `stderr_tail`) shared by every startup-failure variant of
  `LeptosBrowserTestError`.
- `with_startup_timeout` and `with_termination_timeouts` require a `reason: impl Into<String>`
  argument so the rationale for tuned values lives next to the numbers — and follows them
  into the `tracing` log line and (for the startup timeout) into the rendered
  `StartupTimedOut` error message — instead of in a soon-to-rot source comment.
- Automatic retry on transient port-bind collisions: `LeptosTestAppConfig::start` re-spawns
  `cargo leptos` up to three times when the failure stderr looks like
  `Address already in use` (or the Windows equivalent), provided the caller did not pin
  both `site_addr` and `reload_port`.
- `StartupLogBuffer` recovers from a poisoned mutex instead of panicking, so a panic during
  error rendering cannot mask the original startup failure.
- Integration smoke tests under `tests/smoke.rs` driving the full flow against a real
  Leptos SSR fixture at `tests/fixtures/leptos-ssr-app`.
- GitHub Actions workflow at `.github/workflows/ci.yml` covering format, check, clippy,
  test, build, doc, and an MSRV gate at the declared `rust-version` (1.89.0).
