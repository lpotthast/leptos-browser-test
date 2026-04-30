use std::{io, net::TcpListener};

pub(crate) fn find_free_port() -> io::Result<u16> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    Ok(listener.local_addr()?.port())
}

pub(crate) fn find_free_port_excluding(excluded: Option<u16>) -> io::Result<u16> {
    for _ in 0..16 {
        let port = find_free_port()?;
        if Some(port) != excluded {
            return Ok(port);
        }
    }

    Err(io::Error::new(
        io::ErrorKind::AddrInUse,
        "free port allocator repeatedly returned the excluded port",
    ))
}

#[cfg(test)]
mod tests {
    use assertr::prelude::*;

    use super::{find_free_port, find_free_port_excluding};

    #[test]
    fn returns_a_port_when_no_exclusion() {
        let port = find_free_port_excluding(None).expect("a free port should be available");
        assert_that!(port).is_greater_than(0);
    }

    #[test]
    fn skips_the_excluded_port() {
        // Reserve a port to use as the "excluded" candidate. Holding the listener keeps the
        // OS from re-handing this exact port out via bind(0), so the returned port must differ.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind ephemeral");
        let excluded = listener.local_addr().expect("local_addr").port();

        for _ in 0..32 {
            let port = find_free_port_excluding(Some(excluded))
                .expect("free port allocator should succeed");
            assert_that!(port).is_not_equal_to(excluded);
        }
    }

    #[test]
    fn baseline_find_free_port_succeeds() {
        let port = find_free_port().expect("a free port should be available");
        assert_that!(port).is_greater_than(0);
    }
}
