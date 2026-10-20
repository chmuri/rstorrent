use lt_net::addr::AddressList;

use crate::client::ScrapeStats;
use crate::state::{TrackerEvent, TrackerFlags};
use crate::tracker::Tracker;
use crate::udp_router::RandomEngine;

pub struct TrackerList {
    pub trackers: Vec<Tracker>,
    pub numwant: i32,
    pub key: u32,
    /// Last cached time supplied by the owning controller. Used by the
    /// interval-aware scrape helpers when no explicit time is provided.
    cached_time: u64,

    pub on_tracker_enabled: Option<Box<dyn FnMut(usize) + Send + Sync>>,
    pub on_tracker_disabled: Option<Box<dyn FnMut(usize) + Send + Sync>>,
    pub on_success: Option<Box<dyn FnMut(usize, &AddressList) -> u32 + Send + Sync>>,
    pub on_failure: Option<Box<dyn FnMut(usize, &str) + Send + Sync>>,
    pub on_scrape_success: Option<Box<dyn FnMut(usize) + Send + Sync>>,
    pub on_scrape_failure: Option<Box<dyn FnMut(usize, &str) + Send + Sync>>,
}

impl Default for TrackerList {
    fn default() -> Self {
        Self::new()
    }
}

impl TrackerList {
    pub fn new() -> Self {
        Self {
            trackers: Vec::new(),
            numwant: -1,
            key: 1,
            cached_time: 0,
            on_tracker_enabled: None,
            on_tracker_disabled: None,
            on_success: None,
            on_failure: None,
            on_scrape_success: None,
            on_scrape_failure: None,
        }
    }

    pub fn size(&self) -> usize {
        self.trackers.len()
    }

    pub fn cached_time(&self) -> u64 {
        self.cached_time
    }

    pub fn set_cached_time(&mut self, now: u64) {
        self.cached_time = now;
    }

    pub fn is_empty(&self) -> bool {
        self.trackers.is_empty()
    }

    pub fn at(&self, index: usize) -> Option<&Tracker> {
        self.trackers.get(index)
    }

    pub fn at_mut(&mut self, index: usize) -> Option<&mut Tracker> {
        self.trackers.get_mut(index)
    }

    pub fn clear(&mut self) {
        self.trackers.clear();
    }

    pub fn clear_stats(&mut self) {
        for tracker in &mut self.trackers {
            tracker.clear_stats();
        }
    }

    pub fn insert(&mut self, tracker: Tracker) -> usize {
        // Keep sorted by group
        let group = tracker.group();
        let idx = self
            .trackers
            .iter()
            .position(|t| t.group() > group)
            .unwrap_or(self.trackers.len());
        self.trackers.insert(idx, tracker);
        if let Some(ref mut cb) = self.on_tracker_enabled {
            cb(idx);
        }
        idx
    }

    pub fn insert_url(&mut self, group: u32, url: &str, extra_tracker: bool) -> usize {
        let mut flags = TrackerFlags::ENABLED;
        if extra_tracker {
            flags.insert(TrackerFlags::EXTRA_TRACKER);
        }
        let tracker = Tracker::new_with_flags(group, url.to_string(), flags);
        self.insert(tracker)
    }

    pub fn find_url(&self, url: &str) -> Option<usize> {
        self.trackers.iter().position(|t| t.url() == url)
    }

    pub fn begin_group(&self, group: u32) -> usize {
        self.trackers
            .iter()
            .position(|t| t.group() >= group)
            .unwrap_or(self.trackers.len())
    }

    pub fn end_group(&self, group: u32) -> usize {
        self.begin_group(group + 1)
    }

    pub fn size_group(&self) -> usize {
        if self.trackers.is_empty() {
            0
        } else {
            (self.trackers.last().unwrap().group() + 1) as usize
        }
    }

    pub fn cycle_group(&mut self, group: u32) {
        let first = self.begin_group(group);
        let last = self.end_group(group);

        if first >= last || self.trackers[first].group() != group {
            return;
        }

        self.trackers[first..last].rotate_left(1);
    }

    pub fn promote(&mut self, index: usize) -> usize {
        if index >= self.trackers.len() {
            return index;
        }
        let group = self.trackers[index].group();
        let first = self.begin_group(group);
        if first != index {
            self.trackers.swap(first, index);
        }
        first
    }

    pub fn randomize_group_entries(&mut self) {
        let mut rng = RandomEngine::new();
        let mut i = 0;
        while i < self.trackers.len() {
            let group = self.trackers[i].group();
            let end = self.end_group(group);
            let len = end - i;
            if len > 1 {
                for j in (1..len).rev() {
                    let k = (rng.next_u32() as usize) % (j + 1);
                    self.trackers.swap(i + j, i + k);
                }
            }
            i = end;
        }
    }

