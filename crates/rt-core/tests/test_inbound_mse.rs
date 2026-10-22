use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use lt_common::hash_string::HashString;
use lt_engine::constructor::{TorrentFileEntry, TorrentMetadata};
use lt_protocol::encryption::EncryptionPolicy;
use lt_protocol::handshake::HandshakeMessage;
use lt_protocol::mse::initiate;
use rt_core::download::Download;
use rt_core::download_list::DownloadList;
use rt_core::network_runtime::NetworkRuntime;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

fn metadata() -> TorrentMetadata {
    let info_hash = HashString::from_hex("00112233445566778899aabbccddeeff00112233").unwrap();
    TorrentMetadata {
        info_hash,
        name: "inbound_mse".to_string(),
        piece_length: 1,
        pieces: vec![0u8; 20],
        num_pieces: 1,
        total_size: 1,
        is_multi_file: false,
        files: vec![TorrentFileEntry {
            path: {
                let mut p = lt_common::path::Path::new();
                p.push("payload.bin");
                p
            },
            length: 1,
            offset: 0,
        }],
        announce: None,
        announce_list: Vec::new(),
        is_private: false,
        is_meta_download: false,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn accepts_mse_handshake_and_registers_peer() {
    let probe = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = probe.local_addr().unwrap().port();
    drop(probe);

    let metadata = metadata();
    let info_hash = metadata.info_hash;
    let download = Arc::new(RwLock::new(Download::from_metadata(metadata)));

    let mut list = DownloadList::new();
    list.insert(Arc::clone(&download));
    let list = Arc::new(RwLock::new(list));

    let our_peer_id = HashString::from_hex("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa").unwrap();
    let remote_peer_id = HashString::from_hex("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb").unwrap();

    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    NetworkRuntime::spawn(Arc::clone(&list), our_peer_id, port, shutdown_rx, None);

    let deadline = Instant::now() + Duration::from_secs(3);
    let mut stream = loop {
        match TcpStream::connect(("127.0.0.1", port)).await {
            Ok(stream) => break stream,
            Err(_) if Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            Err(err) => panic!("listener did not start: {err}"),
        }
    };

    let policy = EncryptionPolicy::require();
    let session = initiate(&mut stream, info_hash, remote_peer_id, &policy)
        .await
        .expect("MSE handshake should succeed");

    assert!(session.encrypted_stream);
    assert_eq!(session.peer_id, our_peer_id);

    let engine = download.read().unwrap().engine_download();
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut registered = None;
    while Instant::now() < deadline {
        let found = {
            let guard = engine.lock().unwrap();
            guard
                .connection_list()
                .find_by_id(&remote_peer_id)
                .map(|peer| (peer.is_encrypted, peer.is_obfuscated))
        };
        if let Some(flags) = found {
            registered = Some(flags);
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    assert_eq!(
        registered,
        Some((true, true)),
        "MSE peer was not registered as encrypted+obfuscated"
    );
    let _ = shutdown_tx.send(true);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn registers_plaintext_mse_stream_as_obfuscated() {
    let probe = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = probe.local_addr().unwrap().port();
    drop(probe);

    let metadata = metadata();
    let info_hash = metadata.info_hash;
    let download = Arc::new(RwLock::new(Download::from_metadata(metadata)));

    let mut list = DownloadList::new();
    list.insert(Arc::clone(&download));
    let list = Arc::new(RwLock::new(list));

    let our_peer_id = HashString::from_hex("cccccccccccccccccccccccccccccccccccccccc").unwrap();
    let remote_peer_id = HashString::from_hex("dddddddddddddddddddddddddddddddddddddddd").unwrap();

    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    NetworkRuntime::spawn(Arc::clone(&list), our_peer_id, port, shutdown_rx, None);

    let deadline = Instant::now() + Duration::from_secs(3);
    let mut stream = loop {
        match TcpStream::connect(("127.0.0.1", port)).await {
            Ok(stream) => break stream,
            Err(_) if Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            Err(err) => panic!("listener did not start: {err}"),
        }
    };

    // Use MSE for the handshake but only offer a plaintext stream.
    let policy = EncryptionPolicy::new(
        lt_protocol::encryption::EncryptionMode::Prefer,
        lt_protocol::encryption::EncryptionMode::Disabled,
    );
    let session = initiate(&mut stream, info_hash, remote_peer_id, &policy)
        .await
        .expect("MSE handshake should succeed");
    assert!(!session.encrypted_stream);
    assert!(session.is_obfuscated());

    let engine = download.read().unwrap().engine_download();
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut registered = None;
    while Instant::now() < deadline {
        let found = {
            let guard = engine.lock().unwrap();
            guard
                .connection_list()
                .find_by_id(&remote_peer_id)
                .map(|peer| (peer.is_encrypted, peer.is_obfuscated))
        };
        if let Some(flags) = found {
            registered = Some(flags);
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    assert_eq!(
        registered,
        Some((false, true)),
        "plaintext MSE peer should be obfuscated but not encrypted"
    );
    let _ = shutdown_tx.send(true);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn require_policy_rejects_plaintext_inbound() {
    let probe = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = probe.local_addr().unwrap().port();
    drop(probe);

    let metadata = metadata();
    let info_hash = metadata.info_hash;
    let download = Arc::new(RwLock::new(Download::from_metadata(metadata)));

    let mut list = DownloadList::new();
    list.insert(Arc::clone(&download));
    let list = Arc::new(RwLock::new(list));

    let our_peer_id = HashString::from_hex("eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee").unwrap();
    let remote_peer_id = HashString::from_hex("ffffffffffffffffffffffffffffffffffffffff").unwrap();

    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    NetworkRuntime::spawn_with_policy(
        Arc::clone(&list),
        our_peer_id,
        port,
        shutdown_rx,
        None,
        EncryptionPolicy::require(),
    );

    let deadline = Instant::now() + Duration::from_secs(3);
    let mut stream = loop {
        match TcpStream::connect(("127.0.0.1", port)).await {
            Ok(stream) => break stream,
            Err(_) if Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            Err(err) => panic!("listener did not start: {err}"),
        }
    };

    stream
        .write_all(&HandshakeMessage::new(info_hash, remote_peer_id).encode())
        .await
        .unwrap();

    // The listener must drop the connection instead of answering.
    let mut response = [0u8; 68];
    let read = tokio::time::timeout(Duration::from_secs(2), stream.read_exact(&mut response)).await;
    assert!(
        matches!(read, Ok(Err(_)) | Err(_)),
        "require policy answered a plaintext handshake"
    );

    let engine = download.read().unwrap().engine_download();
    tokio::time::sleep(Duration::from_millis(200)).await;
    let registered = {
        let guard = engine.lock().unwrap();
        guard
            .connection_list()
            .find_by_id(&remote_peer_id)
            .is_some()
    };
    assert!(
        !registered,
        "plaintext peer was registered under require policy"
    );

    let _ = shutdown_tx.send(true);
}
