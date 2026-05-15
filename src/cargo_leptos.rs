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

fn resolve_cargo_bin(cargo_bin: Option<&OsStr>) -> OsString {
    cargo_bin
        .map(OsStr::to_owned)
        .or_else(|| env::var_os("CARGO"))
        .unwrap_or_else(|| OsString::from("cargo"))
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn command(
    mode: CargoLeptosMode,
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
    cmd.arg("leptos")
        .arg(mode.as_arg())
        .env("LEPTOS_SITE_ADDR", site_addr)
        .env("LEPTOS_RELOAD_PORT", reload_port.to_string())
        // These require https://github.com/leptos-rs/cargo-leptos/pull/648 to land.
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
    use super::{CargoLeptosMode, cargo_program, command};
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
    fn sets_framework_env_vars() {
        let cmd = command(
            CargoLeptosMode::Serve,
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
