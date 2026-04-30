/// URL scheme used for the browser-facing Leptos app URL.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SiteScheme {
    /// Use `http://`.
    Http,

    /// Use `https://`.
    Https,
}

impl SiteScheme {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Http => "http",
            Self::Https => "https",
        }
    }
}

pub(crate) fn parse_socket_addr(addr: &str) -> Option<std::net::SocketAddr> {
    addr.parse().ok()
}

pub(crate) fn format_base_url(site_scheme: SiteScheme, site_addr: &str) -> String {
    format!("{}://{site_addr}", site_scheme.as_str())
}

#[cfg(test)]
mod tests {
    use assertr::prelude::*;

    use super::{SiteScheme, format_base_url, parse_socket_addr};

    #[test]
    fn formats_http_base_url() {
        assert_that!(format_base_url(SiteScheme::Http, "127.0.0.1:3000"))
            .is_equal_to("http://127.0.0.1:3000");
    }

    #[test]
    fn formats_https_base_url() {
        assert_that!(format_base_url(SiteScheme::Https, "127.0.0.1:3000"))
            .is_equal_to("https://127.0.0.1:3000");
    }

    #[test]
    fn parses_valid_ipv4_socket_addr() {
        let parsed = parse_socket_addr("127.0.0.1:3000").expect("valid ipv4 socket addr");
        assert_that!(parsed.port()).is_equal_to(3000);
    }

    #[test]
    fn parses_valid_ipv6_socket_addr() {
        let parsed = parse_socket_addr("[::1]:3000").expect("valid ipv6 socket addr");
        assert_that!(parsed.port()).is_equal_to(3000);
    }

    #[test]
    fn rejects_addr_without_port() {
        assert_that!(parse_socket_addr("127.0.0.1")).is_equal_to(None);
        assert_that!(parse_socket_addr("[::1]")).is_equal_to(None);
    }

    #[test]
    fn rejects_garbage_input() {
        assert_that!(parse_socket_addr("not-a-socket")).is_equal_to(None);
    }
}
