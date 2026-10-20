use lt_common::hash_string::HashString;
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChunkStatus {
    /// Not offered to any peer yet.
    Unsent,
    /// Currently offered to one peer.
    Offered(HashString),
    /// At least one peer is known to have the piece.
    Done,
}

/// BEP 16 super-seeding state.
///
/// The seeder offers one piece at a time to a peer and only marks it done when
/// that peer (or another peer) reports it through a `have` message. This keeps
/// the number of duplicate uploads low while still allowing the swarm to
/// propagate each piece.
#[derive(Clone, Debug)]
pub struct InitialSeeding {
    total_chunks: usize,
    chunks_left: usize,
    chunk_states: Vec<ChunkStatus>,
    peer_chunks: HashMap<HashString, Vec<u32>>,
}

impl InitialSeeding {
    pub const NO_OFFER: u32 = u32::MAX;

    pub fn new(total_chunks: usize) -> Self {
        Self {
            total_chunks,
            chunks_left: total_chunks,
            chunk_states: vec![ChunkStatus::Unsent; total_chunks],
            peer_chunks: HashMap::new(),
        }
    }

    pub fn is_complete(&self) -> bool {
        self.chunks_left == 0
    }

    pub fn chunks_left(&self) -> usize {
        self.chunks_left
    }

    pub fn total_chunks(&self) -> usize {
        self.total_chunks
    }

    pub fn chunks_done(&self) -> usize {
        self.total_chunks.saturating_sub(self.chunks_left)
    }

    /// Returns the piece currently assigned to `peer_id`, if any.
    pub fn assigned_chunk(&self, peer_id: &HashString) -> Option<u32> {
        self.peer_chunks
            .get(peer_id)
            .and_then(|chunks| chunks.first().copied())
    }

    /// Whether `index` is the piece currently offered to `peer_id`.
    pub fn is_offered(&self, peer_id: &HashString, index: u32) -> bool {
        index < self.total_chunks as u32
            && matches!(self.chunk_states[index as usize], ChunkStatus::Offered(owner) if owner == *peer_id)
    }

    /// Records that `peer_id` has `chunk_index`.
    ///
    /// Returns `true` when this transitions the piece to `Done` and therefore
    /// decreases `chunks_left()`.
    pub fn chunk_seen(&mut self, chunk_index: u32, peer_id: &HashString) -> bool {
        let idx = chunk_index as usize;
        if idx >= self.total_chunks {
            return false;
        }

        match self.chunk_states[idx] {
            ChunkStatus::Done => false,
            ChunkStatus::Offered(owner) => {
                // Either the assigned peer completed the download or a
                // different peer confirmed it already has the piece. In both
                // cases the swarm has the piece and it no longer needs an
                // initial-seed offer.
                self.chunk_states[idx] = ChunkStatus::Done;
                self.chunks_left = self.chunks_left.saturating_sub(1);
                self.remove_assignment(owner, chunk_index);
                if owner != *peer_id {
                    self.remove_assignment(*peer_id, chunk_index);
                }
                true
            }
            ChunkStatus::Unsent => {
                // A peer connected with the piece before we offered it. There
                // is no reason to upload it again from this seeder.
                self.chunk_states[idx] = ChunkStatus::Done;
                self.chunks_left = self.chunks_left.saturating_sub(1);
                true
            }
        }
    }

    /// Offers the next unsent piece to `peer_id`.
    ///
    /// A peer can only have one outstanding offer at a time. `index_done` is
    /// the last piece the peer reported as complete; it is accepted for API
    /// parity and used to release the assignment before selecting the next
    /// piece.
    pub fn chunk_offer(&mut self, peer_id: &HashString, index_done: Option<u32>) -> u32 {
        if let Some(done) = index_done {
            let idx = done as usize;
            if idx < self.total_chunks {
                if let ChunkStatus::Offered(owner) = self.chunk_states[idx] {
                    if owner == *peer_id {
                        self.chunk_states[idx] = ChunkStatus::Done;
                        self.chunks_left = self.chunks_left.saturating_sub(1);
                        self.remove_assignment(*peer_id, done);
                    }
                }
            }
        }

        if self.assigned_chunk(peer_id).is_some() {
            return Self::NO_OFFER;
        }

        for (index, status) in self.chunk_states.iter_mut().enumerate() {
            if *status == ChunkStatus::Unsent {
                *status = ChunkStatus::Offered(*peer_id);
                self.peer_chunks
                    .entry(*peer_id)
                    .or_default()
                    .push(index as u32);
                return index as u32;
            }
        }

        Self::NO_OFFER
    }

    /// Releases every piece assigned to a disconnected peer.
    ///
    /// Released pieces become `Unsent` again and can be offered to a different
    /// peer. Already distributed pieces stay `Done`.
    pub fn release_peer(&mut self, peer_id: &HashString) {
        let released = self.peer_chunks.remove(peer_id).unwrap_or_default();
        for index in released {
            let idx = index as usize;
            if idx < self.total_chunks {
                if let ChunkStatus::Offered(owner) = self.chunk_states[idx] {
                    if owner == *peer_id {
                        self.chunk_states[idx] = ChunkStatus::Unsent;
                    }
                }
            }
        }
    }

    /// Releases a single assignment (used when a transfer fails or is rejected).
    pub fn release_chunk(&mut self, chunk_index: u32) {
        let idx = chunk_index as usize;
        if idx >= self.total_chunks {
            return;
        }
        if let ChunkStatus::Offered(owner) = self.chunk_states[idx] {
            self.chunk_states[idx] = ChunkStatus::Unsent;
            self.remove_assignment(owner, chunk_index);
        }
    }

    fn remove_assignment(&mut self, peer_id: HashString, chunk_index: u32) {
        if let Some(chunks) = self.peer_chunks.get_mut(&peer_id) {
            chunks.retain(|index| *index != chunk_index);
            if chunks.is_empty() {
                self.peer_chunks.remove(&peer_id);
            }
        }
    }
}
