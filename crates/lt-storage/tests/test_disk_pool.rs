//! Sharding, isolation and aggregation of the disk worker pool.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use lt_common::hash_string::HashString;
use lt_storage::disk_pool::{DiskPool, DiskPoolConfig};
use lt_storage::writer::TargetFileInfo;

fn temp_root(tag: &str) -> PathBuf {
    let root =
        std::env::temp_dir().join(format!("rtorrent_disk_pool_{}_{}", tag, std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    root
}

fn hash_for_shard(shard: u8) -> HashString {
    let mut bytes = [0u8; 20];
    // thread_for hashes the first four bytes big-endian; keep 0..3 zero and
    // use the fourth byte as the shard index.
    bytes[3] = shard;
    HashString::new(bytes)
}

fn payload_files(length: u64) -> Arc<Vec<TargetFileInfo>> {
    Arc::new(vec![TargetFileInfo {
        path: PathBuf::from("payload.bin"),
        length,
        offset: 0,
    }])
}

fn pool(threads: usize) -> Arc<DiskPool> {
    DiskPool::new(DiskPoolConfig {
        threads,
        queue_capacity: 16,
        max_open_files: 8,
        idle_timeout: Duration::from_secs(30),
    })
    .unwrap()
}

#[test]
fn shard_assignment_is_deterministic_per_hash() {
    let pool = pool(4);
    for shard in 0..4u8 {
        let hash = hash_for_shard(shard);
        let first = pool.thread_for(&hash);
        let second = pool.thread_for(&hash);
        assert!(
            Arc::ptr_eq(&first, &second),
            "the same hash must always map to the same worker"
        );
        assert!(
            Arc::ptr_eq(&first, &pool.workers()[shard as usize]),
            "hash {shard} should map to worker {shard}"
        );
    }
}

#[test]
fn a_single_worker_pool_keeps_everything_on_one_queue() {
    let pool = pool(1);
    for shard in 0..8u8 {
        assert!(Arc::ptr_eq(
            &pool.thread_for(&hash_for_shard(shard)),
            &pool.workers()[0]
        ));
    }
}

#[test]
fn writes_land_on_the_shard_that_owns_the_hash() {
    let root = temp_root("isolation");
    let pool = pool(2);
    let files = payload_files(8);

    let hash = hash_for_shard(1);
    let worker = pool.thread_for(&hash);
    let before_good = pool.workers()[0].stats().writes_submitted;
    let before_bad = pool.workers()[1].stats().writes_submitted;

    worker
        .submit_write(root.clone(), Arc::clone(&files), 8, 0, vec![7u8; 8])
        .unwrap()
        .wait()
        .unwrap();

    assert_eq!(pool.workers()[0].stats().writes_submitted, before_good);
    assert_eq!(pool.workers()[1].stats().writes_submitted, before_bad + 1);
    assert_eq!(
        std::fs::read(root.join("payload.bin")).unwrap(),
        vec![7u8; 8]
    );

    pool.shutdown();
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn aggregate_stats_sum_every_worker() {
    let root = temp_root("aggregate");
    let pool = pool(2);
    let files = payload_files(16);

    for shard in 0..2u8 {
        pool.thread_for(&hash_for_shard(shard))
            .submit_write(
                root.clone(),
                Arc::clone(&files),
                8,
                shard as u32,
                vec![shard; 8],
            )
            .unwrap()
            .wait()
            .unwrap();
    }

    let stats = pool.stats();
    assert_eq!(stats.writes_submitted, 2);
    assert_eq!(stats.writes_completed, 2);
    assert_eq!(stats.bytes_written, 16);
    assert_eq!(stats.queue_capacity, 32, "queue capacity is summed");

    pool.shutdown();
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn shutdown_drains_pending_writes_on_every_worker() {
    let root = temp_root("shutdown");
    let pool = pool(4);
    let files = payload_files(32);

    for shard in 0..4u8 {
        pool.thread_for(&hash_for_shard(shard))
            .submit_write(
                root.clone(),
                Arc::clone(&files),
                8,
                shard as u32,
                vec![shard; 8],
            )
            .unwrap();
    }

    pool.shutdown();

    let data = std::fs::read(root.join("payload.bin")).unwrap();
    for shard in 0..4u8 {
        let start = shard as usize * 8;
        assert_eq!(&data[start..start + 8], &vec![shard; 8][..]);
    }

    let _ = std::fs::remove_dir_all(&root);
}
