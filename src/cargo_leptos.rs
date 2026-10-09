use std::time::Duration;
use std::{
    env,
    ffi::{OsStr, OsString},
    path::Path,
};
use tokio::process::Command;
use tokio_process_tools::UnixGracefulSignal;

/// `cargo leptos` mode used to start the test app.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CargoLeptosMode {
    /// Run `cargo leptos serve`.
    ///
    /// Prefer this over `Watch`. The watch option is only here for completeness in cases anyone
    /// wants/needs this. You typically do NOT want changes to the underlying app code to affect
    /// your tests in any way, so pick `Serve`.
    Serve,

    /// Run `cargo leptos watch`.
    Watch,
}

impl CargoLeptosMode {
    pub(crate) fn as_arg(self) -> &'static str {
        match self {
            Self::Serve => "serve",
            Self::Watch => "watch",
        }
    }
}

/// The build profile `cargo leptos` builds the app with.
///
/// The app's `Cargo.toml` decides what each profile means for its server and WASM builds.
/// Set `[package.metadata.leptos]`: `bin-profile-dev`/`lib-profile-dev` and
/// `bin-profile-release`/`lib-profile-release` to be explicit about the profiles used. Note that
/// you can configure a custom "wasm-release" profile for example.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum BuildProfile {
    /// Build without `--release`. The default.
    #[default]
    Dev,

    /// Build with `--release` optimizations. This can lead to a much smaller and faster WASM
    /// bundle. Note that every browser test that loads a page (when no caching is active)
    /// downloads, compiles and runs it. The Leptos server will also SSR-render pages notably
    /// faster.
    Release,
}

