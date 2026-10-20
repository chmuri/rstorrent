use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use lt_common::hash_string::HashString;
use lt_crypto::sha1::sha1_hash;
use lt_engine::constructor::{TorrentFileEntry, TorrentMetadata};
use lt_engine::download::Download;
use lt_engine::network::{peer_fast_pieces, pex_statistics, run_inbound_peer_with_pool};
use lt_engine::peer_pool::{PeerPool, PeerPoolHandle, PeerSource};
use lt_protocol::extensions::{extension_handshake_payload, ut_pex_payload};
use lt_protocol::handshake::HandshakeMessage;
use lt_protocol::message::PeerMessage;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;

const LOCAL_PEX_ID: u8 = 2;

fn metadata(payload: &[u8], is_private: bool) -> TorrentMetadata {
    let piece_hash = sha1_hash(payload);
    let mut path = lt_common::path::Path::new();
    path.push("payload.bin");
    TorrentMetadata {
        info_hash: HashString::new(sha1_hash(&piece_hash)),
        name: "pex-fast".to_string(),
        piece_length: payload.len() as u32,
        pieces: piece_hash.to_vec(),
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
        is_private,
        is_meta_download: false,
    }
}

async fn read_message(stream: &mut TcpStream) -> Option<PeerMessage> {
    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf).await.ok()?;
    let length = u32::from_be_bytes(len_buf) as usize;
    if length == 0 {
        return Some(PeerMessage::KeepAlive);
    }
    let mut payload = vec![0u8; length];
    stream.read_exact(&mut payload).await.ok()?;
    PeerMessage::decode(payload[0], &payload[1..]).ok()
}

async fn start(
    download: Arc<Mutex<Download>>,
    pool: Option<PeerPoolHandle>,
) -> (TcpStream, tokio::task::JoinHandle<()>, watch::Sender<bool>) {
    let our_peer_id = HashString::from_hex("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa").unwrap();
    let remote_peer_id = HashString::from_hex("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb").unwrap();
    let info_hash = *download.lock().unwrap().info_hash();

    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (client_res, server_res) = tokio::join!(TcpStream::connect(addr), listener.accept());
    let mut client = client_res.unwrap();
    let (server, _) = server_res.unwrap();

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let handshake = HandshakeMessage::new(info_hash, remote_peer_id);
    let session = tokio::spawn(async move {
        run_inbound_peer_with_pool(server, handshake, our_peer_id, download, shutdown_rx, pool)
            .await;
    });

    let mut response = [0u8; 68];
    tokio::time::timeout(Duration::from_secs(2), client.read_exact(&mut response))
        .await
        .expect("handshake timeout")
        .expect("peer closed");
    (client, session, shutdown_tx)
}

