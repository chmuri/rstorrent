use std::net::{Ipv4Addr, Ipv6Addr};
use std::str::FromStr;

use lt_net::addr::{SocketAddress, SocketAddressKey};
use lt_net::config::{EncryptionMode, NetworkConfig};
use lt_net::socket_manager::{SocketCategory, SocketManager};

struct DefaultSa {
    sin_any: SocketAddress,
    sin_any_5000: SocketAddress,
    sin_bc: SocketAddress,
    sin_bc_5000: SocketAddress,
    sin_1: SocketAddress,
    sin_1_5000: SocketAddress,
    sin_1_5100: SocketAddress,
    sin_2: SocketAddress,
    sin_2_5000: SocketAddress,
    sin_2_5100: SocketAddress,

    sin6_any: SocketAddress,
    sin6_any_5000: SocketAddress,
    sin6_1: SocketAddress,
    sin6_1_5000: SocketAddress,
    sin6_1_5100: SocketAddress,
    sin6_2: SocketAddress,
    sin6_2_5000: SocketAddress,
    sin6_2_5100: SocketAddress,

    sin6_v4_any: SocketAddress,
    sin6_v4_any_5000: SocketAddress,
    sin6_v4_bc: SocketAddress,
    sin6_v4_bc_5000: SocketAddress,
    sin6_v4_1: SocketAddress,
    #[allow(dead_code)]
    sin6_v4_1_5000: SocketAddress,
}

impl DefaultSa {
    fn new() -> Self {
        Self {
            sin_any: SocketAddress::from_str("0.0.0.0").unwrap(),
            sin_any_5000: SocketAddress::from_str("0.0.0.0:5000").unwrap(),
            sin_bc: SocketAddress::from_str("255.255.255.255").unwrap(),
            sin_bc_5000: SocketAddress::from_str("255.255.255.255:5000").unwrap(),
            sin_1: SocketAddress::from_str("1.2.3.4").unwrap(),
            sin_1_5000: SocketAddress::from_str("1.2.3.4:5000").unwrap(),
            sin_1_5100: SocketAddress::from_str("1.2.3.4:5100").unwrap(),
            sin_2: SocketAddress::from_str("4.3.2.1").unwrap(),
            sin_2_5000: SocketAddress::from_str("4.3.2.1:5000").unwrap(),
            sin_2_5100: SocketAddress::from_str("4.3.2.1:5100").unwrap(),

            sin6_any: SocketAddress::from_str("::").unwrap(),
            sin6_any_5000: SocketAddress::from_str("[::]:5000").unwrap(),
            sin6_1: SocketAddress::from_str("ff01::1").unwrap(),
            sin6_1_5000: SocketAddress::from_str("[ff01::1]:5000").unwrap(),
            sin6_1_5100: SocketAddress::from_str("[ff01::1]:5100").unwrap(),
            sin6_2: SocketAddress::from_str("ff02::2").unwrap(),
            sin6_2_5000: SocketAddress::from_str("[ff02::2]:5000").unwrap(),
            sin6_2_5100: SocketAddress::from_str("[ff02::2]:5100").unwrap(),

            sin6_v4_any: SocketAddress::from_str("::ffff:0.0.0.0").unwrap(),
            sin6_v4_any_5000: SocketAddress::from_str("[::ffff:0.0.0.0]:5000").unwrap(),
            sin6_v4_bc: SocketAddress::from_str("::ffff:255.255.255.255").unwrap(),
            sin6_v4_bc_5000: SocketAddress::from_str("[::ffff:255.255.255.255]:5000").unwrap(),
            sin6_v4_1: SocketAddress::from_str("::ffff:1.2.3.4").unwrap(),
            sin6_v4_1_5000: SocketAddress::from_str("[::ffff:1.2.3.4]:5000").unwrap(),
        }
    }
}

#[test]
fn test_sa_is_any() {
    let sa = DefaultSa::new();

    assert!(sa.sin_any.is_any());
    assert!(sa.sin_any_5000.is_any());
    assert!(sa.sin6_v4_any.is_any());
    assert!(sa.sin6_v4_any_5000.is_any());

    assert!(!sa.sin_bc.is_any());
    assert!(!sa.sin_1.is_any());
    assert!(!sa.sin6_1.is_any());
    assert!(!sa.sin_bc_5000.is_any());
    assert!(!sa.sin_1_5000.is_any());
    assert!(!sa.sin6_1_5000.is_any());
}

