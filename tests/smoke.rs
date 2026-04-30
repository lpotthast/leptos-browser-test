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

use leptos_browser_test::LeptosTestAppConfig;

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
        .with_forward_logs(false)
        .with_startup_timeout(
            Duration::from_secs(600),
            "Cold builds of the Leptos fixture (server + wasm) can take several minutes.",
        )
        .start()
        .await
        .expect("cargo leptos serve should bring up the SSR fixture");

    assert!(
        app.base_url().starts_with("http://127.0.0.1:"),
        "unexpected base_url: {}",
        app.base_url(),
    );
    assert_eq!(app.base_url(), &format!("http://{}", app.site_addr()));
    assert!(app.reload_port() > 0);
    assert!(app.app_dir().is_absolute());

    let body = http_get(&format!("{}/", app.base_url()))
        .await
        .expect("homepage should respond with 200");
    assert!(
        body.contains("Welcome to Leptos!"),
        "expected SSR-rendered greeting in body; got: {body}",
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn http_request_fails_after_drop() {
    let app = LeptosTestAppConfig::new(fixture_path())
        .with_app_name("leptos-ssr-app")
        .with_forward_logs(false)
        .with_startup_timeout(
            Duration::from_secs(600),
            "Cold builds of the Leptos fixture (server + wasm) can take several minutes.",
        )
        .with_termination_timeouts(
            Duration::from_millis(500),
            Duration::from_secs(3),
            "Tighten the post-drop kill chain so the test isn't slow on success.",
        )
        .start()
        .await
        .expect("start");

    let url = format!("{}/", app.base_url());
    assert!(
        http_get(&url).await.is_ok(),
        "server should respond before drop"
    );

    drop(app);

    // After drop, requests should eventually fail. The drop-time kill window is bounded by
    // (interrupt_timeout + terminate_timeout); allow generous slack for OS-level teardown.
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline {
        if http_get(&url).await.is_err() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("server still responding to {url} more than 15s after drop");
}

#[tokio::test(flavor = "multi_thread")]
async fn surfaces_startup_timeout_with_log_tail() {
    // Tight timeout so cargo-leptos cannot possibly finish compiling and start serving.
    // The expected_line ("listening on http://...") will not appear in time, and the retry
    // loop should NOT kick in because the failure stderr won't contain a port-bind error.
    let reason = "Tight bound that cargo-leptos cannot meet, used to exercise the timeout path.";
    let result = LeptosTestAppConfig::new(fixture_path())
        .with_app_name("leptos-ssr-app")
        .with_forward_logs(false)
        .with_startup_timeout(Duration::from_millis(250), reason)
        .start()
        .await;

    let Err(err) = result else {
        panic!("250ms startup_timeout should not be enough to compile cargo-leptos")
    };
    let display = err.current_context().to_string();
    assert!(
        display.contains("did not start within"),
        "expected timeout message; got: {display}",
    );
    // The reason flows through into the rendered error so future debuggers see *why* the
    // timeout was set, not just the elapsed duration.
    assert!(
        display.contains(reason),
        "expected timeout reason in error; got: {display}",
    );
}
