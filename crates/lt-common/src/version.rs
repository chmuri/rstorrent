//! Single source of truth for how the client identifies itself.
//!
//! Every user-visible or protocol-visible version string is derived from
//! `CARGO_PKG_VERSION` so a release bump cannot leave a stale identifier
//! behind (peer ID, tracker User-Agent, DHT `v`, BEP 10 handshake, RPC).

/// Crate/workspace version, e.g. `0.12.0`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// HTTP User-Agent sent to trackers and web seeds.
pub const USER_AGENT: &str = concat!("rstorrent/", env!("CARGO_PKG_VERSION"));

/// Verbose User-Agent used by the generic HTTP client.
pub const USER_AGENT_FULL: &str = concat!("rstorrent/", env!("CARGO_PKG_VERSION"), " (Rust)");

/// BEP 10 extended handshake `v` value.
pub const EXTENDED_HANDSHAKE_VERSION: &str = concat!("rstorrent ", env!("CARGO_PKG_VERSION"));

/// BEP 5 KRPC `v` value reported to DHT nodes.
pub const DHT_CLIENT_VERSION: &str = concat!("RS", env!("CARGO_PKG_VERSION"));

/// Parses `major.minor.patch` from the compile-time version string.
///
/// `const fn` so [`PEER_ID_PREFIX`] can be built without runtime state.
const fn version_parts() -> (u8, u8, u8) {
    let bytes = env!("CARGO_PKG_VERSION").as_bytes();
    let mut parts = [0u8; 3];
    let mut part = 0usize;
    let mut index = 0usize;

    while index < bytes.len() && part < 3 {
        let byte = bytes[index];
        if byte == b'.' {
            part += 1;
        } else if byte.is_ascii_digit() {
            parts[part] = parts[part].saturating_mul(10).saturating_add(byte - b'0');
        }
        index += 1;
    }

    (parts[0], parts[1], parts[2])
}

/// Azureus-style peer ID prefix `-RS<major><minor:2><patch>-`.
///
/// `0.12.0` becomes `-RS0120-`. The four digits keep the historical
/// `major/minor(2)/patch` layout used by the previous `-RS0100-` value.
pub const fn peer_id_prefix() -> [u8; 8] {
    let (major, minor, patch) = version_parts();
    let mut prefix = *b"-RS0000-";
    prefix[3] = b'0' + (major % 10);
    prefix[4] = b'0' + ((minor / 10) % 10);
    prefix[5] = b'0' + (minor % 10);
    prefix[6] = b'0' + (patch % 10);
    prefix
}

/// Compile-time peer ID prefix, e.g. `-RS0120-`.
pub const PEER_ID_PREFIX: [u8; 8] = peer_id_prefix();

/// Generates a fresh 20-byte peer ID with the versioned prefix.
pub fn new_peer_id() -> [u8; 20] {
    let mut id = [0u8; 20];
    id[..8].copy_from_slice(&PEER_ID_PREFIX);
    // Random suffix; a failure leaves zeros, which is still a valid unique-enough
    // ID for local tests. Production callers use `getrandom` directly.
    let _ = getrandom::fill(&mut id[8..]);
    id
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifiers_track_the_crate_version() {
        assert_eq!(VERSION, env!("CARGO_PKG_VERSION"));
        assert!(USER_AGENT.starts_with("rstorrent/"));
        assert!(EXTENDED_HANDSHAKE_VERSION.starts_with("rstorrent "));
        assert!(DHT_CLIENT_VERSION.starts_with("RS"));

        let prefix = peer_id_prefix();
        assert_eq!(&prefix[..3], b"-RS");
        assert_eq!(prefix[7], b'-');
        assert!(prefix[3..7].iter().all(|b| b.is_ascii_digit()));
    }

    #[test]
    fn peer_id_prefix_encodes_the_version() {
        let mut parts = VERSION.split('.').map(|part| part.parse::<u8>().unwrap());
        let (major, minor, patch) = (
            parts.next().unwrap_or(0),
            parts.next().unwrap_or(0),
            parts.next().unwrap_or(0),
        );
        let expected = format!("-RS{major}{minor:02}{patch}-");
        assert_eq!(std::str::from_utf8(&PEER_ID_PREFIX).unwrap(), expected);
    }

    #[test]
    fn peer_id_has_prefix_and_random_suffix() {
        let first = new_peer_id();
        let second = new_peer_id();
        assert_eq!(&first[..8], &PEER_ID_PREFIX);
        assert_eq!(&second[..8], &PEER_ID_PREFIX);
        assert_ne!(first[8..], second[8..]);
    }
}
