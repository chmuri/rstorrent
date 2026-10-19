use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use lt_common::{HashString, Object};
use lt_dht::controller::{DhtLifecycleConfig, DhtMode};
use lt_net::config::NetworkConfig;
use rt_core::commands::dht::register_dht_commands;
use rt_core::commands::tracker::DhtConfig;
use rt_core::download_list::DownloadList;
use rt_core::network_runtime::{dht_controller, NetworkRuntime, ShutdownTimeouts};
use rt_rpc::{CommandMap, Target};

fn free_udp_port() -> u16 {
    let socket = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    socket.local_addr().unwrap().port()
}

fn call(map: &CommandMap, name: &str, args: Vec<Object>) -> Result<Object, rt_rpc::RpcError> {
    map.call_command(name, &Object::create_list_with(args), &Target::None, true)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn runtime_publishes_dht_controller_to_commands() {
    let port = free_udp_port();
    let list = Arc::new(RwLock::new(DownloadList::new()));
    let peer_id = HashString::new([0xef; 20]);
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);

    let mut dht_config = DhtLifecycleConfig::default();
    dht_config.mode = DhtMode::On;
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

    let deadline = Instant::now() + Duration::from_secs(4);
    while dht_controller().is_none() {
        assert!(Instant::now() < deadline, "DHT controller was not published");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    let config = Arc::new(RwLock::new(DhtConfig::new()));
    let mut map = CommandMap::new();
    register_dht_commands(&mut map, Arc::clone(&config)).unwrap();

    let deadline = Instant::now() + Duration::from_secs(4);
    loop {
        let stats = call(&map, "dht.statistics", Vec::new()).unwrap();
        let active = stats.get_key("active").unwrap().as_value().unwrap();
        if active != 0 {
            assert!(stats.get_key("nodes").is_ok());
            assert!(stats.get_key("buckets").is_ok());
            break;
        }
        assert!(Instant::now() < deadline, "DHT never became active");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    let reported = call(&map, "dht.port", Vec::new())
        .unwrap()
        .as_value()
        .unwrap();
    assert_eq!(reported, port as i64);
    assert_eq!(
        call(&map, "dht.is_active", Vec::new())
            .unwrap()
            .as_value()
            .unwrap(),
        1
    );
    assert!(call(
        &map,
        "dht.add_node",
        vec![Object::from("127.0.0.1:6881")]
    )
    .is_ok());
    assert!(call(&map, "dht.refresh", Vec::new()).is_ok());

    let _ = shutdown_tx.send(true);
    let report = runtime.drain(ShutdownTimeouts::default()).await;
    assert!(report.all_completed(), "runtime did not drain: {report:?}");
}
