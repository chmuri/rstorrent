use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::time::Duration;

use lt_net::addr::{AddressFamily, SocketAddress};
use lt_net::resolver::{DnsCache, DnsResolver};

#[tokio::test]
async fn test_dns_cache_ttl() {
    let cache = DnsCache::new(Duration::from_millis(50));
    assert!(cache.get("example.com").is_none());

    cache.insert(
        "example.com".to_string(),
        vec![IpAddr::V4(Ipv4Addr::new(93, 184, 216, 34))],
        Some(Duration::from_millis(50)),
    );

    let found = cache.get("example.com");
    assert!(found.is_some());
    assert_eq!(found.unwrap()[0], IpAddr::V4(Ipv4Addr::new(93, 184, 216, 34)));

    // Wait for TTL expiration
    tokio::time::sleep(Duration::from_millis(60)).await;
    assert!(cache.get("example.com").is_none());
}

#[tokio::test]
async fn test_dns_cache_clear() {
    let cache = DnsCache::default();
    cache.insert(
        "test.org".to_string(),
        vec![IpAddr::V4(Ipv4Addr::new(1, 2, 3, 4))],
        None,
    );
    assert!(cache.get("test.org").is_some());
    cache.clear();
    assert!(cache.get("test.org").is_none());
}

#[tokio::test]
async fn test_resolver_numeric_short_circuit() {
    let resolver = DnsResolver::new().unwrap();

    // IPv4 numeric with port
    let (v4, v6) = resolver
        .resolve_both("127.0.0.1:8080", AddressFamily::Unspec)
        .await
        .unwrap();
    assert!(v4.is_some());
    assert!(v6.is_none());
    let sa = v4.unwrap();
    assert_eq!(sa.addr_str(), "127.0.0.1");
    assert_eq!(sa.port(), 8080);

    // IPv6 numeric with port
    let (v4, v6) = resolver
        .resolve_both("[::1]:53", AddressFamily::Unspec)
        .await
        .unwrap();
    assert!(v4.is_none());
    assert!(v6.is_some());
    let sa = v6.unwrap();
    assert_eq!(sa.addr_str(), "::1");
    assert_eq!(sa.port(), 53);
}

#[tokio::test]
async fn test_resolver_with_cache_hit() {
    let resolver = DnsResolver::new().unwrap();

    // Pre-populate cache for a mock host
    resolver.cache().insert(
        "mockhost.local".to_string(),
        vec![
            IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)),
            IpAddr::V6(Ipv6Addr::new(0xfd00, 0, 0, 0, 0, 0, 0, 1)),
        ],
        Some(Duration::from_secs(60)),
    );

    let (v4, v6) = resolver
        .resolve_both("mockhost.local:80", AddressFamily::Unspec)
        .await
        .unwrap();

    assert!(v4.is_some());
    assert!(v6.is_some());
    assert_eq!(v4.unwrap(), SocketAddress::new_inet(Ipv4Addr::new(10, 0, 0, 1), 80));
    assert_eq!(
        v6.unwrap(),
        SocketAddress::new_inet6(Ipv6Addr::new(0xfd00, 0, 0, 0, 0, 0, 0, 1), 80)
    );

    // Test resolve_preferred
    let pref_v4 = resolver
        .resolve_preferred("mockhost.local:80", AddressFamily::Unspec, AddressFamily::Inet)
        .await
        .unwrap();
    assert_eq!(
        pref_v4.unwrap(),
        SocketAddress::new_inet(Ipv4Addr::new(10, 0, 0, 1), 80)
    );

    let pref_v6 = resolver
        .resolve_preferred("mockhost.local:80", AddressFamily::Unspec, AddressFamily::Inet6)
        .await
        .unwrap();
    assert_eq!(
        pref_v6.unwrap(),
        SocketAddress::new_inet6(Ipv6Addr::new(0xfd00, 0, 0, 0, 0, 0, 0, 1), 80)
    );

    // Test resolve_specific
    let spec_v4 = resolver
        .resolve_specific("mockhost.local:80", AddressFamily::Inet)
        .await
        .unwrap();
    assert_eq!(
        spec_v4.unwrap(),
        SocketAddress::new_inet(Ipv4Addr::new(10, 0, 0, 1), 80)
    );
}
