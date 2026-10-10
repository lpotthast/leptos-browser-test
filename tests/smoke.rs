//! Integration smoke tests driving the full `LeptosTestAppConfig` → `LeptosTestApp` flow
//! against a real Leptos SSR app under `tests/fixtures/leptos-ssr-app` (a vendored copy of
//! the official `leptos-rs/start-axum` template).
//!
//! These tests require the `cargo-leptos` binary on `PATH` and the `wasm32-unknown-unknown`
//! Rust target installed. CI installs both; locally:
//!
//! ```sh
//! cargo install cargo-leptos --locked
//! rustup target add wasm32-unknown-unknown
//! ```
//!
//! Each test does a clean Leptos compile on first run, which can take several minutes.
//! Subsequent runs reuse the fixture's `target/` cache and are much faster. The startup
//! timeout used in tests below is generous accordingly.

use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

use assertr::prelude::*;
use leptos_browser_test::{
    CapturedOutput, CargoLeptosMode, LeptosBrowserTestError, LeptosTestAppConfig,
    UnixGracefulSignal,
};

/// Absolute path to the SSR fixture directory.
fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/leptos-ssr-app")
}

/// One-shot HTTP GET with a short timeout. Returns `Err` on connect failure, non-2xx, or
/// body read errors — anything that distinguishes "server is alive" from "server is gone".
async fn http_get(url: &str) -> Result<String, reqwest::Error> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .expect("reqwest client");
    let resp = client.get(url).send().await?.error_for_status()?;
    resp.text().await
}

#[tokio::test(flavor = "multi_thread")]
async fn starts_real_leptos_app_and_serves_http() {
    let app = LeptosTestAppConfig::new(fixture_path())
        .with_app_name("leptos-ssr-app")
        .with_forward_logs(true)
        .with_startup_timeout(
            Duration::from_secs(600),
            "Cold builds of the Leptos fixture (server + wasm) can take several minutes.",
        )
        .start()
        .await
        .expect("cargo leptos serve should bring up the SSR fixture");

    assert_that!(app.base_url())
        .with_detail_message(format!("unexpected base_url: {}", app.base_url()))
        .starts_with("http://127.0.0.1:");

    let expected_base_url = format!("http://{}", app.site_addr());
    assert_that!(app.base_url()).is_equal_to(expected_base_url.as_str());
    assert_that!(app.reload_port()).is_greater_than(0_u16);
    assert_that!(app.app_dir().is_absolute()).is_true();

    let debug = format!("{app:?}");
    assert_that!(debug.as_str()).starts_with("LeptosTestApp {");
    assert_that!(debug.as_str()).contains(app.base_url());

    let body = http_get(&format!("{}/", app.base_url()))
        .await
        .expect("homepage should respond with 200");
    assert_that!(body.as_str())
        .with_detail_message(format!(
            "expected SSR-rendered greeting in body; got: {body}"
        ))
        .contains("Welcome to Leptos!");
}

