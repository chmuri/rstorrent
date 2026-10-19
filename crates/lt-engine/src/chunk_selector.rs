use lt_common::bitfield::Bitfield;
use lt_common::collections::PartialQueue;
use lt_common::ranges::Ranges;
use crate::chunk_statistics::ChunkStatistics;

pub const INVALID_CHUNK: u32 = u32::MAX;

/// Rarest-First & Priority-Aware chunk selector.
#[derive(Debug, Clone)]
pub struct ChunkSelector {
    untouched: Bitfield,
    completed: Bitfield,
    high_priority: Ranges<u32>,
    normal_priority: Ranges<u32>,
    shared_queue: PartialQueue,
    position: u32,
    size: usize,
    sequential: bool,
}

impl ChunkSelector {
    pub fn new(size: usize) -> Self {
        let mut untouched = Bitfield::new(size);
        for i in 0..size {
            untouched.set(i);
        }
        let completed = Bitfield::new(size);
        let mut shared_queue = PartialQueue::new();
        shared_queue.enable(32);

        let mut normal_priority = Ranges::new();
        if size > 0 {
            normal_priority.insert(0, size as u32);
        }

        Self {
            untouched,
            completed,
            high_priority: Ranges::new(),
            normal_priority,
            shared_queue,
            position: 0,
            size,
            sequential: false,
        }
    }

    pub fn size(&self) -> usize {
        self.size
    }

    /// Sequential mode picks the lowest available piece index at/after the
    /// current cursor instead of rarest-first.
    pub fn set_sequential(&mut self, value: bool) {
        self.sequential = value;
    }

    pub fn is_sequential(&self) -> bool {
        self.sequential
    }

    pub fn is_empty(&self) -> bool {
        self.size == 0
    }

    pub fn untouched(&self) -> &Bitfield {
        &self.untouched
    }

    pub fn completed(&self) -> &Bitfield {
        &self.completed
    }

    pub fn high_priority(&self) -> &Ranges<u32> {
        &self.high_priority
    }

    pub fn normal_priority(&self) -> &Ranges<u32> {
        &self.normal_priority
    }

    pub fn set_priorities(&mut self, high: Ranges<u32>, normal: Ranges<u32>) {
        self.high_priority = high;
        self.normal_priority = normal;
        self.update_priorities();
    }

    pub fn update_priorities(&mut self) {
        if self.is_empty() {
            return;
        }
        self.shared_queue.clear();
        self.advance_position();
    }

    pub fn using_index(&mut self, index: u32) {
        let idx = index as usize;
        if idx < self.size && self.untouched.get(idx) {
            self.untouched.unset(idx);
            if index == self.position {
                self.advance_position();
            }
        }
    }

    pub fn not_using_index(&mut self, index: u32) {
        let idx = index as usize;
        if idx < self.size && !self.completed.get(idx) {
            self.untouched.set(idx);
            if self.position == INVALID_CHUNK {
                self.position = index;
            }
        }
    }

    pub fn mark_completed(&mut self, index: u32) {
        let idx = index as usize;
        if idx < self.size {
            self.completed.set(idx);
            self.untouched.unset(idx);
            if index == self.position {
                self.advance_position();
            }
        }
    }

    pub fn is_wanted(&self, index: u32) -> bool {
        let idx = index as usize;
        if idx >= self.size || !self.untouched.get(idx) {
            return false;
        }
        self.high_priority.has(index) || self.normal_priority.has(index)
    }

    pub fn received_have_chunk(&mut self, cache: &mut PartialQueue, index: u32, stats: &ChunkStatistics) -> bool {
        if !self.is_wanted(index) {
            return false;
        }
        if cache.is_enabled() {
            cache.insert(stats.rarity(index as usize), index);
        }
        true
    }

    pub fn find(
        &mut self,
        peer_bf: &Bitfield,
        peer_cache: &mut PartialQueue,
        is_seeder: bool,
        stats: &ChunkStatistics,
    ) -> Option<u32> {
        if self.position == INVALID_CHUNK || self.size == 0 {
            return None;
        }

        if self.sequential {
            return self.find_sequential(peer_bf);
        }

        let mut temp_shared = if is_seeder {
            if !self.shared_queue.is_enabled() {
                self.shared_queue.enable(32);
            }
            std::mem::take(&mut self.shared_queue)
        } else {
            if !peer_cache.is_enabled() {
                peer_cache.enable(8);
            }
            PartialQueue::new()
        };

        let queue = if is_seeder {
            &mut temp_shared
        } else {
            peer_cache
        };

        // Check cached queue first
        let mut found = None;
        while queue.prepare_pop() {
            if let Some(pos) = queue.pop() {
                if (pos as usize) < self.size && self.untouched.get(pos as usize) && self.is_wanted(pos) {
                    found = Some(pos);
                    break;
                }
            }
        }

        if found.is_none() {
            queue.clear();

            // 1. Search high priority
            let size_u32 = self.size as u32;
            let pos = self.position;

            self.search_linear(peer_bf, queue, &self.high_priority.clone(), pos, size_u32, stats);
            self.search_linear(peer_bf, queue, &self.high_priority.clone(), 0, pos, stats);

            if queue.prepare_pop() {
                found = queue.pop();
            }

            if found.is_none() {
                // 2. Search normal priority
                queue.clear();
                self.search_linear(peer_bf, queue, &self.normal_priority.clone(), pos, size_u32, stats);
                self.search_linear(peer_bf, queue, &self.normal_priority.clone(), 0, pos, stats);

                if queue.prepare_pop() {
                    found = queue.pop();
                }
            }
        }

        if is_seeder {
            self.shared_queue = temp_shared;
        }

        found
    }

