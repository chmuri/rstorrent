use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use lt_common::rate::Rate;

#[derive(Clone)]
pub struct Throttle {
    inner: Arc<Mutex<Inner>>,
    max_rate: Arc<AtomicU64>,
    is_throttled: Arc<AtomicBool>,
}

struct Inner {
    rate: Rate,
    unallocated_quota: u32,
    outstanding_quota: u32,
    min_chunk_size: u32,
    max_chunk_size: u32,
    nodes: Vec<Arc<ThrottleNode>>,
}

impl Default for Throttle {
    fn default() -> Self {
        Self::new(0)
    }
}

impl Throttle {
    pub fn new(max_rate: u64) -> Self {
        let is_throttled = max_rate > 0;
        Self {
            inner: Arc::new(Mutex::new(Inner {
                rate: Rate::new(20),
                unallocated_quota: 0,
                outstanding_quota: 0,
                min_chunk_size: 1024,
                max_chunk_size: 16384,
                nodes: Vec::new(),
            })),
            max_rate: Arc::new(AtomicU64::new(max_rate)),
            is_throttled: Arc::new(AtomicBool::new(is_throttled)),
        }
    }

    pub fn is_throttled(&self) -> bool {
        self.is_throttled.load(Ordering::Relaxed)
    }

    pub fn max_rate(&self) -> u64 {
        self.max_rate.load(Ordering::Relaxed)
    }

    pub fn set_max_rate(&self, v: u64) {
        self.max_rate.store(v, Ordering::Relaxed);
        self.is_throttled.store(v > 0, Ordering::Relaxed);
    }

    pub fn rate_bytes_per_sec(&self) -> u64 {
        let mut inner = self.inner.lock().unwrap();
        inner.rate.rate()
    }

    pub fn create_node(&self) -> Arc<ThrottleNode> {
        let node = Arc::new(ThrottleNode::new(20));
        let mut inner = self.inner.lock().unwrap();
        inner.nodes.push(node.clone());
        node
    }

    pub fn update_quota(&self, quota: u32) -> u32 {
        let mut inner = self.inner.lock().unwrap();
        if !self.is_throttled() {
            return quota;
        }

        inner.unallocated_quota += quota;

        // Allocate quota to active nodes
        let min_chunk = inner.min_chunk_size;
        let max_chunk = inner.max_chunk_size;
        let nodes = inner.nodes.clone();

        for node in &nodes {
            if inner.unallocated_quota == 0 {
                break;
            }
            let cur_q = node.quota();
            if cur_q < min_chunk {
                let to_add = (max_chunk.saturating_sub(cur_q)).min(inner.unallocated_quota);
                node.set_quota(cur_q + to_add);
                inner.outstanding_quota += to_add;
                inner.unallocated_quota -= to_add;
            }
        }

        quota
    }

    pub fn node_used(&self, node: &ThrottleNode, used: u32) -> u32 {
        if used == 0 {
            return 0;
        }

        let mut inner = self.inner.lock().unwrap();
        inner.rate.insert(used as u64);
        node.rate_tracker.lock().unwrap().insert(used as u64);

        if !self.is_throttled() {
            return used;
        }

        let cur_q = node.quota();
        let quota = used.min(cur_q);
        node.set_quota(cur_q.saturating_sub(quota));
        inner.outstanding_quota = inner.outstanding_quota.saturating_sub(quota);
        let rem = used.saturating_sub(quota);
        inner.unallocated_quota = inner.unallocated_quota.saturating_sub(rem);

        used
    }
}

pub struct ThrottleNode {
    quota: AtomicU64,
    rate_tracker: Mutex<Rate>,
}

impl ThrottleNode {
    pub fn new(rate_span: usize) -> Self {
        Self {
            quota: AtomicU64::new(0),
            rate_tracker: Mutex::new(Rate::new(rate_span as u32)),
        }
    }

    pub fn quota(&self) -> u32 {
        self.quota.load(Ordering::Relaxed) as u32
    }

    pub fn set_quota(&self, q: u32) {
        self.quota.store(q as u64, Ordering::Relaxed);
    }

    pub fn clear_quota(&self) {
        self.quota.store(0, Ordering::Relaxed);
    }

    pub fn rate_bytes_per_sec(&self) -> u64 {
        let mut r = self.rate_tracker.lock().unwrap();
        r.rate()
    }
}
