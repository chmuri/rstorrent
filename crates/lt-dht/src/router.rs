use std::collections::{HashMap, HashSet};

use lt_common::hash_string::HashString;
use lt_common::object::Object;
use lt_crypto::sha1::sha1_hash;
use lt_net::addr::SocketAddress;

use crate::bucket::DhtBucket;
use crate::node::{is_persistable_address, DhtNode};
use crate::tracker::DhtTracker;

pub const MAX_TRACKERS: usize = 1024;
pub const NUM_TRACKER_EVICT: usize = MAX_TRACKERS / 20; // 51
pub const TIMEOUT_TRACKER_EVICT: u32 = 3600; // 1 hour
pub const TIMEOUT_UPDATE: u32 = 15 * 60; // 15 minutes
pub const TIMEOUT_REMOVE_NODE: u32 = 4 * 60 * 60; // 4 hours
pub const TIMEOUT_PEER_ANNOUNCE: u32 = 30 * 60; // 30 minutes

#[derive(Clone, Debug, Default)]
pub struct DhtStatistics {
    pub num_trackers: usize,
    pub num_buckets: usize,
    pub num_nodes: usize,
    pub num_peers: usize,
    pub max_peers: usize,
}

pub struct DhtRouter {
    pub own_id: HashString,
    pub buckets: Vec<DhtBucket>,
    pub trackers: HashMap<HashString, DhtTracker>,
    pub last_tracker_evict: u32,
    /// Info hashes belonging to private torrents. Their trackers and peers are
    /// never written to the on-disk cache.
    private_info_hashes: HashSet<HashString>,
    token_secret: [u8; 20],
}

impl DhtRouter {
    pub fn new(own_id: HashString) -> Self {
        let mut begin = HashString::default();
        let mut end = HashString::default();
        begin.clear();
        end.clear_byte(0xFF);

        let initial_bucket = DhtBucket::new(begin, end);

        Self {
            own_id,
            buckets: vec![initial_bucket],
            trackers: HashMap::new(),
            last_tracker_evict: 0,
            private_info_hashes: HashSet::new(),
            token_secret: [0x5au8; 20],
        }
    }

    pub fn get_statistics(&self) -> DhtStatistics {
        let num_nodes = self.buckets.iter().map(|b| b.size()).sum();
        let mut num_peers = 0usize;
        let mut max_peers = 0usize;
        for tracker in self.trackers.values() {
            let peers = tracker.size();
            num_peers += peers;
            max_peers = max_peers.max(peers);
        }

        DhtStatistics {
            num_trackers: self.trackers.len(),
            num_buckets: self.buckets.len(),
            num_nodes,
            num_peers,
            max_peers,
        }
    }

    /// Marks an info hash as private. Private trackers are not created and are
    /// not persisted.
    pub fn set_private_info_hash(&mut self, hash: HashString, is_private: bool) {
        if is_private {
            self.private_info_hashes.insert(hash);
            self.trackers.remove(&hash);
        } else {
            self.private_info_hashes.remove(&hash);
        }
    }

    pub fn is_private_info_hash(&self, hash: &HashString) -> bool {
        self.private_info_hashes.contains(hash)
    }

    pub fn get_tracker(&mut self, hash: &HashString, create: bool) -> Option<&mut DhtTracker> {
        if self.trackers.contains_key(hash) {
            return self.trackers.get_mut(hash);
        }

        if !create || self.private_info_hashes.contains(hash) {
            return None;
        }

        if self.trackers.len() >= MAX_TRACKERS {
            return None;
        }

        self.trackers.insert(*hash, DhtTracker::new());
        self.trackers.get_mut(hash)
    }

    pub fn evict_stale_trackers_at(&mut self, now: u32) {
        if self.trackers.len() < MAX_TRACKERS {
            return;
        }

        if self.last_tracker_evict > 0 && now < self.last_tracker_evict + TIMEOUT_TRACKER_EVICT {
            return;
        }

        // Find the least recently announced NUM_TRACKER_EVICT trackers
        let mut tracker_times: Vec<(HashString, u32)> = self
            .trackers
            .iter()
            .map(|(hash, tracker)| (*hash, tracker.last_seen()))
            .collect();

        // Sort by last_seen ascending (oldest first)
        tracker_times.sort_by_key(|(_, seen)| *seen);

        for (hash, _) in tracker_times.iter().take(NUM_TRACKER_EVICT) {
            self.trackers.remove(hash);
        }

        self.last_tracker_evict = now;
    }

    pub fn want_node(&self, id: &HashString) -> bool {
        if *id == self.own_id || id.is_zero() {
            return false;
        }

        let bucket = self.find_bucket_ref(id);
        bucket.has_space() || bucket.is_in_range(&self.own_id)
    }