    pub fn has_active(&self) -> bool {
        self.trackers.iter().any(|t| t.is_requesting())
    }

    pub fn has_active_not_scrape(&self) -> bool {
        self.trackers.iter().any(|t| t.is_requesting_not_scrape())
    }

    pub fn has_active_in_group(&self, group: u32) -> bool {
        let begin = self.begin_group(group);
        let end = self.end_group(group);
        self.trackers[begin..end].iter().any(|t| t.is_requesting())
    }

    pub fn has_active_not_scrape_in_group(&self, group: u32) -> bool {
        let begin = self.begin_group(group);
        let end = self.end_group(group);
        self.trackers[begin..end]
            .iter()
            .any(|t| t.is_requesting_not_scrape())
    }

    pub fn has_usable(&self) -> bool {
        self.trackers.iter().any(|t| t.is_usable())
    }

    pub fn has_in_use(&self) -> bool {
        self.trackers.iter().any(|t| t.is_in_use())
    }

    pub fn find_next_to_request(&self, from_index: usize) -> Option<usize> {
        if from_index >= self.trackers.len() {
            return None;
        }

        let mut preferred = None;
        for i in from_index..self.trackers.len() {
            if self.trackers[i].can_request_state() {
                preferred = Some(i);
                break;
            }
        }

        let mut pref_idx = match preferred {
            Some(idx) => idx,
            None => return None,
        };

        let pref_state = self.trackers[pref_idx].state();
        if pref_state.counters.failed_counter == 0 {
            return Some(pref_idx);
        }

        for i in (pref_idx + 1)..self.trackers.len() {
            if !self.trackers[i].can_request_state() {
                continue;
            }

            let cur_state = self.trackers[i].state();
            let cur_pref_state = self.trackers[pref_idx].state();

            if cur_state.counters.failed_counter != 0 {
                if cur_pref_state.counters.failed_counter == 0 {
                    continue;
                }
                if cur_state.failed_time_next() < cur_pref_state.activity_time_next() {
                    pref_idx = i;
                }
                continue;
            }

            if cur_state.activity_time_next() < cur_pref_state.activity_time_next() {
                pref_idx = i;
            }
        }

        Some(pref_idx)
    }

    pub fn enable_tracker(&mut self, index: usize) {
        if let Some(t) = self.trackers.get_mut(index) {
            let was_enabled = t.is_usable();
            t.enable();
            if !was_enabled && t.is_usable() {
                if let Some(ref mut cb) = self.on_tracker_enabled {
                    cb(index);
                }
            }
        }
    }

    pub fn disable_tracker(&mut self, index: usize) {
        if let Some(t) = self.trackers.get_mut(index) {
            let was_enabled = t.is_usable();
            t.disable();
            if was_enabled && !t.is_usable() {
                if let Some(ref mut cb) = self.on_tracker_disabled {
                    cb(index);
                }
            }
        }
    }

    pub fn send_event(&mut self, index: usize, event: TrackerEvent) {
        if let Some(t) = self.trackers.get_mut(index) {
            if !t.is_usable() || event == TrackerEvent::Scrape {
                return;
            }
            if t.is_requesting() {
                if t.state().latest_event == event {
                    return;
                }
                if t.state().latest_event != TrackerEvent::Scrape && event == TrackerEvent::None {
                    return;
                }
            }
            t.state_mut().flags.insert(TrackerFlags::REQUESTING);
            t.state_mut().latest_event = event;
        }
    }

    /// Marks a tracker for a scrape request using the list's cached time for
    /// the minimum-interval check.
    pub fn send_scrape(&mut self, index: usize) {
        let now = self.cached_time;
        let _ = self.send_scrape_at(index, now);
    }

    /// Marks a tracker for a scrape when it is due at `now`.
    ///
    /// Besides the normal scrapable/usable/busy checks, this enforces the
    /// tracker's minimum interval since the last scrape attempt. Returns
    /// `true` when the request was queued.
    pub fn send_scrape_at(&mut self, index: usize, now: u64) -> bool {
        let Some(t) = self.trackers.get_mut(index) else {
            return false;
        };

        if !t.is_usable() || !t.is_scrapable() || t.is_requesting() {
            return false;
        }

        let state = t.state();
        let last_attempt = state
            .counters
            .scrape_time_last
            .max(state.counters.scrape_failed_time_last);
        let attempted = state.counters.scrape_counter != 0
            || state.counters.scrape_failed_counter != 0
            || last_attempt != 0;

        if attempted {
            let min_interval = state
                .min_interval
                .max(crate::state::TrackerState::MIN_MIN_INTERVAL);
            if now < last_attempt.saturating_add(min_interval) {
                return false;
            }
        }

        t.state_mut().flags.insert(TrackerFlags::REQUESTING);
        t.state_mut().latest_event = TrackerEvent::Scrape;
        true
    }

