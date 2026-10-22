use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use lt_common::hash_string::HashString;
use lt_crypto::dh::mse_req2;
use lt_engine::constructor::{TorrentFileEntry, TorrentMetadata};
use lt_engine::{DownloadNetworkCoordinator, PeerSource};
use lt_protocol::encryption::EncryptionPolicy;
use lt_protocol::handshake::HandshakeMessage;
use lt_protocol::mse::accept as mse_accept;
use rt_core::download::Download;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

fn metadata() -> TorrentMetadata {
    TorrentMetadata {
        info_hash: HashString::from_hex("101112131415161718191a1b1c1d1e1f20212223").unwrap(),
        name: "outbound-mse".to_string(),
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

fn temp_dir(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "rtorrent-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

async fn wait_for_peer(
    engine: &Arc<Mutex<lt_engine::Download>>,
    peer_id: &HashString,
) -> Option<(bool, bool)> {
    let deadline = Instant::now() + Duration::from_secs(4);
    while Instant::now() < deadline {
        {
            let guard = engine.lock().unwrap();
            if let Some(peer) = guard.connection_list().find_by_id(peer_id) {
                return Some((peer.is_encrypted, peer.is_obfuscated));
            }
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    None
}

fn active_engine() -> (Download, Arc<Mutex<lt_engine::Download>>) {
    let dir = temp_dir("outbound-mse");
    let mut download = Download::from_metadata(metadata());
    download.set_directory(dir.display().to_string());
    download.open().unwrap();
    download.try_start().unwrap();
    let engine = download.engine_download();
    (download, engine)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn outbound_mse_registers_encrypted_peer() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let peer_addr = listener.local_addr().unwrap();

    let info_hash = metadata().info_hash;
    let our_peer_id = HashString::from_hex("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa01").unwrap();
    let remote_peer_id = HashString::from_hex("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb01").unwrap();

    let server = tokio::spawn(async move {
        let (mut stream, _) = tokio::time::timeout(Duration::from_secs(5), listener.accept())
            .await
            .expect("outbound connection timed out")
            .unwrap();

        let obfuscated = HashString::new(mse_req2(info_hash.as_bytes()));
        let session = mse_accept(
            &mut stream,
            remote_peer_id,
            &EncryptionPolicy::require(),
            move |hash| {
                if *hash == obfuscated {
                    Some(info_hash)
                } else {
                    None
                }
            },
        )
        .await
        .expect("server MSE handshake");

        assert!(session.encrypted_stream);
        // Keep the connection alive while the engine registers the peer.
        tokio::time::sleep(Duration::from_secs(2)).await;
    });

    let (_download, engine) = active_engine();
    let coordinator = DownloadNetworkCoordinator::new_with_policy(
        Arc::clone(&engine),
        our_peer_id,
        0,
        None,
        EncryptionPolicy::require(),
    );
    coordinator
        .peer_pool()
        .lock()
        .unwrap()
        .add_peer(peer_addr, PeerSource::Tracker, false);
    let shutdown = coordinator.spawn();

    let flags = wait_for_peer(&engine, &remote_peer_id).await;
    assert_eq!(
        flags,
        Some((true, true)),
        "outbound MSE peer should be encrypted and obfuscated"
    );
    {
        let guard = engine.lock().unwrap();
        assert!(lt_engine::network::encrypted_peer_ids(&guard).contains(&remote_peer_id));
        assert!(lt_engine::network::obfuscated_peer_ids(&guard).contains(&remote_peer_id));
    }

    let _ = shutdown.send(true);
    server.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn outbound_require_never_downgrades() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let peer_addr = listener.local_addr().unwrap();

    let our_peer_id = HashString::from_hex("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa03").unwrap();
    let remote_peer_id = HashString::from_hex("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb03").unwrap();

    let server = tokio::spawn(async move {
        // Consume the DH key and drop the connection. A `require` policy must
        // not reconnect with a plaintext handshake.
        let (mut first, _) = tokio::time::timeout(Duration::from_secs(5), listener.accept())
            .await
            .expect("outbound connection timed out")
            .unwrap();
        let mut key = [0u8; 96];
        let _ = first.read_exact(&mut key).await;
        drop(first);

        !matches!(
            tokio::time::timeout(Duration::from_millis(800), listener.accept()).await,
            Ok(Ok(_))
        )
    });

    let (_download, engine) = active_engine();
    let coordinator = DownloadNetworkCoordinator::new_with_policy(
        Arc::clone(&engine),
        our_peer_id,
        0,
        None,
        EncryptionPolicy::require(),
    );
    coordinator
        .peer_pool()
        .lock()
        .unwrap()
        .add_peer(peer_addr, PeerSource::Tracker, false);
    let shutdown = coordinator.spawn();

    let no_downgrade = server.await.unwrap();
    assert!(
        no_downgrade,
        "require policy downgraded to a plaintext retry"
    );

    let registered = {
        let guard = engine.lock().unwrap();
        guard
            .connection_list()
            .find_by_id(&remote_peer_id)
            .is_some()
    };
    assert!(!registered, "require policy registered a plaintext peer");

    let _ = shutdown.send(true);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn outbound_mse_falls_back_to_plaintext_when_allowed() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let peer_addr = listener.local_addr().unwrap();

    let info_hash = metadata().info_hash;
    let our_peer_id = HashString::from_hex("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa02").unwrap();
    let remote_peer_id = HashString::from_hex("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb02").unwrap();

    let server = tokio::spawn(async move {
        // First connection: consume the DH key, then close. This makes the
        // policy-allowed plaintext retry kick in.
        let (mut first, _) = tokio::time::timeout(Duration::from_secs(5), listener.accept())
            .await
            .expect("first outbound connection timed out")
            .unwrap();
        let mut key = [0u8; 96];
        let _ = first.read_exact(&mut key).await;
        drop(first);

        // Second connection: classic plaintext handshake.
        let (mut second, _) = tokio::time::timeout(Duration::from_secs(5), listener.accept())
            .await
            .expect("plaintext retry did not arrive")
            .unwrap();
        let mut handshake = [0u8; 68];
        second
            .read_exact(&mut handshake)
            .await
            .expect("plaintext handshake");
        let decoded = HandshakeMessage::decode(&handshake).unwrap();
        assert_eq!(decoded.info_hash, info_hash);
        second
            .write_all(&HandshakeMessage::new(info_hash, remote_peer_id).encode())
            .await
            .unwrap();

        tokio::time::sleep(Duration::from_secs(2)).await;
    });

    let (_download, engine) = active_engine();
    let coordinator = DownloadNetworkCoordinator::new_with_policy(
        Arc::clone(&engine),
        our_peer_id,
        0,
        None,
        EncryptionPolicy::prefer(),
    );
    coordinator
        .peer_pool()
        .lock()
        .unwrap()
        .add_peer(peer_addr, PeerSource::Tracker, false);
    let shutdown = coordinator.spawn();

    let flags = wait_for_peer(&engine, &remote_peer_id).await;
    assert_eq!(
        flags,
        Some((false, false)),
        "plaintext fallback should register an unencrypted peer"
    );

    let _ = shutdown.send(true);
    server.abort();
}
