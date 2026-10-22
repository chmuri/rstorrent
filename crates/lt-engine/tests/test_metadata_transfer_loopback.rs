use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use lt_common::hash_string::HashString;
use lt_common::object::Object;
use lt_crypto::sha1::sha1_hash;
use lt_engine::constructor::DownloadConstructor;
use lt_engine::download::Download;
use lt_engine::network::{
    register_local_metadata, run_inbound_peer, run_inbound_peer_with_request_timeout,
};
use lt_protocol::extensions::{
    extension_handshake_payload, ut_metadata_data_payload, ut_metadata_request_payload,
    UtMetadataMessage,
};
use lt_protocol::handshake::HandshakeMessage;
use lt_protocol::message::PeerMessage;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;

const LOCAL_METADATA_ID: u8 = 1;

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

async fn start_inbound(
    download: Arc<Mutex<Download>>,
    remote_peer_id: HashString,
    request_timeout: Option<Duration>,
) -> (TcpStream, tokio::task::JoinHandle<()>, watch::Sender<bool>) {
    let our_peer_id = HashString::from_hex("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa").unwrap();
    let info_hash = *download.lock().unwrap().info_hash();
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();

    let (client_res, server_res) = tokio::join!(TcpStream::connect(addr), listener.accept());
    let client = client_res.unwrap();
    let (server, _) = server_res.unwrap();

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let handshake = HandshakeMessage::new(info_hash, remote_peer_id);
    let session = tokio::spawn(async move {
        match request_timeout {
            Some(timeout) => {
                run_inbound_peer_with_request_timeout(
                    server,
                    handshake,
                    our_peer_id,
                    download,
                    shutdown_rx,
                    timeout,
                )
                .await;
            }
            None => {
                run_inbound_peer(server, handshake, our_peer_id, download, shutdown_rx).await;
            }
        }
    });

    (client, session, shutdown_tx)
}

async fn exchange_handshake(client: &mut TcpStream, metadata_id: u8, total_size: Option<u32>) {
    let mut response = [0u8; 68];
    tokio::time::timeout(Duration::from_secs(2), client.read_exact(&mut response))
        .await
        .expect("handshake response timed out")
        .expect("peer closed before handshake response");

    client
        .write_all(
            &PeerMessage::Extended {
                id: 0,
                payload: extension_handshake_payload(0, metadata_id, total_size),
            }
            .encode(),
        )
        .await
        .unwrap();
}

fn padded_info(name: &str, payload_size: usize) -> Vec<u8> {
    let piece_length = 16384usize;
    let data = vec![0x5au8; payload_size];
    let mut pieces = Vec::new();
    for chunk in data.chunks(piece_length) {
        pieces.extend_from_slice(&sha1_hash(chunk));
    }

    let mut info = Object::create_map();
    info.insert_key("name", Object::from(name)).unwrap();
    info.insert_key("piece length", Object::from(piece_length as i64))
        .unwrap();
    info.insert_key("pieces", Object::create_string(pieces))
        .unwrap();
    info.insert_key("length", Object::from(payload_size as i64))
        .unwrap();
    // Unknown keys are preserved because the metadata is transferred as raw
    // bytes. The padding forces a multi-piece metadata transfer.
    info.insert_key("comment", Object::create_string(vec![b'x'; 20_000]))
        .unwrap();
    lt_common::object::bencode::encode(&info).unwrap()
}

