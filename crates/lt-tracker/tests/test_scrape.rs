use std::time::Duration;

use lt_common::hash_string::HashString;
use lt_common::object::{bencode, Object};
use lt_tracker::client::{
    announce_udp_detailed_with_router, scrape_http, scrape_udp, scrape_udp_with_router,
    AnnounceParams, ScrapeStats,
};
use lt_tracker::controller::TrackerController;
use lt_tracker::list::TrackerList;
use lt_tracker::udp::{
    AnnounceRequest, AnnounceResponse, ConnectRequest, ConnectResponse, ScrapeFileStats,
    ScrapeRequest, ScrapeResponse,
};
use lt_tracker::udp_router::UdpRouter;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, UdpSocket};

fn id(byte: u8) -> HashString {
    HashString::new([byte; 20])
}

#[tokio::test]
async fn http_scrape_parses_files_map() {
    let info_hash = id(0x11);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();

    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = [0u8; 4096];
        let read = socket.read(&mut request).await.unwrap();
        let request_text = String::from_utf8_lossy(&request[..read]);
        assert!(request_text.contains("/scrape"), "unexpected request: {request_text}");

        let mut entry = Object::create_map();
        entry.insert_key("complete", Object::from(5i64)).unwrap();
        entry.insert_key("downloaded", Object::from(9i64)).unwrap();
        entry.insert_key("incomplete", Object::from(3i64)).unwrap();

        let mut files = Object::create_map();
        files.insert_key(info_hash.as_slice(), entry).unwrap();

        let mut root = Object::create_map();
        root.insert_key("files", files).unwrap();
        let body = bencode::encode(&root).unwrap();

        let header = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        socket.write_all(header.as_bytes()).await.unwrap();
        socket.write_all(&body).await.unwrap();
    });

    let stats = scrape_http(
        &format!("http://127.0.0.1:{port}/announce"),
        info_hash,
    )
    .await
    .unwrap();

    assert_eq!(
        stats,
        ScrapeStats {
            seeders: 5,
            completed: 9,
            leechers: 3,
        }
    );

    server.await.unwrap();
}

#[tokio::test]
async fn udp_scrape_round_trip() {
    let info_hash = id(0x22);
    let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let port = socket.local_addr().unwrap().port();

    let server = tokio::spawn(async move {
        let mut buffer = [0u8; 2048];

        let (read, client) = socket.recv_from(&mut buffer).await.unwrap();
        let connect = ConnectRequest::decode(&buffer[..read]).unwrap();
        let response = ConnectResponse::new(connect.transaction_id, 0x1122_3344_5566_7788);
        socket.send_to(&response.encode(), client).await.unwrap();

        let (read, _) = socket.recv_from(&mut buffer).await.unwrap();
        let scrape = ScrapeRequest::decode(&buffer[..read]).unwrap();
        assert_eq!(scrape.info_hashes, vec![*info_hash.as_bytes()]);

        let response = ScrapeResponse {
            transaction_id: scrape.transaction_id,
            stats: vec![ScrapeFileStats {
                seeders: 7,
                completed: 9,
                leechers: 3,
            }],
        };
        socket.send_to(&response.encode(), client).await.unwrap();
    });

    let stats = tokio::time::timeout(
        Duration::from_secs(5),
        scrape_udp(&format!("udp://127.0.0.1:{port}/announce"), info_hash),
    )
    .await
    .expect("scrape timed out")
    .unwrap();

    assert_eq!(
        stats,
        ScrapeStats {
            seeders: 7,
            completed: 9,
            leechers: 3,
        }
    );

    server.await.unwrap();
}

#[test]
fn controller_records_scrape_statistics() {
    let mut tracker_list = TrackerList::new();
    tracker_list.insert_url(0, "http://tracker.example.com/announce", false);
    let mut controller = TrackerController::new(tracker_list);

    controller.receive_scrape_stats(
        0,
        ScrapeStats {
            seeders: 11,
            completed: 22,
            leechers: 33,
        },
    );

    let state = controller.tracker_list.at(0).unwrap().state();
    assert_eq!(state.scrape_complete, 11);
    assert_eq!(state.scrape_downloaded, 22);
    assert_eq!(state.scrape_incomplete, 33);
    assert!(!controller.is_scrape_queued());
    assert_eq!(state.counters.scrape_counter, 1);
}

