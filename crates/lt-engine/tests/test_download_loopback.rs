use lt_common::hash_string::HashString;
use lt_crypto::sha1::sha1_hash;
use lt_engine::constructor::TorrentMetadata;
use lt_engine::download::{Download, DownloadState};
use lt_net::addr::SocketAddress;
use lt_protocol::connection::PeerConnection;
use std::str::FromStr;

#[test]
fn test_download_loopback_e2e() {
    // 1. Prepare test payload: 2 pieces of 16384 bytes = 32768 bytes total
    let piece0 = vec![0xAAu8; 16384];
    let piece1 = vec![0xBBu8; 16384];

    let h0 = sha1_hash(&piece0);
    let h1 = sha1_hash(&piece1);
    let mut pieces_hash = Vec::new();
    pieces_hash.extend_from_slice(&h0);
    pieces_hash.extend_from_slice(&h1);

    let info_hash = HashString::new(sha1_hash(&pieces_hash));

    let metadata = TorrentMetadata {
        info_hash,
        name: "test_e2e_torrent".into(),
        piece_length: 16384,
        pieces: pieces_hash,
        num_pieces: 2,
        total_size: 32768,
        is_multi_file: false,
        files: Vec::new(),
        announce: Some("http://127.0.0.1:8000/announce".into()),
        announce_list: vec![vec!["http://127.0.0.1:8000/announce".into()]],
        is_private: false,
        is_meta_download: false,
        v2: None,
    };

    // Fixed peer identities for the two simulated endpoints.
    let seeder_peer_id = HashString::new([0x53; 20]);
    let leecher_peer_id = HashString::new([0x4C; 20]);

    // 2. Set up Seeder download and mark its pieces as completed
    let mut seeder_dl = Download::new(metadata.clone());
    seeder_dl.chunk_selector_mut().mark_completed(0);
    seeder_dl.chunk_selector_mut().mark_completed(1);
    seeder_dl.start();
    assert_eq!(seeder_dl.state(), &DownloadState::Seeding);
    assert!(seeder_dl.is_seeder());

    // 3. Set up Leecher download
    let mut leecher_dl = Download::new(metadata);
    leecher_dl.start();
    assert_eq!(leecher_dl.state(), &DownloadState::Downloading);
    assert!(!leecher_dl.is_seeder());

    // 4. Simulate P2P connection between Leecher and Seeder
    let seeder_addr = SocketAddress::from_str("127.0.0.1:6881").unwrap();
    let leecher_addr = SocketAddress::from_str("127.0.0.1:6882").unwrap();

    let mut seeder_peer_view = PeerConnection::new(seeder_peer_id, seeder_addr, 2);
    // Seeder bitfield has all bits set
    seeder_peer_view.bitfield.set(0);
    seeder_peer_view.bitfield.set(1);

    let leecher_peer_view = PeerConnection::new(leecher_peer_id, leecher_addr, 2);

    // Register peer connections
    leecher_dl.add_peer(seeder_peer_view);
    seeder_dl.add_peer(leecher_peer_view);

    // 5. Leecher delegates pieces from Seeder
    let requested_pieces = leecher_dl.delegate_for_peer(&seeder_peer_id, 4);
    assert_eq!(requested_pieces.len(), 2);
    assert_eq!(requested_pieces[0].index(), 0);
    assert_eq!(requested_pieces[1].index(), 1);

    // 6. Seeder transmits piece data back to Leecher
    for piece in &requested_pieces {
        let piece_data = if piece.index() == 0 { &piece0 } else { &piece1 };
        let completed = leecher_dl
            .receive_block(piece.index(), piece.offset(), piece_data)
            .unwrap();
        assert!(completed);
    }

    // 7. Verify Leecher successfully verified both pieces and transitioned to Seeding!
    assert_eq!(leecher_dl.completed_chunks(), 2);
    assert!(leecher_dl.is_seeder());
    assert_eq!(leecher_dl.state(), &DownloadState::Seeding);
}
