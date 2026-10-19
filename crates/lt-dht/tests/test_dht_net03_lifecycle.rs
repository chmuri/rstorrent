use std::net::{Ipv4Addr, UdpSocket};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use lt_common::hash_string::HashString;
use lt_dht::controller::{DhtController, DhtLifecycleConfig, DhtMode, BOOTSTRAP_COMPLETE_NODES};
use lt_dht::node::DhtNode;
use lt_net::addr::SocketAddress;

fn free_udp_port() -> u16 {
    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    socket.local_addr().unwrap().port()
}

fn free_udp_port_other_than(other: u16) -> u16 {
    loop {
        let port = free_udp_port();
        if port != other {
            return port;
        }
    }
}

fn id(byte: u8) -> HashString {
    HashString::new([byte; 20])
}

fn now_seconds() -> u32 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as u32)
        .unwrap_or(0)
}

#[tokio::test]
async fn bootstrap_failure_is_bounded_and_non_fatal() {
    let controller = DhtController::new(id(0xaa), free_udp_port());
    controller.start();
    controller.server().bind().await.unwrap();

    // Hold the target port open without reading: the query can never get a
    // reply, and no parallel test can accidentally answer on this port.
    let silent_peer = UdpSocket::bind("127.0.0.1:0").unwrap();
    let unreachable = SocketAddress::new_inet(
        Ipv4Addr::LOCALHOST,
        silent_peer.local_addr().unwrap().port(),
    );
    let started = Instant::now();

    let outcome = tokio::time::timeout(
        Duration::from_secs(5),
        controller.bootstrap(&[unreachable]),
    )
    .await
    .expect("bootstrap must not block indefinitely")
    .expect("bootstrap failure is reported as zero replies");

    assert_eq!(outcome, 0);
    assert!(!controller.is_nodes_populated());
    assert!(started.elapsed() < Duration::from_secs(5));

    controller.stop();
    assert_eq!(controller.port(), 0);
}

