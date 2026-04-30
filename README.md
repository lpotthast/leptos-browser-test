# leptos-browser-test

Leptos test-app launcher for browser-driven integration tests.

This crate owns the `cargo leptos serve` / `cargo leptos watch` process management for a crate-local Leptos test app.
It waits until the app is listening and keeps recent stdout/stderr for useful startup failures.

It intentionally leaves browser/WebDriver orchestration to the test harness that uses the returned base URL. Use
`app.base_url()` with `browser-test`, Playwright, Selenium, `thirtyfour`, or any other integration-test harness.

## Runtime requirements

`LeptosTestApp` terminates the managed `cargo leptos` process when it is dropped. This uses
`tokio_process_tools::TerminateOnDrop`, so Tokio browser tests must use a multi-threaded runtime:
`#[tokio::test(flavor = "multi_thread")]`.

```rust,no_run
use leptos_browser_test::{LeptosTestAppConfig, Report};

# async fn run_browser_tests(base_url: &str) -> Result<(), Report> {
#     let _ = base_url;
#     Ok(())
# }
#
# async fn example() -> Result<(), Report> {
let app = LeptosTestAppConfig::new("testing/test-app")
    .with_app_name("my test app")
    .start()
    .await
    .map_err(Report::into_dynamic)?;

run_browser_tests(app.base_url()).await?;
# Ok(())
# }
```

`Report::into_dynamic` erases the typed `Report<LeptosBrowserTestError>` into a generic `Report` so it can compose with
the test harness's own error type.

Typical `browser-test` usage starts the app, passes its base URL into the runner context, and lets drop-based cleanup
handle the app process after `BrowserTestRunner::run(...)` returns:

```rust,ignore
use browser_test::{BrowserTestRunner, BrowserTests};
use leptos_browser_test::{LeptosTestAppConfig, Report};

struct TestContext {
    base_url: String,
}

#[tokio::test(flavor = "multi_thread")]
async fn browser_tests() -> Result<(), Report> {
    let app = LeptosTestAppConfig::new("testing/test-app")
        .with_app_name("my test app")
        .start()
        .await
        .map_err(Report::into_dynamic)?;

    let context = TestContext {
        base_url: app.base_url().to_owned(),
    };

    BrowserTestRunner::new()
        .run(&context, BrowserTests::new().with(MyBrowserTest))
        .await
        .map_err(Report::into_dynamic)?;

    Ok(())
}
```

For apps served over HTTPS, add the scheme override before starting:

```rust,no_run
# use leptos_browser_test::{LeptosTestAppConfig, SiteScheme};
# async fn example() -> Result<(), leptos_browser_test::Report> {
let app = LeptosTestAppConfig::new("testing/test-app")
    .with_site_scheme(SiteScheme::Https)
    .start()
    .await
    .map_err(leptos_browser_test::Report::into_dynamic)?;
# let _ = app;
# Ok(())
# }
```

For local manual debugging, run the integration test target from the consuming crate and enable whatever visibility or
pause flags the browser harness supports:

```sh
cargo test --test your_integration_test -- --nocapture
```
