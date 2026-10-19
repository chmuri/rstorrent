use crate::choke_queue::{ChokeQueue, Heuristics, FLAG_UNCHOKE_ALL_NEW, UNLIMITED_SLOTS};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TrackerMode {
    #[default]
    Normal,
    Aggressive,
}

/// Seeding-ratio rules for a group (`group.seeding.ratio.*`).
///
/// Values use the legacy fixed-point convention: `min = 200.0` means a ratio
/// of 2.00. `upload` is the minimum number of uploaded bytes.
#[derive(Debug, Clone, PartialEq)]
pub struct RatioConfig {
    pub enabled: bool,
    pub min: f64,
    pub max: f64,
    pub upload: u64,
    pub command: String,
}

impl Default for RatioConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            min: 200.0,
            max: 300.0,
            upload: 20 << 20,
            command: "d.try_close= ;d.ignore_commands.set=1".to_string(),
        }
    }
}

impl RatioConfig {
    /// Ratio as a percentage (`100` means 1.00). A torrent that has not
    /// downloaded anything reports an infinite ratio once it uploaded.
    pub fn ratio_percent(uploaded: u64, downloaded: u64) -> f64 {
        if downloaded == 0 {
            if uploaded == 0 {
                0.0
            } else {
                f64::INFINITY
            }
        } else {
            uploaded as f64 * 100.0 / downloaded as f64
        }
    }

    /// Returns whether the group may keep seeding at `ratio` percent.
    pub fn should_seed(&self, ratio: f64) -> bool {
        if !self.enabled {
            return true;
        }
        self.max <= 0.0 || ratio < self.max
    }

    pub fn is_below_min(&self, ratio: f64) -> bool {
        self.enabled && ratio < self.min
    }

    /// Legacy `on_ratio` predicate: the minimum is reached (upload threshold
    /// and ratio) or the maximum was exceeded.
    pub fn ratio_reached(&self, uploaded: u64, downloaded: u64) -> bool {
        if !self.enabled {
            return false;
        }

        // A torrent that never downloaded anything only has the absolute
        // upload threshold to satisfy; this also avoids a division by zero.
        if downloaded == 0 {
            return uploaded >= self.upload;
        }

        let ratio = Self::ratio_percent(uploaded, downloaded);
        let min_reached = uploaded >= self.upload && ratio >= self.min;
        let max_exceeded = self.max > 0.0 && ratio > self.max;
        min_reached || max_exceeded
    }

    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    pub fn set_min(&mut self, min: f64) {
        self.min = min.max(0.0);
    }

    pub fn set_max(&mut self, max: f64) {
        self.max = max.max(0.0);
    }

    pub fn set_upload(&mut self, upload: u64) {
        self.upload = upload;
    }

    pub fn set_command(&mut self, command: impl Into<String>) {
        self.command = command.into();
    }
}

/// ChokeGroup binds an upload and download choke queue, rates, and tracker mode.
#[derive(Debug, Clone)]
pub struct ChokeGroup {
    name: String,
    tracker_mode: TrackerMode,
    up_queue: ChokeQueue,
    down_queue: ChokeQueue,
    up_rate: u64,
    down_rate: u64,
    priority: u32,
    ratio: RatioConfig,
    up_min: u32,
    up_max: u32,
    up_unlimited: bool,
    down_min: u32,
    down_max: u32,
    down_unlimited: bool,
}

impl Default for ChokeGroup {
    fn default() -> Self {
        Self::new("default")
    }
}