#[test]
fn test_sa_is_broadcast() {
    let sa = DefaultSa::new();

    assert!(sa.sin_bc.is_broadcast());
    assert!(sa.sin_bc_5000.is_broadcast());
    assert!(sa.sin6_v4_bc.is_broadcast());
    assert!(sa.sin6_v4_bc_5000.is_broadcast());

    assert!(!sa.sin_any.is_broadcast());
    assert!(!sa.sin_1.is_broadcast());
    assert!(!sa.sin6_any.is_broadcast());
    assert!(!sa.sin6_1.is_broadcast());
    assert!(!sa.sin_any_5000.is_broadcast());
    assert!(!sa.sin_1_5000.is_broadcast());
    assert!(!sa.sin6_any_5000.is_broadcast());
    assert!(!sa.sin6_1_5000.is_broadcast());
}

#[test]
fn test_sa_is_link_local() {
    let sin_ll = SocketAddress::from_str("169.254.169.254").unwrap();
    let sin_ll_first = SocketAddress::from_str("169.254.0.0").unwrap();
    let sin_ll_last = SocketAddress::from_str("169.254.255.255").unwrap();
    let sin_below = SocketAddress::from_str("169.253.255.255").unwrap();
    let sin_above = SocketAddress::from_str("169.255.0.0").unwrap();
    let sin_routable = SocketAddress::from_str("1.2.3.4").unwrap();

    let sin6_v4_ll = SocketAddress::from_str("::ffff:169.254.169.254").unwrap();
    let sin6_v4_above = SocketAddress::from_str("::ffff:169.255.0.0").unwrap();
    let sin6_v4_routable = SocketAddress::from_str("::ffff:1.2.3.4").unwrap();

    let sin6_ll = SocketAddress::from_str("fe80::1").unwrap();
    let sin6_mc_ll = SocketAddress::from_str("ff02::1").unwrap();
    let sin6_mc_if = SocketAddress::from_str("ff01::1").unwrap();
    let sin6_routable = SocketAddress::from_str("2001:4860:4860::8888").unwrap();

    assert!(sin_ll.is_link_local());
    assert!(sin_ll_first.is_link_local());
    assert!(sin_ll_last.is_link_local());
    assert!(sin6_ll.is_link_local());
    assert!(sin6_mc_ll.is_link_local());
    assert!(sin6_v4_ll.is_link_local());

    assert!(!sin_below.is_link_local());
    assert!(!sin_above.is_link_local());
    assert!(!sin_routable.is_link_local());
    assert!(!sin6_v4_above.is_link_local());
    assert!(!sin6_v4_routable.is_link_local());
    assert!(!sin6_mc_if.is_link_local());
    assert!(!sin6_routable.is_link_local());
}

#[test]
fn test_sa_is_loopback() {
    let sin_lo = SocketAddress::from_str("127.0.0.1").unwrap();
    let sin_lo_high = SocketAddress::from_str("127.255.255.255").unwrap();
    let sin_routable = SocketAddress::from_str("1.2.3.4").unwrap();

    let sin6_v4_lo = SocketAddress::from_str("::ffff:127.0.0.1").unwrap();
    let sin6_v4_routable = SocketAddress::from_str("::ffff:1.2.3.4").unwrap();

    let sin6_lo = SocketAddress::from_str("::1").unwrap();
    let sin6_routable = SocketAddress::from_str("2001:4860:4860::8888").unwrap();

    assert!(sin_lo.is_loopback());
    assert!(sin_lo_high.is_loopback());
    assert!(sin6_lo.is_loopback());
    assert!(sin6_v4_lo.is_loopback());

    assert!(!sin_routable.is_loopback());
    assert!(!sin6_v4_routable.is_loopback());
    assert!(!sin6_routable.is_loopback());
}

#[test]
fn test_make() {
    let sa_unspec = SocketAddress::new_unspec();
    assert!(sa_unspec.is_unspec());

    let sa_inet = SocketAddress::new_inet(Ipv4Addr::UNSPECIFIED, 0);
    assert!(sa_inet.is_inet());
    assert_eq!(sa_inet.port(), 0);

    let sa_inet6 = SocketAddress::new_inet6(Ipv6Addr::UNSPECIFIED, 0);
    assert!(sa_inet6.is_inet6());
    assert_eq!(sa_inet6.port(), 0);

    let sa_unix = SocketAddress::new_unix("/tmp/test.sock");
    assert!(sa_unix.is_unix());
}

