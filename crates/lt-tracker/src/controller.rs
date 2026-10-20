use lt_net::addr::AddressList;

use crate::client::ScrapeStats;
use crate::list::TrackerList;
use crate::state::{TrackerEvent, TrackerState};

bitflags::bitflags! {
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
    pub struct ControllerFlags: u32 {
        const SEND_UPDATE      = 0x1;
        const SEND_COMPLETED   = 0x2;
        const SEND_START       = 0x4;
        const SEND_STOP        = 0x8;
        const ACTIVE           = 0x10;
        const REQUESTING       = 0x20;
        const FAILURE_MODE     = 0x40;
        const PROMISCUOUS_MODE = 0x80;
    }
}

pub struct TrackerController {
    pub flags: ControllerFlags,
    pub tracker_list: TrackerList,
    current_time: u64,
    timeout_at: Option<u64>,
    scrape_at: Option<u64>,
    /// Number of scrapes started by the last `do_scrape()` that are still
    /// waiting for a result. The next automatic scrape is scheduled only after
    /// the whole batch completed.
    scrape_pending: usize,
    scrape_in_flight: bool,
    last_scrape_error: Option<String>,

    pub on_timeout: Option<Box<dyn FnMut() + Send + Sync>>,
    pub on_success: Option<Box<dyn FnMut(&AddressList) -> u32 + Send + Sync>>,
    pub on_failure: Option<Box<dyn FnMut(&str) + Send + Sync>>,
    pub on_scrape_failure: Option<Box<dyn FnMut(&str) + Send + Sync>>,
}

impl TrackerController {
    pub const MASK_SEND: ControllerFlags = ControllerFlags::from_bits_truncate(
        ControllerFlags::SEND_UPDATE.bits()
            | ControllerFlags::SEND_COMPLETED.bits()
            | ControllerFlags::SEND_START.bits()
            | ControllerFlags::SEND_STOP.bits(),
    );

    pub fn new(tracker_list: TrackerList) -> Self {
        Self {
            flags: ControllerFlags::empty(),
            tracker_list,
            current_time: 0,
            timeout_at: None,
            scrape_at: None,
            scrape_pending: 0,
            scrape_in_flight: false,
            last_scrape_error: None,
            on_timeout: None,
            on_success: None,
            on_failure: None,
            on_scrape_failure: None,
        }
    }

    pub fn is_active(&self) -> bool {
        self.flags.contains(ControllerFlags::ACTIVE)
    }

    pub fn is_requesting(&self) -> bool {
        self.flags.contains(ControllerFlags::REQUESTING)
    }

    pub fn is_failure_mode(&self) -> bool {
        self.flags.contains(ControllerFlags::FAILURE_MODE)
    }

    pub fn is_promiscuous_mode(&self) -> bool {
        self.flags.contains(ControllerFlags::PROMISCUOUS_MODE)
    }

    pub fn is_timeout_queued(&self) -> bool {
        self.timeout_at.is_some()
    }

    pub fn is_scrape_queued(&self) -> bool {
        self.scrape_at.is_some()
    }

    /// True while at least one scrape started by [`Self::do_scrape`] has not
    /// reported success or failure.
    pub fn is_scrape_in_flight(&self) -> bool {
        self.scrape_in_flight
    }

    pub fn pending_scrape_count(&self) -> usize {
        self.scrape_pending
    }

    pub fn last_scrape_error(&self) -> Option<&str> {
        self.last_scrape_error.as_deref()
    }

    /// Trackers with an outstanding scrape request.
    pub fn scrape_requests(&self) -> Vec<usize> {
        self.tracker_list.scrape_requests()
    }

    pub fn cached_time(&self) -> u64 {
        self.current_time
    }

    pub fn set_cached_time(&mut self, time: u64) {
        self.current_time = time;
        self.tracker_list.set_cached_time(time);
    }

    pub fn add_cached_time(&mut self, seconds: u64) {
        self.current_time += seconds;
        self.tracker_list.set_cached_time(self.current_time);
        self.check_timers();
    }

    pub fn seconds_to_next_timeout(&self) -> u32 {
        match self.timeout_at {
            Some(t) => {
                if t <= self.current_time {
                    0
                } else {
                    (t - self.current_time) as u32
                }
            }
            None => u32::MAX,
        }
    }

