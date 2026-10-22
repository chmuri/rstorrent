use lt_common::hash_string::HashString;
use lt_common::object::Object;
use lt_crypto::sha1::sha1_hash;
use lt_engine::constructor::DownloadConstructor;
use lt_engine::download::Download;

fn info_bytes() -> (Vec<u8>, HashString) {
    let mut info = Object::create_map();
    info.insert_key("name", Object::from("magnet-dir")).unwrap();
    info.insert_key("piece length", Object::from(16384i64))
        .unwrap();
    info.insert_key("pieces", Object::create_string(vec![0u8; 20]))
        .unwrap();
    info.insert_key("length", Object::from(16384i64)).unwrap();

    let bytes = lt_common::object::bencode::encode(&info).unwrap();
    let hash = HashString::new(sha1_hash(&bytes));
    (bytes, hash)
}

#[test]
fn magnet_download_can_apply_verified_metadata() {
    let (bytes, info_hash) = info_bytes();
    let uri = format!("magnet:?xt=urn:btih:{}&dn=magnet-dir", info_hash.to_hex());

    let metadata = DownloadConstructor::from_magnet_uri(&uri).unwrap();
    assert!(metadata.is_meta_download);

    let mut download = Download::new(metadata);
    assert!(download.is_metadata_download());
    assert!(download.metadata_download().is_some());

    download
        .apply_metadata_bytes(&bytes, info_hash, None, Vec::new())
        .expect("metadata should apply");

    assert!(!download.is_metadata_download());
    assert!(download.metadata_download().is_none());
    assert_eq!(download.metadata().name, "magnet-dir");
    assert_eq!(download.metadata().num_pieces, 1);
    assert_eq!(download.metadata().total_size, 16384);
    assert_eq!(download.total_chunks(), 1);

    let recovered = lt_engine::network::download_info_bytes(&download)
        .expect("verified info bytes must be recoverable after metadata applies");
    assert_eq!(recovered.as_slice(), bytes.as_slice());
    assert_eq!(HashString::new(sha1_hash(&recovered)), info_hash);
}

#[test]
fn applying_metadata_to_regular_download_is_rejected() {
    let (bytes, info_hash) = info_bytes();
    let metadata =
        DownloadConstructor::from_info_bytes(&bytes, info_hash, None, Vec::new()).unwrap();
    let mut download = Download::new(metadata);

    assert!(download
        .apply_metadata_bytes(&bytes, info_hash, None, Vec::new())
        .is_err());
}

mod existing_payload {
    use std::net::{Ipv4Addr, SocketAddr};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::Arc;

    use lt_common::hash_string::HashString;
    use lt_common::object::Object;
    use lt_crypto::sha1::sha1_hash;
    use lt_engine::constructor::DownloadConstructor;
    use lt_engine::download::Download;
    use lt_net::addr::SocketAddress;
    use lt_protocol::connection::PeerConnection;

    static COUNTER: AtomicU64 = AtomicU64::new(1);

    const PIECE_LENGTH: usize = 1024;
    const NAME: &str = "payload.bin";

    fn payload() -> Vec<u8> {
        (0..(3 * PIECE_LENGTH)).map(|i| (i % 251) as u8).collect()
    }

    fn info_bytes() -> (Vec<u8>, HashString) {
        let payload = payload();
        let mut pieces = Vec::new();
        for chunk in payload.chunks(PIECE_LENGTH) {
            pieces.extend_from_slice(&sha1_hash(chunk));
        }

        let mut info = Object::create_map();
        info.insert_key("name", Object::from(NAME)).unwrap();
        info.insert_key("piece length", Object::from(PIECE_LENGTH as i64))
            .unwrap();
        info.insert_key("pieces", Object::create_string(pieces))
            .unwrap();
        info.insert_key("length", Object::from(payload.len() as i64))
            .unwrap();

        let bytes = lt_common::object::bencode::encode(&info).unwrap();
        let hash = HashString::new(sha1_hash(&bytes));
        (bytes, hash)
    }

