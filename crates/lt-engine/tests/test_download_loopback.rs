use lt_common::hash_string::HashString;
use lt_crypto::sha1::sha1_hash;
use lt_engine::constructor::TorrentMetadata;
use lt_engine::download::DownloadState;
use lt_engine::engine::EngineManager;
use lt_net::addr::SocketAddress;
use lt_protocol::connection::PeerConnection;
use std::str::FromStr;

#[test]
fn test_download_loopback_e2e() {
    // 1. Prepare test payload: 2 pieces of 16384 bytes = 32768 bytes total
    let piece0 = vec![0xAAu8; 16384];
    let piece1 = vec![0xBBu8; 16384];
    let mut total_payload = Vec::new();
    total_payload.extend_from_slice(&piece0);
    total_payload.extend_from_slice(&piece1);

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
    };

    // 2. Set up Seeder Engine
    let mut seeder_engine = EngineManager::new(6881);
    let seeder_hash = seeder_engine.add_metadata(metadata.clone());
    let seeder_dl = seeder_engine.get_download(&seeder_hash).unwrap();

    // Mark Seeder pieces as completed
    {
        let mut sdl = seeder_dl.lock().unwrap();
        sdl.chunk_selector_mut().mark_completed(0);
        sdl.chunk_selector_mut().mark_completed(1);
        sdl.start();
        assert_eq!(sdl.state(), &DownloadState::Seeding);
        assert!(sdl.is_seeder());
    }

    // 3. Set up Leecher Engine
    let mut leecher_engine = EngineManager::new(6882);
    let leecher_hash = leecher_engine.add_metadata(metadata);
    let leecher_dl = leecher_engine.get_download(&leecher_hash).unwrap();

    {
        let mut ldl = leecher_dl.lock().unwrap();
        ldl.start();
        assert_eq!(ldl.state(), &DownloadState::Downloading);
        assert!(!ldl.is_seeder());
    }

    // 4. Simulate P2P connection between Leecher and Seeder
    let seeder_addr = SocketAddress::from_str("127.0.0.1:6881").unwrap();
    let leecher_addr = SocketAddress::from_str("127.0.0.1:6882").unwrap();

    let mut seeder_peer_view = PeerConnection::new(*seeder_engine.peer_id(), seeder_addr, 2);
    // Seeder bitfield has all bits set
    seeder_peer_view.bitfield.set(0);
    seeder_peer_view.bitfield.set(1);

    let leecher_peer_view = PeerConnection::new(*leecher_engine.peer_id(), leecher_addr, 2);

    // Register peer connections
    leecher_dl.lock().unwrap().add_peer(seeder_peer_view);
    seeder_dl.lock().unwrap().add_peer(leecher_peer_view);

    // 5. Leecher delegates pieces from Seeder
    let requested_pieces = leecher_dl
        .lock()
        .unwrap()
        .delegate_for_peer(seeder_engine.peer_id(), 4);
    assert_eq!(requested_pieces.len(), 2);
    assert_eq!(requested_pieces[0].index(), 0);
    assert_eq!(requested_pieces[1].index(), 1);

    // 6. Seeder transmits piece data back to Leecher
    for piece in &requested_pieces {
        let piece_data = if piece.index() == 0 { &piece0 } else { &piece1 };
        let mut ldl = leecher_dl.lock().unwrap();
        let completed = ldl
            .receive_block(piece.index(), piece.offset(), piece_data)
            .unwrap();
        assert!(completed);
    }

    // 7. Verify Leecher successfully verified both pieces and transitioned to Seeding!
    {
        let ldl = leecher_dl.lock().unwrap();
        assert_eq!(ldl.completed_chunks(), 2);
        assert!(ldl.is_seeder());
        assert_eq!(ldl.state(), &DownloadState::Seeding);
    }
}
