//! Global resource arbitration across choke groups.
//!
//! Ports the legacy `torrent::ResourceManager`: downloads belong to a group
//! (with a priority weight) and the manager distributes the global upload and
//! download unchoke slots across those groups.
//!
//! The allocator first satisfies every group's minimum, then shares the
//! remaining slots according to the group weights. A group that has no
//! interested peers never consumes slots, and `UNLIMITED_SLOTS` (the legacy
//! `-1` sentinel) bypasses the global cap without overflowing the counters.

use std::collections::HashSet;

use lt_common::hash_string::HashString;

use crate::choke_group::{ChokeGroup, RatioConfig};
use crate::choke_queue::UNLIMITED_SLOTS;

/// Per-group choke changes produced by one balance/cycle pass.
///
/// Peer identifiers use the same numeric form as [`crate::download::Download`]
/// (`(peer_id[0] << 8) | peer_id[1]`) so the engine can apply the result to
/// the live connection list.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GroupChokeChanges {
    pub group: usize,
    pub up_unchoked: Vec<usize>,
    pub up_choked: Vec<usize>,
    pub down_unchoked: Vec<usize>,
    pub down_choked: Vec<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceEntry {
    pub download_id: HashString,
    pub priority: u16,
    pub group: u16,
}

impl ResourceEntry {
    pub fn new(download_id: HashString, priority: u16, group: u16) -> Self {
        Self {
            download_id,
            priority,
            group,
        }
    }
}

#[derive(Debug)]
pub struct ResourceManager {
    groups: Vec<ChokeGroup>,
    entries: Vec<ResourceEntry>,
    active: HashSet<HashString>,
    max_upload_unchoked: u32,
    max_download_unchoked: u32,
    min_upload_unchoked: u32,
    min_download_unchoked: u32,
    currently_upload_unchoked: u32,
    currently_download_unchoked: u32,
    tick_count: u64,
    last_changes: Vec<GroupChokeChanges>,
}

impl Default for ResourceManager {
    fn default() -> Self {
        Self::new()
    }
}

impl ResourceManager {
    pub fn new() -> Self {
        Self {
            groups: vec![ChokeGroup::new("global")],
            entries: Vec::new(),
            active: HashSet::new(),
            max_upload_unchoked: 50,
            max_download_unchoked: 50,
            min_upload_unchoked: 0,
            min_download_unchoked: 0,
            currently_upload_unchoked: 0,
            currently_download_unchoked: 0,
            tick_count: 0,
            last_changes: Vec::new(),
        }
    }

    pub fn groups(&self) -> &[ChokeGroup] {
        &self.groups
    }

    pub fn groups_mut(&mut self) -> &mut [ChokeGroup] {
        &mut self.groups
    }

    pub fn group_count(&self) -> usize {
        self.groups.len()
    }

    /// Adds a group and returns its index. Returns the existing index if the
    /// name is already present.
    pub fn push_group(&mut self, name: impl Into<String>) -> usize {
        let name = name.into();
        if let Some(index) = self.group_index_of(&name) {
            return index;
        }

        self.groups.push(ChokeGroup::new(name));
        self.groups.len() - 1
    }

    /// Removes a non-global group and moves its downloads to the global group.
    ///
    /// Group indexes after `index` shift down, matching the legacy vector
    /// storage. The global group (index 0) can never be removed.
    pub fn remove_group(&mut self, index: usize) -> Result<ChokeGroup, String> {
        if index == 0 {
            return Err("Cannot remove the default choke group.".to_string());
        }
        if index >= self.groups.len() {
            return Err("Choke group not found.".to_string());
        }

        let removed = self.groups.remove(index);
        for entry in &mut self.entries {
            let group = entry.group as usize;
            if group == index {
                entry.group = 0;
            } else if group > index {
                entry.group = (group - 1) as u16;
            }
        }
        Ok(removed)
    }

    pub fn remove_group_by_name(&mut self, name: &str) -> Result<ChokeGroup, String> {
        let index = self
            .group_index_of(name)
            .ok_or_else(|| "Choke group not found.".to_string())?;
        self.remove_group(index)
    }

