use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use lt_common::hash_string::HashString;
use lt_crypto::sha1::sha1_hash;
use lt_engine::constructor::{TorrentFileEntry, TorrentMetadata};
use lt_protocol::handshake::HandshakeMessage;
use lt_protocol::message::PeerMessage;
use rt_core::download::Download;
use rt_core::download_list::DownloadList;
use rt_core::network_runtime::NetworkRuntime;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

fn metadata(payload: &[u8]) -> TorrentMetadata {
    let mut path = lt_common::path::Path::new();
    path.push("payload.bin");
    TorrentMetadata {
        info_hash: HashString::new([0x77u8; 20]),
        name: "initial-seed-upload".to_string(),
        piece_length: payload.len() as u32,
        pieces: sha1_hash(payload).to_vec(),
        num_pieces: 1,
        total_size: payload.len() as u64,
        is_multi_file: false,
        files: vec![TorrentFileEntry {
            path,
            length: payload.len() as u64,
            offset: 0,
        }],
        announce: None,
        announce_list: Vec::new(),
        is_private: false,
        is_meta_download: false,
    }
}

async fn read_message(stream: &mut TcpStream) -> (u8, Vec<u8>) {
    let mut len_buf = [0u8; 4];
    tokio::time::timeout(Duration::from_secs(2), stream.read_exact(&mut len_buf))
        .await
        .expect("peer message length timed out")
        .expect("peer closed while reading length");
    let len = u32::from_be_bytes(len_buf) as usize;
    let mut payload = vec![0u8; len];
    stream.read_exact(&mut payload).await.unwrap();
    (payload[0], payload[1..].to_vec())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn initial_seed_offers_one_piece_and_serves_it() {
    let payload = b"seed";
    let dir = std::env::temp_dir().join(format!(
        "rtorrent-initial-seed-upload-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();

    let probe = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = probe.local_addr().unwrap().port();
    drop(probe);

    let mut download = Download::from_metadata(metadata(payload));
    download.set_directory(dir.display().to_string());
    let _ = download.open();
    download.try_start().unwrap();

    {
        // Synchronous write lets the test read the piece back immediately.
        let mut engine = download.download_mut();
        engine.set_disk_thread(None);
        assert!(engine.receive_block(0, 0, payload).unwrap());
        assert!(engine.is_seeder());
    }
    download.set_connection_seed("initial_seed").unwrap();
    assert!(download.is_initial_seeding());

    let download = Arc::new(RwLock::new(download));
    let mut list = DownloadList::new();
    list.insert(Arc::clone(&download));
    let list = Arc::new(RwLock::new(list));

    let our_peer_id = HashString::new([0xA1u8; 20]);
    let remote_peer_id = HashString::new([0xB2u8; 20]);
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

    let info_hash = *download.read().unwrap().info_hash();
    stream
        .write_all(&HandshakeMessage::new(info_hash, remote_peer_id).encode())
        .await
        .unwrap();

    let mut handshake = [0u8; 68];
    stream.read_exact(&mut handshake).await.unwrap();
    assert_eq!(
        HandshakeMessage::decode(&handshake).unwrap().peer_id,
        our_peer_id
    );

    // The initial seeder advertises an empty bitfield and a single HAVE.
    let mut offered = None;
    let mut unchoked = false;
    while offered.is_none() || !unchoked {
        let (id, body) = read_message(&mut stream).await;
        match PeerMessage::decode(id, &body).unwrap() {
            PeerMessage::Have { index } => offered = Some(index),
            PeerMessage::Unchoke => unchoked = true,
            PeerMessage::Bitfield(bits) => {
                assert!(
                    bits.iter().all(|byte| *byte == 0),
                    "initial seeding must not advertise a filled bitfield"
                );
            }
            _ => {}
        }
    }
    assert_eq!(offered, Some(0));

    stream
        .write_all(
            &PeerMessage::Request {
                index: 0,
                begin: 0,
                length: payload.len() as u32,
            }
            .encode(),
        )
        .await
        .unwrap();

    loop {
        let (id, body) = read_message(&mut stream).await;
        if let PeerMessage::Piece { index, begin, data } = PeerMessage::decode(id, &body).unwrap() {
            assert_eq!((index, begin), (0, 0));
            assert_eq!(data, payload);
            break;
        }
    }

    // Confirming the piece switches the seeder back to normal seeding and it
    // advertises the now-complete bitfield to this peer.
    stream
        .write_all(&PeerMessage::Have { index: 0 }.encode())
        .await
        .unwrap();

    loop {
        let (id, body) = read_message(&mut stream).await;
        if let PeerMessage::Bitfield(bits) = PeerMessage::decode(id, &body).unwrap() {
            assert!(bits[0] & 0x80 != 0);
            break;
        }
    }

    let _ = shutdown_tx.send(true);
    let _ = std::fs::remove_dir_all(dir);
}
