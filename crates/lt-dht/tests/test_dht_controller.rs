use std::str::FromStr;

use lt_common::hash_string::HashString;
use lt_common::object::Object;
use lt_dht::controller::{DhtController, BOOTSTRAP_COMPLETE_NODES};
use lt_dht::transaction::{KrpcMessage, KrpcQuery, KrpcResponse};
use lt_net::addr::SocketAddress;

const DHT_PORT: u16 = 43821;

fn create_dht_cache(node_count: usize) -> Object {
    let mut self_id = HashString::default();
    self_id.clear_byte(0x55);

    let mut cache = Object::create_map();
    cache.insert_key("self_id", Object::create_string(self_id.as_slice().to_vec())).unwrap();

    let mut nodes = Object::create_map();

    for i in 0..node_count {
        let mut node_id = self_id;
        node_id.as_mut_slice()[i / 8] ^= 0x80 >> (i % 8);

        let mut node = Object::create_map();
        node.insert_key("i", Object::create_value(0x7f000001)).unwrap();
        node.insert_key("p", Object::create_value((10000 + i) as i64)).unwrap();
        node.insert_key("t", Object::create_value(0)).unwrap();

        nodes.insert_key(node_id.as_slice(), node).unwrap();
    }

    cache.insert_key("nodes", nodes).unwrap();
    cache
}

#[test]
fn test_add_peer_node_while_bootstrapping() {
    let dht = DhtController::new(HashString::default(), DHT_PORT);

    assert!(!dht.is_nodes_populated());

    dht.initialize(&create_dht_cache(0));

    assert!(dht.start());
    assert_eq!(dht.get_statistics().num_nodes, 0);
    assert!(!dht.is_nodes_populated());

    let initial_queries = dht.get_statistics().queries_sent;
    let sa = SocketAddress::from_str("127.0.0.2:6881").unwrap();
    dht.add_peer_node(&sa, 6881);

    assert_eq!(dht.get_statistics().queries_sent - initial_queries, 1);
}

#[test]
fn test_add_peer_node_after_bootstrap() {
    let dht = DhtController::new(HashString::default(), DHT_PORT);

    dht.initialize(&create_dht_cache(BOOTSTRAP_COMPLETE_NODES));

    assert!(dht.start());
    assert_eq!(dht.get_statistics().num_nodes, BOOTSTRAP_COMPLETE_NODES);
    assert!(dht.is_nodes_populated());

    let initial_queries = dht.get_statistics().queries_sent;
    let sa = SocketAddress::from_str("127.0.0.2:6881").unwrap();
    dht.add_peer_node(&sa, 6881);

    assert_eq!(dht.get_statistics().queries_sent - initial_queries, 0);
}

#[test]
fn test_krpc_encoding_decoding() {
    let mut id = HashString::default();
    id.clear_byte(0xaa);

    let mut target = HashString::default();
    target.clear_byte(0xbb);

    // 1. Query: FindNode
    let q = KrpcMessage::Query(KrpcQuery::FindNode { id, target });
    let bytes = q.encode(b"aa").unwrap();

    let (tid, decoded) = KrpcMessage::decode(&bytes).unwrap();
    assert_eq!(&tid[..], b"aa");
    assert_eq!(decoded, q);

    // 2. Response: Ping
    let r = KrpcMessage::Response(KrpcResponse::Ping { id });
    let bytes = r.encode(b"bb").unwrap();

    let (tid, decoded) = KrpcMessage::decode(&bytes).unwrap();
    assert_eq!(&tid[..], b"bb");
    assert_eq!(decoded, r);
}
