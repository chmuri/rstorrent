use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use hickory_resolver::config::{ResolverConfig, ResolverOpts};
use hickory_resolver::TokioAsyncResolver;

use crate::addr::{AddressFamily, SocketAddress};
use crate::error::NetError;

#[derive(Clone, Debug)]
struct CacheEntry {
    ips: Vec<IpAddr>,
    expires_at: Instant,
}

#[derive(Clone)]
pub struct DnsCache {
    entries: Arc<Mutex<HashMap<String, CacheEntry>>>,
    default_ttl: Duration,
}

impl Default for DnsCache {
    fn default() -> Self {
        Self::new(Duration::from_secs(300)) // 5 minute default TTL
    }
}

impl DnsCache {
    pub fn new(default_ttl: Duration) -> Self {
        Self {
            entries: Arc::new(Mutex::new(HashMap::new())),
            default_ttl,
        }
    }

    pub fn get(&self, hostname: &str) -> Option<Vec<IpAddr>> {
        let mut entries = self.entries.lock().unwrap();
        if let Some(entry) = entries.get(hostname) {
            if Instant::now() < entry.expires_at {
                return Some(entry.ips.clone());
            } else {
                entries.remove(hostname);
            }
        }
        None
    }

    pub fn insert(&self, hostname: String, ips: Vec<IpAddr>, ttl: Option<Duration>) {
        let mut entries = self.entries.lock().unwrap();
        let expires_at = Instant::now() + ttl.unwrap_or(self.default_ttl);
        entries.insert(hostname, CacheEntry { ips, expires_at });
    }

    pub fn clear(&self) {
        let mut entries = self.entries.lock().unwrap();
        entries.clear();
    }
}

pub struct DnsResolver {
    resolver: TokioAsyncResolver,
    cache: DnsCache,
}

impl Default for DnsResolver {
    fn default() -> Self {
        Self::new().expect("Failed to initialize system DNS resolver")
    }
}

impl DnsResolver {
    pub fn new() -> Result<Self, NetError> {
        let resolver = match TokioAsyncResolver::tokio_from_system_conf() {
            Ok(r) => r,
            Err(_) => TokioAsyncResolver::tokio(ResolverConfig::default(), ResolverOpts::default()),
        };
        Ok(Self {
            resolver,
            cache: DnsCache::default(),
        })
    }

    pub fn with_resolver_and_cache(resolver: TokioAsyncResolver, cache: DnsCache) -> Self {
        Self { resolver, cache }
    }

    pub fn cache(&self) -> &DnsCache {
        &self.cache
    }

    pub fn clear_cache(&self) {
        self.cache.clear();
    }

    /// Resolve both IPv4 and IPv6 addresses for a given hostname.
    pub async fn resolve_both(
        &self,
        hostname: &str,
        family: AddressFamily,
    ) -> Result<(Option<SocketAddress>, Option<SocketAddress>), NetError> {
        let (host, port) = Self::split_host_port(hostname);

        // Check if host is already numeric IP
        if let Ok(v4) = Ipv4Addr::from_str(host) {
            let sa_v4 = if family == AddressFamily::Unspec || family == AddressFamily::Inet {
                Some(SocketAddress::new_inet(v4, port))
            } else {
                None
            };
            return Ok((sa_v4, None));
        }
        if let Ok(v6) = Ipv6Addr::from_str(host) {
            let sa_v6 = if family == AddressFamily::Unspec || family == AddressFamily::Inet6 {
                Some(SocketAddress::new_inet6(v6, port))
            } else {
                None
            };
            return Ok((None, sa_v6));
        }

        // Check cache first
        let ips = if let Some(cached) = self.cache.get(host) {
            cached
        } else {
            let lookup = self
                .resolver
                .lookup_ip(host)
                .await
                .map_err(|e| NetError::Input(format!("DNS resolution failed for {}: {}", host, e)))?;

            let result_ips: Vec<IpAddr> = lookup.iter().collect();
            let min_ttl = lookup.as_lookup().record_iter().map(|r| r.ttl()).min().unwrap_or(300);
            self.cache.insert(host.to_string(), result_ips.clone(), Some(Duration::from_secs(min_ttl as u64)));
            result_ips
        };

        let mut sa_v4 = None;
        let mut sa_v6 = None;

        for ip in ips {
            match ip {
                IpAddr::V4(v4) => {
                    if sa_v4.is_none() && (family == AddressFamily::Unspec || family == AddressFamily::Inet) {
                        sa_v4 = Some(SocketAddress::new_inet(v4, port));
                    }
                }
                IpAddr::V6(v6) => {
                    if sa_v6.is_none() && (family == AddressFamily::Unspec || family == AddressFamily::Inet6) {
                        sa_v6 = Some(SocketAddress::new_inet6(v6, port));
                    }
                }
            }
        }

        Ok((sa_v4, sa_v6))
    }

    /// Resolve a single address, preferring either IPv4 or IPv6.
    pub async fn resolve_preferred(
        &self,
        hostname: &str,
        family: AddressFamily,
        preferred: AddressFamily,
    ) -> Result<Option<SocketAddress>, NetError> {
        let (v4, v6) = self.resolve_both(hostname, family).await?;
        if preferred == AddressFamily::Inet6 {
            if let Some(res6) = v6 {
                return Ok(Some(res6));
            }
            if let Some(res4) = v4 {
                return Ok(Some(res4));
            }
        } else {
            if let Some(res4) = v4 {
                return Ok(Some(res4));
            }
            if let Some(res6) = v6 {
                return Ok(Some(res6));
            }
        }
        Ok(None)
    }

    /// Resolve a specific address for the requested family.
    pub async fn resolve_specific(
        &self,
        hostname: &str,
        family: AddressFamily,
    ) -> Result<Option<SocketAddress>, NetError> {
        let (v4, v6) = self.resolve_both(hostname, family).await?;
        match family {
            AddressFamily::Inet => Ok(v4),
            AddressFamily::Inet6 => Ok(v6),
            _ => Ok(v4.or(v6)),
        }
    }

    fn split_host_port(target: &str) -> (&str, u16) {
        if target.starts_with('[') {
            if let Some(end_bracket) = target.find(']') {
                let host = &target[1..end_bracket];
                let remainder = &target[end_bracket + 1..];
                if remainder.starts_with(':') {
                    if let Ok(port) = remainder[1..].parse::<u16>() {
                        return (host, port);
                    }
                }
                return (host, 0);
            }
        }

        if let Some(colon_pos) = target.rfind(':') {
            // Check if it's an IPv6 without brackets (multiple colons)
            if target.find(':') == Some(colon_pos) {
                let host = &target[..colon_pos];
                if let Ok(port) = target[colon_pos + 1..].parse::<u16>() {
                    return (host, port);
                }
            }
        }

        (target, 0)
    }
}
