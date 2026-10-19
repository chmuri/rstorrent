use std::net::{Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use lt_common::hash_string::HashString;
use lt_crypto::sha1::sha1_hash;
use lt_engine::constructor::{TorrentFileEntry, TorrentMetadata};
use lt_engine::download::Download;
use lt_engine::network::{run_inbound_peer, run_inbound_peer_with_request_timeout};
use lt_net::addr::SocketAddress;
use lt_protocol::connection::PeerConnection;
use lt_protocol::handshake::HandshakeMessage;
use lt_protocol::message::PeerMessage;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;

fn metadata(payload: &[u8]) -> TorrentMetadata {
    let piece_hash = sha1_hash(payload);
    let mut path = lt_common::path::Path::new();
    path.push("payload.bin");

    TorrentMetadata {
        info_hash: HashString::new(sha1_hash(&piece_hash)),
        name: "loopback-pipeline".to_string(),
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
        is_private: false,
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

async fn read_until_request(stream: &mut TcpStream) -> (u32, u32, u32) {
    for _ in 0..32 {
        let message = tokio::time::timeout(Duration::from_secs(2), read_message(stream))
            .await
            .expect("timed out waiting for a request")
            .expect("peer closed before sending a request");

        if let PeerMessage::Request {
            index,
            begin,
            length,
        } = message
        {
            return (index, begin, length);
        }
    }

    panic!("no Request message received");
}

async fn wait_until_has_piece(download: &Arc<Mutex<Download>>, index: u32) {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if download.lock().unwrap().has_piece(index) {
            return;
        }
        assert!(Instant::now() < deadline, "piece {index} never completed");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

async fn start_session(
    download: &Arc<Mutex<Download>>,
    info_hash: HashString,
    remote_peer_id: HashString,
    request_timeout: Option<Duration>,
) -> (TcpStream, tokio::task::JoinHandle<()>, watch::Sender<bool>) {
    let our_peer_id = HashString::from_hex("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa").unwrap();

    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();

    let (client_res, server_res) = tokio::join!(TcpStream::connect(addr), listener.accept());
    let client = client_res.unwrap();
    let (server, _) = server_res.unwrap();

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let session_download = Arc::clone(download);
    let handshake = HandshakeMessage::new(info_hash, remote_peer_id);
    let session = tokio::spawn(async move {
        match request_timeout {
            Some(timeout) => {
                run_inbound_peer_with_request_timeout(
                    server,
                    handshake,
                    our_peer_id,
                    session_download,
                    shutdown_rx,
                    timeout,
                )
                .await;
            }
            None => {
                run_inbound_peer(
                    server,
                    handshake,
                    our_peer_id,
                    session_download,
                    shutdown_rx,
                )
                .await;
            }
        }
    });

    (client, session, shutdown_tx)
}

async fn finish_session(
    client: TcpStream,
    session: tokio::task::JoinHandle<()>,
    shutdown_tx: watch::Sender<bool>,
) {
    drop(client);
    let _ = shutdown_tx.send(true);
    let _ = tokio::time::timeout(Duration::from_secs(2), session).await;
}

#[tokio::test]
async fn choked_peer_cancels_requeues_and_late_blocks_are_accepted() {
    let payload = vec![0xAAu8; 16384];
    let metadata = metadata(&payload);
    let info_hash = metadata.info_hash;
    let download = Arc::new(Mutex::new(Download::new(metadata)));

    let remote_peer_id = HashString::from_hex("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb").unwrap();
    let (mut client, session, shutdown_tx) =
        start_session(&download, info_hash, remote_peer_id, None).await;

    // Read the engine handshake response.
    let mut response = [0u8; 68];
    tokio::time::timeout(Duration::from_secs(2), client.read_exact(&mut response))
        .await
        .expect("handshake response timed out")
        .expect("peer closed before handshake response");

    // Advertise the only piece and allow the engine to download from us.
    client
        .write_all(&PeerMessage::Bitfield(vec![0x80]).encode())
        .await
        .unwrap();
    client
        .write_all(&PeerMessage::Unchoke.encode())
        .await
        .unwrap();

    let (index, begin, length) = read_until_request(&mut client).await;
    assert_eq!(index, 0);
    assert_eq!(begin, 0);
    assert_eq!(length, payload.len() as u32);

    // Choke the engine. It must cancel the outstanding request and return
    // the block to the delegator.
    client
        .write_all(&PeerMessage::Choke.encode())
        .await
        .unwrap();

    let mut saw_cancel = false;
    for _ in 0..16 {
        let message = tokio::time::timeout(Duration::from_secs(2), read_message(&mut client))
            .await
            .expect("timed out waiting for Cancel")
            .expect("peer closed before Cancel");

        if let PeerMessage::Cancel {
            index: cancel_index,
            begin: cancel_begin,
            ..
        } = message
        {
            if cancel_index == index && cancel_begin == begin {
                saw_cancel = true;
                break;
            }
        }
    }
    assert!(saw_cancel, "choke must cancel outstanding requests");

    // The released block is now available to another peer.
    let second_id = HashString::new([0xCC; 20]);
    let mut second_peer = PeerConnection::new(
        second_id,
        SocketAddress::from(SocketAddr::from((Ipv4Addr::LOCALHOST, 51414))),
        1,
    );
    second_peer.bitfield.set(0);
    {
        let mut dl = download.lock().unwrap();
        assert!(dl.add_peer(second_peer));
        let requeued = dl.delegate_for_peer(&second_id, 1);
        assert_eq!(requeued.len(), 1, "cancelled block should be requeued");
        assert_eq!(requeued[0].index(), index);
    }

    // A block that was already requested may still arrive after Choke. It
    // must be accepted even though the request was cancelled.
    client
        .write_all(
            &PeerMessage::Piece {
                index,
                begin,
                data: payload,
            }
            .encode(),
        )
        .await
        .unwrap();

    wait_until_has_piece(&download, index).await;

    finish_session(client, session, shutdown_tx).await;
}

#[tokio::test]
async fn timed_out_request_is_cancelled_and_reissued() {
    let payload = vec![0xCCu8; 16384];
    let metadata = metadata(&payload);
    let info_hash = metadata.info_hash;
    let download = Arc::new(Mutex::new(Download::new(metadata)));

    let remote_peer_id = HashString::from_hex("eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee").unwrap();
    let (mut client, session, shutdown_tx) = start_session(
        &download,
        info_hash,
        remote_peer_id,
        Some(Duration::from_millis(50)),
    )
    .await;

    let mut response = [0u8; 68];
    tokio::time::timeout(Duration::from_secs(2), client.read_exact(&mut response))
        .await
        .expect("handshake response timed out")
        .expect("peer closed before handshake response");

    client
        .write_all(&PeerMessage::Bitfield(vec![0x80]).encode())
        .await
        .unwrap();
    client
        .write_all(&PeerMessage::Unchoke.encode())
        .await
        .unwrap();

    let (index, begin, _length) = read_until_request(&mut client).await;

    // The timeout tick (1 s) must cancel the stale request and immediately
    // re-issue it because this is the only available peer.
    let mut saw_cancel = false;
    let mut saw_request = false;
    for _ in 0..32 {
        let message = tokio::time::timeout(Duration::from_secs(2), read_message(&mut client))
            .await
            .expect("timed out waiting for timeout actions")
            .expect("peer closed before timeout actions");

        match message {
            PeerMessage::Cancel {
                index: cancel_index,
                begin: cancel_begin,
                ..
            } if cancel_index == index && cancel_begin == begin => {
                saw_cancel = true;
            }
            PeerMessage::Request {
                index: request_index,
                begin: request_begin,
                ..
            } if saw_cancel && request_index == index && request_begin == begin => {
                saw_request = true;
                break;
            }
            _ => {}
        }
    }

    assert!(saw_cancel, "timed-out request must be cancelled");
    assert!(saw_request, "timed-out block must be re-requested");

    client
        .write_all(
            &PeerMessage::Piece {
                index,
                begin,
                data: payload,
            }
            .encode(),
        )
        .await
        .unwrap();
    wait_until_has_piece(&download, index).await;

    finish_session(client, session, shutdown_tx).await;
}

#[tokio::test]
async fn rejected_request_is_reissued_to_the_peer() {
    let payload = vec![0xBBu8; 16384];
    let metadata = metadata(&payload);
    let info_hash = metadata.info_hash;
    let download = Arc::new(Mutex::new(Download::new(metadata)));

    let remote_peer_id = HashString::from_hex("dddddddddddddddddddddddddddddddddddddddd").unwrap();
    let (mut client, session, shutdown_tx) =
        start_session(&download, info_hash, remote_peer_id, None).await;

    let mut response = [0u8; 68];
    tokio::time::timeout(Duration::from_secs(2), client.read_exact(&mut response))
        .await
        .expect("handshake response timed out")
        .expect("peer closed before handshake response");

    client
        .write_all(&PeerMessage::Bitfield(vec![0x80]).encode())
        .await
        .unwrap();
    client
        .write_all(&PeerMessage::Unchoke.encode())
        .await
        .unwrap();

    let (index, begin, length) = read_until_request(&mut client).await;
    assert_eq!(index, 0);

    // Fast-extension reject: the engine must release the block and ask again
    // (this is the only available peer).
    client
        .write_all(
            &PeerMessage::RejectRequest {
                index,
                begin,
                length,
            }
            .encode(),
        )
        .await
        .unwrap();

    let (again_index, again_begin, again_length) = read_until_request(&mut client).await;
    assert_eq!(again_index, index, "rejected block should be re-requested");
    assert_eq!(again_begin, begin);
    assert_eq!(again_length, length);

    client
        .write_all(
            &PeerMessage::Piece {
                index,
                begin,
                data: payload,
            }
            .encode(),
        )
        .await
        .unwrap();
    wait_until_has_piece(&download, index).await;

    finish_session(client, session, shutdown_tx).await;
}