#[tokio::test]
async fn cache_round_trip_keeps_self_id_public_nodes_and_trackers() {
    let self_id = id(0xaa);
    let controller = DhtController::new(self_id, 0);
    let info_hash = id(0x33);
    let private_hash = id(0x34);
    let now = now_seconds();

    {
        let router = controller.router();
        let mut router = router.lock().await;

        let public_id = id(0x01);
        let loopback_id = id(0x02);
        let private_id = id(0x03);

        router.add_node(DhtNode::new(
            public_id,
            SocketAddress::new_inet(Ipv4Addr::new(8, 8, 8, 8), 6881),
        ));
        router.add_node(DhtNode::new(
            loopback_id,
            SocketAddress::new_inet(Ipv4Addr::LOCALHOST, 6881),
        ));
        router.add_node(DhtNode::new(
            private_id,
            SocketAddress::new_inet(Ipv4Addr::new(10, 0, 0, 1), 6881),
        ));

        for node_id in [public_id, loopback_id, private_id] {
            router.mark_replied(&node_id, now);
        }

        router
            .get_tracker(&info_hash, true)
            .unwrap()
            .add_peer(u32::from(Ipv4Addr::new(1, 2, 3, 4)), 51413, now);

        router.set_private_info_hash(private_hash, true);
        assert!(router.get_tracker(&private_hash, true).is_none());
        assert!(router.is_private_info_hash(&private_hash));
    }

    let cache = controller.save_cache();
    assert!(cache.get_key("self_id").is_ok());

    let restored = DhtController::new(HashString::default(), 0);
    restored.initialize(&cache);

    assert_eq!(restored.get_statistics().num_nodes, 1);
    assert_eq!(restored.get_statistics().num_trackers, 1);

    {
        let router = restored.router();
        let mut router = router.lock().await;
        assert_eq!(router.own_id, self_id);

        let tracker = router
            .get_tracker(&info_hash, false)
            .expect("tracker cache was not restored");
        assert_eq!(tracker.size(), 1);
        assert_eq!(tracker.get_peers(1)[0], [1, 2, 3, 4, 0xc8, 0xd5]);
    }

    // File round-trip uses the same object format.
    let path = std::env::temp_dir().join(format!(
        "rtorrent_dht_net03_cache_{}.bin",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&path);
    controller.save_cache_to(&path).unwrap();

    let loaded = DhtController::new(HashString::default(), 0);
    loaded.load_cache_from(&path).unwrap();
    assert_eq!(loaded.get_statistics().num_nodes, 1);
    assert_eq!(loaded.get_statistics().num_trackers, 1);

    let _ = std::fs::remove_file(&path);
}

#[test]
fn corrupt_cache_is_ignored_and_state_is_preserved() {
    let controller = DhtController::new(id(0xbb), 0);

    {
        let router = controller.router();
        let mut router = router.try_lock().unwrap();
        router.add_node(DhtNode::new(
            id(0x11),
            SocketAddress::new_inet(Ipv4Addr::new(1, 1, 1, 1), 6881),
        ));
        router.mark_replied(&id(0x11), now_seconds());
    }

    let path = std::env::temp_dir().join(format!(
        "rtorrent_dht_net03_corrupt_{}.bin",
        std::process::id()
    ));
    std::fs::write(&path, b"this is definitely not bencode").unwrap();

    assert!(controller.load_cache_from(&path).is_ok());
    assert_eq!(controller.get_statistics().num_nodes, 1);

    let _ = std::fs::remove_file(&path);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn periodic_refresh_prunes_stale_peers_and_nodes() {
    let mut config = DhtLifecycleConfig::default();
    config.mode = DhtMode::On;
    config.refresh_interval = Duration::from_millis(50);
    config.bootstrap_retry_interval = Duration::from_millis(50);
    config.peer_timeout = 1;
    config.remove_node_timeout = 0;

    let controller = Arc::new(DhtController::with_config(
        id(0xcc),
        free_udp_port(),
        config,
    ));
    controller.start();

    let stale_hash = id(0x44);
    let stale_node_id = id(0x55);
    let now = now_seconds();

    {
        let router = controller.router();
        let mut router = router.lock().await;

        router
            .get_tracker(&stale_hash, true)
            .unwrap()
            .add_peer(0x01020304, 6881, now.saturating_sub(120));

        router.add_node(DhtNode::new(
            stale_node_id,
            SocketAddress::new_inet(Ipv4Addr::new(9, 9, 9, 9), 6881),
        ));
        router.mark_replied(&stale_node_id, now.saturating_sub(120));
        for _ in 0..5 {
            router.mark_inactive(&stale_node_id);
        }
    }

    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let run_controller = Arc::clone(&controller);
    let task = tokio::spawn(async move { run_controller.run(shutdown_rx).await });

    for _ in 0..50 {
        let (nodes, trackers) = {
            let router = controller.router();
            let guard = router.lock().await;
            let stats = guard.get_statistics();
            (stats.num_nodes, stats.num_trackers)
        };
        if nodes == 0 && trackers == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    {
        let router = controller.router();
        let guard = router.lock().await;
        let stats = guard.get_statistics();
        assert_eq!(stats.num_trackers, 0, "stale tracker was not pruned");
        assert_eq!(stats.num_nodes, 0, "stale node was not evicted");
    }

    let _ = shutdown_tx.send(true);
    let _ = task.await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mode_transitions_and_port_restart_are_applied() {
    assert_eq!(DhtMode::parse("disable"), Some(DhtMode::Disable));
    assert_eq!(DhtMode::parse("off"), Some(DhtMode::Off));
    assert_eq!(DhtMode::parse("auto"), Some(DhtMode::Auto));
    assert_eq!(DhtMode::parse("on"), Some(DhtMode::On));
    assert_eq!(DhtMode::parse("bogus"), None);

    let first_port = free_udp_port();
    let second_port = free_udp_port_other_than(first_port);
    let controller = Arc::new(DhtController::new(id(0xdd), first_port));

    assert!(!controller.set_mode(DhtMode::Off));
    assert!(!controller.start());
    assert!(!controller.is_active());
    assert_eq!(controller.port(), 0);

    assert!(controller.set_mode(DhtMode::On));
    assert!(controller.start());
    assert!(controller.is_active());

    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let run_controller = Arc::clone(&controller);
    let task = tokio::spawn(async move { run_controller.run(shutdown_rx).await });

    for _ in 0..50 {
        if controller.port() != 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(controller.port(), first_port);

    controller.restart_with_port(second_port).await.unwrap();
    assert_eq!(controller.port(), second_port);
    assert_eq!(controller.requested_port(), second_port);
    assert!(controller.server().is_active());

    let _ = shutdown_tx.send(true);
    let _ = task.await;

    assert!(!controller.is_active());
    assert_eq!(controller.port(), 0);

    controller.set_mode(DhtMode::Disable);
    assert!(!controller.is_active());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn add_peer_node_contacts_peer_while_bootstrapping() {
    let peer_id = id(0xbb);
    let peer_port = free_udp_port();
    let peer_router = Arc::new(tokio::sync::Mutex::new(lt_dht::router::DhtRouter::new(
        peer_id,
    )));
    let peer_server = Arc::new(lt_dht::server::DhtServer::new(
        Arc::clone(&peer_router),
        peer_port,
    ));
    let (peer_shutdown_tx, peer_shutdown_rx) = tokio::sync::watch::channel(false);
    let peer_task = {
        let server = Arc::clone(&peer_server);
        tokio::spawn(async move { server.run(peer_shutdown_rx).await })
    };

    let controller = Arc::new(DhtController::new(id(0xaa), free_udp_port()));
    controller.start();
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let run_controller = Arc::clone(&controller);
    let task = tokio::spawn(async move { run_controller.run(shutdown_rx).await });

    for _ in 0..50 {
        if controller.server().is_active() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    let peer_address = SocketAddress::new_inet(Ipv4Addr::LOCALHOST, peer_port);
    controller.add_peer_node(&peer_address, peer_port);

    for _ in 0..50 {
        if controller.get_statistics().num_nodes >= 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    let stats = controller.get_statistics();
    assert_eq!(stats.queries_sent, 1, "peer contact was not counted once");
    assert!(stats.num_nodes >= 1, "peer was never contacted: {stats:?}");

    let _ = shutdown_tx.send(true);
    let _ = peer_shutdown_tx.send(true);
    let _ = task.await;
    let _ = peer_task.await;
}

#[test]
fn bootstrap_population_marker_tracks_cached_nodes() {
    let controller = DhtController::new(id(0xee), 0);

    let mut self_id = HashString::default();
    self_id.clear_byte(0x55);
    let mut cache = lt_common::Object::create_map();
    cache
        .insert_key("self_id", lt_common::Object::create_string(self_id.as_slice().to_vec()))
        .unwrap();

    let mut nodes = lt_common::Object::create_map();
    for index in 0..BOOTSTRAP_COMPLETE_NODES {
        let mut node_id = self_id;
        node_id.as_mut_slice()[index / 8] ^= 0x80 >> (index % 8);
        let mut node = lt_common::Object::create_map();
        node.insert_key("i", lt_common::Object::create_value(0x08080808))
            .unwrap();
        node.insert_key("p", lt_common::Object::create_value((10000 + index) as i64))
            .unwrap();
        node.insert_key("t", lt_common::Object::create_value(1)).unwrap();
        nodes.insert_key(node_id.as_slice(), node).unwrap();
    }
    cache.insert_key("nodes", nodes).unwrap();

    controller.initialize(&cache);
    assert!(controller.is_nodes_populated());
}
