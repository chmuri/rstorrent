pub const ORDER_BASE: u32 = 1 << 30;
pub const UNLIMITED_SLOTS: u32 = u32::MAX;

pub const FLAG_UNCHOKE_ALL_NEW: u32 = 0x1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Heuristics {
    #[default]
    UploadLeech,
    UploadSeed,
    UploadLeechExperimental,
    DownloadLeech,
    RateBasedBitTyrant,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WeightedPeer {
    pub peer_id: usize,
    pub weight: u32,
    pub rate_up: u64,
    pub rate_down: u64,
    pub is_snubbed: bool,
    pub is_preferred: bool,
    pub is_local_unchoked: bool,
    pub time_last_choke: u64,
}

impl WeightedPeer {
    pub fn new(peer_id: usize) -> Self {
        Self {
            peer_id,
            weight: 0,
            rate_up: 0,
            rate_down: 0,
            is_snubbed: false,
            is_preferred: false,
            is_local_unchoked: false,
            time_last_choke: 0,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ChokeEntry {
    pub max_slots: u32,
    pub min_slots: u32,
    pub queued: Vec<WeightedPeer>,
    pub unchoked: Vec<WeightedPeer>,
}

impl Default for ChokeEntry {
    fn default() -> Self {
        Self {
            max_slots: UNLIMITED_SLOTS,
            min_slots: 0,
            queued: Vec::new(),
            unchoked: Vec::new(),
        }
    }
}

impl ChokeEntry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn size_connections(&self) -> usize {
        self.queued.len() + self.unchoked.len()
    }
}

/// Choke queue implementing BitTorrent unchoke slot management and tit-for-tat heuristics.
#[derive(Debug, Clone)]
pub struct ChokeQueue {
    flags: u32,
    heuristics: Heuristics,
    max_unchoked: u32,
    currently_unchoked: u32,
    currently_queued: u32,
    entries: Vec<ChokeEntry>,
}

impl Default for ChokeQueue {
    fn default() -> Self {
        Self::new(0)
    }
}

impl ChokeQueue {
    pub fn new(flags: u32) -> Self {
        Self {
            flags,
            heuristics: Heuristics::UploadLeech,
            max_unchoked: UNLIMITED_SLOTS,
            currently_unchoked: 0,
            currently_queued: 0,
            entries: Vec::new(),
        }
    }

    pub fn flags(&self) -> u32 {
        self.flags
    }

    pub fn heuristics(&self) -> Heuristics {
        self.heuristics
    }

    pub fn set_heuristics(&mut self, h: Heuristics) {
        self.heuristics = h;
    }

    pub fn max_unchoked(&self) -> u32 {
        self.max_unchoked
    }

    pub fn set_max_unchoked(&mut self, max: u32) {
        self.max_unchoked = max;
    }

    pub fn size_unchoked(&self) -> u32 {
        self.currently_unchoked
    }

    pub fn size_queued(&self) -> u32 {
        self.currently_queued
    }

    pub fn size_total(&self) -> u32 {
        self.currently_unchoked + self.currently_queued
    }

    pub fn is_full(&self) -> bool {
        self.max_unchoked != UNLIMITED_SLOTS && self.currently_unchoked >= self.max_unchoked
    }

    pub fn entries(&self) -> &[ChokeEntry] {
        &self.entries
    }

    pub fn entries_mut(&mut self) -> &mut [ChokeEntry] {
        &mut self.entries
    }

    pub fn add_entry(&mut self, entry: ChokeEntry) {
        self.entries.push(entry);
    }

    pub fn max_alternate(&self) -> u32 {
        if self.currently_unchoked < 31 {
            self.currently_unchoked.saturating_add(7) / 8
        } else {
            self.currently_unchoked.saturating_add(9) / 10
        }
    }

    pub fn set_queued(&mut self, entry_idx: usize, peer: WeightedPeer) {
        if entry_idx < self.entries.len() {
            let entry = &mut self.entries[entry_idx];
            if !entry.queued.iter().any(|p| p.peer_id == peer.peer_id)
                && !entry.unchoked.iter().any(|p| p.peer_id == peer.peer_id)
            {
                entry.queued.push(peer);
                self.currently_queued = self.currently_queued.saturating_add(1);
            }
        }
    }

    pub fn set_not_queued(&mut self, entry_idx: usize, peer_id: usize) {
        if entry_idx < self.entries.len() {
            let entry = &mut self.entries[entry_idx];
            if let Some(pos) = entry.queued.iter().position(|p| p.peer_id == peer_id) {
                entry.queued.swap_remove(pos);
                self.currently_queued = self.currently_queued.saturating_sub(1);
            }
            if let Some(pos) = entry.unchoked.iter().position(|p| p.peer_id == peer_id) {
                entry.unchoked.swap_remove(pos);
                self.currently_unchoked = self.currently_unchoked.saturating_sub(1);
            }
        }
    }

    pub fn disconnected(&mut self, peer_id: usize) {
        for entry in &mut self.entries {
            if let Some(pos) = entry.unchoked.iter().position(|p| p.peer_id == peer_id) {
                entry.unchoked.swap_remove(pos);
                self.currently_unchoked = self.currently_unchoked.saturating_sub(1);
                return;
            }
            if let Some(pos) = entry.queued.iter().position(|p| p.peer_id == peer_id) {
                entry.queued.swap_remove(pos);
                self.currently_queued = self.currently_queued.saturating_sub(1);
                return;
            }
        }
    }

    /// Rebalances unchoke slots across groups with custom quota.
    ///
    /// The quota is the global number of peers this queue may keep unchoked.
    /// It is applied on top of each entry's `min_slots`/`max_slots`, and
    /// lowering the quota chokes the lowest-weighted peers immediately
    /// instead of waiting for a later cycle.
    pub fn balance_with_quota(&mut self, quota: u32) -> (Vec<usize>, Vec<usize>) {
        let mut newly_unchoked = Vec::new();
        let mut newly_choked = Vec::new();
        let limited = quota != UNLIMITED_SLOTS;

        self.calculate_weights();

        // 1. Enforce per-entry maximums. Unchoked peers are sorted by weight
        //    ascending, so index 0 is the least useful peer.
        for entry in &mut self.entries {
            let max_slots = entry.max_slots as usize;
            while entry.unchoked.len() > max_slots {
                let choked = entry.unchoked.remove(0);
                newly_choked.push(choked.peer_id);
                self.currently_unchoked = self.currently_unchoked.saturating_sub(1);
                self.currently_queued = self.currently_queued.saturating_add(1);
                entry.queued.push(choked);
            }
        }

        // 2. Enforce the global quota before filling minimums. This also
        //    handles a quota that shrank since the previous tick.
        if limited {
            while self.currently_unchoked > quota {
                let mut best: Option<(usize, u32)> = None;

                for (index, entry) in self.entries.iter().enumerate() {
                    if let Some(peer) = entry.unchoked.first() {
                        if best.map(|(_, weight)| peer.weight < weight).unwrap_or(true) {
                            best = Some((index, peer.weight));
                        }
                    }
                }

                let Some((entry_index, _)) = best else {
                    break;
                };

                let choked = self.entries[entry_index].unchoked.remove(0);
                newly_choked.push(choked.peer_id);
                self.currently_unchoked = self.currently_unchoked.saturating_sub(1);
                self.currently_queued = self.currently_queued.saturating_add(1);
                self.entries[entry_index].queued.push(choked);
            }
        }

        // 3. Satisfy entry minimums (as far as the global quota allows).
        for index in 0..self.entries.len() {
            let min_slots = self.entries[index]
                .min_slots
                .min(self.entries[index].max_slots) as usize;

            while self.entries[index].unchoked.len() < min_slots {
                if limited && self.currently_unchoked >= quota {
                    break;
                }

                let Some(peer) = self.pop_highest_queued(index) else {
                    break;
                };

                newly_unchoked.push(peer.peer_id);
                self.currently_queued = self.currently_queued.saturating_sub(1);
                self.currently_unchoked = self.currently_unchoked.saturating_add(1);
                self.entries[index].unchoked.push(peer);
            }
        }

        // 4. Fill the remaining global quota with the highest-weighted queued
        //    peers that still have entry capacity.
        while !limited || self.currently_unchoked < quota {
            let mut best_entry = None;
            let mut best_weight = 0;
            let mut found = false;

            for (index, entry) in self.entries.iter().enumerate() {
                if entry.unchoked.len() >= entry.max_slots as usize {
                    continue;
                }
                if let Some(weight) = entry.queued.iter().map(|peer| peer.weight).max() {
                    if !found || weight > best_weight {
                        best_entry = Some(index);
                        best_weight = weight;
                        found = true;
                    }
                }
            }

            let Some(entry_index) = best_entry else {
                break;
            };

            let Some(peer) = self.pop_highest_queued(entry_index) else {
                // Defensive: no candidate despite the weight check.
                break;
            };

            newly_unchoked.push(peer.peer_id);
            self.currently_queued = self.currently_queued.saturating_sub(1);
            self.currently_unchoked = self.currently_unchoked.saturating_add(1);
            self.entries[entry_index].unchoked.push(peer);
        }

        (newly_unchoked, newly_choked)
    }

    /// Removes and returns the highest-weighted queued peer of an entry.
    fn pop_highest_queued(&mut self, entry_index: usize) -> Option<WeightedPeer> {
        let entry = self.entries.get_mut(entry_index)?;
        let position = entry
            .queued
            .iter()
            .enumerate()
            .max_by_key(|(position, peer)| (peer.weight, *position))
            .map(|(position, _)| position)?;
        Some(entry.queued.remove(position))
    }

    /// Rebalances unchoke slots across groups using max_unchoked.
    pub fn balance(&mut self) -> (Vec<usize>, Vec<usize>) {
        let quota = self.max_unchoked;
        self.balance_with_quota(quota)
    }

    /// Periodic unchoke cycle: re-weights peers, rotates a few optimistic
    /// unchoke slots and then balances against `quota`.
    ///
    /// Up to `max_alternate()` highest-weighted queued peers are unchoked
    /// first. The following balance pass chokes the lowest-weighted old
    /// unchoked peers when the quota is exceeded, which actually rotates the
    /// optimistic slots instead of re-selecting the same peer.
    pub fn cycle(&mut self, quota: u32) -> (Vec<usize>, Vec<usize>) {
        let quota = if self.max_unchoked == UNLIMITED_SLOTS {
            quota
        } else {
            quota.min(self.max_unchoked)
        };

        self.calculate_weights();

        let alternate = self.max_alternate() as usize;
        let mut newly_unchoked = Vec::new();
        let mut newly_choked = Vec::new();

        if alternate > 0 && quota != 0 {
            for _ in 0..alternate {
                let mut best_entry = None;
                let mut best_weight = 0;
                let mut found = false;

                for (index, entry) in self.entries.iter().enumerate() {
                    if entry.max_slots == 0 || entry.queued.is_empty() {
                        continue;
                    }
                    if let Some(weight) = entry.queued.iter().map(|peer| peer.weight).max() {
                        if !found || weight > best_weight {
                            best_entry = Some(index);
                            best_weight = weight;
                            found = true;
                        }
                    }
                }

                let Some(entry_index) = best_entry else {
                    break;
                };
                let Some(peer) = self.pop_highest_queued(entry_index) else {
                    break;
                };

                // If the entry is already at its slot cap, rotate out the
                // lowest-weight unchoked peer to make room for this queued
                // peer (optimistic unchoke rotation).
                let entry = &mut self.entries[entry_index];
                if entry.unchoked.len() >= entry.max_slots as usize {
                    if let Some((pos, _)) = entry
                        .unchoked
                        .iter()
                        .enumerate()
                        .min_by_key(|(_, existing)| existing.weight)
                    {
                        let removed = entry.unchoked.remove(pos);
                        newly_choked.push(removed.peer_id);
                        self.currently_unchoked = self.currently_unchoked.saturating_sub(1);
                    }
                }

                newly_unchoked.push(peer.peer_id);
                self.currently_queued = self.currently_queued.saturating_sub(1);
                self.currently_unchoked = self.currently_unchoked.saturating_add(1);
                entry.unchoked.push(peer);
            }
        }

        let (unchoked, choked) = self.balance_with_quota(quota);
        newly_unchoked.extend(unchoked);
        newly_choked.extend(choked);

        // A peer may be touched twice during one cycle (optimistic unchoke
        // followed by a quota choke). Report the final state only so the
        // caller never applies contradictory choke decisions.
        let final_unchoked: Vec<usize> = self
            .entries
            .iter()
            .flat_map(|entry| entry.unchoked.iter().map(|peer| peer.peer_id))
            .collect();
        newly_unchoked.retain(|peer| final_unchoked.contains(peer));
        newly_choked.retain(|peer| !final_unchoked.contains(peer));

        (newly_unchoked, newly_choked)
    }

    fn calculate_weights(&mut self) {
        let h = self.heuristics;
        for entry in &mut self.entries {
            for peer in &mut entry.unchoked {
                peer.weight = Self::calculate_peer_weight(h, peer, true);
            }
            entry.unchoked.sort_by_key(|p| p.weight);

            for peer in &mut entry.queued {
                peer.weight = Self::calculate_peer_weight(h, peer, false);
            }
            entry.queued.sort_by_key(|p| p.weight);
        }
    }

    fn calculate_peer_weight(
        heuristics: Heuristics,
        peer: &WeightedPeer,
        is_unchoked: bool,
    ) -> u32 {
        match heuristics {
            Heuristics::UploadLeech => {
                if is_unchoked {
                    // Choke weighting: lower download rate gets choked first
                    let rate = (peer.rate_down / 16) as u32;
                    ORDER_BASE.saturating_sub(1).saturating_sub(rate)
                } else {
                    // Unchoke weighting: fast downloaders get high weight
                    if peer.is_local_unchoked {
                        let rate = (peer.rate_down / 16) as u32;
                        if rate < 128 {
                            rate
                        } else {
                            3 * ORDER_BASE + rate
                        }
                    } else {
                        // Optimistic unchoke
                        let order = 1 + (peer.is_preferred as u32);
                        order * ORDER_BASE + (peer.peer_id as u32 % 1024)
                    }
                }
            }
            Heuristics::UploadSeed => {
                if is_unchoked {
                    let rate = (peer.rate_up / 16) as u32;
                    ORDER_BASE.saturating_sub(1).saturating_sub(rate)
                } else {
                    let order = peer.is_preferred as u32;
                    order * ORDER_BASE + (peer.peer_id as u32 % 1024)
                }
            }
            Heuristics::UploadLeechExperimental => {
                let multiplier = 1 + 3 * (peer.is_preferred as u32);
                let down = (peer.rate_down / 64) as u32;
                let up = (peer.rate_up / 256) as u32;
                if is_unchoked {
                    ORDER_BASE
                        .saturating_sub(1)
                        .saturating_sub((down + up) * multiplier)
                } else {
                    ORDER_BASE + down * multiplier
                }
            }
            Heuristics::DownloadLeech => {
                let rate = (peer.rate_down / 16) as u32;
                if is_unchoked {
                    ORDER_BASE.saturating_sub(1).saturating_sub(rate)
                } else {
                    rate
                }
            }
            Heuristics::RateBasedBitTyrant => {
                if is_unchoked {
                    // Lower efficiency (D_i / max(U_i, 1)) choked first
                    let ratio = (peer.rate_down.saturating_mul(1000) / peer.rate_up.max(1))
                        .min((ORDER_BASE - 1) as u64) as u32;
                    ORDER_BASE.saturating_sub(1).saturating_sub(ratio)
                } else {
                    if peer.is_local_unchoked {
                        let ratio = (peer.rate_down.saturating_mul(1000) / peer.rate_up.max(1))
                            .min((ORDER_BASE - 1) as u64)
                            as u32;
                        3 * ORDER_BASE + ratio
                    } else {
                        // Optimistic unchoke with newcomer boost (peers not yet choked or newly connected)
                        let newcomer_boost = if peer.time_last_choke == 0 { 2 } else { 1 };
                        let order = newcomer_boost + (peer.is_preferred as u32);
                        order * ORDER_BASE + (peer.peer_id as u32 % 1024)
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_choke_queue_basic() {
        let mut queue = ChokeQueue::new(0);
        queue.set_max_unchoked(2);

        let mut entry = ChokeEntry::new();
        entry.max_slots = 4;
        queue.add_entry(entry);

        let mut p1 = WeightedPeer::new(1);
        p1.rate_down = 10000;
        p1.is_local_unchoked = true;

        let mut p2 = WeightedPeer::new(2);
        p2.rate_down = 50000;
        p2.is_local_unchoked = true;

        let mut p3 = WeightedPeer::new(3);
        p3.rate_down = 1000;

        queue.set_queued(0, p1);
        queue.set_queued(0, p2);
        queue.set_queued(0, p3);

        assert_eq!(queue.size_queued(), 3);
        assert_eq!(queue.size_unchoked(), 0);

        // Run balance with max_unchoked = 2
        let (unchoked, choked) = queue.balance();
        assert_eq!(unchoked.len(), 2);
        assert!(choked.is_empty());
        assert_eq!(queue.size_unchoked(), 2);
        assert_eq!(queue.size_queued(), 1);

        // Fastest downloader (p2 = peer_id 2) should be unchoked
        assert!(unchoked.contains(&2));

        // Disconnect peer 2
        queue.disconnected(2);
        assert_eq!(queue.size_unchoked(), 1);

        // Balance again should unchoke remaining peer
        let (unchoked2, _) = queue.balance();
        assert_eq!(unchoked2.len(), 1);
        assert_eq!(queue.size_unchoked(), 2);
    }

    #[test]
    fn test_balance_chokes_when_quota_shrinks() {
        let mut queue = ChokeQueue::new(0);
        queue.set_max_unchoked(3);
        queue.add_entry(ChokeEntry::new());

        for peer in 0..5 {
            queue.set_queued(0, WeightedPeer::new(peer));
        }

        queue.balance();
        assert_eq!(queue.size_unchoked(), 3);

        let (_, choked) = queue.balance_with_quota(1);
        assert_eq!(queue.size_unchoked(), 1);
        assert_eq!(choked.len(), 2);
        assert_eq!(queue.size_queued(), 4);
    }

    #[test]
    fn test_entry_min_and_max_are_bounded_by_global_quota() {
        let mut queue = ChokeQueue::new(0);
        queue.set_max_unchoked(10);

        let mut entry = ChokeEntry::new();
        entry.min_slots = 3;
        entry.max_slots = 4;
        queue.add_entry(entry);

        for peer in 0..6 {
            queue.set_queued(0, WeightedPeer::new(peer));
        }

        queue.balance_with_quota(2);
        assert_eq!(queue.size_unchoked(), 2);

        queue.balance_with_quota(4);
        assert_eq!(queue.size_unchoked(), 4);
        assert_eq!(queue.size_queued(), 2);
    }

    #[test]
    fn test_cycle_rotates_but_respects_minimums() {
        let mut queue = ChokeQueue::new(0);
        queue.set_max_unchoked(3);

        let mut entry = ChokeEntry::new();
        entry.min_slots = 2;
        entry.max_slots = 3;
        queue.add_entry(entry);

        for peer in 0..5 {
            queue.set_queued(0, WeightedPeer::new(peer));
        }

        queue.balance_with_quota(3);
        assert_eq!(queue.size_unchoked(), 3);

        let (unchoked, choked) = queue.cycle(3);
        assert_eq!(queue.size_unchoked(), 3);
        assert!(!choked.is_empty());
        assert_eq!(unchoked.len(), choked.len());

        // An empty queue is a no-op and must not panic.
        let mut empty = ChokeQueue::new(0);
        empty.add_entry(ChokeEntry::new());
        let (u, c) = empty.cycle(UNLIMITED_SLOTS);
        assert!(u.is_empty() && c.is_empty());
        assert_eq!(empty.size_unchoked(), 0);
    }

    #[test]
    fn test_unlimited_quota_unchokes_everything() {
        let mut queue = ChokeQueue::new(FLAG_UNCHOKE_ALL_NEW);
        queue.add_entry(ChokeEntry::new());
        for peer in 0..4 {
            queue.set_queued(0, WeightedPeer::new(peer));
        }

        let (unchoked, choked) = queue.balance_with_quota(UNLIMITED_SLOTS);
        assert_eq!(unchoked.len(), 4);
        assert!(choked.is_empty());
        assert_eq!(queue.size_unchoked(), 4);
        assert_eq!(queue.size_queued(), 0);
    }
}
