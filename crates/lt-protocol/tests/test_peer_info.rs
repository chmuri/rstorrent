use std::net::{Ipv4Addr, SocketAddr};

use lt_common::hash_string::HashString;
use lt_net::addr::SocketAddress;
use lt_protocol::client_list::ClientType;
use lt_protocol::connection::{PeerConnection, PeerDirection, PeerFlags, PeerInfo};

fn peer_id_with_prefix(prefix: &[u8]) -> HashString {
    let mut bytes = [0u8; 20];
    bytes[..prefix.len()].copy_from_slice(prefix);
    for (index, byte) in bytes.iter_mut().enumerate().skip(prefix.len()) {
        *byte = (index as u8).wrapping_add(1);
    }
    HashString::new(bytes)
}

#[test]
fn peer_info_tracks_client_direction_and_counters() {
    let id = peer_id_with_prefix(b"-lt0100-");
    let address = SocketAddress::from(SocketAddr::from((Ipv4Addr::LOCALHOST, 51413)));

    let mut peer =
        PeerConnection::new_with_direction(id, address, 8, PeerDirection::Incoming, true);

    assert!(peer.is_incoming());
    assert!(!peer.is_outgoing());
    assert!(peer.is_encrypted);
    assert_eq!(peer.client.name, "rTorrent");
    assert!(peer.client_version().starts_with("rTorrent"));

    peer.add_downloaded(16 * 1024);
    peer.add_uploaded(4 * 1024);

    assert_eq!(peer.downloaded, 16 * 1024);
    assert_eq!(peer.uploaded, 4 * 1024);
    assert!(peer.rate_down.total() >= 16 * 1024);
    assert!(peer.rate_up.total() >= 4 * 1024);
}

#[test]
fn outgoing_is_the_default_direction() {
    let peer = PeerConnection::new(
        HashString::new([0x42; 20]),
        SocketAddress::from(SocketAddr::from((Ipv4Addr::LOCALHOST, 6881))),
        1,
    );

    assert!(!peer.is_incoming());
    assert!(!peer.is_encrypted);
    assert_eq!(peer.direction, PeerDirection::Outgoing);
    assert!(peer.is_connected);
}

#[test]
fn snapshot_is_owned_and_captures_flags_and_counters() {
    let id = peer_id_with_prefix(b"-qB4600-");
    let address = SocketAddress::from(SocketAddr::from((Ipv4Addr::LOCALHOST, 51413)));
    let mut peer =
        PeerConnection::new_with_direction(id, address.clone(), 8, PeerDirection::Incoming, true);

    peer.set_snubbed(true);
    peer.set_preferred(true);
    peer.set_banned(true);
    peer.set_unwanted(true);
    peer.set_obfuscated(true);
    peer.set_peer_choked(false);
    peer.set_peer_interested(true);
    peer.add_downloaded(1234);
    peer.add_uploaded(5678);
    peer.inc_failed_counter();
    peer.inc_transfer_counter();

    let info: PeerInfo = peer.info();
    assert_eq!(info.peer_id, id);
    assert_eq!(info.socket_address, address);
    assert_eq!(info.direction, PeerDirection::Incoming);
    assert!(info.is_incoming());
    assert!(info.is_encrypted);
    assert!(info.is_obfuscated);
    assert!(info.is_snubbed);
    assert!(info.is_preferred);
    assert!(info.is_banned);
    assert!(info.is_unwanted);
    assert_eq!(info.downloaded, 1234);
    assert_eq!(info.uploaded, 5678);
    assert_eq!(info.failed_counter, 1);
    assert_eq!(info.transfer_counter, 1);
    assert_eq!(info.queue_depth, 0);
    assert_eq!(info.flags(), peer.flags());

    // The snapshot must not track later mutations of the live session.
    peer.add_downloaded(10_000);
    peer.set_snubbed(false);
    assert_eq!(info.downloaded, 1234);
    assert!(info.is_snubbed);
    assert_eq!(peer.snapshot().downloaded, 11_234);
    assert_eq!(peer.to_peer_info().downloaded, 11_234);
}

#[test]
fn flags_expose_legacy_bits() {
    let mut peer = PeerConnection::new(
        HashString::new([0x44; 20]),
        SocketAddress::from(SocketAddr::from((Ipv4Addr::LOCALHOST, 6881))),
        1,
    );
    peer.set_direction(PeerDirection::Incoming);
    peer.set_preferred(true);
    peer.set_snubbed(true);

    let flags: PeerFlags = peer.flags();
    assert!(flags.connected);
    assert!(flags.incoming);
    assert!(flags.preferred);
    assert!(flags.snubbed);
    assert_eq!(flags.bits() & 0b1, 0b1);
    assert_ne!(flags.bits() & (1 << 1), 0);
    assert_ne!(flags.bits() & (1 << 6), 0);
    assert_ne!(flags.bits() & (1 << 7), 0);
}

#[test]
fn counter_arithmetic_saturates() {
    let mut peer = PeerConnection::new(
        HashString::new([0x55; 20]),
        SocketAddress::from(SocketAddr::from((Ipv4Addr::LOCALHOST, 6881))),
        1,
    );

    peer.uploaded = u64::MAX - 1;
    peer.downloaded = u64::MAX - 1;
    peer.add_uploaded(10);
    peer.add_downloaded(10);
    assert_eq!(peer.uploaded, u64::MAX);
    assert_eq!(peer.downloaded, u64::MAX);

    peer.failed_counter = u32::MAX;
    peer.inc_failed_counter();
    assert_eq!(peer.failed_counter, u32::MAX);

    peer.transfer_counter = u32::MAX;
    peer.inc_transfer_counter();
    assert_eq!(peer.transfer_counter, u32::MAX);
    peer.transfer_counter = 0;
    peer.dec_transfer_counter();
    assert_eq!(peer.transfer_counter, 0);
}

#[test]
fn malformed_client_ids_are_tolerated() {
    // Arbitrary binary data must not panic or be interpreted as a known client.
    let bytes: Vec<u8> = (0u8..20).map(|value| value.wrapping_mul(13)).collect();
    let mut id = HashString::default();
    id.as_mut_slice().copy_from_slice(&bytes);
    let peer = PeerConnection::new(
        id,
        SocketAddress::from(SocketAddr::from((Ipv4Addr::LOCALHOST, 6881))),
        1,
    );

    assert_eq!(peer.client.name, "Unknown");
    assert_eq!(peer.client.client_type, ClientType::Unknown);
    assert_eq!(peer.client_version(), "Unknown");

    // A Mainline-shaped prefix with a non-digit version byte must not panic in
    // the legacy parser's subtraction path.
    let mut mainline_shaped = [0u8; 20];
    mainline_shaped[..7].copy_from_slice(b"M1-\0-5-");
    let peer = PeerConnection::new(
        HashString::new(mainline_shaped),
        SocketAddress::from(SocketAddr::from((Ipv4Addr::LOCALHOST, 6881))),
        1,
    );
    assert_eq!(peer.client.name, "Unknown");
}

#[test]
fn handshake_state_and_support_flags_are_tracked() {
    let mut peer = PeerConnection::new(
        HashString::new([0x66; 20]),
        SocketAddress::from(SocketAddr::from((Ipv4Addr::LOCALHOST, 6881))),
        1,
    );

    peer.mark_handshake_started();
    assert!(peer.is_handshake);
    peer.mark_handshake_complete();
    assert!(!peer.is_handshake);

    peer.options = [0, 0, 0, 0, 0, 0x10, 0, 0x01];
    assert!(peer.supports_extensions());
    assert!(peer.supports_dht());
}