#[tokio::test]
async fn http_scrape_missing_optional_keys_uses_zero() {
    let info_hash = id(0x33);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();

    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = [0u8; 4096];
        let _ = socket.read(&mut request).await.unwrap();

        let mut entry = Object::create_map();
        entry.insert_key("complete", Object::from(2i64)).unwrap();

        let mut files = Object::create_map();
        files.insert_key(info_hash.as_slice(), entry).unwrap();

        let mut root = Object::create_map();
        root.insert_key("files", files).unwrap();
        let body = bencode::encode(&root).unwrap();
        let header = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        socket.write_all(header.as_bytes()).await.unwrap();
        socket.write_all(&body).await.unwrap();
    });

    let stats = scrape_http(&format!("http://127.0.0.1:{port}/announce"), info_hash)
        .await
        .unwrap();

    assert_eq!(
        stats,
        ScrapeStats {
            seeders: 2,
            completed: 0,
            leechers: 0,
        }
    );
    server.await.unwrap();
}

#[tokio::test]
async fn http_scrape_malformed_bencode_is_an_error() {
    let info_hash = id(0x44);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();

    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = [0u8; 4096];
        let _ = socket.read(&mut request).await.unwrap();

        let body = b"this is not bencode";
        let header = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        socket.write_all(header.as_bytes()).await.unwrap();
        socket.write_all(body).await.unwrap();
    });

    let result = scrape_http(&format!("http://127.0.0.1:{port}/announce"), info_hash).await;
    assert!(result.is_err(), "malformed bencode must not panic");
    server.await.unwrap();
}

#[test]
fn unsupported_scrape_url_is_classified() {
    let err = lt_tracker::client::TrackerClientError::TrackerHttp(
        lt_tracker::http::HttpTrackerError::InvalidScrapeUrl,
    );
    assert!(err.is_scrape_unsupported());
}

#[tokio::test]
async fn udp_announce_uses_router_and_returns_interval() {
    let info_hash = id(0x55);
    let peer_id = id(0x66);
    let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let port = socket.local_addr().unwrap().port();

    let server = tokio::spawn(async move {
        let mut buffer = [0u8; 2048];

        let (read, client) = socket.recv_from(&mut buffer).await.unwrap();
        let connect = ConnectRequest::decode(&buffer[..read]).unwrap();
        let connection_id = 0x0102_0304_0506_0708;
        socket
            .send_to(
                &ConnectResponse::new(connect.transaction_id, connection_id).encode(),
                client,
            )
            .await
            .unwrap();

        let (read, _) = socket.recv_from(&mut buffer).await.unwrap();
        let announce = AnnounceRequest::decode(&buffer[..read]).unwrap();
        assert_eq!(announce.info_hash, *info_hash.as_bytes());
        assert_eq!(announce.peer_id, *peer_id.as_bytes());

        let mut peers = lt_net::addr::AddressList::new();
        peers.push(lt_net::addr::SocketAddress::new_inet(
            std::net::Ipv4Addr::new(127, 0, 0, 1),
            7000,
        ));
        let response = AnnounceResponse {
            transaction_id: announce.transaction_id,
            interval: 1234,
            leechers: 4,
            seeders: 5,
            peers,
        };
        socket.send_to(&response.encode(), client).await.unwrap();
    });

    let mut router = UdpRouter::new();
    let params = AnnounceParams {
        info_hash,
        peer_id,
        port: 6881,
        uploaded: 10,
        downloaded: 20,
        left: 30,
        numwant: 50,
        event: Some("started".into()),
    };

    let result = tokio::time::timeout(
        Duration::from_secs(10),
        announce_udp_detailed_with_router(
            &format!("udp://127.0.0.1:{port}/announce"),
            &params,
            &mut router,
        ),
    )
    .await
    .expect("announce timed out")
    .unwrap();

    assert_eq!(result.interval, Some(1234));
    assert_eq!(result.complete, Some(5));
    assert_eq!(result.incomplete, Some(4));
    assert_eq!(result.peers.len(), 1);
    assert_eq!(router.active_transaction_count(), 0);

    server.await.unwrap();
}

