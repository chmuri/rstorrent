//! Crate `lt-dht`
//!
//! BitTorrent Distributed Hash Table (DHT) based on Kademlia (BEP 5).

pub mod bucket;
pub mod controller;
pub mod error;
pub mod node;
pub mod router;
pub mod search;
pub mod server;
pub mod tracker;
pub mod transaction;

pub use bucket::{DhtBucket, NUM_NODES};
pub use controller::{
    ControllerStatistics, DhtController, DhtHandle, DhtLifecycleConfig, DhtMode,
    BOOTSTRAP_COMPLETE_NODES, DEFAULT_BOOTSTRAP_RETRY_INTERVAL, DEFAULT_REFRESH_INTERVAL,
    MAX_BOOTSTRAP_CONTACTS,
};
pub use error::DhtError;
pub use node::{is_persistable_address, DhtNode, MAX_FAILED_REPLIES};
pub use router::{
    DhtRouter, DhtStatistics, MAX_TRACKERS, NUM_TRACKER_EVICT, TIMEOUT_PEER_ANNOUNCE,
    TIMEOUT_REMOVE_NODE, TIMEOUT_TRACKER_EVICT, TIMEOUT_UPDATE,
};
pub use search::{announce_peer, get_peers, ALPHA, MAX_QUERIES};
pub use server::{DhtServer, ServerStatistics, QUERY_TIMEOUT};
pub use tracker::{DhtTracker, MAX_PEERS, MAX_SIZE};
pub use transaction::{KrpcError, KrpcMessage, KrpcQuery, KrpcResponse};
