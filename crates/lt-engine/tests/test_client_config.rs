use std::net::{Ipv4Addr, SocketAddr};

use lt_common::hash_string::HashString;
use lt_crypto::sha1::sha1_hash;
use lt_engine::client_config::ClientConfigError;
use lt_engine::constructor::{TorrentFileEntry, TorrentMetadata};
use lt_engine::download::{Download, PeerAddError};
use lt_net::addr::SocketAddress;
use lt_protocol::connection::{PeerConnection, PeerDirection};

fn metadata() -> TorrentMetadata {
    let piece_hash = sha1_hash(&vec![0x55u8; 16384]);
    let mut path = lt_common::path::Path::new();
    path.push("payload.bin");

    TorrentMetadata {
        info_hash: HashString::new(sha1_hash(&piece_hash)),
        name: "client-config".to_string(),
        piece_length: 16384,
        pieces: piece_hash.to_vec(),
        num_pieces: 1,
        total_size: 16384,
        is_multi_file: false,
        files: vec![TorrentFileEntry {
            path,
            length: 16384,
            offset: 0,
        }],
        announce: None,
        announce_list: Vec::new(),
        is_private: false,
        is_meta_download: false,
    }
}

fn peer(id_byte: u8, seeder: bool) -> PeerConnection {
    let mut peer = PeerConnection::new_with_direction(
        HashString::new([id_byte; 20]),
        SocketAddress::from(SocketAddr::from((Ipv4Addr::LOCALHOST, 6881))),
        1,
        PeerDirection::Outgoing,
        false,
    );
    if seeder {
        peer.bitfield.set(0);
    }
    peer
}

fn peer_id(id_byte: u8) -> HashString {
    HashString::new([id_byte; 20])
}

#[test]
fn max_peers_is_enforced() {
    let mut download = Download::new(metadata());
    download.client_config_mut().set_max_peers(2);

    assert!(download.add_peer(peer(0x01, false)));
    assert!(download.add_peer(peer(0x02, false)));
    assert!(!download.add_peer(peer(0x03, false)));
    assert_eq!(download.connection_list().size(), 2);
}

#[test]
fn max_seed_peers_is_enforced() {
    let mut download = Download::new(metadata());
    download.client_config_mut().set_max_peers_seed(0);

    assert!(!download.add_peer(peer(0x11, true)));
    assert!(download.add_peer(peer(0x12, false)));
}

#[test]
fn seed_limit_allows_unlimited_when_negative() {
    let mut download = Download::new(metadata());
    download.client_config_mut().set_max_peers_seed(-1);

    assert!(download.add_peer(peer(0x21, true)));
}

#[test]
fn hard_connection_limit_is_enforced_on_top_of_max_peers() {
    let mut download = Download::new(metadata());
    download.client_config_mut().set_max_peers(10);
    download.client_config_mut().set_max_connections(1);

    assert_eq!(download.peer_limit(), 1);
    assert!(download.can_accept_incoming());
    assert_eq!(download.try_add_peer(peer(0x31, false)), Ok(()));
    assert!(!download.can_accept_incoming());
    assert!(!download.can_dial_outgoing());
    assert_eq!(
        download.try_add_peer(peer(0x32, false)),
        Err(PeerAddError::ConnectionLimit)
    );
}

#[test]
fn explicit_connection_list_limit_is_respected() {
    let mut download = Download::new(metadata());
    download.connection_list_mut().set_max_size(1).unwrap();

    assert!(download.add_peer(peer(0x35, false)));
    assert!(!download.add_peer(peer(0x36, false)));
    assert_eq!(download.connection_list().size(), 1);
}

#[test]
fn duplicate_peer_id_is_rejected() {
    let mut download = Download::new(metadata());

    assert!(download.add_peer(peer(0x41, false)));
    assert_eq!(
        download.try_add_peer(peer(0x41, false)),
        Err(PeerAddError::DuplicatePeer)
    );
}