impl ChokeGroup {
    pub fn new(name: impl Into<String>) -> Self {
        let mut down_queue = ChokeQueue::new(FLAG_UNCHOKE_ALL_NEW);
        down_queue.set_heuristics(Heuristics::DownloadLeech);

        Self {
            name: name.into(),
            tracker_mode: TrackerMode::Normal,
            up_queue: ChokeQueue::new(0),
            down_queue,
            up_rate: 0,
            down_rate: 0,
            priority: 1,
            ratio: RatioConfig::default(),
            up_min: 0,
            up_max: UNLIMITED_SLOTS,
            up_unlimited: true,
            down_min: 0,
            down_max: UNLIMITED_SLOTS,
            down_unlimited: true,
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn set_name(&mut self, name: impl Into<String>) {
        self.name = name.into();
    }

    pub fn tracker_mode(&self) -> TrackerMode {
        self.tracker_mode
    }

    pub fn set_tracker_mode(&mut self, mode: TrackerMode) {
        self.tracker_mode = mode;
    }

    pub fn up_queue(&self) -> &ChokeQueue {
        &self.up_queue
    }

    pub fn up_queue_mut(&mut self) -> &mut ChokeQueue {
        &mut self.up_queue
    }

    pub fn down_queue(&self) -> &ChokeQueue {
        &self.down_queue
    }

    pub fn down_queue_mut(&mut self) -> &mut ChokeQueue {
        &mut self.down_queue
    }

    pub fn up_rate(&self) -> u64 {
        self.up_rate
    }

    pub fn set_up_rate(&mut self, rate: u64) {
        self.up_rate = rate;
    }

    pub fn down_rate(&self) -> u64 {
        self.down_rate
    }

    pub fn set_down_rate(&mut self, rate: u64) {
        self.down_rate = rate;
    }

    pub fn priority(&self) -> u32 {
        self.priority
    }

    pub fn set_priority(&mut self, priority: u32) {
        self.priority = priority.max(1);
    }

    pub fn ratio(&self) -> &RatioConfig {
        &self.ratio
    }

    pub fn ratio_mut(&mut self) -> &mut RatioConfig {
        &mut self.ratio
    }

    pub fn up_unchoked(&self) -> u32 {
        self.up_queue.size_unchoked()
    }

    pub fn down_unchoked(&self) -> u32 {
        self.down_queue.size_unchoked()
    }

    pub fn up_queued(&self) -> u32 {
        self.up_queue.size_queued()
    }

    pub fn down_queued(&self) -> u32 {
        self.down_queue.size_queued()
    }

    pub fn up_total(&self) -> u32 {
        self.up_queue.size_total()
    }

    pub fn down_total(&self) -> u32 {
        self.down_queue.size_total()
    }

    /// Persistent upload limit. `UNLIMITED_SLOTS` means no group cap.
    pub fn up_max(&self) -> u32 {
        self.up_max
    }

    pub fn down_max(&self) -> u32 {
        self.down_max
    }

    pub fn up_unlimited(&self) -> bool {
        self.up_unlimited
    }

    pub fn down_unlimited(&self) -> bool {
        self.down_unlimited
    }

    pub fn up_min(&self) -> u32 {
        self.up_min
    }

    pub fn down_min(&self) -> u32 {
        self.down_min
    }

    /// Sets the upload cap and keeps `min <= max`.
    pub fn set_up_max(&mut self, max: u32) {
        if max == UNLIMITED_SLOTS {
            self.up_unlimited = true;
            self.up_max = UNLIMITED_SLOTS;
        } else {
            self.up_unlimited = false;
            self.up_max = max;
            self.up_min = self.up_min.min(max);
        }
        self.up_queue.set_max_unchoked(self.up_max);
    }

    pub fn set_down_max(&mut self, max: u32) {
        if max == UNLIMITED_SLOTS {
            self.down_unlimited = true;
            self.down_max = UNLIMITED_SLOTS;
        } else {
            self.down_unlimited = false;
            self.down_max = max;
            self.down_min = self.down_min.min(max);
        }
        self.down_queue.set_max_unchoked(self.down_max);
    }

    /// Sets the group minimum, clamped to the group maximum.
    pub fn set_up_min(&mut self, min: u32) {
        let min = if self.up_unlimited {
            min
        } else {
            min.min(self.up_max)
        };
        self.up_min = min;
    }

    pub fn set_down_min(&mut self, min: u32) {
        let min = if self.down_unlimited {
            min
        } else {
            min.min(self.down_max)
        };
        self.down_min = min;
    }

    pub fn set_up_rate_limit(&mut self, limit: u32) {
        self.set_up_max(limit);
    }

    pub fn set_down_rate_limit(&mut self, limit: u32) {
        self.set_down_max(limit);
    }

    /// Number of upload slots this group would use if it had them.
    ///
    /// Both the configured group limit and the queue's own limit (which
    /// external callers may set directly) are honoured. A group without
    /// interested peers requests zero slots.
    pub fn up_requested(&self) -> u32 {
        let cap = self.up_max.min(self.up_queue.max_unchoked());
        self.up_queue.size_total().min(cap)
    }

    pub fn down_requested(&self) -> u32 {
        let cap = self.down_max.min(self.down_queue.max_unchoked());
        self.down_queue.size_total().min(cap)
    }

    /// Maps the legacy string names to queue heuristics.
    pub fn set_up_heuristics(&mut self, heuristics: Heuristics) {
        self.up_queue.set_heuristics(heuristics);
    }

    pub fn set_down_heuristics(&mut self, heuristics: Heuristics) {
        self.down_queue.set_heuristics(heuristics);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_choke_group_basic() {
        let mut group = ChokeGroup::new("high_priority");
        assert_eq!(group.name(), "high_priority");
        assert_eq!(group.tracker_mode(), TrackerMode::Normal);

        group.set_tracker_mode(TrackerMode::Aggressive);
        assert_eq!(group.tracker_mode(), TrackerMode::Aggressive);

        group.set_up_max(10);
        group.set_down_max(5);

        assert_eq!(group.up_queue().max_unchoked(), 10);
        assert_eq!(group.down_queue().max_unchoked(), 5);
        assert!(!group.up_unlimited());
        assert!(!group.down_unlimited());

        group.set_up_max(UNLIMITED_SLOTS);
        assert!(group.up_unlimited());
    }

    #[test]
    fn ratio_helpers_use_percent_units() {
        let mut config = RatioConfig::default();
        config.enabled = true;
        config.min = 200.0;
        config.max = 300.0;
        config.upload = 100;

        assert_eq!(RatioConfig::ratio_percent(200, 100), 200.0);
        assert!(!config.ratio_reached(10, 0));
        assert!(config.ratio_reached(100, 0));
        assert!(!config.ratio_reached(199, 100));
        assert!(config.ratio_reached(200, 100));
        assert!(config.ratio_reached(301, 100));
        assert!(config.should_seed(299.0));
        assert!(!config.should_seed(300.0));
    }
}
