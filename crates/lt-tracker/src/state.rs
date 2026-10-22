#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrackerEvent {
    None = 0,
    Completed = 1,
    Started = 2,
    Stopped = 3,
    Scrape = 4,
}

bitflags::bitflags! {
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
    pub struct TrackerFlags: u32 {
        const ENABLED          = 0x1;
        const DELETED          = 0x2;
        const REQUESTING       = 0x4;
        const STARTING_REQUEST = 0x8;
        const EXTRA_TRACKER    = 0x10;
        const SCRAPABLE        = 0x20;
        const DISOWNABLE       = 0x40;
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TrackerParams {
    pub numwant: i32,
    pub uploaded_adjusted: u64,
    pub completed_adjusted: u64,
    pub download_left: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TrackerCounters {
    pub success_time_last: u64,
    pub success_counter: u32,
    pub failed_time_last: u64,
    pub failed_counter: u32,
    pub scrape_time_last: u64,
    pub scrape_counter: u32,
    /// Last failed scrape. Kept separate from announce failures so that a
    /// tracker which simply does not implement scrape is never demoted or
    /// marked as announce-failed.
    pub scrape_failed_time_last: u64,
    pub scrape_failed_counter: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrackerState {
    pub flags: TrackerFlags,
    pub normal_interval: u64,
    pub min_interval: u64,
    pub latest_event: TrackerEvent,
    pub latest_new_peers: u32,
    pub latest_new_peers_delta: u32,
    pub latest_sum_peers: u32,
    pub counters: TrackerCounters,
    pub scrape_complete: u32,
    pub scrape_incomplete: u32,
    pub scrape_downloaded: u32,
}

impl Default for TrackerState {
    fn default() -> Self {
        Self {
            flags: TrackerFlags::ENABLED,
            normal_interval: Self::MIN_NORMAL_INTERVAL,
            min_interval: Self::MIN_MIN_INTERVAL,
            latest_event: TrackerEvent::None,
            latest_new_peers: 0,
            latest_new_peers_delta: 0,
            latest_sum_peers: 0,
            counters: TrackerCounters::default(),
            scrape_complete: 0,
            scrape_incomplete: 0,
            scrape_downloaded: 0,
        }
    }
}

impl TrackerState {
    pub const DEFAULT_MIN_INTERVAL: u64 = 600;
    pub const MIN_MIN_INTERVAL: u64 = 300;
    pub const MAX_MIN_INTERVAL: u64 = 4 * 3600;

    pub const DEFAULT_NORMAL_INTERVAL: u64 = 1800;
    pub const MIN_NORMAL_INTERVAL: u64 = 600;
    pub const MAX_NORMAL_INTERVAL: u64 = 8 * 3600;

    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_enabled(&self) -> bool {
        self.flags.contains(TrackerFlags::ENABLED)
    }

    pub fn is_deleted(&self) -> bool {
        self.flags.contains(TrackerFlags::DELETED)
    }

    pub fn is_requesting(&self) -> bool {
        self.flags.contains(TrackerFlags::REQUESTING)
    }

    pub fn is_starting_request(&self) -> bool {
        self.flags.contains(TrackerFlags::STARTING_REQUEST)
    }

    pub fn is_extra_tracker(&self) -> bool {
        self.flags.contains(TrackerFlags::EXTRA_TRACKER)
    }

    pub fn is_in_use(&self) -> bool {
        self.is_enabled() && self.counters.success_counter != 0
    }

    pub fn is_scrapable(&self) -> bool {
        self.flags.contains(TrackerFlags::SCRAPABLE)
    }

    pub fn is_disownable(&self) -> bool {
        self.flags.contains(TrackerFlags::DISOWNABLE)
    }

    pub fn enable(&mut self) {
        self.flags.insert(TrackerFlags::ENABLED);
    }

    pub fn disable(&mut self) {
        self.flags.remove(TrackerFlags::ENABLED);
    }

    pub fn set_scrapable(&mut self, scrapable: bool) {
        if scrapable {
            self.flags.insert(TrackerFlags::SCRAPABLE);
        } else {
            self.flags.remove(TrackerFlags::SCRAPABLE);
        }
    }

    pub fn clear_stats(&mut self) {
        self.latest_new_peers = 0;
        self.latest_sum_peers = 0;
        self.counters.success_counter = 0;
        self.counters.failed_counter = 0;
        self.counters.scrape_counter = 0;
        self.counters.scrape_failed_counter = 0;
    }

    pub fn set_normal_interval(&mut self, interval: u64) {
        self.normal_interval = interval.clamp(Self::MIN_NORMAL_INTERVAL, Self::MAX_NORMAL_INTERVAL);
    }

    pub fn set_min_interval(&mut self, interval: u64) {
        self.min_interval = interval.clamp(Self::MIN_MIN_INTERVAL, Self::MAX_MIN_INTERVAL);
    }

    pub fn add_success_request(&mut self, time: u64) {
        self.counters.success_time_last = time;
        self.counters.success_counter = self.counters.success_counter.saturating_add(1);
        self.counters.failed_counter = 0;
    }

    pub fn add_failed_request(&mut self, time: u64) {
        self.counters.failed_time_last = time;
        self.counters.failed_counter = self.counters.failed_counter.saturating_add(1);
    }

    pub fn add_scrape_request(&mut self, time: u64) {
        self.counters.scrape_time_last = time;
        self.counters.scrape_counter = self.counters.scrape_counter.saturating_add(1);
        self.counters.scrape_failed_counter = 0;
    }

    /// Records a failed scrape without touching the announce success/failure
    /// counters. Scrape is optional, so a tracker that does not support it must
    /// remain usable.
    pub fn add_scrape_failure(&mut self, time: u64) {
        self.counters.scrape_failed_time_last = time;
        self.counters.scrape_failed_counter = self.counters.scrape_failed_counter.saturating_add(1);
    }

    /// Earliest time at which a normal (successful) scrape cycle is due.
    pub fn scrape_time_next(&self) -> u64 {
        if self.counters.scrape_counter == 0 {
            0
        } else {
            self.counters.scrape_time_last
                + self
                    .normal_interval
                    .max(self.min_interval)
                    .max(Self::MIN_NORMAL_INTERVAL)
        }
    }

    /// Earliest time at which a retry after a scrape failure is allowed. This
    /// deliberately uses the tracker's minimum interval so a failing scrape
    /// endpoint cannot be hammered.
    pub fn scrape_time_next_after_failure(&self) -> u64 {
        if self.counters.scrape_failed_counter == 0 {
            self.scrape_time_next()
        } else {
            self.counters.scrape_failed_time_last + self.min_interval.max(Self::MIN_MIN_INTERVAL)
        }
    }

    /// Earliest time at which any scrape (success or retry) may be started.
    pub fn scrape_time_next_minimum(&self) -> u64 {
        if self.counters.scrape_failed_counter != 0 {
            self.scrape_time_next_after_failure()
        } else {
            self.scrape_time_next()
        }
    }

    pub fn success_time_next(&self) -> u64 {
        if self.counters.success_counter == 0 {
            0
        } else {
            self.counters.success_time_last + self.normal_interval.max(Self::MIN_NORMAL_INTERVAL)
        }
    }

    pub fn failed_time_next(&self) -> u64 {
        if self.counters.failed_counter == 0 {
            return self.min_interval.min(Self::MIN_MIN_INTERVAL);
        }

        if self.min_interval > Self::MIN_MIN_INTERVAL {
            return self.counters.failed_time_last + self.min_interval;
        }

        let shift = (self.counters.failed_counter - 1).min(6);
        self.counters.failed_time_last + (5u64 << shift).min(Self::MIN_MIN_INTERVAL)
    }

    pub fn activity_time_last(&self) -> u64 {
        if self.counters.failed_counter != 0 {
            self.counters.failed_time_last
        } else {
            self.counters.success_time_last
        }
    }

    pub fn activity_time_next(&self) -> u64 {
        if self.counters.failed_counter != 0 {
            self.failed_time_next()
        } else {
            self.success_time_next()
        }
    }

    pub fn activity_time_next_minimum(&self) -> u64 {
        if self.counters.failed_counter != 0 {
            self.failed_time_next()
        } else if self.counters.success_counter == 0 {
            0
        } else {
            self.counters.success_time_last + self.min_interval.max(Self::MIN_MIN_INTERVAL)
        }
    }
}
