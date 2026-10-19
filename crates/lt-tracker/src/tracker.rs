use crate::http::uri_can_scrape;
use crate::state::{TrackerEvent, TrackerFlags, TrackerState};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrackerType {
    Http,
    Udp,
    Dht,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tracker {
    group: u32,
    url: String,
    tracker_type: TrackerType,
    state: TrackerState,
}

impl Tracker {
    pub fn new(group: u32, url: String) -> Self {
        Self::new_with_flags(group, url, TrackerFlags::ENABLED)
    }

    pub fn new_with_flags(group: u32, url: String, flags: TrackerFlags) -> Self {
        let tracker_type = if url.starts_with("udp://") {
            TrackerType::Udp
        } else if url.starts_with("dht://") {
            TrackerType::Dht
        } else {
            TrackerType::Http
        };

        let mut state = TrackerState::default();
        state.flags = flags;
        // HTTP trackers are scrapable when the announce URL can be rewritten
        // to `/scrape`. BEP 15 UDP trackers support scrape natively.
        if (tracker_type == TrackerType::Http && uri_can_scrape(&url))
            || tracker_type == TrackerType::Udp
        {
            state.flags.insert(TrackerFlags::SCRAPABLE);
        }

        Self {
            group,
            url,
            tracker_type,
            state,
        }
    }

    pub fn group(&self) -> u32 {
        self.group
    }

    pub fn set_group(&mut self, g: u32) {
        self.group = g;
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    pub fn tracker_type(&self) -> TrackerType {
        self.tracker_type
    }

    pub fn state(&self) -> &TrackerState {
        &self.state
    }

    pub fn state_mut(&mut self) -> &mut TrackerState {
        &mut self.state
    }

    pub fn is_usable(&self) -> bool {
        self.state.is_enabled() && !self.state.is_deleted()
    }

    pub fn is_in_use(&self) -> bool {
        self.state.is_in_use()
    }

    pub fn is_requesting(&self) -> bool {
        self.state.is_requesting()
    }

    pub fn is_requesting_not_scrape(&self) -> bool {
        self.state.is_requesting() && self.state.latest_event != TrackerEvent::Scrape
    }

    pub fn can_request_state(&self) -> bool {
        self.is_usable() && !self.is_requesting()
    }

    pub fn is_scrapable(&self) -> bool {
        self.state.is_scrapable()
    }

    pub fn enable(&mut self) {
        self.state.enable();
    }

    pub fn disable(&mut self) {
        self.state.disable();
    }

    pub fn clear_stats(&mut self) {
        self.state.clear_stats();
    }
}
