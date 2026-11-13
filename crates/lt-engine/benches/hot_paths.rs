//! Criterion benchmarks for the engine hot paths (plan.md :260).
//!
//! Run with `cargo bench -p lt-engine --bench hot_paths`. The five groups track
//! piece selection, SHA-1 hashing, disk I/O, rarest-first accounting and the BDP
//! request pipeline so a regression surfaces as a relative slowdown.

use std::fs;
use std::hint::black_box;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use criterion::{criterion_group, criterion_main, BatchSize, BenchmarkId, Criterion, Throughput};

use lt_common::{Bitfield, HashString, PartialQueue};
use lt_crypto::sha1::sha1_hash;
use lt_engine::{ChunkSelector, ChunkStatistics};
use lt_protocol::request_list::{BlockTransfer, RequestList};
use lt_storage::disk_thread::DiskThread;
use lt_storage::writer::TargetFileInfo;

const PIECES: usize = 4096;
const BLOCK: u32 = 16_384;
const PIECE_BYTES: u32 = 262_144;

fn bench_sha1(c: &mut Criterion) {
    let mut group = c.benchmark_group("sha1");
    for size in [16_384usize, 262_144, 1_048_576] {
        let data = vec![0xABu8; size];
        group.throughput(Throughput::Bytes(size as u64));
        group.bench_with_input(BenchmarkId::from_parameter(size), &data, |b, data| {
            b.iter(|| black_box(sha1_hash(black_box(data.as_slice()))));
        });
    }
    group.finish();
}

fn bench_rarest_first(c: &mut Criterion) {
    c.bench_function("rarest_first/accounting", |b| {
        b.iter_batched(
            || {
                (0..32u32)
                    .map(|peer| {
                        let mut id = [0u8; 20];
                        id[..4].copy_from_slice(&peer.to_be_bytes());
                        let mut bf = Bitfield::new(PIECES);
                        for piece in 0..PIECES {
                            if !(piece * 31 + peer as usize * 7).is_multiple_of(5) {
                                bf.set(piece);
                            }
                        }
                        (HashString::new(id), bf)
                    })
                    .collect::<Vec<_>>()
            },
            |peers| {
                let mut stats = ChunkStatistics::new(PIECES);
                for (id, bf) in &peers {
                    stats.add_peer(*id, bf);
                }
                let mut rarest = u16::MAX;
                for piece in 0..PIECES {
                    rarest = rarest.min(stats.rarity(piece));
                }
                black_box(rarest)
            },
            BatchSize::SmallInput,
        );
    });
}

fn bench_piece_selection(c: &mut Criterion) {
    c.bench_function("piece_selection/find", |b| {
        b.iter_batched(
            || {
                let selector = ChunkSelector::new(PIECES);
                let stats = ChunkStatistics::new(PIECES);
                let mut peer_bf = Bitfield::new(PIECES);
                for piece in 0..PIECES {
                    if piece.is_multiple_of(2) {
                        peer_bf.set(piece);
                    }
                }
                let cache = PartialQueue::new();
                (selector, stats, peer_bf, cache)
            },
            |(mut selector, stats, peer_bf, mut cache)| {
                let mut picked = 0u32;
                for _ in 0..256 {
                    if let Some(index) = selector.find(&peer_bf, &mut cache, false, &stats) {
                        selector.mark_completed(index);
                        picked += 1;
                    }
                }
                black_box(picked)
            },
            BatchSize::SmallInput,
        );
    });

    c.bench_function("piece_selection/collect_missing", |b| {
        b.iter_batched(
            || ChunkSelector::new(PIECES),
            |selector| black_box(selector.collect_missing(64)),
            BatchSize::SmallInput,
        );
    });
}

fn temp_root(tag: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "rtorrent_bench_{}_{}_{}",
        tag,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    root
}

fn bench_disk_io(c: &mut Criterion) {
    let root = temp_root("disk");
    let files = Arc::new(vec![TargetFileInfo {
        path: PathBuf::from("bench.bin"),
        length: PIECE_BYTES as u64,
        offset: 0,
    }]);
    let thread = DiskThread::spawn(16).unwrap();
    let piece = vec![0xCDu8; PIECE_BYTES as usize];

    let mut group = c.benchmark_group("disk_io");
    group.throughput(Throughput::Bytes(PIECE_BYTES as u64));
    group.bench_function("write_piece", |b| {
        b.iter(|| {
            thread
                .submit_write(
                    &root,
                    Arc::clone(&files),
                    PIECE_BYTES,
                    0,
                    black_box(piece.clone()),
                )
                .unwrap()
                .wait()
                .unwrap();
        });
    });
    // The upload read path: a block-sized read misses the whole-piece write-through
    // entry, then warms the piece cache exactly like a real upload request.
    group.bench_function("read_block", |b| {
        thread
            .submit_write(&root, Arc::clone(&files), PIECE_BYTES, 0, piece.clone())
            .unwrap()
            .wait()
            .unwrap();
        b.iter(|| {
            black_box(
                thread
                    .read_block(&root, Arc::clone(&files), PIECE_BYTES, 0, 0, BLOCK)
                    .unwrap(),
            )
        });
    });
    group.finish();

    thread.shutdown();
    let _ = fs::remove_dir_all(&root);
}

fn bench_bdp_pipeline(c: &mut Criterion) {
    c.bench_function("bdp_pipeline/register_receive", |b| {
        b.iter(|| {
            let mut requests = RequestList::with_queue_limit(64);
            requests.enable_dynamic_pipelining(true);
            requests.set_block_size(BLOCK);
            let start = Instant::now();
            let mut now = start;
            for index in 0..512u32 {
                let transfer = BlockTransfer::new(index / 64, (index % 64) * BLOCK, BLOCK);
                let sent_at = now;
                now += Duration::from_micros(200);
                if requests.register(transfer, sent_at) {
                    requests.on_block_received(BLOCK as usize, Some(sent_at), now);
                    let _ = requests.take_block(transfer);
                }
            }
            black_box(requests.queue_limit())
        });
    });
}

criterion_group!(
    benches,
    bench_sha1,
    bench_rarest_first,
    bench_piece_selection,
    bench_disk_io,
    bench_bdp_pipeline
);
criterion_main!(benches);
