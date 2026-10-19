//! Crate `lt-engine`
//!
//! Core BitTorrent engine components:
//! - ChunkSelector: Rarest-First, priority-based, sequential piece picking
//! - ChunkStatistics: Swarm availability and piece rarity accounting
//! - Delegator: Block delegation and Endgame mode
//! - ChokeQueue: BitTorrent unchoke slot management & tit-for-tat heuristics
//! - ChokeGroup: Upload & download choke queues and bandwidth grouping
//! - DownloadConstructor: Parser for .torrent Bencode and magnet URIs
//! - Download: Complete download controller integrating storage, peers, tracking
//! - PeerPool: Unified tracker/DHT/PEX/inbound discovery with connection scheduling
//! - EngineManager: Central BitTorrent engine coordinator

pub mod choke_group;
pub mod choke_queue;
pub mod chunk_selector;
pub mod client_config;
pub mod chunk_statistics;
pub mod constructor;
pub mod delegator;
pub mod download;
pub mod engine;
pub mod network;
pub mod peer_pool;
pub mod resource_manager;

pub use client_config::ClientConfig;
pub use choke_group::{ChokeGroup, TrackerMode};
pub use choke_queue::{ChokeEntry, ChokeQueue, Heuristics, WeightedPeer, FLAG_UNCHOKE_ALL_NEW, ORDER_BASE, UNLIMITED_SLOTS};
pub use chunk_selector::{ChunkSelector, INVALID_CHUNK};
pub use chunk_statistics::{AvailableList, ChunkStatistics, PeerAvailability, MAX_ACCOUNTED};
pub use constructor::{ConstructorError, DownloadConstructor, TorrentFileEntry, TorrentMetadata};
pub use delegator::Delegator;
pub use download::{Download, DownloadState};
pub use engine::EngineManager;
pub use resource_manager::{allocate_quotas, ResourceEntry, ResourceManager};
pub use network::{
    run_inbound_mse_peer, run_inbound_mse_peer_with_pool, run_inbound_peer,
    run_inbound_peer_with_pool, DownloadNetworkCoordinator,
};
pub use peer_pool::{
    max_connecting_for, normalize_socket_addr, PeerEntry, PeerPool, PeerPoolConfig,
    PeerPoolHandle, PeerPoolStats, PeerSource, PeerState, DEFAULT_DISCONNECT_COOLDOWN,
    DEFAULT_MAX_CONNECTING, DEFAULT_RETRY_BASE, DEFAULT_RETRY_MAX,
};
