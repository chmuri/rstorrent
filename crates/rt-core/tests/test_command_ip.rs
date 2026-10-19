use std::sync::{Arc, RwLock};
use lt_common::Object;
use rt_core::commands::ip::{ipv4_range_parse, register_ip_commands, IpTableManager};
use rt_rpc::{CommandMap, Target};

fn ipv4(a: u32, b: u32, c: u32, d: u32) -> u32 {
    (a << 24) | (b << 16) | (c << 8) | d
}

#[test]
fn test_single_address() {
    let (start, end) = ipv4_range_parse("10.1.2.3").unwrap();
    assert_eq!(ipv4(10, 1, 2, 3), start);
    assert_eq!(ipv4(10, 1, 2, 3), end);
}

#[test]
fn test_explicit_range() {
    let (start, end) = ipv4_range_parse("10.1.2.3-10.1.2.9").unwrap();
    assert_eq!(ipv4(10, 1, 2, 3), start);
    assert_eq!(ipv4(10, 1, 2, 9), end);
}

#[test]
fn test_cidr() {
    let (s1, e1) = ipv4_range_parse("10.0.0.0/8").unwrap();
    assert_eq!(ipv4(10, 0, 0, 0), s1);
    assert_eq!(ipv4(10, 255, 255, 255), e1);

    let (s2, e2) = ipv4_range_parse("10.1.2.0/24").unwrap();
    assert_eq!(ipv4(10, 1, 2, 0), s2);
    assert_eq!(ipv4(10, 1, 2, 255), e2);

    let (s3, e3) = ipv4_range_parse("10.1.2.128/25").unwrap();
    assert_eq!(ipv4(10, 1, 2, 128), s3);
    assert_eq!(ipv4(10, 1, 2, 255), e3);

    let (s4, e4) = ipv4_range_parse("10.1.2.3/31").unwrap();
    assert_eq!(ipv4(10, 1, 2, 2), s4);
    assert_eq!(ipv4(10, 1, 2, 3), e4);
}

#[test]
fn test_cidr_zero_mask() {
    let (s1, e1) = ipv4_range_parse("0.0.0.0/0").unwrap();
    assert_eq!(ipv4(0, 0, 0, 0), s1);
    assert_eq!(ipv4(255, 255, 255, 255), e1);

    let (s2, e2) = ipv4_range_parse("10.1.2.3/0").unwrap();
    assert_eq!(ipv4(0, 0, 0, 0), s2);
    assert_eq!(ipv4(255, 255, 255, 255), e2);
}

#[test]
fn test_cidr_full_mask() {
    let (s1, e1) = ipv4_range_parse("10.1.2.3/32").unwrap();
    assert_eq!(ipv4(10, 1, 2, 3), s1);
    assert_eq!(ipv4(10, 1, 2, 3), e1);

    let (s2, e2) = ipv4_range_parse("0.0.0.0/32").unwrap();
    assert_eq!(ipv4(0, 0, 0, 0), s2);
    assert_eq!(ipv4(0, 0, 0, 0), e2);

    let (s3, e3) = ipv4_range_parse("255.255.255.255/32").unwrap();
    assert_eq!(ipv4(255, 255, 255, 255), s3);
    assert_eq!(ipv4(255, 255, 255, 255), e3);
}

#[test]
fn test_ip_tables_commands() {
    let mut map = CommandMap::new();
    let mgr = Arc::new(RwLock::new(IpTableManager::new()));
    register_ip_commands(&mut map, Arc::clone(&mgr)).unwrap();

    // Create table
    map.call_command(
        "ip_tables.insert_table",
        &Object::from("filter"),
        &Target::None,
        true,
    )
    .unwrap();

    // Add address
    map.call_command(
        "ip_tables.add_address",
        &Object::create_list_with(vec![
            Object::from("filter"),
            Object::from("192.168.1.0/24"),
            Object::from(1i64),
        ]),
        &Target::None,
        true,
    )
    .unwrap();

    // Size data
    let size = map
        .call_command(
            "ip_tables.size_data",
            &Object::from("filter"),
            &Target::None,
            true,
        )
        .unwrap()
        .as_value()
        .unwrap();
    assert_eq!(1, size);

    // Get
    let val = map
        .call_command(
            "ip_tables.get",
            &Object::create_list_with(vec![
                Object::from("filter"),
                Object::from("192.168.1.50"),
            ]),
            &Target::None,
            true,
        )
        .unwrap()
        .as_value()
        .unwrap();
    assert_eq!(1, val);

    // Undefined address
    assert!(map
        .call_command(
            "ip_tables.get",
            &Object::create_list_with(vec![
                Object::from("filter"),
                Object::from("10.0.0.1"),
            ]),
            &Target::None,
            true,
        )
        .is_err());
}
