use std::fmt;

/// Which port a startup attempt failed to bind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PortCollision {
    /// The app could not bind an address, most likely its site address. The output doesn't
    /// reliably say which one.
    App,
    /// cargo-leptos (watch mode) could not bind the reload port.
    Reload,
}

impl PortCollision {
    /// Classifies a single output line as a port-bind collision, if it is one.
    pub(crate) fn detect(line: &str) -> Option<Self> {
        let lowered = line.to_ascii_lowercase();
        // cargo-leptos probes the reload port before binding it and reports
        // "Reload TCP port 127.0.0.1:3001 already in use. ..." when it is taken.
        if lowered.contains("reload tcp port") && lowered.contains("already in use") {
            return Some(Self::Reload);
        }
        // Linux/macOS: "Address already in use" / "address already in use (os error 48)"
        // Windows:     "Only one usage of each socket address (protocol/network address/port)"
        if lowered.contains("address already in use")
            || lowered.contains("only one usage of each socket address")
        {
            return Some(Self::App);
        }
        None
    }
}

/// A port collision found in the output, with the line that reported it.
///
/// Attached to [`PortInUse`](crate::LeptosBrowserTestError::PortInUse) reports. The line is the
/// only hint at which port failed, and it may have dropped out of the captured output tail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DetectedCollision {
    pub(crate) kind: PortCollision,
    pub(crate) line: String,
}

impl fmt::Display for DetectedCollision {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.kind {
            PortCollision::App => write!(f, "The app reported an address already in use: "),
            PortCollision::Reload => write!(f, "cargo-leptos reported the reload port as in use: "),
        }?;
        f.write_str(&self.line)
    }
}

#[cfg(test)]
mod tests {
    use assertr::prelude::*;

    use super::PortCollision;

    #[test]
    fn detects_unix_address_already_in_use() {
        assert_that!(PortCollision::detect(
            "Error: Address already in use (os error 48)"
        ))
        .is_equal_to(Some(PortCollision::App));
    }

    #[test]
    fn detects_lowercase_address_already_in_use() {
        assert_that!(PortCollision::detect(
            "thread 'main' panicked: address already in use"
        ))
        .is_equal_to(Some(PortCollision::App));
    }

    #[test]
    fn detects_windows_phrasing() {
        assert_that!(PortCollision::detect(
            "Only one usage of each socket address (protocol/network address/port) is normally permitted"
        ))
        .is_equal_to(Some(PortCollision::App));
    }

    #[test]
    fn detects_cargo_leptos_reload_port_collision() {
        assert_that!(PortCollision::detect(
            "Reload TCP port 127.0.0.1:3001 already in use. You can set the port in the server integration's RenderOptions reload_port"
        ))
        .is_equal_to(Some(PortCollision::Reload));
    }

    #[test]
    fn rejects_unrelated_errors() {
        assert_that!(PortCollision::detect(
            "error: linking with `cc` failed: exit status: 1"
        ))
        .is_equal_to(None);
        assert_that!(PortCollision::detect("")).is_equal_to(None);
    }
}
