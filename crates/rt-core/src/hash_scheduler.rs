//! Bounded executor for hash passes.
//!
//! Every recheck used to spawn its own OS thread in
//! [`crate::download_list::DownloadList::begin_hash_check`], so restoring a
//! session with N pending rechecks created N threads. This executor keeps a
//! fixed pool of `workers` threads and a FIFO queue: at most `workers` passes
//! run at once, and the rest wait without consuming a thread.
//!
//! Jobs are plain closures so the executor itself is independent of the
//! download model and can be tested with counters instead of real disk I/O.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;

/// A unit of work submitted to [`HashScheduler`].
pub type HashJob = Box<dyn FnOnce() + Send + 'static>;

/// Snapshot of executor counters. `peak` is the high-water mark since the
/// scheduler was created and is what the concurrency tests assert on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HashSchedulerStats {
    pub workers: usize,
    pub active: usize,
    pub peak: usize,
    pub queued: usize,
}

struct Shared {
    receiver: Mutex<Receiver<HashJob>>,
    active: AtomicUsize,
    peak: AtomicUsize,
    queued: AtomicUsize,
    workers: usize,
}

/// Fixed-size worker pool with a FIFO job queue.
pub struct HashScheduler {
    sender: Sender<HashJob>,
    shared: Arc<Shared>,
}

impl HashScheduler {
    /// Spawns `workers` threads (at least one) and returns a shared handle.
    ///
    /// Threads exit when the last `Arc<HashScheduler>` is dropped, after
    /// finishing the job they are running. They are never joined, so shutdown
    /// cannot block on a long hash pass.
    pub fn new(workers: usize) -> Arc<Self> {
        let workers = workers.max(1);
        let (sender, receiver) = mpsc::channel();
        let shared = Arc::new(Shared {
            receiver: Mutex::new(receiver),
            active: AtomicUsize::new(0),
            peak: AtomicUsize::new(0),
            queued: AtomicUsize::new(0),
            workers,
        });

        for index in 0..workers {
            let shared = Arc::clone(&shared);
            let spawned = thread::Builder::new()
                .name(format!("hash-worker-{index}"))
                .spawn(move || worker_loop(shared));
            if let Err(err) = spawned {
                tracing::error!(error = %err, "Could not spawn hash scheduler worker");
                break;
            }
        }

        Arc::new(Self { sender, shared })
    }

    /// Queues `job`, returning `false` only when the scheduler is shutting
    /// down (the job is dropped in that case).
    ///
    /// The queue is unbounded by design: the number of pending rechecks is
    /// bounded by the number of torrents, and rejecting a recheck would leave
    /// a download permanently unchecked. Only [`workers`](Self::workers) jobs
    /// execute at once.
    pub fn submit(&self, job: HashJob) -> bool {
        // Count before handing the job over: a worker could otherwise pick it
        // up and decrement the counter before this call increments it.
        self.shared.queued.fetch_add(1, Ordering::AcqRel);
        match self.sender.send(job) {
            Ok(()) => true,
            Err(_) => {
                self.shared.queued.fetch_sub(1, Ordering::AcqRel);
                false
            }
        }
    }

    pub fn workers(&self) -> usize {
        self.shared.workers
    }

    pub fn active(&self) -> usize {
        self.shared.active.load(Ordering::Acquire)
    }

    pub fn peak(&self) -> usize {
        self.shared.peak.load(Ordering::Acquire)
    }

    pub fn queued(&self) -> usize {
        self.shared.queued.load(Ordering::Acquire)
    }

    pub fn stats(&self) -> HashSchedulerStats {
        HashSchedulerStats {
            workers: self.workers(),
            active: self.active(),
            peak: self.peak(),
            queued: self.queued(),
        }
    }
}

fn worker_loop(shared: Arc<Shared>) {
    loop {
        // Hold the receiver lock only while waiting for the next job: the lock
        // is released before the job runs, so all workers can execute
        // concurrently.
        let job = {
            let receiver = match shared.receiver.lock() {
                Ok(receiver) => receiver,
                Err(poisoned) => poisoned.into_inner(),
            };
            receiver.recv()
        };

        let Ok(job) = job else {
            return;
        };
        shared.queued.fetch_sub(1, Ordering::AcqRel);

        let now = shared.active.fetch_add(1, Ordering::AcqRel) + 1;
        shared.peak.fetch_max(now, Ordering::AcqRel);

        if catch_unwind(AssertUnwindSafe(job)).is_err() {
            // A panicking pass must not take the worker down with it, or the
            // pool silently shrinks for the rest of the process lifetime.
            tracing::error!("hash job panicked; worker continues");
        }

        shared.active.fetch_sub(1, Ordering::AcqRel);
    }
}
