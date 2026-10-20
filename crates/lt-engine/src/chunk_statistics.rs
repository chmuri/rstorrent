use std::collections::HashMap;

use lt_common::bitfield::Bitfield;
use lt_common::hash_string::HashString;

pub const MAX_ACCOUNTED: u32 = 255;

/// One peer's availability snapshot in the `AvailableList`.
#[derive(Debug, Clone)]
pub struct PeerAvailability {
    pub bitfield: Bitfield,
    /// Whether this peer currently contributes to the per-piece counts.
    pub counted: bool,
}

/// Per-peer piece ownership table.
///
/// This mirrors libtorrent's `AvailableList`: the statistics object can answer
/// which peers have a piece and can rebuild its per-piece counts when peers
/// disconnect or replace their bitfield.
#[derive(Debug, Clone, Default)]
pub struct AvailableList {
    peers: HashMap<HashString, PeerAvailability>,
}

impl AvailableList {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.peers.is_empty()
    }

    pub fn len(&self) -> usize {
        self.peers.len()
    }

    pub fn contains(&self, peer_id: &HashString) -> bool {
        self.peers.contains_key(peer_id)
    }

    pub fn bitfield(&self, peer_id: &HashString) -> Option<&Bitfield> {
        self.peers.get(peer_id).map(|peer| &peer.bitfield)
    }

    pub fn peers(&self) -> impl Iterator<Item = (&HashString, &PeerAvailability)> {
        self.peers.iter()
    }

    pub(crate) fn insert(
        &mut self,
        peer_id: HashString,
        bitfield: Bitfield,
        counted: bool,
    ) -> Option<PeerAvailability> {
        self.peers
            .insert(peer_id, PeerAvailability { bitfield, counted })
    }

    pub(crate) fn get_mut(&mut self, peer_id: &HashString) -> Option<&mut PeerAvailability> {
        self.peers.get_mut(peer_id)
    }

    pub(crate) fn remove(&mut self, peer_id: &HashString) -> Option<PeerAvailability> {
        self.peers.remove(peer_id)
    }

    pub(crate) fn clear(&mut self) {
        self.peers.clear();
    }
}

/// Tracks peer availability statistics and piece rarity across all connected peers.
#[derive(Debug, Clone, Default)]
pub struct ChunkStatistics {
    counts: Vec<u8>,
    complete: u32,
    accounted: u32,
    available: AvailableList,
}

