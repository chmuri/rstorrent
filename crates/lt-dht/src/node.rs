use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use lt_common::hash_string::HashString;
use lt_common::object::Object;
use lt_net::addr::SocketAddress;

use crate::error::DhtError;

pub const MAX_FAILED_REPLIES: u32 = 5;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DhtNode {
    pub id: HashString,
    pub address: SocketAddress,
    pub last_seen: u32,
    pub recently_active: bool,
    pub recently_inactive: u32,
}

impl DhtNode {
    pub fn new(id: HashString, address: SocketAddress) -> Self {
        Self {
            id,
            address,
            last_seen: 0,
            recently_active: false,
            recently_inactive: 0,
        }
    }

    pub fn from_cache(id: HashString, cache: &Object) -> Result<Self, DhtError> {
        let port_val = cache
            .get_key("p")
            .map_err(|_| DhtError::Input("Cache missing 'p' key".into()))?
            .as_value()
            .map_err(|_| DhtError::Input("'p' key must be integer".into()))?;

        let last_seen_val = cache
            .get_key("t")
            .map_err(|_| DhtError::Input("Cache missing 't' key".into()))?
            .as_value()
            .map_err(|_| DhtError::Input("'t' key must be integer".into()))?;

        let port = port_val as u16;
        let last_seen = last_seen_val as u32;

        // BEP 32: IPv6 nodes cache the 16 address octets under `i6`; IPv4
        // nodes keep the legacy `i` integer. `i6` takes precedence.
        let address = if let Ok(i6_obj) = cache.get_key("i6") {
            let octets = i6_obj
                .as_string()
                .map_err(|_| DhtError::Input("'i6' must be string".into()))?;
            if octets.len() != 16 {
                return Err(DhtError::Input("'i6' must be 16 bytes".into()));
            }
            let mut arr = [0u8; 16];
            arr.copy_from_slice(octets);
            SocketAddress::new_inet6(Ipv6Addr::from(arr), port)
        } else {
            let ip_val = cache
                .get_key("i")
                .map_err(|_| DhtError::Input("Cache missing 'i' key".into()))?
                .as_value()
                .map_err(|_| DhtError::Input("'i' key must be integer".into()))?;
            SocketAddress::new_inet(Ipv4Addr::from(ip_val as u32), port)
        };

        Ok(Self {
            id,
            address,
            last_seen,
            recently_active: last_seen > 0,
            recently_inactive: 0,
        })
    }

    pub fn is_good(&self) -> bool {
        self.recently_active
    }

    pub fn is_questionable(&self) -> bool {
        !self.recently_active
    }

    pub fn is_bad(&self) -> bool {
        self.recently_inactive >= MAX_FAILED_REPLIES
    }

    pub fn is_active(&self) -> bool {
        self.last_seen > 0
    }

    pub fn age(&self, now: u32) -> u32 {
        now.saturating_sub(self.last_seen)
    }

    pub fn update(&mut self, now: u32) {
        self.recently_active = self.age(now) < 15 * 60;
    }

    pub fn replied(&mut self, now: u32) {
        self.last_seen = now;
        self.recently_inactive = 0;
        self.recently_active = true;
    }

    pub fn queried(&mut self, now: u32) {
        if self.last_seen > 0 {
            self.replied(now);
        }
    }

    pub fn inactive(&mut self) {
        self.recently_inactive += 1;
        if self.recently_inactive >= MAX_FAILED_REPLIES {
            self.recently_active = false;
        }
    }

    /// Appends the compact node form: 20-byte id + address + port. IPv4
    /// nodes are 26 bytes (`nodes`), IPv6 nodes 38 bytes (`nodes6`, BEP 32).
    pub fn store_compact(&self, buffer: &mut Vec<u8>) {
        buffer.extend_from_slice(self.id.as_slice());
        match self.address.ip() {
            Some(IpAddr::V4(v4)) => buffer.extend_from_slice(&v4.octets()),
            Some(IpAddr::V6(v6)) => buffer.extend_from_slice(&v6.octets()),
            None => buffer.extend_from_slice(&[0, 0, 0, 0]),
        }
        buffer.extend_from_slice(&self.address.port().to_be_bytes());
    }

    pub fn store_cache(&self) -> Object {
        let mut map = Object::create_map();
        match self.address.ip() {
            Some(IpAddr::V4(v4)) => {
                map.insert_key("i", Object::create_value(u32::from(v4) as i64))
                    .unwrap();
            }
            Some(IpAddr::V6(v6)) => {
                map.insert_key("i6", Object::create_string(v6.octets().to_vec()))
                    .unwrap();
            }
            None => {
                map.insert_key("i", Object::create_value(0)).unwrap();
            }
        }
        map.insert_key("p", Object::create_value(self.address.port() as i64))
            .unwrap();
        map.insert_key("t", Object::create_value(self.last_seen as i64))
            .unwrap();
        map
    }
}

/// True for addresses that are safe to persist in the DHT cache.
///
/// Loopback, unspecified, link-local, multicast, broadcast and RFC 1918 /
/// RFC 4193 (unique-local) addresses are excluded, as are IPv4-mapped
/// versions of the same ranges.
pub fn is_persistable_address(address: &SocketAddress) -> bool {
    if let Some(v4) = address.v4_mapped_ip() {
        return is_public_v4(v4);
    }

    match address.ip() {
        Some(IpAddr::V4(v4)) => is_public_v4(v4),
        Some(IpAddr::V6(v6)) => {
            let octets = v6.octets();
            let unique_local = (octets[0] & 0xfe) == 0xfc; // fc00::/7
            let link_local = octets[0] == 0xfe && (octets[1] & 0xc0) == 0x80; // fe80::/10
            let multicast = octets[0] == 0xff;

            !(v6.is_unspecified() || v6.is_loopback() || unique_local || link_local || multicast)
        }
        None => false,
    }
}

fn is_public_v4(v4: Ipv4Addr) -> bool {
    let octets = v4.octets();
    let carrier_grade_nat = octets[0] == 100 && (octets[1] & 0xc0) == 64; // 100.64/10

    !(v4.is_unspecified()
        || v4.is_loopback()
        || v4.is_private()
        || v4.is_link_local()
        || v4.is_broadcast()
        || v4.is_multicast()
        || octets[0] == 0
        || carrier_grade_nat)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ipv6_node_compact_is_38_bytes_and_cache_round_trips() {
        let id = HashString::new([5u8; 20]);
        let addr = SocketAddress::new_inet6(Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 1), 6881);
        let node = DhtNode::new(id, addr);
        let mut buf = Vec::new();
        node.store_compact(&mut buf);
        assert_eq!(buf.len(), 38);

        let cache = node.store_cache();
        assert!(cache.get_key("i6").is_ok());
        let restored = DhtNode::from_cache(id, &cache).expect("from_cache");
        assert!(restored.address.is_inet6());
        assert_eq!(restored.address.port(), 6881);
    }

    #[test]
    fn ipv4_node_compact_is_26_bytes() {
        let id = HashString::new([6u8; 20]);
        let node = DhtNode::new(id, SocketAddress::new_inet(Ipv4Addr::new(1, 2, 3, 4), 6881));
        let mut buf = Vec::new();
        node.store_compact(&mut buf);
        assert_eq!(buf.len(), 26);
        let cache = node.store_cache();
        assert!(cache.get_key("i").is_ok());
        let restored = DhtNode::from_cache(id, &cache).expect("from_cache");
        assert!(restored.address.is_inet());
    }
}