#[test]
fn test_sa_equal() {
    let sa = DefaultSa::new();

    assert!(SocketAddress::new_unspec()
        .equal(&SocketAddress::new_unspec())
        .unwrap());
    assert!(SocketAddress::new_inet_any()
        .equal(&SocketAddress::new_inet_any())
        .unwrap());
    assert!(SocketAddress::new_inet6_any()
        .equal(&SocketAddress::new_inet6_any())
        .unwrap());

    assert!(!SocketAddress::new_unspec()
        .equal(&SocketAddress::new_inet_any())
        .unwrap());
    assert!(!SocketAddress::new_unspec()
        .equal(&SocketAddress::new_inet6_any())
        .unwrap());
    assert!(!SocketAddress::new_inet_any()
        .equal(&SocketAddress::new_inet6_any())
        .unwrap());
    assert!(!SocketAddress::new_inet6_any()
        .equal(&SocketAddress::new_inet_any())
        .unwrap());

    assert!(sa.sin_1.equal(&sa.sin_1).unwrap());
    assert!(!sa.sin_1.equal(&sa.sin_2).unwrap());

    assert!(sa.sin6_1.equal(&sa.sin6_1).unwrap());
    assert!(!sa.sin6_1.equal(&sa.sin6_2).unwrap());

    assert!(sa.sin_1_5000.equal(&sa.sin_1_5000).unwrap());
    assert!(sa.sin6_1_5000.equal(&sa.sin6_1_5000).unwrap());
    assert!(!sa.sin_1_5000.equal(&sa.sin_1_5100).unwrap());
    assert!(!sa.sin6_1_5000.equal(&sa.sin6_1_5100).unwrap());
    assert!(!sa.sin_1_5000.equal(&sa.sin_2_5000).unwrap());
    assert!(!sa.sin6_1_5000.equal(&sa.sin6_2_5000).unwrap());
    assert!(!sa.sin_1_5000.equal(&sa.sin_2_5100).unwrap());
    assert!(!sa.sin6_1_5000.equal(&sa.sin6_2_5100).unwrap());

    let sa_unix = SocketAddress::new_unix("");
    assert!(sa_unix.equal(&sa_unix).is_err());
    assert!(sa_unix.equal(&sa.sin6_1).is_err());
    assert!(sa.sin6_1.equal(&sa_unix).is_err());
}

#[test]
fn test_sa_equal_addr() {
    let sa = DefaultSa::new();

    assert!(SocketAddress::new_unspec()
        .equal_addr(&SocketAddress::new_unspec())
        .unwrap());
    assert!(SocketAddress::new_inet_any()
        .equal_addr(&SocketAddress::new_inet_any())
        .unwrap());
    assert!(SocketAddress::new_inet6_any()
        .equal_addr(&SocketAddress::new_inet6_any())
        .unwrap());

    assert!(!SocketAddress::new_unspec()
        .equal_addr(&SocketAddress::new_inet_any())
        .unwrap());
    assert!(!SocketAddress::new_unspec()
        .equal_addr(&SocketAddress::new_inet6_any())
        .unwrap());
    assert!(!SocketAddress::new_inet_any()
        .equal_addr(&SocketAddress::new_inet6_any())
        .unwrap());
    assert!(!SocketAddress::new_inet6_any()
        .equal_addr(&SocketAddress::new_inet_any())
        .unwrap());

    assert!(sa.sin_1.equal_addr(&sa.sin_1).unwrap());
    assert!(!sa.sin_1.equal_addr(&sa.sin_2).unwrap());

    assert!(sa.sin6_1.equal_addr(&sa.sin6_1).unwrap());
    assert!(!sa.sin6_1.equal_addr(&sa.sin6_2).unwrap());

    assert!(sa.sin_1_5000.equal_addr(&sa.sin_1_5000).unwrap());
    assert!(sa.sin6_1_5000.equal_addr(&sa.sin6_1_5000).unwrap());
    assert!(sa.sin_1_5000.equal_addr(&sa.sin_1_5100).unwrap());
    assert!(sa.sin6_1_5000.equal_addr(&sa.sin6_1_5100).unwrap());
    assert!(!sa.sin_1_5000.equal_addr(&sa.sin_2_5000).unwrap());
    assert!(!sa.sin6_1_5000.equal_addr(&sa.sin_2_5000).unwrap());
    assert!(!sa.sin_1_5000.equal_addr(&sa.sin_2_5100).unwrap());
    assert!(!sa.sin6_1_5000.equal_addr(&sa.sin_2_5100).unwrap());

    let sa_unix = SocketAddress::new_unix("");
    assert!(sa_unix.equal_addr(&sa_unix).is_err());
    assert!(sa_unix.equal_addr(&sa.sin6_1).is_err());
    assert!(sa.sin6_1.equal_addr(&sa_unix).is_err());
}

