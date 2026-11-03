//! Multi-threaded concurrency subsystem for lock-free UI rendering and Rayon computation.

pub mod aggregator;
pub mod compute;

pub use aggregator::{
    sample_engine_snapshot, sample_engine_snapshot_with_stats, EngineSnapshotBroadcaster,
};
pub use compute::UiComputeEngine;