async fn send_extension_handshake(client: &mut TcpStream, pex_id: u8) {
    client
        .write_all(
            &PeerMessage::Extended {
                id: 0,
                payload: extension_handshake_payload(pex_id, 2, None),
            }
            .encode(),
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn seeder_sends_have_all_to_fast_peer() {
    let payload = vec![0x11u8; 64];
    let metadata = metadata(&payload, false);
    let info_hash = metadata.info_hash;
    let mut download = Download::new(metadata);
    download.chunk_selector_mut().mark_completed(0);
    assert!(download.is_seeder());
    let download = Arc::new(Mutex::new(download));

    let (mut client, session, shutdown_tx) = start(download, None).await;
    send_extension_handshake(&mut client, 1).await;

    let deadline = Instant::now() + Duration::from_secs(2);
    let mut saw_have_all = false;
    while Instant::now() < deadline && !saw_have_all {
        let message = tokio::time::timeout(Duration::from_secs(1), read_message(&mut client))
            .await
            .expect("timed out")
            .expect("peer closed");
        match message {
            PeerMessage::HaveAll => saw_have_all = true,
            PeerMessage::Bitfield(_) => panic!("fast seeder sent a bitfield instead of have_all"),
            _ => {}
        }
    }
    assert!(saw_have_all, "have_all not received");

    let _ = info_hash;
    let _ = shutdown_tx.send(true);
    let _ = session.await;
}

#[tokio::test]
async fn fast_allowed_and_suggested_pieces_are_tracked() {
    let payload = vec![0x22u8; 64];
    let metadata = metadata(&payload, false);
    let info_hash = metadata.info_hash;
    let download = Arc::new(Mutex::new(Download::new(metadata)));
    let remote_peer_id = HashString::from_hex("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb").unwrap();

    let (mut client, session, shutdown_tx) = start(Arc::clone(&download), None).await;
    send_extension_handshake(&mut client, 1).await;
    client
        .write_all(&PeerMessage::AllowedFast { index: 0 }.encode())
        .await
        .unwrap();
    client
        .write_all(&PeerMessage::SuggestPiece { index: 0 }.encode())
        .await
        .unwrap();

    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if let Some((allowed, suggested)) = peer_fast_pieces(&info_hash, &remote_peer_id) {
            if allowed == vec![0] && suggested == vec![0] {
                break;
            }
        }
        assert!(Instant::now() < deadline, "fast state not recorded");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    let _ = shutdown_tx.send(true);
    let _ = session.await;
    // Session teardown drops the per-peer Fast state.
    assert!(peer_fast_pieces(&info_hash, &remote_peer_id).is_none());
}

#[tokio::test]
async fn pex_sends_connected_peers_and_accepts_updates() {
    let payload = vec![0x33u8; 64];
    let metadata = metadata(&payload, false);
    let info_hash = metadata.info_hash;
    let download = Arc::new(Mutex::new(Download::new(metadata)));

    let peer: SocketAddr = "127.0.0.1:7001".parse().unwrap();
    let pool: PeerPoolHandle = Arc::new(Mutex::new(PeerPool::new(16)));
    {
        let mut pool = pool.lock().unwrap();
        assert!(pool.add_peer(peer, PeerSource::Tracker, false));
        pool.record_success(peer, Instant::now());
    }

    let (mut client, session, shutdown_tx) =
        start(Arc::clone(&download), Some(Arc::clone(&pool))).await;
    send_extension_handshake(&mut client, 1).await;

    // We advertise PEX with id 2, so our client sends updates on id 2.
    let new_peer: SocketAddr = "127.0.0.1:7002".parse().unwrap();
    client
        .write_all(
            &PeerMessage::Extended {
                id: LOCAL_PEX_ID,
                payload: ut_pex_payload(&[lt_net::addr::SocketAddress::from(new_peer)], &[]),
            }
            .encode(),
        )
        .await
        .unwrap();

    // The server sends its own PEX update on the id advertised by the client.
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut saw_pex = false;
    while Instant::now() < deadline && !saw_pex {
        let message = tokio::time::timeout(Duration::from_secs(3), read_message(&mut client))
            .await
            .expect("timed out")
            .expect("peer closed");
        if let PeerMessage::Extended { id: 1, payload } = message {
            let pex = lt_protocol::extensions::UtPexMessage::parse(&payload).unwrap();
            if pex
                .all_added()
                .any(|address| address.as_socket_addr() == Some(peer))
            {
                saw_pex = true;
            }
        }
    }
    assert!(saw_pex, "periodic PEX update was not sent");

    // The peer list from the client must be merged.
    let deadline = Instant::now() + Duration::from_secs(2);
    while pool.lock().unwrap().get(new_peer).is_none() {
        assert!(Instant::now() < deadline, "PEX peer was not merged");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(pex_statistics().added >= 1);
    let _ = info_hash;

    let _ = shutdown_tx.send(true);
    let _ = session.await;
}

#[tokio::test]
async fn private_torrents_ignore_pex() {
    let payload = vec![0x44u8; 64];
    let metadata = metadata(&payload, true);
    let download = Arc::new(Mutex::new(Download::new(metadata)));

    let pool: PeerPoolHandle = Arc::new(Mutex::new(PeerPool::new(16)));
    let (mut client, session, shutdown_tx) =
        start(Arc::clone(&download), Some(Arc::clone(&pool))).await;
    send_extension_handshake(&mut client, 1).await;

    let injected: SocketAddr = "127.0.0.1:7003".parse().unwrap();
    client
        .write_all(
            &PeerMessage::Extended {
                id: LOCAL_PEX_ID,
                payload: ut_pex_payload(&[lt_net::addr::SocketAddress::from(injected)], &[]),
            }
            .encode(),
        )
        .await
        .unwrap();

    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(pool.lock().unwrap().get(injected).is_none());
    assert!(pex_statistics().private_blocked >= 1);

    let _ = shutdown_tx.send(true);
    let _ = session.await;
}