    /// Forces a scrape request even when an announce is already in flight.
    ///
    /// The tracker view uses this for the explicit user action; normal
    /// scheduled scrapes still use [`Self::send_scrape_at`] and respect
    /// intervals and announce exclusion.
    pub fn force_scrape(&mut self, index: usize) -> bool {
        let Some(tracker) = self.trackers.get_mut(index) else {
            return false;
        };
        if !tracker.is_usable() || !tracker.is_scrapable() {
            return false;
        }
        tracker.state_mut().flags.insert(TrackerFlags::REQUESTING);
        tracker.state_mut().latest_event = TrackerEvent::Scrape;
        true
    }

    /// Indexes of trackers with an outstanding scrape request.
    pub fn scrape_requests(&self) -> Vec<usize> {
        self.trackers
            .iter()
            .enumerate()
            .filter(|(_, t)| t.is_requesting() && t.state().latest_event == TrackerEvent::Scrape)
            .map(|(index, _)| index)
            .collect()
    }

    pub fn has_active_scrape(&self) -> bool {
        self.trackers
            .iter()
            .any(|t| t.is_requesting() && t.state().latest_event == TrackerEvent::Scrape)
    }

    pub fn set_scrapable(&mut self, index: usize, scrapable: bool) {
        if let Some(t) = self.trackers.get_mut(index) {
            t.state_mut().set_scrapable(scrapable);
        }
    }

    pub fn receive_success(
        &mut self,
        index: usize,
        peers: &AddressList,
        new_peers: u32,
        now: u64,
    ) -> usize {
        let promoted_idx = self.promote(index);
        if let Some(t) = self.trackers.get_mut(promoted_idx) {
            t.state_mut().flags.remove(TrackerFlags::REQUESTING);
            t.state_mut().add_success_request(now);
            t.state_mut().latest_sum_peers = peers.len() as u32;
            t.state_mut().latest_new_peers = new_peers;
        }
        promoted_idx
    }

    pub fn receive_failure(&mut self, index: usize, now: u64) {
        if let Some(t) = self.trackers.get_mut(index) {
            t.state_mut().flags.remove(TrackerFlags::REQUESTING);
            t.state_mut().add_failed_request(now);
        }
    }

    pub fn receive_scrape_success(&mut self, index: usize, now: u64) {
        if let Some(t) = self.trackers.get_mut(index) {
            t.state_mut().flags.remove(TrackerFlags::REQUESTING);
            t.state_mut().add_scrape_request(now);
            t.state_mut().counters.scrape_failed_counter = 0;
        }
    }

    /// Applies a successful scrape response to the per-tracker state.
    ///
    /// Missing optional keys are already normalised to zero by the client, so
    /// every counter is always written here.
    pub fn receive_scrape_stats(&mut self, index: usize, stats: ScrapeStats, now: u64) {
        if let Some(t) = self.trackers.get_mut(index) {
            t.state_mut().flags.remove(TrackerFlags::REQUESTING);
            t.state_mut().add_scrape_request(now);
            t.state_mut().scrape_complete = stats.seeders;
            t.state_mut().scrape_incomplete = stats.leechers;
            t.state_mut().scrape_downloaded = stats.completed;
        }
    }

    pub fn receive_scrape_failure(&mut self, index: usize, now: u64) {
        if let Some(t) = self.trackers.get_mut(index) {
            t.state_mut().flags.remove(TrackerFlags::REQUESTING);
            // Scrape is optional: do not touch failed_counter/success_counter.
            t.state_mut().add_scrape_failure(now);
        }
    }

    /// Disables scraping on a tracker that reported that it does not support
    /// the scrape endpoint. The tracker remains usable for announces.
    pub fn receive_scrape_unsupported(&mut self, index: usize, now: u64) {
        if let Some(t) = self.trackers.get_mut(index) {
            t.state_mut().flags.remove(TrackerFlags::REQUESTING);
            t.state_mut().set_scrapable(false);
            t.state_mut().add_scrape_failure(now);
        }
    }

    pub fn close_all(&mut self) {
        for t in &mut self.trackers {
            t.state_mut().flags.remove(TrackerFlags::REQUESTING);
        }
    }

    pub fn close_all_excluding(&mut self, allowed_mask: u32) {
        for t in &mut self.trackers {
            let ev_bit = 1u32 << (t.state().latest_event as u32);
            if (allowed_mask & ev_bit) == 0 {
                t.state_mut().flags.remove(TrackerFlags::REQUESTING);
            }
        }
    }
}