    pub fn seconds_to_next_scrape(&self) -> u32 {
        match self.scrape_at {
            Some(t) => {
                if t <= self.current_time {
                    0
                } else {
                    (t - self.current_time) as u32
                }
            }
            None => u32::MAX,
        }
    }

    pub fn update_timeout(&mut self, seconds_to_next: u32) {
        if !self.is_active() {
            return;
        }
        self.timeout_at = Some(self.current_time + seconds_to_next as u64);
    }

    pub fn scrape_request(&mut self, seconds_to_request: u32) {
        self.scrape_at = Some(self.current_time + seconds_to_request as u64);
    }

    /// Schedules the next scrape automatically from the per-tracker scrape
    /// intervals. Trackers that have never been scraped are due immediately;
    /// otherwise the earliest `scrape_time_next_minimum()` wins.
    ///
    /// Returns the delay in seconds, or `u32::MAX` when there is no scrapable
    /// tracker.
    pub fn schedule_next_scrape(&mut self) -> u32 {
        let now = self.current_time;
        let mut next: Option<u64> = None;

        for tracker in &self.tracker_list.trackers {
            if !tracker.is_usable() || !tracker.is_scrapable() {
                continue;
            }

            let tracker_next = tracker.state().scrape_time_next_minimum();
            let tracker_next = if tracker_next == 0 { now } else { tracker_next };
            next = Some(next.map_or(tracker_next, |value| value.min(tracker_next)));
        }

        match next {
            Some(next) => {
                self.scrape_at = Some(next);
                next.saturating_sub(now).min(u32::MAX as u64) as u32
            }
            None => {
                self.scrape_at = None;
                u32::MAX
            }
        }
    }

    /// Alias for [`Self::schedule_next_scrape`] used by the engine loop.
    pub fn request_next_scrape(&mut self) -> u32 {
        self.schedule_next_scrape()
    }

    /// Clears a pending scrape timer without sending anything.
    pub fn clear_scrape_request(&mut self) {
        self.scrape_at = None;
    }

    pub fn enable(&mut self, _enable_flags: u32) {
        self.flags.insert(ControllerFlags::ACTIVE);
        if self.flags.intersects(Self::MASK_SEND) {
            if self.flags.contains(ControllerFlags::SEND_START) {
                self.send_start_event();
            } else if self.flags.contains(ControllerFlags::SEND_STOP) {
                self.send_stop_event();
            } else if self.flags.contains(ControllerFlags::SEND_COMPLETED) {
                self.send_completed_event();
            } else {
                self.send_update_event();
            }
        } else {
            self.send_update_event();
        }
    }

    pub fn disable(&mut self) {
        self.flags.remove(ControllerFlags::ACTIVE);
        self.timeout_at = None;
        self.scrape_at = None;
        self.tracker_list.close_all();
    }

    pub fn close(&mut self) {
        self.tracker_list.close_all();
    }

    pub fn send_start_event(&mut self) {
        self.flags.remove(Self::MASK_SEND);
        self.flags.insert(ControllerFlags::SEND_START);

        if !self.is_active() || !self.tracker_list.has_usable() {
            return;
        }

        self.close();

        let mut found_usable = false;
        let count = self.tracker_list.size();
        for i in 0..count {
            if !self
                .tracker_list
                .at(i)
                .map(|t| t.is_usable())
                .unwrap_or(false)
            {
                continue;
            }

            if found_usable {
                self.flags.insert(ControllerFlags::PROMISCUOUS_MODE);
                self.update_timeout(3);
                break;
            }

            self.tracker_list.send_event(i, TrackerEvent::Started);
            found_usable = true;
        }
    }

    pub fn send_stop_event(&mut self) {
        self.flags.remove(Self::MASK_SEND);
        self.flags.insert(ControllerFlags::SEND_STOP);

        if !self.is_active() || !self.tracker_list.has_usable() {
            return;
        }

        self.close();

        let count = self.tracker_list.size();
        for i in 0..count {
            if self
                .tracker_list
                .at(i)
                .map(|t| t.is_in_use())
                .unwrap_or(false)
            {
                self.tracker_list.send_event(i, TrackerEvent::Stopped);
            }
        }
    }