#[tokio::test]
async fn udp_scrape_uses_router_and_releases_transaction() {
    let info_hash = id(0x77);
    let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let port = socket.local_addr().unwrap().port();

    let server = tokio::spawn(async move {
        let mut buffer = [0u8; 2048];

        let (read, client) = socket.recv_from(&mut buffer).await.unwrap();
        let connect = ConnectRequest::decode(&buffer[..read]).unwrap();
        socket
            .send_to(
                &ConnectResponse::new(connect.transaction_id, 0x1122_3344_5566_7788).encode(),
                client,
            )
            .await
            .unwrap();

        let (read, _) = socket.recv_from(&mut buffer).await.unwrap();
        let scrape = ScrapeRequest::decode(&buffer[..read]).unwrap();
        let response = ScrapeResponse {
            transaction_id: scrape.transaction_id,
            stats: vec![ScrapeFileStats {
                seeders: 8,
                completed: 9,
                leechers: 10,
            }],
        };
        socket.send_to(&response.encode(), client).await.unwrap();
    });

    let mut router = UdpRouter::new();
    let stats = tokio::time::timeout(
        Duration::from_secs(10),
        scrape_udp_with_router(
            &format!("udp://127.0.0.1:{port}/announce"),
            info_hash,
            &mut router,
        ),
    )
    .await
    .expect("scrape timed out")
    .unwrap();

    assert_eq!(
        stats,
        ScrapeStats {
            seeders: 8,
            completed: 9,
            leechers: 10,
        }
    );
    assert_eq!(router.active_transaction_count(), 0);

    server.await.unwrap();
}

#[tokio::test]
async fn udp_malformed_response_is_a_protocol_error() {
    let info_hash = id(0x88);
    let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let port = socket.local_addr().unwrap().port();

    let server = tokio::spawn(async move {
        let mut buffer = [0u8; 2048];
        let (_, client) = socket.recv_from(&mut buffer).await.unwrap();
        // Shorter than the 16-byte connect header.
        socket.send_to(&[0u8; 4], client).await.unwrap();
    });

    let mut router = UdpRouter::new();
    let result = tokio::time::timeout(
        Duration::from_secs(5),
        scrape_udp_with_router(
            &format!("udp://127.0.0.1:{port}/announce"),
            info_hash,
            &mut router,
        ),
    )
    .await
    .expect("malformed UDP response must not hang");

    assert!(result.is_err());
    assert_eq!(router.active_transaction_count(), 0);
    server.await.unwrap();
}

#[tokio::test]
async fn udp_announce_retries_after_lost_connect() {
    let info_hash = id(0x99);
    let peer_id = id(0xaa);
    let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let port = socket.local_addr().unwrap().port();

    let server = tokio::spawn(async move {
        let mut buffer = [0u8; 2048];

        // Drop the first connect request entirely.
        let _ = socket.recv_from(&mut buffer).await.unwrap();

        let (read, client) = socket.recv_from(&mut buffer).await.unwrap();
        let connect = ConnectRequest::decode(&buffer[..read]).unwrap();
        socket
            .send_to(
                &ConnectResponse::new(connect.transaction_id, 0x2222_3333_4444_5555).encode(),
                client,
            )
            .await
            .unwrap();

        let (read, _) = socket.recv_from(&mut buffer).await.unwrap();
        let announce = AnnounceRequest::decode(&buffer[..read]).unwrap();
        socket
            .send_to(
                &AnnounceResponse {
                    transaction_id: announce.transaction_id,
                    interval: 60,
                    leechers: 0,
                    seeders: 0,
                    peers: lt_net::addr::AddressList::new(),
                }
                .encode(),
                client,
            )
            .await
            .unwrap();
    });

    let mut router = UdpRouter::new();
    let params = AnnounceParams {
        info_hash,
        peer_id,
        port: 6881,
        uploaded: 0,
        downloaded: 0,
        left: 0,
        numwant: 0,
        event: Some("started".into()),
    };

    let result = tokio::time::timeout(
        Duration::from_secs(10),
        announce_udp_detailed_with_router(
            &format!("udp://127.0.0.1:{port}/announce"),
            &params,
            &mut router,
        ),
    )
    .await
    .expect("retry timed out")
    .unwrap();

    assert_eq!(result.interval, Some(60));
    assert_eq!(router.active_transaction_count(), 0);
    server.await.unwrap();
}