#[test]
fn test_sa_copy_addr() {
    let sa = DefaultSa::new();

    assert!(SocketAddress::new_unspec()
        .copy_addr(0)
        .unwrap()
        .equal(&SocketAddress::new_unspec())
        .unwrap());
    assert!(SocketAddress::new_inet_any()
        .copy_addr(0)
        .unwrap()
        .equal(&sa.sin_any)
        .unwrap());
    assert!(SocketAddress::new_inet6_any()
        .copy_addr(0)
        .unwrap()
        .equal(&sa.sin6_any)
        .unwrap());
    assert!(SocketAddress::new_unspec()
        .copy_addr(5000)
        .unwrap()
        .equal(&SocketAddress::new_unspec())
        .unwrap());
    assert!(SocketAddress::new_inet_any()
        .copy_addr(5000)
        .unwrap()
        .equal(&sa.sin_any_5000)
        .unwrap());
    assert!(SocketAddress::new_inet6_any()
        .copy_addr(5000)
        .unwrap()
        .equal(&sa.sin6_any_5000)
        .unwrap());

    assert!(sa.sin_1.copy_addr(0).unwrap().equal(&sa.sin_1).unwrap());
    assert!(sa
        .sin_1
        .copy_addr(5000)
        .unwrap()
        .equal(&sa.sin_1_5000)
        .unwrap());
    assert!(sa.sin6_1.copy_addr(0).unwrap().equal(&sa.sin6_1).unwrap());
    assert!(sa
        .sin6_1
        .copy_addr(5000)
        .unwrap()
        .equal(&sa.sin6_1_5000)
        .unwrap());

    assert!(sa
        .sin_1_5000
        .copy_addr(0)
        .unwrap()
        .equal(&sa.sin_1)
        .unwrap());
    assert!(sa
        .sin6_1_5000
        .copy_addr(0)
        .unwrap()
        .equal(&sa.sin6_1)
        .unwrap());
    assert!(sa
        .sin_1_5000
        .copy_addr(5100)
        .unwrap()
        .equal(&sa.sin_1_5100)
        .unwrap());
    assert!(sa
        .sin6_1_5000
        .copy_addr(5100)
        .unwrap()
        .equal(&sa.sin6_1_5100)
        .unwrap());
}

#[test]
fn test_sa_from_v4mapped() {
    let sa = DefaultSa::new();

    assert!(sa
        .sin6_v4_any
        .from_v4mapped()
        .unwrap()
        .equal_addr(&sa.sin_any)
        .unwrap());
    assert!(sa.sin6_v4_any.from_v4mapped().unwrap().is_port_any());

    assert!(sa
        .sin6_v4_1
        .from_v4mapped()
        .unwrap()
        .equal_addr(&sa.sin_1)
        .unwrap());
    assert!(sa.sin6_v4_1.from_v4mapped().unwrap().is_port_any());

    assert!(sa
        .sin6_v4_bc
        .from_v4mapped()
        .unwrap()
        .equal_addr(&sa.sin_bc)
        .unwrap());
    assert!(sa.sin6_v4_bc.from_v4mapped().unwrap().is_port_any());

    assert!(SocketAddress::new_unspec().from_v4mapped().is_err());
    assert!(SocketAddress::new_inet_any().from_v4mapped().is_err());
    assert!(SocketAddress::new_unix("").from_v4mapped().is_err());
    assert!(sa.sin_any.from_v4mapped().is_err());
    assert!(sa.sin_bc.from_v4mapped().is_err());
    assert!(sa.sin_1.from_v4mapped().is_err());
}

