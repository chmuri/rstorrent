//! Behaviour of the bounded hash scheduler and its wiring into
//! `DownloadList`.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use rt_core::download_list::DownloadList;
use rt_core::hash_scheduler::HashScheduler;

fn wait_until(mut predicate: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !predicate() {
        assert!(Instant::now() < deadline, "condition not reached in time");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn concurrency_never_exceeds_the_worker_count() {
    let scheduler = HashScheduler::new(2);
    let completed = Arc::new(AtomicUsize::new(0));

    for _ in 0..20 {
        let completed = Arc::clone(&completed);
        assert!(scheduler.submit(Box::new(move || {
            std::thread::sleep(Duration::from_millis(10));
            completed.fetch_add(1, Ordering::SeqCst);
        })));
    }

    wait_until(|| {
        completed.load(Ordering::SeqCst) == 20 && scheduler.active() == 0 && scheduler.queued() == 0
    });
    assert!(
        scheduler.peak() <= 2,
        "peak concurrency {} exceeded the pool size",
        scheduler.peak()
    );
    assert_eq!(scheduler.active(), 0);
    assert_eq!(scheduler.queued(), 0);
}

#[test]
fn a_single_worker_runs_jobs_strictly_one_at_a_time() {
    let scheduler = HashScheduler::new(1);
    let completed = Arc::new(AtomicUsize::new(0));

    for _ in 0..10 {
        let completed = Arc::clone(&completed);
        assert!(scheduler.submit(Box::new(move || {
            completed.fetch_add(1, Ordering::SeqCst);
        })));
    }

    wait_until(|| completed.load(Ordering::SeqCst) == 10 && scheduler.active() == 0);
    assert_eq!(scheduler.peak(), 1);
}

#[test]
fn every_queued_job_runs() {
    let scheduler = HashScheduler::new(4);
    let completed = Arc::new(AtomicUsize::new(0));

    for _ in 0..500 {
        let completed = Arc::clone(&completed);
        assert!(scheduler.submit(Box::new(move || {
            completed.fetch_add(1, Ordering::SeqCst);
        })));
    }

    wait_until(|| completed.load(Ordering::SeqCst) == 500 && scheduler.active() == 0);
}

#[test]
fn jobs_can_be_submitted_from_several_threads() {
    let scheduler = HashScheduler::new(3);
    let completed = Arc::new(AtomicUsize::new(0));
    let (done_tx, done_rx) = mpsc::channel();

    for _ in 0..8 {
        let scheduler = Arc::clone(&scheduler);
        let completed = Arc::clone(&completed);
        let done_tx = done_tx.clone();
        std::thread::spawn(move || {
            for _ in 0..25 {
                let completed = Arc::clone(&completed);
                assert!(scheduler.submit(Box::new(move || {
                    completed.fetch_add(1, Ordering::SeqCst);
                })));
            }
            done_tx.send(()).unwrap();
        });
    }
    drop(done_tx);

    for _ in 0..8 {
        done_rx.recv_timeout(Duration::from_secs(30)).unwrap();
    }
    wait_until(|| completed.load(Ordering::SeqCst) == 200 && scheduler.active() == 0);
    assert!(scheduler.peak() <= 3);
}

#[test]
fn a_panicking_job_does_not_kill_the_worker() {
    let scheduler = HashScheduler::new(1);
    assert!(scheduler.submit(Box::new(|| panic!("intentional test panic"))));

    let ran = Arc::new(AtomicUsize::new(0));
    let ran_clone = Arc::clone(&ran);
    assert!(scheduler.submit(Box::new(move || {
        ran_clone.fetch_add(1, Ordering::SeqCst);
    })));

    wait_until(|| ran.load(Ordering::SeqCst) == 1 && scheduler.active() == 0);
}

#[test]
fn download_list_defaults_to_a_bounded_pool_and_locks_after_first_use() {
    let list = DownloadList::new();
    assert!(list.hash_concurrency() >= 1);
    assert!(list.hash_scheduler_stats().is_none());

    assert!(list.set_hash_concurrency(2));
    assert_eq!(list.hash_concurrency(), 2);

    let scheduler = list.hash_scheduler();
    assert_eq!(scheduler.workers(), 2);
    let stats = list.hash_scheduler_stats().expect("stats after creation");
    assert_eq!(stats.workers, 2);

    // The pool cannot change size without a restart.
    assert!(!list.set_hash_concurrency(8));
    assert_eq!(list.hash_concurrency(), 2);
}

#[test]
fn zero_concurrency_is_clamped_to_one() {
    let list = DownloadList::new();
    assert!(list.set_hash_concurrency(0));
    assert_eq!(list.hash_concurrency(), 1);
}
