//! Disk write throughput across 1 vs N pool workers.
//!
//! Ignored by default: it writes real data and quotes timings. Run with
//! `cargo test -p lt-storage --test test_disk_pool_throughput -- --ignored --nocapture`.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use lt_common::hash_string::HashString;
use lt_storage::disk_pool::{DiskPool, DiskPoolConfig};
use lt_storage::writer::TargetFileInfo;

const PIECE: usize = 1024 * 1024;
const PIECES: usize = 64;
const TORRENTS: usize = 4;

fn hash_for(index: usize) -> HashString {
    let mut bytes = [0u8; 20];
    bytes[3] = index as u8;
    HashString::new(bytes)
}

fn pool(threads: usize) -> Arc<DiskPool> {
    DiskPool::new(DiskPoolConfig {
        threads,
        queue_capacity: 32,
        max_open_files: 16,
        idle_timeout: Duration::from_secs(30),
    })
    .unwrap()
}

fn write_workload(pool: &DiskPool, root: &Path) -> Duration {
    let started = Instant::now();
    std::thread::scope(|scope| {
        for torrent in 0..TORRENTS {
            let thread = pool.thread_for(&hash_for(torrent));
            let files = Arc::new(vec![TargetFileInfo {
                path: PathBuf::from(format!("payload-{torrent}.bin")),
                length: (PIECE * PIECES) as u64,
                offset: 0,
            }]);
            let root = root.to_path_buf();
            scope.spawn(move || {
                for piece in 0..PIECES {
                    thread
                        .submit_write(
                            root.clone(),
                            Arc::clone(&files),
                            PIECE as u32,
                            piece as u32,
                            vec![torrent as u8; PIECE],
                        )
                        .unwrap()
                        .wait()
                        .unwrap();
                }
            });
        }
    });
    started.elapsed()
}

#[test]
#[ignore = "scale: writes 256 MiB twice; run with --ignored --nocapture"]
fn parallel_writes_scale_with_the_worker_count() {
    let root =
        std::env::temp_dir().join(format!("rtorrent_pool_throughput_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();

    let total_mb = (TORRENTS * PIECES * PIECE) as f64 / 1e6;

    let serial_pool = pool(1);
    let serial = write_workload(&serial_pool, &root);
    serial_pool.shutdown();
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();

    let parallel_pool = pool(4);
    let parallel = write_workload(&parallel_pool, &root);
    parallel_pool.shutdown();

    for torrent in 0..TORRENTS {
        let data = std::fs::read(root.join(format!("payload-{torrent}.bin"))).unwrap();
        assert_eq!(data.len(), PIECE * PIECES);
        assert!(
            data.chunks(PIECE)
                .all(|chunk| chunk.iter().all(|&byte| byte == torrent as u8)),
            "torrent {torrent} payload is corrupted"
        );
    }
    let _ = std::fs::remove_dir_all(&root);

    println!(
        "1 worker: {:.2}s ({:.1} MB/s) | 4 workers: {:.2}s ({:.1} MB/s) | speedup {:.2}x",
        serial.as_secs_f64(),
        total_mb / serial.as_secs_f64(),
        parallel.as_secs_f64(),
        total_mb / parallel.as_secs_f64(),
        serial.as_secs_f64() / parallel.as_secs_f64(),
    );
}