#[test]
fn test_sa_to_v4mapped() {
    let sa = DefaultSa::new();

    assert!(sa
        .sin_any
        .to_v4mapped()
        .unwrap()
        .equal_addr(&sa.sin6_v4_any)
        .unwrap());
    assert!(sa.sin_any.to_v4mapped().unwrap().is_v4mapped());
    assert!(sa.sin_any.to_v4mapped().unwrap().is_port_any());

    assert!(sa
        .sin_bc
        .to_v4mapped()
        .unwrap()
        .equal_addr(&sa.sin6_v4_bc)
        .unwrap());
    assert!(sa.sin_bc.to_v4mapped().unwrap().is_v4mapped());
    assert!(sa.sin_bc.to_v4mapped().unwrap().is_port_any());

    assert!(sa
        .sin_1
        .to_v4mapped()
        .unwrap()
        .equal_addr(&sa.sin6_v4_1)
        .unwrap());
    assert!(sa.sin_1.to_v4mapped().unwrap().is_v4mapped());
    assert!(sa.sin_1.to_v4mapped().unwrap().is_port_any());

    assert!(SocketAddress::new_unspec().to_v4mapped().is_err());
    assert!(SocketAddress::new_inet6_any().to_v4mapped().is_err());
    assert!(SocketAddress::new_unix("").to_v4mapped().is_err());
    assert!(sa.sin6_any.to_v4mapped().is_err());
    assert!(sa.sin6_1.to_v4mapped().is_err());
    assert!(sa.sin6_v4_any.to_v4mapped().is_err());
    assert!(sa.sin6_v4_bc.to_v4mapped().is_err());
    assert!(sa.sin6_v4_1.to_v4mapped().is_err());
}

#[test]
fn test_socket_address_key() {
    let sa_v4 = SocketAddress::from_str("1.2.3.4:5000").unwrap();
    let sa_v4_other_port = SocketAddress::from_str("1.2.3.4:6000").unwrap();
    let sa_v6 = SocketAddress::from_str("[ff01::1]:5000").unwrap();

    let key1 = SocketAddressKey::from_socket_address(&sa_v4);
    let key2 = SocketAddressKey::from_socket_address(&sa_v4_other_port);
    let key3 = SocketAddressKey::from_socket_address(&sa_v6);

    assert_eq!(key1, key2);
    assert_ne!(key1, key3);
    assert!(key1.is_valid());
    assert!(key3.is_valid());
    assert!(!SocketAddressKey::Unspec.is_valid());
}

#[test]
fn test_socket_manager() {
    let sm = SocketManager::new();
    assert_eq!(sm.max_size(), 1024);
    assert_eq!(sm.size(), 0);

    assert!(sm.can_open_socket(SocketCategory::Generic));
    assert!(sm.can_open_socket(SocketCategory::Http));

    sm.record_open_socket(10, SocketCategory::Http).unwrap();
    assert_eq!(sm.size(), 1);
    assert_eq!(sm.category_managed_size(SocketCategory::Http), 1);
    assert_eq!(sm.category_managed_size(SocketCategory::Generic), 0);

    sm.record_close_socket(10).unwrap();
    assert_eq!(sm.size(), 0);
    assert_eq!(sm.category_managed_size(SocketCategory::Http), 0);

    sm.set_max_size_and_adjust(2048).unwrap();
    assert_eq!(sm.max_size(), 2048);
}

#[test]
fn test_network_config() {
    let nc = NetworkConfig::new();
    assert!(!nc.is_block_ipv4());
    assert!(!nc.is_block_ipv6());

    nc.set_bind_address("127.0.0.1").unwrap();
    let best = nc.bind_address_best_match();
    assert_eq!(best.addr_str(), "127.0.0.1");

    nc.set_encryption_modes(EncryptionMode::Allow, EncryptionMode::Allow)
        .unwrap();
    let (h, s) = nc.encryption_modes();
    assert_eq!(h, EncryptionMode::Allow);
    assert_eq!(s, EncryptionMode::Allow);

    assert!(nc
        .set_encryption_modes(EncryptionMode::Deny, EncryptionMode::Require)
        .is_err());
}
