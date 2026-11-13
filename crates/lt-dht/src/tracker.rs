use lt_common::object::Object;

pub const MAX_PEERS: usize = 32;
pub const MAX_SIZE: usize = 128;

#[derive(Clone, Debug, Default)]
pub struct DhtTracker {
    peers: Vec<(Vec<u8>, u32)>,
}

impl DhtTracker {
    pub fn new() -> Self {
        Self {
            peers: Vec::with_capacity(MAX_SIZE),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.peers.is_empty()
    }

    pub fn size(&self) -> usize {
        self.peers.len()
    }

    pub fn peers(&self) -> &[(Vec<u8>, u32)] {
        &self.peers
    }

    pub fn add_peer(&mut self, addr_n: u32, port: u16, now: u32) {
        let mut compact = Vec::with_capacity(6);
        compact.extend_from_slice(&addr_n.to_be_bytes());
        compact.extend_from_slice(&port.to_be_bytes());
        self.insert_peer(compact, now);
    }

    /// BEP 32: records an IPv6 announcer (16 address octets + port).
    pub fn add_peer6(&mut self, addr: [u8; 16], port: u16, now: u32) {
        let mut compact = Vec::with_capacity(18);
        compact.extend_from_slice(&addr);
        compact.extend_from_slice(&port.to_be_bytes());
        self.insert_peer(compact, now);
    }

    fn insert_peer(&mut self, compact: Vec<u8>, now: u32) {
        // Check if already exists
        if let Some(pos) = self.peers.iter().position(|(p, _)| *p == compact) {
            self.peers[pos].1 = now;
            return;
        }

        if self.peers.len() >= MAX_SIZE {
            // Replace the oldest peer
            if let Some((oldest_idx, _)) = self
                .peers
                .iter()
                .enumerate()
                .min_by_key(|(_, (_, seen))| *seen)
            {
                self.peers[oldest_idx] = (compact, now);
                return;
            }
        }

        self.peers.push((compact, now));
    }

    pub fn get_peers(&self, max_peers: usize) -> Vec<Vec<u8>> {
        let count = max_peers.min(self.peers.len()).min(MAX_PEERS);
        self.peers
            .iter()
            .take(count)
            .map(|(p, _)| p.clone())
            .collect()
    }

    pub fn prune(&mut self, max_age: u32, now: u32) {
        self.peers
            .retain(|(_, seen)| now.saturating_sub(*seen) <= max_age);
    }

    pub fn last_seen(&self) -> u32 {
        self.peers.iter().map(|(_, seen)| *seen).max().unwrap_or(0)
    }

    /// Serializes the peer list as a bencode-friendly list of maps with the
    /// same `i`/`p`/`t` keys used by the node cache.
    pub fn store_cache(&self) -> Object {
        let mut list = Object::create_list();
        for (compact, last_seen) in &self.peers {
            let port = u16::from_be_bytes([compact[compact.len() - 2], compact[compact.len() - 1]]);
            let mut entry = Object::create_map();
            if compact.len() == 18 {
                entry
                    .insert_key("i6", Object::create_string(compact[0..16].to_vec()))
                    .unwrap();
            } else {
                let ip = u32::from_be_bytes([compact[0], compact[1], compact[2], compact[3]]);
                entry
                    .insert_key("i", Object::create_value(ip as i64))
                    .unwrap();
            }
            entry
                .insert_key("p", Object::create_value(port as i64))
                .unwrap();
            entry
                .insert_key("t", Object::create_value(*last_seen as i64))
                .unwrap();
            list.insert_back(entry).unwrap();
        }
        list
    }

    /// Tolerant loader: malformed entries are skipped, never fatal.
    pub fn load_cache(&mut self, cache: &Object) {
        let Ok(entries) = cache.as_list() else {
            return;
        };

        for entry in entries {
            let Ok(port) = entry.get_key_value("p") else {
                continue;
            };
            let last_seen = entry.get_key_value("t").unwrap_or(0).max(0) as u32;
            if let Some(i6) = entry.get_key("i6").ok().and_then(|o| o.as_string().ok()) {
                if i6.len() == 16 {
                    let mut addr = [0u8; 16];
                    addr.copy_from_slice(i6);
                    self.add_peer6(addr, port as u16, last_seen);
                }
            } else if let Ok(ip) = entry.get_key_value("i") {
                self.add_peer(ip as u32, port as u16, last_seen);
            }
        }
    }

    pub fn from_cache(cache: &Object) -> Self {
        let mut tracker = Self::new();
        tracker.load_cache(cache);
        tracker
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ipv6_peer_round_trips_through_cache() {
        let mut tracker = DhtTracker::new();
        let addr = [0x20u8; 16];
        tracker.add_peer6(addr, 6881, 100);
        tracker.add_peer(0x01020304, 6882, 100);
        let peers = tracker.get_peers(32);
        assert_eq!(peers.len(), 2);
        assert!(peers.iter().any(|p| p.len() == 18));
        assert!(peers.iter().any(|p| p.len() == 6));

        let cache = tracker.store_cache();
        let restored = DhtTracker::from_cache(&cache);
        let rpeers = restored.get_peers(32);
        assert_eq!(rpeers.len(), 2);
        assert!(rpeers.iter().any(|p| p.len() == 18 && p[0..16] == addr));
    }
}
