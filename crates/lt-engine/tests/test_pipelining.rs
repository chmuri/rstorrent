use std::net::{Ipv4Addr, SocketAddr};
use std::time::{Duration, Instant};

use lt_common::hash_string::HashString;
use lt_crypto::sha1::sha1_hash;
use lt_engine::constructor::{TorrentFileEntry, TorrentMetadata};
use lt_engine::download::Download;
use lt_net::addr::SocketAddress;
use lt_protocol::connection::PeerConnection;
use lt_protocol::request_list::{BlockTransfer, RequestList};

fn metadata(payload_len: usize) -> TorrentMetadata {
    let piece_hash = sha1_hash(&vec![0x42u8; payload_len]);
    let mut path = lt_common::path::Path::new();
    path.push("payload.bin");

    TorrentMetadata {
        info_hash: HashString::new(sha1_hash(&piece_hash)),
        name: "pipelining".to_string(),
        piece_length: payload_len as u32,
        pieces: piece_hash.to_vec(),
        num_pieces: 1,
        total_size: payload_len as u64,
        is_multi_file: false,
        files: vec![TorrentFileEntry {
            path,
            length: payload_len as u64,
            offset: 0,
        }],
        announce: None,
        announce_list: Vec::new(),
        is_private: false,
        is_meta_download: false,
    }
}

fn peer_id() -> HashString {
    HashString::from_hex("aabbcccccccccccccccccccccccccccccccccccc").unwrap()
}

fn seeder_peer() -> PeerConnection {
    let mut peer = PeerConnection::new(
        peer_id(),
        SocketAddress::from(SocketAddr::from((Ipv4Addr::LOCALHOST, 51413))),
        1,
    );
    peer.bitfield.set(0);
    peer
}

#[test]
fn cancelling_a_peer_returns_blocks_to_the_delegator() {
    let mut download = Download::new(metadata(16384));
    download.add_peer(seeder_peer());

    let first = download.delegate_for_peer(&peer_id(), 1);
    assert_eq!(first.len(), 1, "block should be delegated");

    assert_eq!(download.cancel_peer(&peer_id()), 1);

    let after_cancel = download.delegate_for_peer(&peer_id(), 1);
    assert_eq!(
        after_cancel.len(),
        1,
        "cancelled block should be re-delegated"
    );
}

#[test]
fn cancelling_one_block_keeps_others_available() {
    let mut download = Download::new(metadata(16384));
    download.add_peer(seeder_peer());

    let first = download.delegate_for_peer(&peer_id(), 1);
    assert_eq!(first.len(), 1);

    let piece = first[0];
    assert!(download.cancel_block(&peer_id(), piece.index(), piece.offset()));

    let again = download.delegate_for_peer(&peer_id(), 1);
    assert_eq!(again.len(), 1);
    assert_eq!(again[0].index(), piece.index());
}

#[test]
fn discovered_peers_are_queued_and_drained() {
    let mut download = Download::new(metadata(16384));
    let address: SocketAddr = "127.0.0.1:51413".parse().unwrap();

    download.add_discovered_peer(address);
    assert_eq!(download.discovered_peer_count(), 1);
    assert_eq!(download.take_discovered_peers(), vec![address]);
    assert_eq!(download.discovered_peer_count(), 0);
}

fn numbered_peer(number: u8) -> (HashString, PeerConnection) {
    let id = HashString::new([number; 20]);
    let address = SocketAddr::from((Ipv4Addr::LOCALHOST, 50000 + number as u16));
    let mut peer = PeerConnection::new(id, SocketAddress::from(address), 1);
    peer.bitfield.set(0);
    (id, peer)
}

#[test]
fn timed_out_requests_return_to_the_delegator() {
    let mut download = Download::new(metadata(16384));
    download.add_peer(seeder_peer());

    let mut request_list = RequestList::with_queue_limit(4);
    request_list.set_request_timeout(Duration::from_millis(1));

    let delegated = download.delegate_for_peer(&peer_id(), 1);
    assert_eq!(delegated.len(), 1);
    let piece = delegated[0];
    let transfer = BlockTransfer::new(piece.index(), piece.offset(), piece.length());

    assert!(request_list.register(transfer, Instant::now()));
    std::thread::sleep(Duration::from_millis(5));

    let expired = request_list.expire(Instant::now());
    assert_eq!(expired.len(), 1, "request should have timed out");
    assert_eq!(request_list.queue_depth(), 0);

    for record in expired {
        assert!(download.cancel_block(
            &peer_id(),
            record.transfer.piece_index,
            record.transfer.block_offset,
        ));
    }

    let again = download.delegate_for_peer(&peer_id(), 1);
    assert_eq!(again.len(), 1, "timed-out block must be re-delegated");
    assert_eq!(again[0].index(), piece.index());
}

#[test]
fn rejected_requests_are_returned_and_reassigned() {
    let mut download = Download::new(metadata(16384));
    download.add_peer(seeder_peer());

    let mut request_list = RequestList::with_queue_limit(4);
    let delegated = download.delegate_for_peer(&peer_id(), 1);
    assert_eq!(delegated.len(), 1);
    let piece = delegated[0];
    let transfer = BlockTransfer::new(piece.index(), piece.offset(), piece.length());
    assert!(request_list.register(transfer, Instant::now()));

    // Fast-extension reject: drop the outstanding record and release the
    // block so another peer (or this peer later) can request it.
    let rejected = request_list.take_block_at(piece.index(), piece.offset());
    assert_eq!(rejected.len(), 1);
    assert!(download.cancel_block(&peer_id(), piece.index(), piece.offset()));

    let again = download.delegate_for_peer(&peer_id(), 1);
    assert_eq!(again.len(), 1, "rejected block must be re-delegated");
}

#[test]
fn endgame_duplicates_are_bounded_across_peers() {
    let mut download = Download::new(metadata(16384));
    download.delegator_mut().set_aggressive(true);

    let mut total = 0;
    for number in 1..=12u8 {
        let (id, peer) = numbered_peer(number);
        download.add_peer(peer);
        total += download.delegate_for_peer(&id, 8).len();
    }

    assert!(total > 1, "endgame should overlap requests");
    assert!(
        total <= 5,
        "endgame overlap must be capped, got {total} copies"
    );
}
