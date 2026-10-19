use lt_common::hash_string::HashString;
use lt_common::object::Object;
use lt_dht::bucket::{DhtBucket, NUM_NODES};
use lt_dht::node::DhtNode;

fn make_cached_node(index: usize, last_seen: u32) -> DhtNode {
    let mut cache = Object::create_map();
    cache.insert_key("i", Object::create_value(0x01020300 + index as i64)).unwrap();
    cache.insert_key("p", Object::create_value(6881)).unwrap();
    cache.insert_key("t", Object::create_value(last_seen as i64)).unwrap();

    let mut id = HashString::default();
    id.clear_byte(0x11);
    id.as_mut_slice()[0] = (0x20 + index) as u8;

    DhtNode::from_cache(id, &cache).unwrap()
}

fn make_bucket() -> DhtBucket {
    let mut begin = HashString::default();
    let mut end = HashString::default();
    begin.clear();
    end.clear_byte(0xFF);

    DhtBucket::new(begin, end)
}

#[test]
fn test_empty_bucket_has_no_candidate() {
    let bucket = make_bucket();
    assert_eq!(bucket.find_replacement_candidate(false), None);
}

#[test]
fn test_oldest_node_is_the_candidate() {
    let mut bucket = make_bucket();

    for i in 0..NUM_NODES {
        let last_seen = 500 + if i == 3 { 0 } else { 100 + i as u32 };
        bucket.add_node(make_cached_node(i, last_seen));
    }

    let candidate = bucket.find_replacement_candidate(false);
    assert!(candidate.is_some());
    let idx = candidate.unwrap();
    assert_eq!(bucket.nodes[idx].last_seen, 500);
}

#[test]
fn test_full_bucket_of_max_last_seen_nodes_has_a_candidate() {
    let mut bucket = make_bucket();

    for i in 0..NUM_NODES {
        bucket.add_node(make_cached_node(i, u32::MAX));
    }

    assert!(bucket.is_full());
    assert!(bucket.find_replacement_candidate(false).is_some());
}

#[test]
fn test_bucket_split_and_midpoint() {
    let mut bucket = make_bucket();
    let mid = bucket.get_mid_point();
    assert_eq!(mid.as_slice()[0], 0x7F);

    // Add nodes in lower and upper halves
    let node_lower = make_cached_node(0, 100); // 0x20...
    let mut node_upper = make_cached_node(1, 200);
    node_upper.id.as_mut_slice()[0] = 0x90; // 0x90...

    bucket.add_node(node_lower);
    bucket.add_node(node_upper);

    let other = bucket.split(&HashString::default());
    assert_eq!(other.size(), 1); // 0x20 is in other [0..7F]
    assert_eq!(bucket.size(), 1); // 0x90 is in bucket [80..FF]
}