    pub fn send_completed_event(&mut self) {
        self.flags.remove(Self::MASK_SEND);
        self.flags.insert(ControllerFlags::SEND_COMPLETED);

        if !self.is_active() || !self.tracker_list.has_in_use() {
            return;
        }

        self.close();

        let count = self.tracker_list.size();
        for i in 0..count {
            if self
                .tracker_list
                .at(i)
                .map(|t| t.is_in_use())
                .unwrap_or(false)
            {
                self.tracker_list.send_event(i, TrackerEvent::Completed);
            }
        }
    }

    pub fn send_update_event(&mut self) {
        if !self.is_active() || !self.tracker_list.has_usable() {
            return;
        }

        if self.flags.intersects(Self::MASK_SEND) && self.tracker_list.has_active() {
            return;
        }

        if !self.flags.intersects(Self::MASK_SEND) {
            self.flags.insert(ControllerFlags::SEND_UPDATE);
        }

        // Prefer the tracker with the earliest activity deadline. A tracker
        // that just failed therefore moves behind healthy trackers in the same
        // or a later tier until its backoff expires.
        if let Some(index) = self.tracker_list.find_next_to_request(0) {
            self.tracker_list.send_event(index, TrackerEvent::None);
        }
    }

    pub fn manual_request(&mut self, _request_now: bool) {
        if !self.is_timeout_queued() {
            return;
        }
        self.send_update_event();
    }

    pub fn receive_success(&mut self, index: usize, peers: &AddressList) -> u32 {
        let new_peers = if let Some(ref mut cb) = self.on_success {
            cb(peers)
        } else {
            peers.len() as u32
        };

        let now = self.current_time;
        let promoted = self
            .tracker_list
            .receive_success(index, peers, new_peers, now);
        // A completed announce clears the pending send event and promiscuous
        // retry mode, matching TrackerController::receive_success().
        self.flags.remove(
            Self::MASK_SEND | ControllerFlags::FAILURE_MODE | ControllerFlags::PROMISCUOUS_MODE,
        );

        // Schedule next timeout according to the successful tracker's normal
        // interval (it was promoted to the front of its group).
        let next_sec = self
            .tracker_list
            .at(promoted)
            .map(|t| t.state().normal_interval)
            .unwrap_or(TrackerState::DEFAULT_NORMAL_INTERVAL);
        self.update_timeout(next_sec as u32);

        new_peers
    }

    pub fn receive_failure(&mut self, index: usize, msg: &str) {
        let now = self.current_time;
        self.tracker_list.receive_failure(index, now);
        self.flags.insert(ControllerFlags::FAILURE_MODE);

        if let Some(ref mut cb) = self.on_failure {
            cb(msg);
        }

        // Schedule next timeout with exponential backoff
        let next_timeout = self
            .tracker_list
            .at(index)
            .map(|t| {
                let n = t.state().failed_time_next();
                if n > now {
                    (n - now) as u32
                } else {
                    5
                }
            })
            .unwrap_or(5);
        self.update_timeout(next_timeout);
    }

    pub fn receive_scrape_stats(&mut self, index: usize, stats: ScrapeStats) {
        let now = self.current_time;
        self.last_scrape_error = None;
        self.tracker_list.receive_scrape_stats(index, stats, now);
        self.finish_scrape(index, true);
    }

    pub fn receive_scrape_success(&mut self, index: usize) {
        let now = self.current_time;
        self.last_scrape_error = None;
        self.tracker_list.receive_scrape_success(index, now);
        self.finish_scrape(index, true);
    }

    pub fn receive_scrape_failure(&mut self, index: usize) {
        self.receive_scrape_failure_with_reason(index, "");
    }

    /// Records a scrape failure and reschedules the next attempt using the
    /// tracker minimum interval. This never touches announce success/failure
    /// counters and therefore never demotes a tracker whose scrape endpoint is
    /// unsupported.
    pub fn receive_scrape_failure_with_reason(&mut self, index: usize, reason: &str) {
        let now = self.current_time;
        self.last_scrape_error = if reason.is_empty() {
            None
        } else {
            Some(reason.to_string())
        };
        self.tracker_list.receive_scrape_failure(index, now);

        if let Some(ref mut cb) = self.on_scrape_failure {
            cb(reason);
        }

        self.finish_scrape(index, false);
    }