fn resolve_cargo_bin(cargo_bin: Option<&OsStr>) -> OsString {
    cargo_bin
        .map(OsStr::to_owned)
        .or_else(|| env::var_os("CARGO"))
        .unwrap_or_else(|| OsString::from("cargo"))
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn command(
    mode: CargoLeptosMode,
    build_profile: BuildProfile,
    cargo_bin: Option<&OsStr>,
    app_dir: &Path,
    site_addr: &str,
    reload_port: u16,
    graceful_shutdown_timeout: Duration,
    graceful_shutdown_unix_signal: UnixGracefulSignal,
    extra_env: &[(OsString, OsString)],
) -> Command {
    let cargo = resolve_cargo_bin(cargo_bin);
    let mut cmd = Command::new(&cargo);
    cmd.arg("leptos").arg(mode.as_arg());
    if build_profile == BuildProfile::Release {
        cmd.arg("--release");
    }
    cmd.env("LEPTOS_SITE_ADDR", site_addr)
        .env("LEPTOS_RELOAD_PORT", reload_port.to_string())
        .env(
            "LEPTOS_GRACEFUL_SHUTDOWN_TIMEOUT_SECS",
            graceful_shutdown_timeout.as_secs().to_string(),
        )
        .env(
            "LEPTOS_GRACEFUL_SHUTDOWN_UNIX_SIGNAL",
            match graceful_shutdown_unix_signal {
                UnixGracefulSignal::Interrupt => "SIGINT",
                UnixGracefulSignal::Terminate => "SIGTERM",
                _ => unreachable!(
                    "tokio-process-tools added a new UnixGracefulSignal variant. \
                     Extend this match to map it to the signal label expected by cargo-leptos."
                ),
            },
        )
        .current_dir(app_dir);

    if env::var_os("RUST_BACKTRACE").is_none() {
        cmd.env("RUST_BACKTRACE", "1");
    }

    for (key, value) in extra_env {
        cmd.env(key, value);
    }

    cmd
}

#[cfg(test)]
pub(crate) fn cargo_program(cmd: &Command) -> &OsStr {
    cmd.as_std().get_program()
}

#[cfg(test)]
mod tests {
    use super::{BuildProfile, CargoLeptosMode, cargo_program, command};
    use assertr::prelude::*;
    use std::time::Duration;
    use std::{ffi::OsStr, path::Path};
    use tokio_process_tools::UnixGracefulSignal;

    fn env_pairs(cmd: &tokio::process::Command) -> Vec<(String, Option<String>)> {
        cmd.as_std()
            .get_envs()
            .map(|(k, v)| {
                (
                    k.to_string_lossy().into_owned(),
                    v.map(|val| val.to_string_lossy().into_owned()),
                )
            })
            .collect()
    }

    fn args(cmd: &tokio::process::Command) -> Vec<String> {
        cmd.as_std()
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn defaults_to_cargo_program() {
        // Snapshot whatever CARGO is in the parent env so the assertion is deterministic.
        let expected =
            std::env::var_os("CARGO").unwrap_or_else(|| std::ffi::OsString::from("cargo"));
        let cmd = command(
            CargoLeptosMode::Serve,
            BuildProfile::Dev,
            None,
            Path::new("."),
            "127.0.0.1:3000",
            3001,
            Duration::from_secs(10),
            UnixGracefulSignal::Interrupt,
            &[],
        );
        assert_that!(cargo_program(&cmd)).is_equal_to(expected.as_os_str());
    }

    #[test]
    fn with_cargo_overrides_program() {
        let cmd = command(
            CargoLeptosMode::Watch,
            BuildProfile::Dev,
            Some(OsStr::new("/usr/local/bin/my-cargo")),
            Path::new("."),
            "127.0.0.1:3000",
            3001,
            Duration::from_secs(10),
            UnixGracefulSignal::Interrupt,
            &[],
        );
        assert_that!(cargo_program(&cmd)).is_equal_to(OsStr::new("/usr/local/bin/my-cargo"));
    }

    #[test]
    fn passes_mode_arg() {
        let serve = command(
            CargoLeptosMode::Serve,
            BuildProfile::Dev,
            None,
            Path::new("."),
            "127.0.0.1:3000",
            3001,
            Duration::from_secs(10),
            UnixGracefulSignal::Interrupt,
            &[],
        );
        assert_that!(args(&serve)).is_equal_to(vec!["leptos".to_owned(), "serve".to_owned()]);

        let watch = command(
            CargoLeptosMode::Watch,
            BuildProfile::Dev,
            None,
            Path::new("."),
            "127.0.0.1:3000",
            3001,
            Duration::from_secs(10),
            UnixGracefulSignal::Interrupt,
            &[],
        );
        assert_that!(args(&watch)).is_equal_to(vec!["leptos".to_owned(), "watch".to_owned()]);
    }

    #[test]
    fn release_profile_passes_release_flag() {
        let cmd = command(
            CargoLeptosMode::Serve,
            BuildProfile::Release,
            None,
            Path::new("."),
            "127.0.0.1:3000",
            3001,
            Duration::from_secs(10),
            UnixGracefulSignal::Interrupt,
            &[],
        );
        assert_that!(args(&cmd)).is_equal_to(vec![
            "leptos".to_owned(),
            "serve".to_owned(),
            "--release".to_owned(),
        ]);
    }

    #[test]
    fn sets_framework_env_vars() {
        let cmd = command(
            CargoLeptosMode::Serve,
            BuildProfile::Dev,
            None,
            Path::new("."),
            "127.0.0.1:3000",
            3001,
            Duration::from_secs(10),
            UnixGracefulSignal::Interrupt,
            &[],
        );
        let envs = env_pairs(&cmd);
        assert_that!(&envs).contains((
            "LEPTOS_SITE_ADDR".to_string(),
            Some("127.0.0.1:3000".to_string()),
        ));
        assert_that!(&envs).contains(("LEPTOS_RELOAD_PORT".to_string(), Some("3001".to_string())));
        assert_that!(&envs).contains((
            "LEPTOS_GRACEFUL_SHUTDOWN_TIMEOUT_SECS".to_string(),
            Some("10".to_string()),
        ));
        assert_that!(&envs).contains((
            "LEPTOS_GRACEFUL_SHUTDOWN_UNIX_SIGNAL".to_string(),
            Some("SIGINT".to_string()),
        ));
    }

    #[test]
    fn extra_env_overrides_framework_env() {
        let cmd = command(
            CargoLeptosMode::Serve,
            BuildProfile::Dev,
            None,
            Path::new("."),
            "127.0.0.1:3000",
            3001,
            Duration::from_secs(10),
            UnixGracefulSignal::Interrupt,
            &[("LEPTOS_SITE_ADDR".into(), "127.0.0.1:9999".into())],
        );
        let envs = env_pairs(&cmd);
        let overrides = envs
            .iter()
            .filter(|(k, _)| k == "LEPTOS_SITE_ADDR")
            .collect::<Vec<_>>();
        assert_that!(overrides.last().and_then(|(_, v)| v.as_deref()))
            .is_equal_to(Some("127.0.0.1:9999"));
    }
}