#[test]
fn controller_schedules_scrape_from_intervals() {
    let mut tracker_list = TrackerList::new();
    tracker_list.insert_url(0, "http://tracker.example.com/announce", false);
    let mut controller = TrackerController::new(tracker_list);
    // Do not enable the announce controller here: `do_scrape` intentionally
    // skips a tier with an outstanding announce, which `enable()` queues.
    controller.scrape_request(0);
    controller.do_scrape();
    assert!(controller.is_scrape_in_flight());
    assert_eq!(controller.pending_scrape_count(), 1);

    controller.receive_scrape_stats(
        0,
        ScrapeStats {
            seeders: 1,
            completed: 2,
            leechers: 3,
        },
    );

    assert!(!controller.is_scrape_in_flight());
    assert!(controller.is_scrape_queued());
    let normal = controller.tracker_list.at(0).unwrap().state().normal_interval as u32;
    assert_eq!(controller.seconds_to_next_scrape(), normal);
}

#[test]
fn do_scrape_queues_one_tracker_per_tier() {
    let mut tracker_list = TrackerList::new();
    tracker_list.insert_url(0, "http://tier0-a.example.com/announce", false);
    tracker_list.insert_url(0, "http://tier0-b.example.com/announce", false);
    tracker_list.insert_url(1, "http://tier1.example.com/announce", false);
    let mut controller = TrackerController::new(tracker_list);

    controller.do_scrape();

    assert!(controller.tracker_list.at(0).unwrap().is_requesting());
    assert!(!controller.tracker_list.at(1).unwrap().is_requesting());
    assert!(controller.tracker_list.at(2).unwrap().is_requesting());
    assert_eq!(controller.pending_scrape_count(), 2);

    controller.receive_scrape_stats(
        0,
        ScrapeStats {
            seeders: 1,
            completed: 1,
            leechers: 1,
        },
    );
    assert!(controller.is_scrape_in_flight());

    controller.receive_scrape_stats(
        2,
        ScrapeStats {
            seeders: 2,
            completed: 2,
            leechers: 2,
        },
    );
    assert!(!controller.is_scrape_in_flight());
    assert!(controller.is_scrape_queued());
}

#[test]
fn scrape_failure_uses_min_interval_and_does_not_fail_tracker() {
    let mut tracker_list = TrackerList::new();
    tracker_list.insert_url(0, "http://tracker.example.com/announce", false);
    let mut controller = TrackerController::new(tracker_list);
    controller.enable(0);

    controller.scrape_request(0);
    controller.do_scrape();
    controller.receive_scrape_failure_with_reason(0, "scrape not supported");

    let state = controller.tracker_list.at(0).unwrap().state();
    assert_eq!(state.counters.failed_counter, 0);
    assert_eq!(state.counters.scrape_failed_counter, 1);
    assert!(controller.tracker_list.at(0).unwrap().is_usable());
    assert!(controller.is_scrape_queued());
    assert_eq!(
        controller.seconds_to_next_scrape(),
        state.min_interval as u32
    );
}

#[test]
fn unsupported_scrape_keeps_tracker_usable_but_disables_scraping() {
    let mut tracker_list = TrackerList::new();
    tracker_list.insert_url(0, "http://tracker.example.com/announce", false);
    let mut controller = TrackerController::new(tracker_list);
    controller.enable(0);

    controller.receive_scrape_unsupported(0, "scrape unsupported");

    let tracker = controller.tracker_list.at(0).unwrap();
    assert!(tracker.is_usable());
    assert!(!tracker.is_scrapable());
    assert_eq!(tracker.state().counters.failed_counter, 0);
    assert!(!controller.is_scrape_queued());
}

#[test]
fn failure_moves_preference_to_next_tier_entry_and_success_promotes() {
    let mut tracker_list = TrackerList::new();
    tracker_list.insert_url(0, "http://tracker-a.example.com/announce", false);
    tracker_list.insert_url(0, "http://tracker-b.example.com/announce", false);
    let mut controller = TrackerController::new(tracker_list);
    controller.enable(0);

    controller.send_update_event();
    assert!(controller.tracker_list.at(0).unwrap().is_requesting());

    controller.receive_failure(0, "connection refused");
    controller.send_update_event();

    assert!(!controller.tracker_list.at(0).unwrap().is_requesting());
    assert!(controller.tracker_list.at(1).unwrap().is_requesting());

    let empty = lt_net::addr::AddressList::new();
    controller.receive_success(1, &empty);

    assert_eq!(
        controller.tracker_list.at(0).unwrap().url(),
        "http://tracker-b.example.com/announce"
    );
}