    pub fn group_at(&self, index: usize) -> Option<&ChokeGroup> {
        self.groups.get(index)
    }

    pub fn group_at_mut(&mut self, index: usize) -> Option<&mut ChokeGroup> {
        self.groups.get_mut(index)
    }

    pub fn group_at_name(&self, name: &str) -> Option<&ChokeGroup> {
        self.groups.iter().find(|group| group.name() == name)
    }

    pub fn group_at_name_mut(&mut self, name: &str) -> Option<&mut ChokeGroup> {
        self.groups.iter_mut().find(|group| group.name() == name)
    }

    pub fn group_index_of(&self, name: &str) -> Option<usize> {
        self.groups.iter().position(|group| group.name() == name)
    }

    pub fn rename_group(&mut self, index: usize, name: impl Into<String>) -> bool {
        match self.groups.get_mut(index) {
            Some(group) => {
                group.set_name(name);
                true
            }
            None => false,
        }
    }

    pub fn entries(&self) -> &[ResourceEntry] {
        &self.entries
    }

    pub fn entries_mut(&mut self) -> &mut [ResourceEntry] {
        &mut self.entries
    }

    pub fn entry_count(&self) -> usize {
        self.entries.len()
    }

    pub fn entry(&self, download_id: &HashString) -> Option<&ResourceEntry> {
        self.entries
            .iter()
            .find(|entry| entry.download_id == *download_id)
    }

    pub fn entry_mut(&mut self, download_id: &HashString) -> Option<&mut ResourceEntry> {
        self.entries
            .iter_mut()
            .find(|entry| entry.download_id == *download_id)
    }

    /// Number of downloads registered in a group.
    pub fn group_entry_count(&self, group: usize) -> usize {
        self.entries
            .iter()
            .filter(|entry| entry.group as usize == group)
            .count()
    }