    pub fn find_bucket_ref(&self, id: &HashString) -> &DhtBucket {
        for bucket in &self.buckets {
            if bucket.is_in_range(id) {
                return bucket;
            }
        }
        &self.buckets[0]
    }

    pub fn find_bucket_mut(&mut self, id: &HashString) -> &mut DhtBucket {
        for i in 0..self.buckets.len() {
            if self.buckets[i].is_in_range(id) {
                return &mut self.buckets[i];
            }
        }
        &mut self.buckets[0]
    }

    pub fn add_node(&mut self, node: DhtNode) -> bool {
        let id = node.id;
        let own_id = self.own_id;

        // Never insert ourselves or the all-zero id (a common broken-client
        // signal that would otherwise pollute a bucket).
        if id == own_id || id.is_zero() {
            return false;
        }

        // Keep one entry per node id. Refreshing an existing entry is cheap
        // and prevents duplicate cache loads from inflating the table.
        for bucket in &mut self.buckets {
            if let Some(existing) = bucket.nodes.iter_mut().find(|node| node.id == id) {
                if existing.address == node.address {
                    existing.last_seen = existing.last_seen.max(node.last_seen);
                    existing.recently_active |= node.recently_active;
                }
                return true;
            }
        }

        loop {
            let bucket_idx = match self.buckets.iter().position(|b| b.is_in_range(&id)) {
                Some(idx) => idx,
                None => 0,
            };

            if !self.buckets[bucket_idx].is_full() {
                self.buckets[bucket_idx].add_node(node);
                return true;
            }

            if let Some(candidate) = self.buckets[bucket_idx].find_replacement_candidate(false) {
                if self.buckets[bucket_idx].nodes[candidate].is_bad() {
                    self.buckets[bucket_idx].nodes[candidate] = node;
                    return true;
                }
            }

            // Bucket is full of good nodes
            if self.buckets[bucket_idx].is_in_range(&own_id) {
                let other = self.buckets[bucket_idx].split(&own_id);
                self.buckets.push(other);
                continue;
            } else {
                return false;
            }
        }
    }

    pub fn get_closest_nodes(&self, id: &HashString) -> Vec<u8> {
        let bucket = self.find_bucket_ref(id);
        let mut buffer = Vec::new();
        bucket.full_bucket(&mut buffer);
        buffer
    }

    /// Returns up to `max` known nodes ordered by XOR distance to `target`.
    pub fn closest_nodes(&self, target: &HashString, max: usize) -> Vec<DhtNode> {
        let mut nodes: Vec<DhtNode> = self
            .buckets
            .iter()
            .flat_map(|bucket| bucket.nodes.iter().cloned())
            .collect();

        nodes.sort_by(|a, b| {
            xor_distance(&a.id, target).cmp(&xor_distance(&b.id, target))
        });
        nodes.truncate(max);
        nodes
    }

    /// Marks a node as having answered.
    pub fn mark_replied(&mut self, id: &HashString, now: u32) {
        for bucket in &mut self.buckets {
            if let Some(node) = bucket.nodes.iter_mut().find(|node| node.id == *id) {
                node.replied(now);
                return;
            }
        }
    }

    /// Records a failed query; after enough failures the node becomes bad.
    pub fn mark_inactive(&mut self, id: &HashString) {
        for bucket in &mut self.buckets {
            if let Some(node) = bucket.nodes.iter_mut().find(|node| node.id == *id) {
                node.inactive();
                return;
            }
        }
    }

    /// Marks the node at `address` inactive. Used when a refresh query times
    /// out and the node id is not known to the caller.
    pub fn mark_inactive_by_address(&mut self, address: &SocketAddress) {
        for bucket in &mut self.buckets {
            if let Some(node) = bucket.nodes.iter_mut().find(|node| &node.address == address) {
                node.inactive();
                return;
            }
        }
    }

    /// Ages nodes, removes nodes that have been bad for `remove_age` seconds,
    /// and returns nodes that should be pinged/refreshed.
    pub fn update_nodes(&mut self, now: u32, remove_age: u32) -> Vec<DhtNode> {
        let mut to_ping = Vec::new();

        for bucket in &mut self.buckets {
            for node in &mut bucket.nodes {
                node.update(now);
                if node.is_questionable() && (!node.is_bad() || node.age(now) >= remove_age) {
                    to_ping.push(node.clone());
                }
            }
        }

        self.evict_stale_nodes_at(now, remove_age);
        to_ping
    }

    /// Removes bad nodes whose last successful reply is older than
    /// `remove_age`. Returns the number of removed nodes.
    pub fn evict_stale_nodes_at(&mut self, now: u32, remove_age: u32) -> usize {
        let before: usize = self.buckets.iter().map(|bucket| bucket.nodes.len()).sum();

        for bucket in &mut self.buckets {
            bucket
                .nodes
                .retain(|node| !(node.is_bad() && node.age(now) >= remove_age));
        }

        before - self.buckets.iter().map(|bucket| bucket.nodes.len()).sum::<usize>()
    }

