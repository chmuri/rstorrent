//! Crate `lt-tracker`
//!
//! BitTorrent tracker subsystem:
//! - HTTP Tracker client & Scrape URI parsing
//! - UDP Tracker binary protocol (BEP 15) & UdpRouter with 256-bit PRNG
//! - Tracker list management with tiered multi-trackers, cycling, and promotion (BEP 12)
//! - Tracker controller with timeout scheduling and exponential backoff

pub mod client;
pub mod controller;
pub mod http;
pub mod list;
pub mod state;
pub mod tracker;
pub mod udp;
pub mod udp_router;

pub use client::{
    announce, announce_detailed, announce_http, announce_http_detailed, announce_udp,
    announce_udp_detailed_with_router, announce_udp_with_router, scrape, scrape_http, scrape_udp,
    scrape_udp_with_router, scrape_with_router, AnnounceParams, ScrapeStats, TrackerAnnounceResult,
    TrackerClientError,
};
pub use controller::{ControllerFlags, TrackerController};
pub use http::{
    build_announce_url, uri_can_scrape, uri_generate_scrape_url, url_encode_bytes,
    HttpAnnounceResponse, HttpTrackerError,
};
pub use list::TrackerList;
pub use state::{TrackerCounters, TrackerEvent, TrackerFlags, TrackerParams, TrackerState};
pub use tracker::{Tracker, TrackerType};
pub use udp::{
    AnnounceRequest, AnnounceResponse, ConnectRequest, ConnectResponse, ErrorResponse,
    ScrapeFileStats, ScrapeRequest, ScrapeResponse, UdpTrackerError, ACTION_ANNOUNCE,
    ACTION_CONNECT, ACTION_ERROR, ACTION_SCRAPE, MAGIC_CONNECTION_ID,
};
pub use udp_router::{CachedConnection, RandomEngine, UdpRouter};