#[test]
fn incoming_direction_survives_admission() {
    let mut download = Download::new(metadata());
    let id = peer_id(0x51);
    let address = SocketAddress::from(SocketAddr::from((Ipv4Addr::LOCALHOST, 51413)));

    assert!(download.add_incoming_peer(id, address.clone(), 1, true));

    let info = download.peer_info(&id).expect("peer snapshot");
    assert_eq!(info.direction, PeerDirection::Incoming);
    assert!(info.is_incoming());
    assert!(info.is_encrypted);

    // The live connection keeps the same incoming bit.
    assert!(download
        .connection_list()
        .find_by_id(&id)
        .unwrap()
        .is_incoming());
}

#[test]
fn add_outgoing_peer_keeps_default_direction() {
    let mut download = Download::new(metadata());
    let id = peer_id(0x52);
    let address = SocketAddress::from(SocketAddr::from((Ipv4Addr::LOCALHOST, 51414)));

    assert!(download.add_outgoing_peer(id, address, 1, false));
    let info = download.peer_info(&id).unwrap();
    assert!(info.is_outgoing());
    assert!(!info.is_encrypted);
}

#[test]
fn max_uploads_is_enforced_by_unchoke() {
    let mut download = Download::new(metadata());
    download.client_config_mut().set_max_uploads(1);

    let first = peer_id(0x61);
    let second = peer_id(0x62);
    assert!(download.add_peer(peer(0x61, false)));
    assert!(download.add_peer(peer(0x62, false)));

    assert!(download.unchoke_peer(&first));
    assert!(!download.unchoke_peer(&second));
    assert_eq!(download.unchoked_peer_count(), 1);

    // A peer already unchoked can be updated idempotently.
    assert!(download.unchoke_peer(&first));

    assert!(download.choke_peer(&first));
    assert!(download.unchoke_peer(&second));
    assert_eq!(download.unchoked_peer_count(), 1);
}

#[test]
fn malformed_client_id_is_accepted() {
    let mut download = Download::new(metadata());
    let mut id = HashString::default();
    let bytes: Vec<u8> = (0u8..20).map(|value| value.wrapping_mul(7)).collect();
    id.as_mut_slice().copy_from_slice(&bytes);
    let address = SocketAddress::from(SocketAddr::from((Ipv4Addr::LOCALHOST, 51515)));

    assert!(download.add_peer(PeerConnection::new(id, address, 1)));
    let info = download.peer_info(&id).expect("malformed id snapshot");
    assert_eq!(info.client.name, "Unknown");
}

#[test]
fn removing_a_peer_cancels_outstanding_transfers() {
    let mut download = Download::new(metadata());
    let id = peer_id(0x71);
    assert!(download.add_peer(peer(0x71, true)));

    let pieces = download.delegate_for_peer(&id, 1);
    assert!(!pieces.is_empty());
    assert!(download.peer_queue_depth(&id) > 0);

    assert!(download.remove_peer(&id));
    assert_eq!(download.peer_queue_depth(&id), 0);
    assert_eq!(download.connection_list().size(), 0);
    assert!(download.peer_info(&id).is_none());

    // A second removal is a no-op rather than a panic.
    assert!(!download.remove_peer(&id));
}

#[test]
fn handshake_timeout_sweeps_only_pending_peers() {
    let mut download = Download::new(metadata());
    download.client_config_mut().set_handshake_timeout_secs(1);

    let pending = peer_id(0x81);
    let connected = peer_id(0x82);
    assert!(download.add_peer(peer(0x81, false)));
    assert!(download.add_peer(peer(0x82, false)));

    assert!(download
        .connection_list_mut()
        .find_by_id_mut(&pending)
        .map(|peer| peer.mark_handshake_started())
        .is_some());

    assert_eq!(download.sweep_peer_timeouts(u64::MAX), 1);
    assert!(download.connection_list().find_by_id(&pending).is_none());
    assert!(download.connection_list().find_by_id(&connected).is_some());
}

#[test]
fn listen_port_range_is_validated() {
    let mut config = lt_engine::client_config::ClientConfig::default();
    assert_eq!(config.listen_port_range(), (6881, 6999));
    assert_eq!(config.set_listen_port_range(7000, 7100), Ok(()));
    assert_eq!(
        config.set_listen_port_range(0, 7100),
        Err(ClientConfigError::InvalidListenPortRange {
            first: 0,
            last: 7100
        })
    );
}
