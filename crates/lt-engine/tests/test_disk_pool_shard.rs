//! End-to-end check that a download's I/O follows its pool shard.
//!
//! Kept in its own test binary because the pool is process-global: the first
//! initialisation wins for the whole process.

use std::sync::Arc;
use std::time::{Duration, Instant};

use lt_common::hash_string::HashString;
use lt_common::path::Path as TorrentPath;
use lt_crypto::sha1::sha1_hash;
use lt_engine::constructor::{TorrentFileEntry, TorrentMetadata};
use lt_engine::download::Download;
use lt_storage::{
    global_disk_thread_for, init_global_disk_pool, DiskPoolConfig, DEFAULT_DISK_QUEUE,
    DEFAULT_IDLE_TIMEOUT,
};

fn metadata_for(payload: &[u8], name: &str) -> TorrentMetadata {
    let piece_hash = sha1_hash(payload);
    let mut file_path = TorrentPath::new();
    file_path.push("payload.bin");

    TorrentMetadata {
        info_hash: HashString::new(sha1_hash(&piece_hash)),
        name: name.to_string(),
        piece_length: payload.len() as u32,
        pieces: piece_hash.to_vec(),
        num_pieces: 1,
        total_size: payload.len() as u64,
        is_multi_file: false,
        files: vec![TorrentFileEntry {
            path: file_path,
            length: payload.len() as u64,
            offset: 0,
        }],
        announce: None,
        announce_list: Vec::new(),
        is_private: false,
        is_meta_download: false,
    }
}

#[test]
fn download_io_uses_the_pool_shard_for_writes_and_syncs() {
    let _ = init_global_disk_pool(DiskPoolConfig {
        threads: 4,
        queue_capacity: DEFAULT_DISK_QUEUE,
        max_open_files: 8,
        idle_timeout: DEFAULT_IDLE_TIMEOUT,
    });

    let payload = b"pool-sharded-payload".to_vec();
    let metadata = metadata_for(&payload, "pool_shard");
    let hash = metadata.info_hash;

    let root = std::env::temp_dir().join(format!("rtorrent_pool_shard_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();

    let mut download = Download::new(metadata);
    let shard = global_disk_thread_for(&hash).expect("pool must exist");
    assert!(
        Arc::ptr_eq(download.disk_thread().expect("shard captured"), &shard),
        "a download must capture the shard that owns its info hash"
    );

    download.set_save_directory(&root);
    download.start();

    assert!(download.receive_block(0, 0, &payload).unwrap());

    let deadline = Instant::now() + Duration::from_secs(10);
    while !download.is_seeder() && Instant::now() < deadline {
        download.poll_disk_completions();
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(download.is_seeder(), "write never completed on the shard");

    // The write went through the shard, so a sync on that same shard makes it
    // durable; a different worker's sync would not.
    download.flush_disk().expect("flush on the shard");
    assert_eq!(std::fs::read(root.join("payload.bin")).unwrap(), payload);

    let _ = std::fs::remove_dir_all(&root);
}
