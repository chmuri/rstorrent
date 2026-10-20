use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use lt_common::hash_string::HashString;
use lt_engine::constructor::{TorrentFileEntry, TorrentMetadata};
use lt_protocol::handshake::HandshakeMessage;
use rt_core::download::Download;
use rt_core::download_list::DownloadList;
use rt_core::network_runtime::{listener_statistics, NetworkRuntime};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

fn metadata() -> TorrentMetadata {
    let info_hash = HashString::from_hex("00112233445566778899aabbccddeeff00112233").unwrap();
    TorrentMetadata {
        info_hash,
        name: "inbound_listener".to_string(),
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
async fn accepts_inbound_handshake_and_registers_peer() {
    // Find a free port, then release it for the listener.
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

    // Wait for the listener to become reachable.
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

    // Send the client handshake and read the server response.
    let handshake = HandshakeMessage::new(info_hash, remote_peer_id);
    stream.write_all(&handshake.encode()).await.unwrap();

    let mut response = [0u8; 68];
    tokio::time::timeout(Duration::from_secs(2), stream.read_exact(&mut response))
        .await
        .expect("server handshake timed out")
        .expect("server closed before handshake");

    let server_handshake = HandshakeMessage::decode(&response).unwrap();
    assert_eq!(server_handshake.info_hash, info_hash);
    assert_eq!(server_handshake.peer_id, our_peer_id);

    // The engine must now know about the inbound peer.
    let engine = download.read().unwrap().engine_download();
    let poll_deadline = Instant::now() + Duration::from_secs(2);
    let mut registered = false;
    while Instant::now() < poll_deadline {
        let guard = engine.lock().unwrap();
        if guard
            .connection_list()
            .find_by_id(&remote_peer_id)
            .is_some()
        {
            registered = true;
            break;
        }
        drop(guard);
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(registered, "inbound peer was not registered");

    let stats = listener_statistics();
    assert!(
        stats.open,
        "listener statistics should report an open socket"
    );
    assert!(
        stats.ports.contains(&port),
        "bound port missing from statistics"
    );
    assert!(stats.accepted >= 1);
    assert!(stats.handshakes >= 1);

    let _ = shutdown_tx.send(true);
}
