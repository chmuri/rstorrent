use std::net::{Ipv4Addr, UdpSocket};
use std::sync::Arc;
use std::time::Duration;

use lt_common::hash_string::HashString;
use lt_dht::node::DhtNode;
use lt_dht::router::DhtRouter;
use lt_dht::search::{announce_peer, get_peers};
use lt_dht::server::DhtServer;
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
async fn lookup_and_announce_over_loopback() {
    let info_hash = id(0x11);
    let id_a = id(0xaa);
    let id_b = id(0xbb);
    let port_b = free_udp_port();

    // Node B answers get_peers and stores announces.
    let router_b = Arc::new(Mutex::new(DhtRouter::new(id_b)));
    {
        let mut router = router_b.lock().await;
        let tracker = router.get_tracker(&info_hash, true).unwrap();
        tracker.add_peer(u32::from(Ipv4Addr::new(127, 0, 0, 9)), 51413, 0);
    }

    let server_b = Arc::new(DhtServer::new(Arc::clone(&router_b), port_b));
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let server_b_task = {
        let server = Arc::clone(&server_b);
        tokio::spawn(async move { server.run(shutdown_rx).await })
    };

    // Node A knows B and can query it.
    let router_a = Arc::new(Mutex::new(DhtRouter::new(id_a)));
    {
        let mut router = router_a.lock().await;
        router.add_node(DhtNode::new(
            id_b,
            SocketAddress::new_inet(Ipv4Addr::LOCALHOST, port_b),
        ));
    }

    let server_a = Arc::new(DhtServer::new(Arc::clone(&router_a), free_udp_port()));
    let (shutdown_a_tx, shutdown_a_rx) = tokio::sync::watch::channel(false);
    let server_a_task = {
        let server = Arc::clone(&server_a);
        tokio::spawn(async move { server.run(shutdown_a_rx).await })
    };

    // Wait for B's receive loop to be ready, then look up the seeded peer.
    let mut peers = Vec::new();
    for _ in 0..20 {
        peers = get_peers(
            Arc::clone(&server_a),
            Arc::clone(&router_a),
            info_hash,
            8,
        )
        .await
        .unwrap();

        if !peers.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    assert!(
        peers.iter().any(|peer| {
            peer.as_socket_addr() == Some("127.0.0.9:51413".parse().unwrap())
        }),
        "expected seeded peer, got {peers:?}"
    );

    // Announce A's listen port; B must record the source address.
    let announced = announce_peer(
        Arc::clone(&server_a),
        Arc::clone(&router_a),
        info_hash,
        40000,
        8,
    )
    .await
    .unwrap();
    assert!(announced >= 1, "no successful announce");

    let stored = {
        let mut router = router_b.lock().await;
        router
            .get_tracker(&info_hash, false)
            .map(|tracker| {
                tracker.get_peers(32).iter().any(|peer| {
                    let ip = Ipv4Addr::new(peer[0], peer[1], peer[2], peer[3]);
                    let port = u16::from_be_bytes([peer[4], peer[5]]);
                    ip == Ipv4Addr::LOCALHOST && port == 40000
                })
            })
            .unwrap_or(false)
    };
    assert!(stored, "announce was not stored by node B");

    let _ = shutdown_tx.send(true);
    let _ = shutdown_a_tx.send(true);
    let _ = server_a_task.await;
    let _ = server_b_task.await;
}
