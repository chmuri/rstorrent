use lt_common::object::Object;

pub const MAX_PEERS: usize = 32;
pub const MAX_SIZE: usize = 128;

#[derive(Clone, Debug, Default)]
pub struct DhtTracker {
    peers: Vec<([u8; 6], u32)>,
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

    pub fn peers(&self) -> &[([u8; 6], u32)] {
        &self.peers
    }

    pub fn add_peer(&mut self, addr_n: u32, port: u16, now: u32) {
        let mut compact = [0u8; 6];
        compact[0..4].copy_from_slice(&addr_n.to_be_bytes());
        compact[4..6].copy_from_slice(&port.to_be_bytes());

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

    pub fn get_peers(&self, max_peers: usize) -> Vec<[u8; 6]> {
        let count = max_peers.min(self.peers.len()).min(MAX_PEERS);
        self.peers.iter().take(count).map(|(p, _)| *p).collect()
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
            let ip = u32::from_be_bytes([compact[0], compact[1], compact[2], compact[3]]);
            let port = u16::from_be_bytes([compact[4], compact[5]]);
            let mut entry = Object::create_map();
            entry
                .insert_key("i", Object::create_value(ip as i64))
                .unwrap();
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
            let Ok(ip) = entry.get_key_value("i") else {
                continue;
            };
            let Ok(port) = entry.get_key_value("p") else {
                continue;
            };
            let last_seen = entry.get_key_value("t").unwrap_or(0).max(0) as u32;
            self.add_peer(ip as u32, port as u16, last_seen);
        }
    }

    pub fn from_cache(cache: &Object) -> Self {
        let mut tracker = Self::new();
        tracker.load_cache(cache);
        tracker
    }
}