    fn temp_root(label: &str) -> PathBuf {
        let id = COUNTER.fetch_add(1, Ordering::SeqCst);
        let root = std::env::temp_dir().join(format!(
            "rstorrent_metadata_existing_{}_{}_{}",
            label,
            std::process::id(),
            id
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    fn magnet_download() -> Download {
        let (_, info_hash) = info_bytes();
        let uri = format!("magnet:?xt=urn:btih:{}&dn={}", info_hash.to_hex(), NAME);
        Download::new(DownloadConstructor::from_magnet_uri(&uri).unwrap())
    }

    fn apply(download: &mut Download) {
        let (bytes, info_hash) = info_bytes();
        download
            .apply_metadata_bytes(&bytes, info_hash, None, Vec::new())
            .expect("metadata should apply");
    }

    #[test]
    fn applying_metadata_queues_an_existing_payload() {
        let root = temp_root("complete");
        std::fs::write(root.join(NAME), payload()).unwrap();

        let mut download = magnet_download();
        download.set_save_directory(&root);
        apply(&mut download);

        assert!(!download.is_metadata_download());
        assert!(download.has_pending_hash_pieces());
        assert_eq!(
            download.pending_hash_pieces(),
            &[0, 1, 2],
            "all pieces of the existing payload must be verified"
        );
    }

    #[test]
    fn applying_metadata_without_files_queues_nothing() {
        let root = temp_root("empty");

        let mut download = magnet_download();
        download.set_save_directory(&root);
        apply(&mut download);

        assert!(!download.has_pending_hash_pieces());
    }

    #[test]
    fn multi_file_payload_nested_in_the_torrent_directory_is_verified() {
        let root = temp_root("multi");
        std::fs::create_dir_all(root.join("bundle")).unwrap();

        let first: Vec<u8> = (0..PIECE_LENGTH).map(|i| (i % 251) as u8).collect();
        let second: Vec<u8> = (0..PIECE_LENGTH).map(|i| (i % 97) as u8).collect();
        std::fs::write(root.join("bundle/a.bin"), &first).unwrap();
        std::fs::write(root.join("bundle/b.bin"), &second).unwrap();

        let mut info = Object::create_map();
        info.insert_key("name", Object::from("bundle")).unwrap();
        info.insert_key("piece length", Object::from(PIECE_LENGTH as i64))
            .unwrap();
        let mut pieces = Vec::new();
        pieces.extend_from_slice(&sha1_hash(&first));
        pieces.extend_from_slice(&sha1_hash(&second));
        info.insert_key("pieces", Object::create_string(pieces))
            .unwrap();
        let mut files = Object::create_list();
        for (name, length) in [("a.bin", PIECE_LENGTH), ("b.bin", PIECE_LENGTH)] {
            let mut entry = Object::create_map();
            entry
                .insert_key("length", Object::from(length as i64))
                .unwrap();
            let mut path = Object::create_list();
            path.insert_back(Object::from(name)).unwrap();
            entry.insert_key("path", path).unwrap();
            files.insert_back(entry).unwrap();
        }
        info.insert_key("files", files).unwrap();

        let bytes = lt_common::object::bencode::encode(&info).unwrap();
        let info_hash = HashString::new(sha1_hash(&bytes));
        let uri = format!("magnet:?xt=urn:btih:{}&dn=bundle", info_hash.to_hex());

        let mut download = Download::new(DownloadConstructor::from_magnet_uri(&uri).unwrap());
        download.set_save_directory(&root);
        download
            .apply_metadata_bytes(&bytes, info_hash, None, Vec::new())
            .expect("metadata should apply");

        assert_eq!(
            download.pending_hash_pieces(),
            &[0, 1],
            "nested files must still be discovered"
        );

        let pending = download.take_pending_hash_pieces();
        let prepared = download
            .prepare_recheck(Some(&pending), Arc::new(AtomicBool::new(false)))
            .expect("recheck should prepare");
        let result = prepared.execute();
        download
            .finish_recheck(prepared, result)
            .expect("recheck should finish");

        assert!(download.is_done(), "both nested files must verify");
        assert_eq!(download.completed_chunks(), 2);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn blocks_and_requests_are_blocked_while_the_payload_is_verified() {
        let root = temp_root("blocking");
        std::fs::write(root.join(NAME), payload()).unwrap();

        let mut download = magnet_download();
        download.set_save_directory(&root);
        apply(&mut download);

        let peer_id = HashString::from_hex("aabbcccccccccccccccccccccccccccccccccccc").unwrap();
        let mut peer = PeerConnection::new(
            peer_id,
            SocketAddress::from(SocketAddr::from((Ipv4Addr::LOCALHOST, 51413))),
            1,
        );
        for index in 0..3 {
            peer.bitfield.set(index);
        }
        download.add_peer(peer);

        // Queued verification already owns the data.
        assert!(download.delegate_for_peer(&peer_id, 1).is_empty());
        assert!(!download
            .receive_block(0, 0, &payload()[0..PIECE_LENGTH])
            .unwrap());

        let pending = download.take_pending_hash_pieces();
        let prepared = download
            .prepare_recheck(Some(&pending), Arc::new(AtomicBool::new(false)))
            .expect("recheck should prepare");
        assert!(download.is_checking());

        // A running verification must not accept new blocks either.
        assert!(download.delegate_for_peer(&peer_id, 1).is_empty());
        assert!(!download
            .receive_block(0, 0, &payload()[0..PIECE_LENGTH])
            .unwrap());

        let result = prepared.execute();
        download
            .finish_recheck(prepared, result)
            .expect("recheck should finish");
        let _ = std::fs::remove_dir_all(&root);
    }
}