    /// Downloads registered in a group.
    pub fn group_entries(&self, group: usize) -> impl Iterator<Item = &ResourceEntry> + '_ {
        self.entries
            .iter()
            .filter(move |entry| entry.group as usize == group)
    }

    pub fn group_entry_ids(&self, group: usize) -> Vec<HashString> {
        self.group_entries(group)
            .map(|entry| entry.download_id)
            .collect()
    }

    /// Marks whether a registered download is currently active. Stopped
    /// downloads remain group members but do not request slots.
    pub fn set_entry_active(&mut self, download_id: &HashString, active: bool) -> bool {
        if self.entry(download_id).is_none() {
            return false;
        }

        if active {
            self.active.insert(*download_id);
        } else {
            self.active.remove(download_id);
        }
        true
    }

    pub fn is_entry_active(&self, download_id: &HashString) -> bool {
        self.active.contains(download_id)
    }

    pub fn active_entry_count(&self) -> usize {
        self.active.len()
    }

    /// Inserts or updates a download. Returns `true` if a new entry was added.
    ///
    /// The group index is clamped to the global group when it is out of range
    /// so a stale index can never panic the allocator.
    pub fn insert_entry(
        &mut self,
        download_id: HashString,
        priority: u16,
        group: u16,
    ) -> bool {
        let group = if (group as usize) < self.groups.len() {
            group
        } else {
            0
        };

        if let Some(existing) = self
            .entries
            .iter_mut()
            .find(|entry| entry.download_id == download_id)
        {
            existing.priority = priority;
            existing.group = group;
            return false;
        }

        self.entries
            .push(ResourceEntry::new(download_id, priority, group));
        true
    }

    pub fn erase_entry(&mut self, download_id: &HashString) -> bool {
        let before = self.entries.len();
        self.entries
            .retain(|entry| entry.download_id != *download_id);
        self.active.remove(download_id);
        before != self.entries.len()
    }

    /// Moves a download to another group. Returns `false` when either the
    /// download or the destination group does not exist.
    pub fn set_group(&mut self, download_id: &HashString, group: u16) -> bool {
        if (group as usize) >= self.groups.len() {
            return false;
        }
        if let Some(entry) = self
            .entries
            .iter_mut()
            .find(|entry| entry.download_id == *download_id)
        {
            entry.group = group;
            true
        } else {
            false
        }
    }

    pub fn set_group_checked(
        &mut self,
        download_id: &HashString,
        group: u16,
    ) -> Result<(), String> {
        if (group as usize) >= self.groups.len() {
            return Err("Choke group not found.".to_string());
        }
        if self.set_group(download_id, group) {
            Ok(())
        } else {
            Err("Could not find download in resource manager.".to_string())
        }
    }

    pub fn set_priority(&mut self, download_id: &HashString, priority: u16) -> bool {
        if let Some(entry) = self
            .entries
            .iter_mut()
            .find(|entry| entry.download_id == *download_id)
        {
            entry.priority = priority;
            true
        } else {
            false
        }
    }

    pub fn max_upload_unchoked(&self) -> u32 {
        self.max_upload_unchoked
    }

    pub fn max_download_unchoked(&self) -> u32 {
        self.max_download_unchoked
    }

    pub fn min_upload_unchoked(&self) -> u32 {
        self.min_upload_unchoked
    }

    pub fn min_download_unchoked(&self) -> u32 {
        self.min_download_unchoked
    }

    pub fn currently_upload_unchoked(&self) -> u32 {
        self.currently_upload_unchoked
    }

    pub fn currently_download_unchoked(&self) -> u32 {
        self.currently_download_unchoked
    }

    pub fn tick_count(&self) -> u64 {
        self.tick_count
    }

    pub fn set_max_upload_unchoked(&mut self, value: u32) {
        self.max_upload_unchoked = value;
        if value != UNLIMITED_SLOTS {
            self.min_upload_unchoked = self.min_upload_unchoked.min(value);
        }
    }

    pub fn set_max_download_unchoked(&mut self, value: u32) {
        self.max_download_unchoked = value;
        if value != UNLIMITED_SLOTS {
            self.min_download_unchoked = self.min_download_unchoked.min(value);
        }
    }

    /// Sets the global minimum. It is clamped to the global maximum (unless
    /// the maximum is unlimited) so `min <= max` is always true.
    pub fn set_min_upload_unchoked(&mut self, value: u32) {
        self.min_upload_unchoked = if self.max_upload_unchoked == UNLIMITED_SLOTS {
            value
        } else {
            value.min(self.max_upload_unchoked)
        };
    }

    pub fn set_min_download_unchoked(&mut self, value: u32) {
        self.min_download_unchoked = if self.max_download_unchoked == UNLIMITED_SLOTS {
            value
        } else {
            value.min(self.max_download_unchoked)
        };
    }

    pub fn set_upload_unchoke_limits(&mut self, min: u32, max: u32) {
        self.max_upload_unchoked = max;
        self.min_upload_unchoked = if max == UNLIMITED_SLOTS {
            min
        } else {
            min.min(max)
        };
    }

    pub fn set_download_unchoke_limits(&mut self, min: u32, max: u32) {
        self.max_download_unchoked = max;
        self.min_download_unchoked = if max == UNLIMITED_SLOTS {
            min
        } else {
            min.min(max)
        };
    }

    /// Convenience setters for a group's persistent limits.
    pub fn set_group_up_max(&mut self, index: usize, max: u32) -> bool {
        match self.groups.get_mut(index) {
            Some(group) => {
                group.set_up_max(max);
                true
            }
            None => false,
        }
    }

    pub fn set_group_down_max(&mut self, index: usize, max: u32) -> bool {
        match self.groups.get_mut(index) {
            Some(group) => {
                group.set_down_max(max);
                true
            }
            None => false,
        }
    }

    pub fn set_group_up_min(&mut self, index: usize, min: u32) -> bool {
        match self.groups.get_mut(index) {
            Some(group) => {
                group.set_up_min(min);
                true
            }
            None => false,
        }
    }

    pub fn set_group_down_min(&mut self, index: usize, min: u32) -> bool {
        match self.groups.get_mut(index) {
            Some(group) => {
                group.set_down_min(min);
                true
            }
            None => false,
        }
    }

    /// Computes quotas for every group for one direction.
    pub fn allocate_upload_quotas(&self) -> Vec<u32> {
        allocate_quotas_with_min(
            &self
                .groups
                .iter()
                .map(ChokeGroup::up_requested)
                .collect::<Vec<_>>(),
            &self
                .groups
                .iter()
                .map(ChokeGroup::up_min)
                .collect::<Vec<_>>(),
            &self.group_weights(),
            self.max_upload_unchoked,
        )
    }

    pub fn allocate_download_quotas(&self) -> Vec<u32> {
        allocate_quotas_with_min(
            &self
                .groups
                .iter()
                .map(ChokeGroup::down_requested)
                .collect::<Vec<_>>(),
            &self
                .groups
                .iter()
                .map(ChokeGroup::down_min)
                .collect::<Vec<_>>(),
            &self.group_weights(),
            self.max_download_unchoked,
        )
    }

    /// Weight of each group for quota distribution.
    ///
    /// A group with registered downloads uses the sum of their priorities
    /// (the legacy `resource_manager_entry::priority` weighting). An empty
    /// group falls back to its configured group priority, which keeps
    /// configuration-driven groups useful before the first download arrives.
    pub fn group_weights(&self) -> Vec<u32> {
        self.groups
            .iter()
            .enumerate()
            .map(|(index, group)| {
                let sum: u32 = self
                    .entries
                    .iter()
                    .filter(|entry| entry.group as usize == index)
                    .fold(0u32, |sum, entry| sum.saturating_add(u32::from(entry.priority)));
                if sum > 0 {
                    sum
                } else {
                    group.priority()
                }
            })
            .collect()
    }

    /// Recomputes quotas, applies them to each group's choke queues and
    /// rebalances peer slots. Call once per engine tick.
    ///
    /// The queue's `max_unchoked` keeps the persistent group limit; the
    /// dynamically allocated quota is passed to `cycle` so a shrink in the
    /// global limit is not written back into the group configuration.
    pub fn receive_tick(&mut self) {
        self.tick_count = self.tick_count.wrapping_add(1);
        self.rebalance(true, true);
    }

    /// Explicit `choke_group.all.*.update_balance` entry point.
    pub fn update_balance_upload(&mut self) {
        self.rebalance(true, false);
    }

    pub fn update_balance_download(&mut self) {
        self.rebalance(false, true);
    }

    fn rebalance(&mut self, upload: bool, download: bool) {
        let upload_quotas = if upload {
            Some(self.allocate_upload_quotas())
        } else {
            None
        };
        let download_quotas = if download {
            Some(self.allocate_download_quotas())
        } else {
            None
        };

        let mut up_unchoked = self.currently_upload_unchoked;
        let mut down_unchoked = self.currently_download_unchoked;
        let mut changes = Vec::with_capacity(self.groups.len());

        if upload {
            up_unchoked = 0;
        }
        if download {
            down_unchoked = 0;
        }

        for (index, group) in self.groups.iter_mut().enumerate() {
            let mut change = GroupChokeChanges {
                group: index,
                ..GroupChokeChanges::default()
            };

            if let Some(quotas) = &upload_quotas {
                let quota = quotas.get(index).copied().unwrap_or(0);
                let (unchoked, choked) = group.up_queue_mut().cycle(quota);
                change.up_unchoked = unchoked;
                change.up_choked = choked;
                up_unchoked = up_unchoked.saturating_add(group.up_unchoked());
            }

            if let Some(quotas) = &download_quotas {
                let quota = quotas.get(index).copied().unwrap_or(0);
                let (unchoked, choked) = group.down_queue_mut().cycle(quota);
                change.down_unchoked = unchoked;
                change.down_choked = choked;
                down_unchoked = down_unchoked.saturating_add(group.down_unchoked());
            }

            changes.push(change);
        }

        if upload {
            self.currently_upload_unchoked = up_unchoked;
        }
        if download {
            self.currently_download_unchoked = down_unchoked;
        }
        self.last_changes = changes;
    }

    /// Returns the choke decisions produced by the most recent balance pass.
    pub fn last_choke_changes(&self) -> &[GroupChokeChanges] {
        &self.last_changes
    }

    pub fn take_choke_changes(&mut self) -> Vec<GroupChokeChanges> {
        std::mem::take(&mut self.last_changes)
    }

    pub fn ratio(&self, group: usize) -> Option<&RatioConfig> {
        self.groups.get(group).map(ChokeGroup::ratio)
    }

    pub fn ratio_mut(&mut self, group: usize) -> Option<&mut RatioConfig> {
        self.groups.get_mut(group).map(ChokeGroup::ratio_mut)
    }

    pub fn ratio_config(&self, group: usize) -> Option<RatioConfig> {
        self.ratio(group).cloned()
    }

    pub fn set_ratio_config(&mut self, group: usize, config: RatioConfig) -> bool {
        match self.ratio_mut(group) {
            Some(target) => {
                *target = config;
                true
            }
            None => false,
        }
    }

    pub fn set_ratio_enabled(&mut self, group: usize, enabled: bool) -> bool {
        match self.ratio_mut(group) {
            Some(config) => {
                config.enabled = enabled;
                true
            }
            None => false,
        }
    }

    /// Applies the `group.seeding.ratio.*` rules for a group.
    pub fn ratio_reached(&self, group: usize, uploaded: u64, downloaded: u64) -> bool {
        self.ratio(group)
            .map(|config| config.ratio_reached(uploaded, downloaded))
            .unwrap_or(false)
    }
}