#[tokio::test]
async fn magnet_metadata_is_downloaded_from_pieces_and_applied() {
    let info_bytes = padded_info("magnet-loopback", 40_000);
    let info_hash = HashString::new(sha1_hash(&info_bytes));
    assert!(info_bytes.len() > 16_384, "test fixture must span pieces");

    let uri = format!(
        "magnet:?xt=urn:btih:{}&dn=magnet-loopback",
        info_hash.to_hex()
    );
    let metadata = DownloadConstructor::from_magnet_uri(&uri).unwrap();
    let download = Arc::new(Mutex::new(Download::new(metadata)));
    let remote_peer_id = HashString::from_hex("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb").unwrap();

    let (mut client, session, shutdown_tx) =
        start_inbound(Arc::clone(&download), remote_peer_id, None).await;
    exchange_handshake(&mut client, 2, Some(info_bytes.len() as u32)).await;

    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        let message = tokio::time::timeout(Duration::from_secs(2), read_message(&mut client))
            .await
            .expect("timed out waiting for metadata request")
            .expect("peer closed before metadata request");

        if let PeerMessage::Extended { id: 2, payload } = message {
            match UtMetadataMessage::parse(&payload).unwrap() {
                UtMetadataMessage::Request { piece } => {
                    let offset = piece as usize * 16_384;
                    let end = (offset + 16_384).min(info_bytes.len());
                    let payload = ut_metadata_data_payload(
                        piece,
                        info_bytes.len() as u32,
                        &info_bytes[offset..end],
                    );
                    client
                        .write_all(
                            &PeerMessage::Extended {
                                id: LOCAL_METADATA_ID,
                                payload,
                            }
                            .encode(),
                        )
                        .await
                        .unwrap();
                }
                UtMetadataMessage::Reject { .. } => panic!("unexpected server reject"),
                UtMetadataMessage::Data { .. } => {}
            }
        }

        if !download.lock().unwrap().is_metadata_download() {
            break;
        }
    }

    {
        let download_guard = download.lock().unwrap();
        assert!(
            !download_guard.is_metadata_download(),
            "metadata was never applied"
        );
        assert_eq!(download_guard.metadata().name, "magnet-loopback");
        assert_eq!(download_guard.metadata().total_size, 40_000);
    }

    let _ = shutdown_tx.send(true);
    let _ = session.await;
}

#[tokio::test]
async fn metadata_requests_are_served_from_local_cache() {
    let info_bytes = padded_info("serve-me", 20_000);
    let info_hash = HashString::new(sha1_hash(&info_bytes));
    assert!(register_local_metadata(info_hash, info_bytes.clone()));

    let metadata =
        DownloadConstructor::from_info_bytes(&info_bytes, info_hash, None, Vec::new()).unwrap();
    let download = Arc::new(Mutex::new(Download::new(metadata)));
    let remote_peer_id = HashString::from_hex("cccccccccccccccccccccccccccccccccccccccc").unwrap();

    let (mut client, session, shutdown_tx) =
        start_inbound(Arc::clone(&download), remote_peer_id, None).await;
    exchange_handshake(&mut client, 2, Some(info_bytes.len() as u32)).await;

    client
        .write_all(
            &PeerMessage::Extended {
                id: LOCAL_METADATA_ID,
                payload: ut_metadata_request_payload(1),
            }
            .encode(),
        )
        .await
        .unwrap();

    let deadline = Instant::now() + Duration::from_secs(3);
    let data = loop {
        let message = tokio::time::timeout(Duration::from_secs(1), read_message(&mut client))
            .await
            .expect("timed out waiting for metadata data")
            .expect("peer closed before metadata data");
        if let PeerMessage::Extended {
            id: LOCAL_METADATA_ID,
            payload,
        } = message
        {
            if let Ok(UtMetadataMessage::Data {
                piece,
                total_size,
                data,
            }) = UtMetadataMessage::parse(&payload)
            {
                break (piece, total_size, data);
            }
        }
        assert!(Instant::now() < deadline, "metadata data not received");
    };

    assert_eq!(data.0, 1);
    assert_eq!(data.1 as usize, info_bytes.len());
    assert_eq!(data.2, info_bytes[16_384..]);

    let _ = shutdown_tx.send(true);
    let _ = session.await;
}

#[tokio::test]
async fn malformed_metadata_is_rejected_without_panicking() {
    let info_bytes = padded_info("reject-me", 20_000);
    let info_hash = HashString::new(sha1_hash(&info_bytes));
    let metadata =
        DownloadConstructor::from_info_bytes(&info_bytes, info_hash, None, Vec::new()).unwrap();
    let download = Arc::new(Mutex::new(Download::new(metadata)));
    let remote_peer_id = HashString::from_hex("dddddddddddddddddddddddddddddddddddddddd").unwrap();

    let (mut client, session, shutdown_tx) = start_inbound(download, remote_peer_id, None).await;
    exchange_handshake(&mut client, 2, Some(info_bytes.len() as u32)).await;

    // Out-of-range piece and an absurd advertised size must not crash or
    // allocate unbounded memory.
    for payload in [
        ut_metadata_request_payload(999_999),
        ut_metadata_data_payload(0, u32::MAX, b"bad"),
    ] {
        client
            .write_all(
                &PeerMessage::Extended {
                    id: LOCAL_METADATA_ID,
                    payload,
                }
                .encode(),
            )
            .await
            .unwrap();
    }

    let _ = shutdown_tx.send(true);
    let _ = session.await;
}