impl ChunkStatistics {
    pub fn new(size: usize) -> Self {
        Self {
            counts: vec![0; size],
            complete: 0,
            accounted: 0,
            available: AvailableList::new(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.counts.is_empty()
    }

    pub fn len(&self) -> usize {
        self.counts.len()
    }

    pub fn complete(&self) -> u32 {
        self.complete
    }

    pub fn accounted(&self) -> u32 {
        self.accounted
    }

    pub fn available(&self) -> &AvailableList {
        &self.available
    }

    pub fn peer_count(&self) -> usize {
        self.available.len()
    }

    pub fn rarity(&self, index: usize) -> u8 {
        if index < self.counts.len() {
            self.counts[index]
        } else {
            0
        }
    }

    pub fn should_add(&self) -> bool {
        self.accounted < MAX_ACCOUNTED
    }

    pub fn clear(&mut self) {
        self.counts.clear();
        self.complete = 0;
        self.accounted = 0;
        self.available.clear();
    }

    /// Adds (or replaces) a peer and its current availability.
    ///
    /// Seeders (`have_all`) are tracked separately in `complete`; partial peers
    /// are counted in `accounted` up to [`MAX_ACCOUNTED`]. Peers with no pieces
    /// stay in the `AvailableList` so a later `have` can promote them.
    pub fn add_peer(&mut self, peer_id: HashString, bitfield: &Bitfield) {
        if let Some(old) = self.available.remove(&peer_id) {
            self.remove_contribution(&old);
        }

        let counted = if bitfield.is_all_set() {
            self.complete = self.complete.saturating_add(1);
            false
        } else if bitfield.is_all_unset() {
            false
        } else if self.should_add() {
            self.accounted = self.accounted.saturating_add(1);
            self.increment_counts(bitfield);
            true
        } else {
            false
        };

        self.available.insert(peer_id, bitfield.clone(), counted);
    }

    /// Replaces a peer's bitfield after a `bitfield`/`have_all`/`have_none`
    /// message without losing the peer identity.
    pub fn received_bitfield(&mut self, peer_id: HashString, bitfield: &Bitfield) {
        self.add_peer(peer_id, bitfield);
    }

    /// Removes a peer and all of its contributions.
    pub fn remove_peer(&mut self, peer_id: &HashString) {
        if let Some(peer) = self.available.remove(peer_id) {
            self.remove_contribution(&peer);
        }
    }

    /// Records one `have` message. Returns `true` when the peer did not already
    /// have the piece and the statistics changed.
    pub fn received_have_chunk(&mut self, peer_id: &HashString, index: usize) -> bool {
        if index >= self.counts.len() {
            return false;
        }

        let (was_counted, became_complete, bitfield) = {
            let Some(peer) = self.available.get_mut(peer_id) else {
                return false;
            };

            if peer.bitfield.get(index) {
                return false;
            }

            peer.bitfield.set(index);
            let was_counted = peer.counted;
            let became_complete = peer.bitfield.is_all_set();
            if was_counted && became_complete {
                peer.counted = false;
            }
            (was_counted, became_complete, peer.bitfield.clone())
        };

        if was_counted {
            // Add the new piece first; when the peer became complete the whole
            // contribution is removed below, so the net effect is correct.
            self.counts[index] = self.counts[index].saturating_add(1);
            if became_complete {
                self.complete = self.complete.saturating_add(1);
                self.accounted = self.accounted.saturating_sub(1);
                self.decrement_counts(&bitfield);
            }
        } else if became_complete {
            self.complete = self.complete.saturating_add(1);
        } else if self.should_add() {
            self.accounted = self.accounted.saturating_add(1);
            if let Some(peer) = self.available.get_mut(peer_id) {
                peer.counted = true;
            }
            self.increment_counts(&bitfield);
        }

        true
    }

    /// Whether a peer is known to have `index`.
    pub fn peer_has(&self, peer_id: &HashString, index: usize) -> bool {
        self.available
            .bitfield(peer_id)
            .map(|bitfield| bitfield.get(index))
            .unwrap_or(false)
    }

    fn remove_contribution(&mut self, peer: &PeerAvailability) {
        if peer.bitfield.is_all_set() {
            self.complete = self.complete.saturating_sub(1);
        } else if peer.counted {
            self.accounted = self.accounted.saturating_sub(1);
            self.decrement_counts(&peer.bitfield);
        }
    }

    fn increment_counts(&mut self, bitfield: &Bitfield) {
        let n = self.counts.len().min(bitfield.size_bits());
        for index in 0..n {
            if bitfield.get(index) {
                self.counts[index] = self.counts[index].saturating_add(1);
            }
        }
    }

    fn decrement_counts(&mut self, bitfield: &Bitfield) {
        let n = self.counts.len().min(bitfield.size_bits());
        for index in 0..n {
            if bitfield.get(index) {
                self.counts[index] = self.counts[index].saturating_sub(1);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peer_id(value: u8) -> HashString {
        HashString::new([value; 20])
    }

    fn bitfield(size: usize, set: &[usize]) -> Bitfield {
        let mut bitfield = Bitfield::new(size);
        for &index in set {
            bitfield.set(index);
        }
        bitfield
    }

    #[test]
    fn test_chunk_statistics_connect_disconnect() {
        let mut stats = ChunkStatistics::new(10);
        let peer1 = peer_id(1);
        let bf1 = bitfield(10, &[0, 2]);

        stats.add_peer(peer1, &bf1);
        assert_eq!(stats.accounted(), 1);
        assert_eq!(stats.complete(), 0);
        assert_eq!(stats.rarity(0), 1);
        assert_eq!(stats.rarity(1), 0);
        assert_eq!(stats.rarity(2), 1);
        assert!(stats.peer_has(&peer1, 0));

        // Seeder connect
        let seeder = peer_id(2);
        let seeder_bf = bitfield(10, &(0..10).collect::<Vec<_>>());
        stats.add_peer(seeder, &seeder_bf);
        assert_eq!(stats.complete(), 1);
        assert_eq!(stats.accounted(), 1);

        // Disconnect bf1
        stats.remove_peer(&peer1);
        assert_eq!(stats.accounted(), 0);
        assert_eq!(stats.rarity(0), 0);
        assert_eq!(stats.rarity(2), 0);
        assert!(!stats.peer_has(&peer1, 0));

        // Disconnect seeder
        stats.remove_peer(&seeder);
        assert_eq!(stats.complete(), 0);
        assert_eq!(stats.peer_count(), 0);
    }

    #[test]
    fn test_chunk_statistics_have_chunk_transition() {
        let mut stats = ChunkStatistics::new(3);
        let peer = peer_id(3);
        let empty = Bitfield::new(3);
        stats.add_peer(peer, &empty);

        assert!(stats.received_have_chunk(&peer, 0));
        assert_eq!(stats.accounted(), 1);
        assert_eq!(stats.rarity(0), 1);

        assert!(stats.received_have_chunk(&peer, 1));
        assert_eq!(stats.rarity(1), 1);

        // Duplicate have is idempotent.
        assert!(!stats.received_have_chunk(&peer, 1));
        assert_eq!(stats.rarity(1), 1);

        // Have chunk 2 completes the peer and removes it from rarity counts.
        assert!(stats.received_have_chunk(&peer, 2));
        assert_eq!(stats.complete(), 1);
        assert_eq!(stats.accounted(), 0);
        assert_eq!(stats.rarity(0), 0);
        assert_eq!(stats.rarity(1), 0);
        assert_eq!(stats.rarity(2), 0);
        assert!(stats.peer_has(&peer, 2));
    }

    #[test]
    fn test_have_none_and_out_of_range_are_safe() {
        let mut stats = ChunkStatistics::new(2);
        let peer = peer_id(4);
        stats.add_peer(peer, &Bitfield::new(2));
        assert_eq!(stats.accounted(), 0);

        assert!(!stats.received_have_chunk(&peer, 99));
        assert_eq!(stats.accounted(), 0);
        assert_eq!(stats.rarity(0), 0);

        stats.received_bitfield(peer, &Bitfield::new(2));
        assert_eq!(stats.accounted(), 0);
        assert_eq!(stats.complete(), 0);
    }

    #[test]
    fn random_have_and_disconnect_sequences_keep_counts_consistent() {
        const PIECES: usize = 8;
        const PEERS: usize = 5;

        let mut state = 0x1234_5678_9ABC_DEF0u64;
        let mut next = || {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            state
        };

        let mut stats = ChunkStatistics::new(PIECES);
        let mut model: HashMap<HashString, Bitfield> = HashMap::new();

        for _ in 0..600 {
            let peer = peer_id((next() % PEERS as u64) as u8 + 1);
            match next() % 5 {
                0 | 1 => {
                    let bits = next() as usize;
                    let mut bf = Bitfield::new(PIECES);
                    for index in 0..PIECES {
                        if bits & (1 << index) != 0 {
                            bf.set(index);
                        }
                    }
                    model.insert(peer, bf.clone());
                    stats.received_bitfield(peer, &bf);
                }
                2 => {
                    let index = (next() as usize) % PIECES;
                    let existed = model.contains_key(&peer);
                    if existed {
                        model.get_mut(&peer).unwrap().set(index);
                        stats.received_have_chunk(&peer, index);
                    }
                }
                3 => {
                    model.remove(&peer);
                    stats.remove_peer(&peer);
                }
                _ => {
                    // Adding an empty peer models a bitfield-less handshake.
                    let bf = model
                        .entry(peer)
                        .or_insert_with(|| Bitfield::new(PIECES))
                        .clone();
                    stats.received_bitfield(peer, &bf);
                }
            }

            let mut expected_counts = vec![0u8; PIECES];
            let mut expected_complete = 0;
            let mut expected_accounted = 0;
            for bf in model.values() {
                if bf.is_all_set() {
                    expected_complete += 1;
                } else if !bf.is_all_unset() {
                    expected_accounted += 1;
                    for index in 0..PIECES {
                        if bf.get(index) {
                            expected_counts[index] += 1;
                        }
                    }
                }
            }

            assert_eq!(stats.complete(), expected_complete);
            assert_eq!(stats.accounted(), expected_accounted);
            for index in 0..PIECES {
                assert_eq!(
                    stats.rarity(index),
                    expected_counts[index],
                    "rarity mismatch at piece {index}"
                );
            }
        }
    }
}