/// Distributes `max` slots across groups by weight, never exceeding each
/// group's requested amount. Minimums are satisfied first.
pub fn allocate_quotas(requested: &[u32], weights: &[u32], max: u32) -> Vec<u32> {
    let minimums = vec![0u32; requested.len()];
    allocate_quotas_with_min(requested, &minimums, weights, max)
}

/// Weighted allocator with per-group minimums.
///
/// Invariants:
/// - The sum of the result never exceeds `max` (unless `max` is
///   [`UNLIMITED_SLOTS`], in which case every request is returned).
/// - A quota never exceeds its request, so empty groups consume nothing.
/// - When the minimums alone exceed `max`, they are assigned deterministically
///   from the lowest group index until the budget is exhausted.
/// - `min <= max` is guaranteed by the group configuration setters; this
///   function additionally clamps defensively.
pub fn allocate_quotas_with_min(
    requested: &[u32],
    minimums: &[u32],
    weights: &[u32],
    max: u32,
) -> Vec<u32> {
    let count = requested.len();
    let mut quotas = vec![0u32; count];

    if count == 0 {
        return quotas;
    }

    if max == UNLIMITED_SLOTS {
        quotas.copy_from_slice(requested);
        return quotas;
    }

    if max == 0 {
        return quotas;
    }

    // Demand is bounded by the global budget: no single group can use more
    // than all available slots.
    let demand: Vec<u32> = requested.iter().map(|request| (*request).min(max)).collect();

    let mut remaining = max;
    for index in 0..count {
        let wanted = minimums
            .get(index)
            .copied()
            .unwrap_or(0)
            .min(demand[index])
            .min(remaining);
        quotas[index] = wanted;
        remaining -= wanted;
    }

    distribute_remaining(&mut quotas, &demand, weights, &mut remaining);
    quotas
}

