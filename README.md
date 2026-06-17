# leptos-browser-test

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
leptos-browser-test = "0.3.0"
```

### cargo-leptos requirement

`leptos-browser-test` v0.2.0 sets `LEPTOS_GRACEFUL_SHUTDOWN_TIMEOUT_SECS` and `LEPTOS_GRACEFUL_SHUTDOWN_UNIX_SIGNAL` on
the managed cargo-leptos child to drive the Leptos app's graceful-shutdown path on drop. These env vars require
[cargo-leptos PR #648](https://github.com/leptos-rs/cargo-leptos/pull/648), which has not yet shipped. Until it lands
upstream, install cargo-leptos from here:

```sh
cargo install --locked --git https://github.com/lpotthast/cargo-leptos --branch graceful-shutdown-v2 cargo-leptos
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

```rust,no_run
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