    /// Prunes peers that have not re-announced for `max_age` seconds, drops
    /// empty trackers and enforces the tracker table cap.
    pub fn prune_trackers(&mut self, max_age: u32, now: u32) -> usize {
        let before = self.trackers.len();

        for tracker in self.trackers.values_mut() {
            tracker.prune(max_age, now);
        }
        self.trackers.retain(|_, tracker| !tracker.is_empty());

        self.evict_stale_trackers_at(now);
        before.saturating_sub(self.trackers.len())
    }

    pub fn make_token(&self, sa: &SocketAddress) -> [u8; 8] {
        let mut data = Vec::new();
        data.extend_from_slice(&self.token_secret);
        if let Some(ip) = sa.ip() {
            match ip {
                std::net::IpAddr::V4(v4) => data.extend_from_slice(&v4.octets()),
                std::net::IpAddr::V6(v6) => data.extend_from_slice(&v6.octets()),
            }
        }
        let digest = sha1_hash(&data);
        let mut token = [0u8; 8];
        token.copy_from_slice(&digest[0..8]);
        token
    }

    pub fn token_valid(&self, token: &[u8], sa: &SocketAddress) -> bool {
        if token.len() != 8 {
            return false;
        }
        let expected = self.make_token(sa);
        token == expected
    }

    pub fn store_cache(&self) -> Object {
        let mut list = Object::create_list();
        for bucket in &self.buckets {
            for node in &bucket.nodes {
                // Never persist bad nodes, and never store loopback/private/
                // link-local addresses or non-IPv4 entries.
                if !node.is_bad()
                    && node.address.ip().map(|ip| ip.is_ipv4()).unwrap_or(false)
                    && is_persistable_address(&node.address)
                {
                    let mut node_map = node.store_cache();
                    node_map
                        .insert_key("id", Object::create_string(node.id.as_slice().to_vec()))
                        .unwrap();
                    list.insert_back(node_map).unwrap();
                }
            }
        }
        let mut root = Object::create_map();
        root.insert_key("nodes", list).unwrap();

        let mut trackers = Object::create_map();
        for (hash, tracker) in &self.trackers {
            if self.private_info_hashes.contains(hash) || tracker.is_empty() {
                continue;
            }
            trackers
                .insert_key(hash.as_slice().to_vec(), tracker.store_cache())
                .unwrap();
        }
        root.insert_key("trackers", trackers).unwrap();
        root
    }

    pub fn load_cache(&mut self, cache: &Object) {
        if let Ok(nodes_obj) = cache.get_key("nodes") {
            if let Ok(nodes_map) = nodes_obj.as_map() {
                for (id_bytes, node_obj) in nodes_map {
                    if id_bytes.len() == 20 {
                        let mut id = HashString::default();
                        id.as_mut_slice().copy_from_slice(id_bytes);
                        if let Ok(node) = DhtNode::from_cache(id, node_obj) {
                            self.add_node(node);
                        }
                    }
                }
            } else if let Ok(nodes_list) = nodes_obj.as_list() {
                for node_obj in nodes_list {
                    if let Ok(id_obj) = node_obj.get_key("id") {
                        if let Ok(id_bytes) = id_obj.as_string() {
                            if id_bytes.len() == 20 {
                                let mut id = HashString::default();
                                id.as_mut_slice().copy_from_slice(id_bytes);
                                if let Ok(node) = DhtNode::from_cache(id, node_obj) {
                                    self.add_node(node);
                                }
                            }
                        }
                    }
                }
            }
        }

        if let Ok(trackers_obj) = cache.get_key("trackers") {
            if let Ok(trackers_map) = trackers_obj.as_map() {
                for (hash_bytes, tracker_obj) in trackers_map {
                    let Some(hash) = HashString::from_slice(hash_bytes) else {
                        continue;
                    };
                    if self.private_info_hashes.contains(&hash)
                        || self.trackers.len() >= MAX_TRACKERS
                    {
                        continue;
                    }
                    let tracker = DhtTracker::from_cache(tracker_obj);
                    if !tracker.is_empty() {
                        self.trackers.insert(hash, tracker);
                    }
                }
            }
        }
    }
}

/// XOR distance between two 160-bit ids, most-significant byte first.
pub fn xor_distance(a: &HashString, b: &HashString) -> [u8; 20] {
    let mut distance = [0u8; 20];
    for (index, byte) in distance.iter_mut().enumerate() {
        *byte = a.as_slice()[index] ^ b.as_slice()[index];
    }
    distance
}
