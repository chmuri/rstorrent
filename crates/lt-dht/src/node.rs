use std::net::{IpAddr, Ipv4Addr};

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
        let ip_val = cache
            .get_key("i")
            .map_err(|_| DhtError::Input("Cache missing 'i' key".into()))?
            .as_value()
            .map_err(|_| DhtError::Input("'i' key must be integer".into()))?;

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

        let ip = Ipv4Addr::from(ip_val as u32);
        let port = port_val as u16;
        let last_seen = last_seen_val as u32;

        Ok(Self {
            id,
            address: SocketAddress::new_inet(ip, port),
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

    pub fn store_compact(&self, buffer: &mut Vec<u8>) {
        buffer.extend_from_slice(self.id.as_slice());
        match self.address.ip() {
            Some(IpAddr::V4(v4)) => buffer.extend_from_slice(&v4.octets()),
            _ => buffer.extend_from_slice(&[0, 0, 0, 0]),
        }
        buffer.extend_from_slice(&self.address.port().to_be_bytes());
    }

    pub fn store_cache(&self) -> Object {
        let mut map = Object::create_map();
        let ip_u32 = match self.address.ip() {
            Some(IpAddr::V4(v4)) => u32::from(v4),
            _ => 0,
        };
        map.insert_key("i", Object::create_value(ip_u32 as i64))
            .unwrap();
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