    /// Marks a scrape as unsupported. The tracker remains usable for announces
    /// but is removed from future scrape scheduling.
    pub fn receive_scrape_unsupported(&mut self, index: usize, reason: &str) {
        let now = self.current_time;
        self.last_scrape_error = if reason.is_empty() {
            None
        } else {
            Some(reason.to_string())
        };
        self.tracker_list.receive_scrape_unsupported(index, now);

        if let Some(ref mut cb) = self.on_scrape_failure {
            cb(reason);
        }

        // Unsupported scrape endpoints are removed from the schedule instead
        // of being retried.
        self.finish_scrape_batch(index, None);
    }

    fn finish_scrape(&mut self, index: usize, success: bool) {
        self.finish_scrape_batch(index, Some(success));
    }

    fn finish_scrape_batch(&mut self, index: usize, success: Option<bool>) {
        if !self.scrape_in_flight {
            // A caller applied a scrape result directly. A success must not
            // invent a schedule (the caller may be updating `t.scrape_*` only),
            // but a failure still arms the minimum-interval retry so a broken
            // scrape endpoint cannot be hammered. Unsupported endpoints are
            // simply removed from the schedule.
            match success {
                Some(true) | None => self.scrape_at = None,
                Some(false) => {
                    let next = self
                        .tracker_list
                        .at(index)
                        .map(|tracker| tracker.state().scrape_time_next_after_failure())
                        .unwrap_or(self.current_time);
                    self.scrape_at = Some(next.max(self.current_time));
                }
            }
            return;
        }

        self.scrape_pending = self.scrape_pending.saturating_sub(1);
        if self.scrape_pending != 0 {
            return;
        }

        self.scrape_in_flight = false;

        let Some(success) = success else {
            self.scrape_at = None;
            return;
        };

        let next = self
            .tracker_list
            .at(index)
            .map(|tracker| {
                if success {
                    tracker.state().scrape_time_next()
                } else {
                    tracker.state().scrape_time_next_after_failure()
                }
            })
            .unwrap_or(0);

        // Both state helpers already return an absolute time. Clamp to the
        // current time so a stale result cannot queue a request in the past.
        self.scrape_at = Some(next.max(self.current_time));
    }

    pub fn check_timers(&mut self) {
        if let Some(t) = self.timeout_at {
            if self.current_time >= t {
                self.timeout_at = None;
                self.do_timeout();
            }
        }
        if let Some(s) = self.scrape_at {
            if self.current_time >= s {
                self.scrape_at = None;
                self.do_scrape();
            }
        }
    }

    /// Processes only the scrape timer. The engine uses this so that announce
    /// scheduling (which it drives itself) is not touched.
    pub fn check_scrape_timer(&mut self) -> bool {
        if let Some(s) = self.scrape_at {
            if self.current_time >= s {
                self.scrape_at = None;
                self.do_scrape();
                return true;
            }
        }
        false
    }

    pub fn do_timeout(&mut self) {
        if let Some(ref mut cb) = self.on_timeout {
            cb();
        }
        self.send_update_event();
    }

    /// Queues at most one scrape per tracker tier. A tier with an outstanding
    /// announce is skipped, matching the C++ `TrackerController::do_scrape()`.
    pub fn do_scrape(&mut self) {
        if self.scrape_in_flight {
            return;
        }

        self.scrape_at = None;

        let mut due = Vec::new();
        let groups = self.tracker_list.size_group();

        for group in 0..groups {
            if self
                .tracker_list
                .has_active_not_scrape_in_group(group as u32)
            {
                continue;
            }

            let begin = self.tracker_list.begin_group(group as u32);
            let end = self.tracker_list.end_group(group as u32);

            for index in begin..end {
                let usable = self
                    .tracker_list
                    .at(index)
                    .map(|t| t.is_usable() && t.is_scrapable() && !t.is_requesting())
                    .unwrap_or(false);
                if usable {
                    due.push(index);
                    break;
                }
            }
        }

        let now = self.current_time;
        let mut sent = 0usize;
        for &index in &due {
            if self.tracker_list.send_scrape_at(index, now) {
                sent += 1;
            }
        }

        if sent != 0 {
            self.scrape_pending = sent;
            self.scrape_in_flight = true;
        } else if !due.is_empty() {
            // Candidates existed but the minimum interval blocked them. Keep
            // a retry queued instead of silently dropping the schedule.
            let next = due
                .iter()
                .filter_map(|&index| {
                    self.tracker_list
                        .at(index)
                        .map(|tracker| tracker.state().scrape_time_next_minimum())
                })
                .min()
                .unwrap_or(now);

            self.scrape_at = Some(next.max(now));
        }
    }
}