    fn find_sequential(&self, peer_bf: &Bitfield) -> Option<u32> {
        let size = self.size as u32;
        let start = if self.position == INVALID_CHUNK || self.position >= size {
            0
        } else {
            self.position
        };

        for ranges in [&self.high_priority, &self.normal_priority] {
            if let Some(index) = self.find_next_available(ranges, start, size, peer_bf) {
                return Some(index);
            }
            if start > 0 {
                if let Some(index) = self.find_next_available(ranges, 0, start, peer_bf) {
                    return Some(index);
                }
            }
        }

        None
    }

    fn find_next_available(
        &self,
        ranges: &Ranges<u32>,
        first: u32,
        last: u32,
        peer_bf: &Bitfield,
    ) -> Option<u32> {
        if first >= last {
            return None;
        }

        for &(r_start, r_end) in ranges.iter() {
            if r_start >= last {
                break;
            }
            let start = first.max(r_start);
            let end = last.min(r_end);

            for idx in start..end {
                let i = idx as usize;
                if i < self.size && self.untouched.get(i) && peer_bf.get(i) {
                    return Some(idx);
                }
            }
        }

        None
    }

    fn search_linear(
        &self,
        peer_bf: &Bitfield,
        queue: &mut PartialQueue,
        ranges: &Ranges<u32>,
        first: u32,
        last: u32,
        stats: &ChunkStatistics,
    ) {
        if first >= last || queue.is_full() {
            return;
        }

        for &(r_start, r_end) in ranges.iter() {
            if r_start >= last {
                break;
            }
            let start = first.max(r_start);
            let end = last.min(r_end);

            for idx in start..end {
                let i = idx as usize;
                if i < self.size && self.untouched.get(i) && peer_bf.get(i) {
                    let rarity = stats.rarity(i);
                    if !queue.insert(rarity, idx) && queue.is_full() {
                        return;
                    }
                }
            }
        }
    }

    fn advance_position(&mut self) {
        if self.is_empty() {
            self.position = INVALID_CHUNK;
            return;
        }

        let start = if self.position == INVALID_CHUNK || self.position as usize >= self.size {
            0
        } else {
            self.position
        };

        let size_u32 = self.size as u32;

        let pos = self.find_next_wanted(&self.high_priority, start, size_u32)
            .or_else(|| self.find_next_wanted(&self.high_priority, 0, start))
            .or_else(|| self.find_next_wanted(&self.normal_priority, start, size_u32))
            .or_else(|| self.find_next_wanted(&self.normal_priority, 0, start));

        self.position = pos.unwrap_or(INVALID_CHUNK);
    }

    fn find_next_wanted(&self, ranges: &Ranges<u32>, first: u32, last: u32) -> Option<u32> {
        if first >= last {
            return None;
        }

        for &(r_start, r_end) in ranges.iter() {
            if r_start >= last {
                break;
            }
            let start = first.max(r_start);
            let end = last.min(r_end);

            for idx in start..end {
                let i = idx as usize;
                if i < self.size && self.untouched.get(i) {
                    return Some(idx);
                }
            }
        }

        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chunk_selector_basic() {
        let mut selector = ChunkSelector::new(10);
        assert_eq!(selector.size(), 10);
        assert!(!selector.is_empty());

        let stats = ChunkStatistics::new(10);
        let mut peer_bf = Bitfield::new(10);
        peer_bf.set(2);
        peer_bf.set(5);

        let mut peer_cache = PartialQueue::new();

        // Should find index 2 or 5
        let picked = selector.find(&peer_bf, &mut peer_cache, false, &stats);
        assert!(picked == Some(2) || picked == Some(5));
        let picked_idx = picked.unwrap();

        // Mark it as downloading
        selector.using_index(picked_idx);
        assert!(!selector.untouched().get(picked_idx as usize));

        // Next find should pick the other one
        let next_picked = selector.find(&peer_bf, &mut peer_cache, false, &stats);
        assert!(next_picked.is_some() && next_picked != picked);

        // Deselect first picked
        selector.not_using_index(picked_idx);
        assert!(selector.untouched().get(picked_idx as usize));
    }

    #[test]
    fn test_chunk_selector_rarest_first() {
        let mut selector = ChunkSelector::new(5);
        let mut stats = ChunkStatistics::new(5);
        let mut peer_bf = Bitfield::new(5);
        for i in 0..5 {
            peer_bf.set(i);
        }

        // Set chunk 1 as common (rarity 5); the other pieces stay rare.
        for index in 0..5u8 {
            let peer_id = lt_common::hash_string::HashString::new([index + 1; 20]);
            let mut bf = Bitfield::new(5);
            bf.set(1);
            stats.add_peer(peer_id, &bf);
        }

        let mut peer_cache = PartialQueue::new();
        let picked = selector.find(&peer_bf, &mut peer_cache, false, &stats);
        // Rarest pieces (rarity 0: chunks 0, 2, 3, 4) should be picked before chunk 1 (rarity 5)
        assert!(picked.is_some());
        assert_ne!(picked.unwrap(), 1);
    }
}
