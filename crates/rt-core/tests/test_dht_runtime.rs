use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use lt_common::hash_string::HashString;
use lt_dht::controller::{DhtLifecycleConfig, DhtMode};
use lt_dht::transaction::{KrpcMessage, KrpcQuery, KrpcResponse};
use lt_net::config::NetworkConfig;
use rt_core::download_list::DownloadList;
use rt_core::network_runtime::{NetworkRuntime, ShutdownTimeouts};

fn free_udp_port() -> u16 {
    let socket = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    socket.local_addr().unwrap().port()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn runtime_starts_dht_on_override_port() {
    let port = free_udp_port();
    let list = Arc::new(RwLock::new(DownloadList::new()));
    let peer_id = HashString::new([0xab; 20]);
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);

    let mut dht_config = DhtLifecycleConfig::default();
    dht_config.mode = DhtMode::On;
    dht_config.port_override = port;
    dht_config.refresh_interval = Duration::from_secs(60);
    dht_config.bootstrap_retry_interval = Duration::from_secs(60);

    let runtime = NetworkRuntime::spawn_with_network_config_and_dht(
        list,
        peer_id,
        0,
        shutdown_rx,
        None,
        NetworkConfig::new(),
        dht_config,
    );

    let socket = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let query = KrpcMessage::Query(KrpcQuery::Ping { id: peer_id })
        .encode(b"n3")
        .unwrap();

    let deadline = Instant::now() + Duration::from_secs(4);
    let mut buffer = [0u8; 2048];

    let reply = loop {
        socket.send_to(&query, ("127.0.0.1", port)).await.unwrap();

        match tokio::time::timeout(Duration::from_millis(250), socket.recv_from(&mut buffer)).await
        {
            Ok(Ok((size, _))) => break buffer[..size].to_vec(),
            Ok(Err(err)) => panic!("DHT socket receive failed: {err}"),
            Err(_) if Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
            Err(_) => panic!("DHT did not answer on override port {port}"),
        }
    };

    let (_, message) = KrpcMessage::decode(&reply).unwrap();
    match message {
        KrpcMessage::Response(KrpcResponse::Ping { id }) => {
            assert!(!id.is_zero(), "DHT replied with an empty node id");
        }
        other => panic!("unexpected DHT reply: {other:?}"),
    }

    let _ = shutdown_tx.send(true);
    let report = runtime.drain(ShutdownTimeouts::default()).await;
    assert!(
        report.all_completed(),
        "network runtime did not drain: {report:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn runtime_off_mode_does_not_bind_dht() {
    let port = free_udp_port();
    let list = Arc::new(RwLock::new(DownloadList::new()));
    let peer_id = HashString::new([0xac; 20]);
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);

    let mut dht_config = DhtLifecycleConfig::default();
    dht_config.mode = DhtMode::Off;
    dht_config.port_override = port;

    let runtime = NetworkRuntime::spawn_with_network_config_and_dht(
        list,
        peer_id,
        0,
        shutdown_rx,
        None,
        NetworkConfig::new(),
        dht_config,
    );

    // Give the runtime a chance to (incorrectly) bind the port.
    tokio::time::sleep(Duration::from_millis(100)).await;

    let socket = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let query = KrpcMessage::Query(KrpcQuery::Ping { id: peer_id })
        .encode(b"n4")
        .unwrap();
    socket.send_to(&query, ("127.0.0.1", port)).await.unwrap();

    let mut buffer = [0u8; 2048];
    let received =
        tokio::time::timeout(Duration::from_millis(300), socket.recv_from(&mut buffer)).await;
    match received {
        Ok(Ok(_)) => panic!("DHT answered even though mode is off"),
        Ok(Err(_)) | Err(_) => {}
    }

    let _ = shutdown_tx.send(true);
    let report = runtime.drain(ShutdownTimeouts::default()).await;
    assert!(
        report.all_completed(),
        "network runtime did not drain: {report:?}"
    );
}