/// Shares `remaining` slots proportionally to `weights`, respecting demand.
fn distribute_remaining(
    quotas: &mut [u32],
    demand: &[u32],
    weights: &[u32],
    remaining: &mut u32,
) {
    while *remaining > 0 {
        let mut active: Vec<usize> = (0..quotas.len())
            .filter(|index| quotas[*index] < demand[*index])
            .collect();

        if active.is_empty() {
            break;
        }

        // Deterministic order: higher weight first, then group index.
        active.sort_by_key(|index| {
            (
                std::cmp::Reverse(weights.get(*index).copied().unwrap_or(1).max(1)),
                *index,
            )
        });

        let total_weight: u64 = active
            .iter()
            .map(|index| u64::from(weights.get(*index).copied().unwrap_or(1).max(1)))
            .sum();

        if total_weight == 0 {
            break;
        }

        let snapshot = *remaining;
        let mut progressed = false;

        for &index in &active {
            let weight = u64::from(weights.get(index).copied().unwrap_or(1).max(1));
            let share = ((u64::from(snapshot) * weight) / total_weight) as u32;
            let grant = share.min(demand[index] - quotas[index]).min(*remaining);
            if grant > 0 {
                quotas[index] += grant;
                *remaining -= grant;
                progressed = true;
            }
        }

        if !progressed {
            // Rounding/capacity left a remainder smaller than every weighted
            // share: hand out one slot at a time in deterministic order.
            for &index in &active {
                if *remaining == 0 {
                    break;
                }
                if quotas[index] < demand[index] {
                    quotas[index] += 1;
                    *remaining -= 1;
                    progressed = true;
                }
            }
        }

        if !progressed {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn group_with_requests(
        manager: &mut ResourceManager,
        name: &str,
        priority: u32,
        up: u32,
        down: u32,
    ) -> usize {
        let index = manager.push_group(name);
        let group = manager.group_at_mut(index).unwrap();
        group.set_priority(priority);
        group.set_up_max(UNLIMITED_SLOTS);
        group.set_down_max(UNLIMITED_SLOTS);
        group
            .up_queue_mut()
            .add_entry(crate::choke_queue::ChokeEntry::new());
        for peer in 0..up {
            group.up_queue_mut().set_queued(
                0,
                crate::choke_queue::WeightedPeer::new(peer as usize),
            );
        }

        group
            .down_queue_mut()
            .add_entry(crate::choke_queue::ChokeEntry::new());
        for peer in 0..down {
            group.down_queue_mut().set_queued(
                0,
                crate::choke_queue::WeightedPeer::new(peer as usize),
            );
        }
        index
    }

    #[test]
    fn quotas_respect_weights_and_demand() {
        let mut manager = ResourceManager::new();
        group_with_requests(&mut manager, "a", 1, 10, 10);
        group_with_requests(&mut manager, "b", 2, 10, 10);
        manager.set_max_upload_unchoked(9);

        let quotas = manager.allocate_upload_quotas();
        assert_eq!(quotas.iter().sum::<u32>(), 9);
        assert!(quotas[2] > quotas[1], "higher weight should get more slots");
    }

    #[test]
    fn quotas_never_exceed_requested() {
        let mut manager = ResourceManager::new();
        group_with_requests(&mut manager, "a", 1, 2, 2);
        group_with_requests(&mut manager, "b", 1, 3, 3);
        manager.set_max_upload_unchoked(100);

        let quotas = manager.allocate_upload_quotas();
        assert_eq!(quotas, vec![0, 2, 3]);
    }

    #[test]
    fn unlimited_global_slots_return_all_requests() {
        let mut manager = ResourceManager::new();
        group_with_requests(&mut manager, "a", 1, 4, 1);
        manager.set_max_upload_unchoked(u32::MAX);

        let quotas = manager.allocate_upload_quotas();
        assert_eq!(quotas, vec![0, 4]);
    }

    #[test]
    fn group_minimums_are_satisfied_before_weighted_share() {
        let mut manager = ResourceManager::new();
        let a = group_with_requests(&mut manager, "a", 1, 10, 10);
        let b = group_with_requests(&mut manager, "b", 1, 10, 10);
        manager.group_at_mut(a).unwrap().set_up_min(4);
        manager.group_at_mut(b).unwrap().set_up_min(4);
        manager.set_max_upload_unchoked(10);

        let quotas = manager.allocate_upload_quotas();
        assert_eq!(quotas.iter().sum::<u32>(), 10);
        assert!(quotas[a] >= 4);
        assert!(quotas[b] >= 4);
    }

    #[test]
    fn empty_group_does_not_consume_slots() {
        let mut manager = ResourceManager::new();
        let active = group_with_requests(&mut manager, "active", 1, 5, 5);
        manager.push_group("empty");
        manager.set_max_upload_unchoked(5);

        let quotas = manager.allocate_upload_quotas();
        assert_eq!(quotas[0], 0);
        assert_eq!(quotas[active], 5);
        assert_eq!(quotas.iter().sum::<u32>(), 5);
    }

    #[test]
    fn group_min_is_clamped_to_group_max() {
        let mut manager = ResourceManager::new();
        let index = group_with_requests(&mut manager, "clamped", 1, 10, 10);
        let group = manager.group_at_mut(index).unwrap();
        group.set_up_max(2);
        group.set_up_min(9);
        assert_eq!(group.up_min(), 2);
        assert_eq!(group.up_requested(), 2);

        group.set_up_max(6);
        assert_eq!(group.up_min(), 2);
        group.set_up_min(9);
        assert_eq!(group.up_min(), 6);
    }

    #[test]
    fn minimums_exceeding_budget_are_clamped_deterministically() {
        let quotas = allocate_quotas_with_min(&[10, 10, 10], &[5, 5, 5], &[1, 1, 1], 7);
        assert_eq!(quotas.iter().sum::<u32>(), 7);
        assert_eq!(quotas, vec![5, 2, 0]);
    }

    #[test]
    fn group_lookup_and_entry_management() {
        let mut manager = ResourceManager::new();
        let index = manager.push_group("fast");
        assert_eq!(manager.push_group("fast"), index);
        assert_eq!(manager.group_index_of("fast"), Some(index));

        let id = HashString::new([0x42; 20]);
        assert!(manager.insert_entry(id, 5, index as u16));
        assert!(!manager.insert_entry(id, 7, index as u16));
        assert!(manager.set_priority(&id, 9));
        assert_eq!(manager.entry(&id).unwrap().priority, 9);

        assert!(manager.set_entry_active(&id, true));
        assert!(manager.is_entry_active(&id));
        assert_eq!(manager.group_entry_count(index), 1);

        assert!(manager.erase_entry(&id));
        assert!(manager.entry(&id).is_none());
        assert!(!manager.is_entry_active(&id));
    }

    #[test]
    fn removing_a_group_reassigns_members() {
        let mut manager = ResourceManager::new();
        let first = manager.push_group("first");
        let second = manager.push_group("second");

        let a = HashString::new([0x11; 20]);
        let b = HashString::new([0x22; 20]);
        manager.insert_entry(a, 1, first as u16);
        manager.insert_entry(b, 1, second as u16);

        manager.remove_group(first).unwrap();

        assert_eq!(manager.group_count(), 2);
        assert_eq!(manager.entry(&a).unwrap().group, 0);
        assert_eq!(manager.entry(&b).unwrap().group, 1);
        assert!(manager.remove_group(0).is_err());
    }

    #[test]
    fn invalid_group_indexes_do_not_panic() {
        let mut manager = ResourceManager::new();
        let id = HashString::new([0x33; 20]);
        assert!(manager.insert_entry(id, 0, 99));
        assert_eq!(manager.entry(&id).unwrap().group, 0);
        assert!(!manager.set_group(&id, 99));
    }

    #[test]
    fn ratio_state_is_per_group() {
        let mut manager = ResourceManager::new();
        let high = manager.push_group("high");
        assert!(manager.set_ratio_enabled(high, true));
        manager.ratio_mut(high).unwrap().min = 100.0;
        manager.ratio_mut(high).unwrap().max = 200.0;
        manager.ratio_mut(high).unwrap().upload = 10;

        assert!(!manager.ratio_reached(high, 1, 1));
        assert!(manager.ratio_reached(high, 200, 100));
        assert!(!manager.ratio_reached(0, 200, 100));
        assert!(manager.ratio_reached(high, 300, 100));
    }
}
