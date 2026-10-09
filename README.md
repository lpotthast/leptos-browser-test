# leptos-browser-test

[![Crates.io](https://img.shields.io/crates/v/leptos-browser-test.svg)](https://crates.io/crates/leptos-browser-test)
[![Docs.rs](https://docs.rs/leptos-browser-test/badge.svg)](https://docs.rs/leptos-browser-test)
[![CI](https://github.com/lpotthast/leptos-browser-test/actions/workflows/ci.yml/badge.svg)](https://github.com/lpotthast/leptos-browser-test/actions/workflows/ci.yml)
[![MSRV](https://img.shields.io/badge/MSRV-1.89.0-blue.svg)](https://github.com/lpotthast/leptos-browser-test/blob/main/Cargo.toml)
[![License: MIT OR Apache-2.0](https://img.shields.io/crates/l/leptos-browser-test.svg)](#license)

Leptos test-app launcher for browser-driven integration tests.

This crate owns the `cargo leptos serve` / `cargo leptos watch` process management for a local Leptos test app:

- starts cargo-leptos for you, letting it serve your application
- waits until the app is fully started
- keeps a recent tail of stdout/stderr logs for failure diagnostics
- provides a handle to the launched application
- terminates the started cargo-leptos process gracefully when the handle is dropped, which in turn gracefully terminates
  the Leptos app
- the handle provides `.base_url()`, conveniently telling you where (randomized port) the app is reachable

Test orchestration is intentionally left to the consumer. Take a look at
[browser-test](https://crates.io/crates/browser-test), for a convenient Rust-native integration test runner using
`thirtyfour`.

## Installation

```toml
[dev-dependencies]
leptos-browser-test = "0.3.1"
```

## Runtime requirements

`LeptosTestApp` terminates the managed `cargo leptos` process when it is dropped, using
`tokio_process_tools::TerminateOnDrop`. This requires an active multithreaded Tokio runtime, so browser tests must use:

```rust,ignore
#[tokio::test(flavor = "multi_thread")]
```

## Usage

### Starting an app

```rust,no_run
use leptos_browser_test::{LeptosTestAppConfig, Report};

#[tokio::main]
async fn main() -> Result<(), Report> {
    let app = LeptosTestAppConfig::new("testing/test-app")
        .with_app_name("my test app")
        .start()
        .await
        .map_err(Report::into_dynamic)?;

    let url = app.base_url();
    // run tests...
    Ok(())
}
```

`Report::into_dynamic` erases the typed `Report<LeptosBrowserTestError>` into a generic `Report` so it composes with
the test harness's own error type.

### Running `browser-test`

Start the app, pass its base URL into the runner context, and let drop-based cleanup handle the app process after
`BrowserTestRunner::run(...)` returns:

```rust,ignore
use browser_test::{BrowserTestRunner, BrowserTests};
use leptos_browser_test::{LeptosTestAppConfig, Report};

struct Context {
    base_url: String,
}

#[tokio::test(flavor = "multi_thread")]
async fn browser_tests() -> Result<(), Report> {
    let app = LeptosTestAppConfig::new("testing/test-app")
        .with_app_name("my test app")
        .start()
        .await
        .map_err(Report::into_dynamic)?;

    let context = Context {
        base_url: app.base_url().to_owned(),
    };

    BrowserTestRunner::new()
        .run(&context, BrowserTests::new().with(MyFirstTest))
        .await
        .map_err(Report::into_dynamic)?;

    Ok(())
}
pub struct MyFirstTest {}

#[async_trait]
impl BrowserTest<Context> for MyFirstTest {
    fn name(&self) -> Cow<'_, str> {
        "classes_tests".into()
    }

    async fn run(&self, driver: &WebDriver, ctx: &Context) -> Result<(), Report> {
        // TODO: Use `driver` to query the page and run assertions.
        Ok(())
    }
}
```

### HTTPS

For apps served over HTTPS, override the URL scheme before starting:

```rust,no_run
use leptos_browser_test::{LeptosTestAppConfig, SiteScheme};

async fn example() -> Result<(), leptos_browser_test::Report> {
    let app = LeptosTestAppConfig::new("testing/test-app")
        .with_site_scheme(SiteScheme::Https)
        .start()
        .await
        .map_err(leptos_browser_test::Report::into_dynamic)?;
    let _ = app;
    Ok(())
}
```

`SiteScheme` only affects the `base_url()` returned to your harness. Configuring TLS on the served app itself is the
app's responsibility.

### Build profile

Every browser test that loads a page downloads, compiles and runs the app's WASM bundle, often in a fresh browser
context with a cold cache. A dev build's bundle can be tens or hundreds of megabytes of unoptimized code. Build with
`--release` to make pages load and hydration faster. This can also reduce browser memory consumption. With the right
profile settings, this might not even cost you (incremental) compile times.

```rust,no_run
use leptos_browser_test::{BuildProfile, LeptosTestAppConfig};

let app = LeptosTestAppConfig::new("testing/test-app").with_build_profile(BuildProfile::Release);
```

The app's `Cargo.toml` decides what a release build means (`[package.metadata.leptos]`): e.g. an optimized
`lib-profile-release` for the WASM, and a `bin-profile-release` inheriting `dev` so that the server still builds
quickly.

These are recommended settings that give very small WASM binary sizes and slightly optimized server-side-rendering
performance with little to none compile-time reduction drawbacks.

```toml
[package.metadata.leptos]
lib-profile-release = "wasm-release"
bin-profile-release = "server-release"

[profile.wasm-release]
inherits = "release"
opt-level = "z"
incremental = true
debug-assertions = true
overflow-checks = true

[profile.server-release]
inherits = "dev"
opt-level = 1
```


### Tuning graceful shutdown

By default, the managed Leptos app gets 10 seconds to shut down on drop and is signalled with `SIGINT` on Unix (always
`CTRL_BREAK_EVENT` on Windows). Override this via:

```rust,no_run
use std::time::Duration;
use leptos_browser_test::{LeptosTestAppConfig, UnixGracefulSignal};

async fn example() -> Result<(), leptos_browser_test::Report> {
    let app = LeptosTestAppConfig::new("testing/test-app")
        .with_graceful_shutdown_timeout(Duration::from_secs(30))
        .with_graceful_shutdown_unix_signal(UnixGracefulSignal::Terminate)
        .start()
        .await
        .map_err(leptos_browser_test::Report::into_dynamic)?;
    let _ = app;
    Ok(())
}
```

The timeout is forwarded to `cargo leptos` via `LEPTOS_GRACEFUL_SHUTDOWN_TIMEOUT_SECS`, the signal via
`LEPTOS_GRACEFUL_SHUTDOWN_UNIX_SIGNAL`. The signal selector is ignored on Windows.

### Manual debugging

Run the integration-test target from the consuming crate with `--nocapture` so the managed app's forwarded
stdout/stderr stays visible:

```sh
cargo test --test your_integration_test -- --nocapture
```

## License

Licensed under either [Apache License, Version 2.0](https://github.com/lpotthast/leptos-browser-test/blob/main/LICENSE-APACHE)
or [MIT license](https://github.com/lpotthast/leptos-browser-test/blob/main/LICENSE-MIT),
at your option.
