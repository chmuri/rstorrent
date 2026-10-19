use std::net::SocketAddr;

use lt_net::addr::SocketAddress;
use lt_protocol::extensions::{ut_pex_payload, UtPexMessage};

#[test]
fn pex_round_trip_ipv4_and_ipv6() {
    let added = vec![
        SocketAddress::from("127.0.0.1:6881".parse::<SocketAddr>().unwrap()),
        SocketAddress::from("[::1]:6882".parse::<SocketAddr>().unwrap()),
    ];
    let dropped = vec![SocketAddress::from(
        "10.0.0.1:51413".parse::<SocketAddr>().unwrap(),
    )];

    let payload = ut_pex_payload(&added, &dropped);
    let parsed = UtPexMessage::parse(&payload).unwrap();

    assert_eq!(parsed.added.len(), 1);
    assert_eq!(parsed.added6.len(), 1);
    assert_eq!(parsed.dropped.len(), 1);
    assert_eq!(parsed.dropped6.len(), 0);

    let combined: Vec<_> = parsed.all_added().cloned().collect();
    assert_eq!(combined.len(), 2);
    assert!(combined.contains(&added[0]));
    assert!(combined.contains(&added[1]));
}

#[test]
fn malformed_compact_lists_are_ignored() {
    // 5 bytes is not a valid IPv4 compact entry.
    let mut map = lt_common::Object::create_map();
    map.insert_key("added", lt_common::Object::create_string(vec![1, 2, 3, 4, 5]))
        .unwrap();
    let payload = lt_common::object::bencode::encode(&map).unwrap();

    let parsed = UtPexMessage::parse(&payload).unwrap();
    assert!(parsed.added.is_empty());

    // A truncated list with one full entry plus one trailing byte must be
    // ignored entirely, not silently accepted as a single peer.
    let mut map = lt_common::Object::create_map();
    map.insert_key(
        "added",
        lt_common::Object::create_string(vec![127, 0, 0, 1, 0x1a, 0xe1, 0xff]),
    )
    .unwrap();
    let payload = lt_common::object::bencode::encode(&map).unwrap();
    let parsed = UtPexMessage::parse(&payload).unwrap();
    assert!(parsed.added.is_empty());
}

#[test]
fn generated_pex_keeps_ipv6_in_its_own_list() {
    let added = vec![
        SocketAddress::from("127.0.0.1:1".parse::<SocketAddr>().unwrap()),
        SocketAddress::from("[::1]:2".parse::<SocketAddr>().unwrap()),
    ];
    let bytes = lt_protocol::extensions::ProtocolExtension::generate_ut_pex_message(
        3,
        &added,
        &[],
    );
    assert_eq!(bytes[4], 20);
    assert_eq!(bytes[5], 3);

    let payload = &bytes[6..];
    let parsed = UtPexMessage::parse(payload).unwrap();
    assert_eq!(parsed.added.len(), 1);
    assert_eq!(parsed.added6.len(), 1);
}