#[tokio::test(flavor = "multi_thread")]
async fn http_request_fails_after_drop() {
    let app = LeptosTestAppConfig::new(fixture_path())
        .with_app_name("leptos-ssr-app")
        .with_startup_timeout(
            Duration::from_secs(600),
            "Cold builds of the Leptos fixture (server + wasm) can take several minutes.",
        )
        .with_forward_logs(true)
        .with_graceful_shutdown_timeout(Duration::from_secs(5))
        .with_graceful_shutdown_unix_signal(UnixGracefulSignal::Terminate)
        .start()
        .await
        .expect("start");

    let url = format!("{}/", app.base_url());
    assert_that!(http_get(&url).await)
        .with_detail_message("server should respond before drop")
        .is_ok();

    drop(app);

    // After drop, requests should eventually fail. The app gets a realistic graceful-shutdown
    // budget, while the assertion still bounds the overall OS-level teardown.
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut stopped = false;
    while Instant::now() < deadline {
        if http_get(&url).await.is_err() {
            stopped = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_that!(stopped)
        .with_detail_message(format!(
            "server still responding to {url} more than 15s after drop"
        ))
        .is_true();
}

#[tokio::test(flavor = "multi_thread")]
async fn surfaces_startup_timeout_with_log_tail() {
    // Tight timeout so cargo-leptos cannot possibly finish compiling and start serving.
    // The expected_line ("listening on http://...") will not appear in time, and the retry
    // loop should NOT kick in because the failure stderr won't contain a port-bind error.
    let reason = "Tight bound that cargo-leptos cannot meet, used to exercise the timeout path.";
    let result = LeptosTestAppConfig::new(fixture_path())
        .with_app_name("leptos-ssr-app")
        .with_forward_logs(true)
        .with_startup_timeout(Duration::from_millis(250), reason)
        .start()
        .await;

    let err = assert_that_owned!(result)
        .with_detail_message("250ms startup_timeout should not be enough to compile cargo-leptos")
        .get_err()
        .unwrap_inner();
    let display = err.current_context().to_string();
    assert_that!(display.as_str())
        .with_detail_message(format!("expected timeout message; got: {display}"))
        .contains("did not start within");
    // The reason flows through into the rendered error so future debuggers see *why* the
    // timeout was set, not just the elapsed duration.
    assert_that!(display.as_str())
        .with_detail_message(format!("expected timeout reason in error; got: {display}"))
        .contains(reason);

    // The captured process output travels with the report as an attachment, and printing the
    // whole report shows it.
    assert_that!(CapturedOutput::find_in(&err).is_some())
        .with_detail_message("expected a CapturedOutput attachment on the report")
        .is_true();
    let full_report = err.to_string();
    assert_that!(full_report.as_str())
        .with_detail_message(format!(
            "expected captured output in report; got: {full_report}"
        ))
        .contains("Recent stderr:");
}

#[tokio::test(flavor = "multi_thread")]
async fn pinned_site_port_in_use_fails_fast_in_serve_mode() {
    // In `serve` mode, cargo-leptos exits after the app's bind panic.
    assert_pinned_site_port_in_use_fails(CargoLeptosMode::Serve).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn pinned_site_port_in_use_fails_fast_in_watch_mode() {
    // Build the fixture first, so that the timing below only covers startup.
    let warm_up = LeptosTestAppConfig::new(fixture_path())
        .with_app_name("leptos-ssr-app")
        .with_mode(CargoLeptosMode::Watch)
        .with_startup_timeout(
            Duration::from_secs(600),
            "Cold builds of the Leptos fixture (server + wasm) can take several minutes.",
        )
        .start()
        .await
        .expect("warm-up start in watch mode");
    drop(warm_up);

    // In `watch` mode, cargo-leptos keeps running after the app's bind panic, so its output never
    // closes. Without fast-fail detection, startup would only give up after the 600s timeout.
    let started = Instant::now();
    assert_pinned_site_port_in_use_fails(CargoLeptosMode::Watch).await;
    assert_that!(started.elapsed())
        .with_detail_message("watch-mode startup should fail fast on a port collision")
        .is_less_than(Duration::from_secs(120));
}

/// Occupies a port and pins the app to it. The app's bind panics with "Address already in use",
/// and because the port is pinned, startup must fail with `PortInUse` instead of retrying or
/// waiting for the startup timeout.
async fn assert_pinned_site_port_in_use_fails(mode: CargoLeptosMode) {
    let blocker = std::net::TcpListener::bind("127.0.0.1:0").expect("bind blocker port");
    let site_addr = blocker.local_addr().expect("blocker addr").to_string();

    let result = LeptosTestAppConfig::new(fixture_path())
        .with_app_name("leptos-ssr-app")
        .with_mode(mode)
        .with_site_addr(site_addr.clone())
        .with_startup_timeout(
            Duration::from_secs(600),
            "Cold builds of the Leptos fixture (server + wasm) can take several minutes.",
        )
        .start()
        .await;

    let err = assert_that_owned!(result)
        .with_detail_message("startup on an occupied pinned port should fail")
        .get_err()
        .unwrap_inner();
    let context = err.current_context();
    assert_that!(matches!(
        context,
        LeptosBrowserTestError::PortInUse { site_addr: addr, .. } if *addr == site_addr
    ))
    .with_detail_message(format!(
        "expected PortInUse for {site_addr}, got: {context:?}"
    ))
    .is_true();

    let output = CapturedOutput::find_in(&err).expect("captured output attachment");
    let combined = format!("{}\n{}", output.stdout(), output.stderr()).to_ascii_lowercase();
    assert_that!(combined.as_str())
        .with_detail_message(format!(
            "expected the bind failure in the captured output: {output}"
        ))
        .contains("address already in use");

    drop(blocker);
}
