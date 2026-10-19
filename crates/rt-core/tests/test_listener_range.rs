use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use lt_common::hash_string::HashString;
use rt_core::download_list::DownloadList;
use rt_core::network_runtime::{
    listener_statistics, NetworkRuntime, PeerListenerConfig, ShutdownTimeouts,
};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn listener_port_range_selects_an_available_port() {
    // Reserve a two-port window, then release it for the listener. A random
    // start still has to land inside the configured range.
    let probe_a = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port_a = probe_a.local_addr().unwrap().port();
    let probe_b = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port_b = probe_b.local_addr().unwrap().port();
    drop(probe_a);
    drop(probe_b);
    let (first, last) = if port_a < port_b {
        (port_a, port_b)
    } else {
        (port_b, port_a)
    };

    let list = Arc::new(RwLock::new(DownloadList::new()));
    let peer_id = HashString::new([0xcd; 20]);
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);

    let listener = PeerListenerConfig::new(0).with_port_range(first, last, true);
    let runtime = NetworkRuntime::spawn_with_listener_config(
        list,
        peer_id,
        0,
        shutdown_rx,
        None,
        lt_net::config::NetworkConfig::new(),
        lt_dht::controller::DhtLifecycleConfig {
            mode: lt_dht::controller::DhtMode::Off,
            ..Default::default()
        },
        listener,
    );

    let deadline = Instant::now() + Duration::from_secs(3);
    let bound = loop {
        let stats = listener_statistics();
        if let Some(port) = stats.ports.first().copied() {
            break port;
        }
        assert!(Instant::now() < deadline, "listener did not bind");
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    assert!(
        (first..=last).contains(&bound),
        "bound port {bound} outside range {first}-{last}"
    );

    let _ = shutdown_tx.send(true);
    let report = runtime.drain(ShutdownTimeouts::default()).await;
    assert!(report.all_completed(), "listener did not drain: {report:?}");
    assert!(!listener_statistics().open);
}
