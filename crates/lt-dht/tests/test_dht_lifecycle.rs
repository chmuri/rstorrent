use std::net::{Ipv4Addr, UdpSocket};
use std::sync::Arc;
use std::time::Duration;

use lt_common::hash_string::HashString;
use lt_dht::controller::DhtController;
use lt_dht::node::DhtNode;
use lt_dht::router::DhtRouter;
use lt_net::addr::SocketAddress;
use tokio::sync::Mutex;

fn free_udp_port() -> u16 {
    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    socket.local_addr().unwrap().port()
}

fn id(byte: u8) -> HashString {
    HashString::new([byte; 20])
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn bootstrap_adds_responding_node() {
    let id_b = id(0xbb);
    let port_b = free_udp_port();

    let router_b = Arc::new(Mutex::new(DhtRouter::new(id_b)));
    let server_b = Arc::new(lt_dht::server::DhtServer::new(
        Arc::clone(&router_b),
        port_b,
    ));
    let (shutdown_b_tx, shutdown_b_rx) = tokio::sync::watch::channel(false);
    let server_b_task = {
        let server = Arc::clone(&server_b);
        tokio::spawn(async move { server.run(shutdown_b_rx).await })
    };

    let id_a = id(0xaa);
    let controller_a = DhtController::new(id_a, free_udp_port());
    controller_a.start();
    let (shutdown_a_tx, shutdown_a_rx) = tokio::sync::watch::channel(false);
    let server_a_task = {
        let server = controller_a.server();
        tokio::spawn(async move { server.run(shutdown_a_rx).await })
    };

    let bootstrap_address = SocketAddress::new_inet(Ipv4Addr::LOCALHOST, port_b);

    let mut replied = 0;
    for _ in 0..20 {
        replied = controller_a
            .bootstrap(&[bootstrap_address.clone()])
            .await
            .unwrap();
        if replied > 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(replied > 0, "bootstrap node did not answer");

    let stats = controller_a.get_statistics();
    assert!(stats.num_nodes >= 1, "bootstrap node not added: {stats:?}");

    let _ = shutdown_a_tx.send(true);
    let _ = shutdown_b_tx.send(true);
    let _ = server_a_task.await;
    let _ = server_b_task.await;
}

#[tokio::test]
async fn cache_round_trip_preserves_self_id_and_nodes() {
    let id_a = id(0xaa);
    let controller = DhtController::new(id_a, 0);

    {
        let router = controller.router();
        let mut router = router.lock().await;
        router.add_node(DhtNode::new(
            id(0x01),
            SocketAddress::new_inet(Ipv4Addr::new(8, 8, 8, 8), 6881),
        ));
        router.add_node(DhtNode::new(
            id(0x02),
            SocketAddress::new_inet(Ipv4Addr::new(8, 8, 4, 4), 6882),
        ));
        // Mark the nodes active so the round-trip reflects a live table.
        router.mark_replied(&id(0x01), 1);
        router.mark_replied(&id(0x02), 1);
    }

    let cache = controller.save_cache();
    assert!(cache.get_key("self_id").is_ok());

    let restored = DhtController::new(HashString::default(), 0);
    restored.initialize(&cache);

    assert_eq!(restored.get_statistics().num_nodes, 2);
    let router = restored.router();
    assert_eq!(router.lock().await.own_id, id_a);

    // File round-trip.
    let path = std::env::temp_dir().join(format!("rtorrent_dht_cache_{}.bin", std::process::id()));
    let _ = std::fs::remove_file(&path);
    controller.save_cache_to(&path).unwrap();

    let loaded = DhtController::new(HashString::default(), 0);
    loaded.load_cache_from(&path).unwrap();
    assert_eq!(loaded.get_statistics().num_nodes, 2);

    let _ = std::fs::remove_file(&path);
}

#[test]
fn cache_is_bencode_encodable() {
    let controller = DhtController::new(id(0xcc), 0);
    let cache = controller.save_cache();
    let encoded = lt_common::object::bencode::encode(&cache).unwrap();
    let decoded = lt_common::object::bencode::decode(&encoded).unwrap();
    assert!(decoded.get_key("self_id").is_ok());
}
