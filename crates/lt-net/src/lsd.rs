//! Local Peer Discovery (LSD - BEP 14).
//!
//! Discovers peers on the local area network using UDP multicast on
//! `239.192.152.143:6771`.

use std::net::Ipv4Addr;

use lt_common::hash_string::HashString;

/// Default IPv4 multicast group for BitTorrent Local Peer Discovery.
pub const LSD_MULTICAST_IPV4: Ipv4Addr = Ipv4Addr::new(239, 192, 152, 143);

/// Standard port for Local Peer Discovery.
pub const LSD_PORT: u16 = 6771;

/// Parsed Local Peer Discovery announcement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LsdPacket {
    pub port: u16,
    pub info_hashes: Vec<HashString>,
    pub cookie: Option<String>,
}

/// Formats a BEP 14 announcement packet.
pub fn format_lsd_packet(port: u16, info_hashes: &[HashString], cookie: Option<&str>) -> Vec<u8> {
    let mut msg = format!(
        "BT-SEARCH * HTTP/1.1\r\nHost: {}:{}\r\nPort: {}\r\n",
        LSD_MULTICAST_IPV4, LSD_PORT, port
    );

    for ih in info_hashes {
        msg.push_str(&format!("Infohash: {}\r\n", ih.to_hex()));
    }

    if let Some(c) = cookie {
        msg.push_str(&format!("cookie: {}\r\n", c));
    }

    msg.push_str("\r\n\r\n");
    msg.into_bytes()
}

/// Parses an incoming BEP 14 announcement packet.
pub fn parse_lsd_packet(buf: &[u8]) -> Option<LsdPacket> {
    let text = std::str::from_utf8(buf).ok()?;

    let mut lines = text.lines();
    let request_line = lines.next()?.trim();
    if !request_line.starts_with("BT-SEARCH * HTTP/1.1") {
        return None;
    }

    let mut port = None;
    let mut info_hashes = Vec::new();
    let mut cookie = None;

    for line in lines {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        if let Some((header, value)) = line.split_once(':') {
            let header = header.trim();
            let value = value.trim();

            if header.eq_ignore_ascii_case("Port") {
                if let Ok(p) = value.parse::<u16>() {
                    port = Some(p);
                }
            } else if header.eq_ignore_ascii_case("Infohash") || header.eq_ignore_ascii_case("ih") {
                if let Some(hash) = HashString::from_hex(value) {
                    info_hashes.push(hash);
                }
            } else if header.eq_ignore_ascii_case("cookie") {
                cookie = Some(value.to_string());
            }
        }
    }

    let port = port?;
    if info_hashes.is_empty() {
        return None;
    }

    Some(LsdPacket {
        port,
        info_hashes,
        cookie,
    })
}

/// Creates and binds a UDP socket for BEP 14 Local Peer Discovery.
///
/// Binds with `SO_REUSEADDR` and `SO_REUSEPORT` to port 6771 and joins the
/// multicast group `239.192.152.143`. If port 6771 cannot be bound, it falls
/// back to an ephemeral port for broadcasting.
#[cfg(unix)]
pub fn create_lsd_socket() -> std::io::Result<tokio::net::UdpSocket> {
    use std::os::unix::io::FromRawFd;

    unsafe {
        let fd = libc::socket(libc::AF_INET, libc::SOCK_DGRAM, 0);
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }

        let one: libc::c_int = 1;
        libc::setsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_REUSEADDR,
            &one as *const _ as *const libc::c_void,
            std::mem::size_of_val(&one) as libc::socklen_t,
        );

        #[cfg(target_os = "linux")]
        libc::setsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_REUSEPORT,
            &one as *const _ as *const libc::c_void,
            std::mem::size_of_val(&one) as libc::socklen_t,
        );

        let mut addr: libc::sockaddr_in = std::mem::zeroed();
        addr.sin_family = libc::AF_INET as libc::sa_family_t;
        addr.sin_port = LSD_PORT.to_be();
        addr.sin_addr.s_addr = libc::INADDR_ANY.to_be();

        let res = libc::bind(
            fd,
            &addr as *const _ as *const libc::sockaddr,
            std::mem::size_of::<libc::sockaddr_in>() as libc::socklen_t,
        );

        let std_sock = if res == 0 {
            std::net::UdpSocket::from_raw_fd(fd)
        } else {
            libc::close(fd);
            std::net::UdpSocket::bind("0.0.0.0:0")?
        };

        std_sock.set_nonblocking(true)?;
        let _ = std_sock.join_multicast_v4(&LSD_MULTICAST_IPV4, &std::net::Ipv4Addr::UNSPECIFIED);
        tokio::net::UdpSocket::from_std(std_sock)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_and_parse_roundtrip() {
        let ih1 = HashString::from_hex("0123456789abcdef0123456789abcdef01234567").unwrap();
        let ih2 = HashString::from_hex("abcdef0123456789abcdef0123456789abcdef01").unwrap();
        let hashes = vec![ih1, ih2];

        let packet_bytes = format_lsd_packet(51413, &hashes, Some("my_cookie_123"));
        let parsed = parse_lsd_packet(&packet_bytes).expect("should parse correctly");

        assert_eq!(parsed.port, 51413);
        assert_eq!(parsed.cookie.as_deref(), Some("my_cookie_123"));
        assert_eq!(parsed.info_hashes, hashes);
    }

    #[test]
    fn rejects_invalid_request_line() {
        let bad = b"GET / HTTP/1.1\r\nHost: 239.192.152.143:6771\r\nPort: 6881\r\n\r\n";
        assert_eq!(parse_lsd_packet(bad), None);
    }

    #[test]
    fn parses_case_insensitive_headers() {
        let raw = b"BT-SEARCH * HTTP/1.1\r\nhost: 239.192.152.143:6771\r\nport: 7000\r\nih: 0123456789abcdef0123456789abcdef01234567\r\n\r\n";
        let parsed = parse_lsd_packet(raw).unwrap();
        assert_eq!(parsed.port, 7000);
        assert_eq!(parsed.info_hashes.len(), 1);
        assert_eq!(parsed.cookie, None);
    }
}
