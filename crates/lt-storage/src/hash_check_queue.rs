//! Priority queue for chunk hash verification.
//!
//! Mirrors the legacy `torrent::HashCheckQueue`: chunks that peers currently
//! need are verified before chunks that may be needed later. Entries are
//! de-duplicated and can be promoted or cancelled while queued.
//!
//! Unlike the legacy queue this implementation also tracks the chunk that has
//! been handed to the worker ("in flight"). [`HashCheckQueue::remove`] marks
//! such an entry as cancelled, so a hash result that arrives afterwards can be
//! discarded safely. Every dequeue is tagged with a monotonically increasing
//! token; a result for an older token (for example after the same index was
//! removed and re-enqueued) is reported as stale by
//! [`HashCheckQueue::complete_token`].

use std::collections::{HashMap, VecDeque};

/// The chunk is required by at least one peer right now.
pub const FLAG_NEED: u8 = 0x01;
/// The chunk may be needed later (e.g. after the current piece completes).
pub const FLAG_WILL_NEED: u8 = 0x02;

/// Number of consecutive need-priority chunks that may be served before one
/// will-need chunk is promoted to keep the latter from starving.
pub const DEFAULT_STARVATION_LIMIT: usize = 64;

const PRIORITY_MASK: u8 = FLAG_NEED | FLAG_WILL_NEED;

/// A chunk handed out by [`HashCheckQueue::pop_next_job`].
///
/// `token` must be passed back to [`HashCheckQueue::complete_token`] or
/// [`HashCheckQueue::fail_token`] when the hash finishes. It lets the queue
/// distinguish the current result from a stale one after removal/re-enqueue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HashCheckJob {
    pub index: u32,
    pub flags: u8,
    pub token: u64,
}

/// Snapshot of queue depths and lifetime counters.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HashCheckQueueStats {
    /// Chunks waiting to be handed to the worker.
    pub queued: usize,
    /// Queued chunks in the need tier.
    pub need: usize,
    /// Queued chunks in the will-need tier.
    pub will_need: usize,
    /// Chunks handed to the worker that have not reported back yet.
    pub in_flight: usize,
    /// Chunks accepted by [`HashCheckQueue::insert`] for the first time.
    pub enqueued: u64,
    /// Duplicate inserts that did not schedule a second hash.
    pub duplicates: u64,
    /// Queued chunks promoted from will-need to need.
    pub promotions: u64,
    /// Chunks dequeued with [`HashCheckQueue::pop_next_job`].
    pub started: u64,
    /// In-flight chunks that completed successfully.
    pub completed: u64,
    /// Queued chunks removed by the caller.
    pub removed: u64,
    /// In-flight chunks cancelled through [`HashCheckQueue::remove`]/clear.
    pub cancelled_in_flight: u64,
    /// In-flight chunks whose worker reported an error.
    pub failed: u64,
    /// Results discarded because their job token no longer matched.
    pub stale_results: u64,
    /// Need-priority chunks served.
    pub need_served: u64,
    /// Will-need chunks served (including fairness promotions).
    pub will_need_served: u64,
}

#[derive(Debug, Clone, Copy)]
struct InFlightEntry {
    token: u64,
    cancelled: bool,
}

#[derive(Debug)]
pub struct HashCheckQueue {
    need: VecDeque<u32>,
    will_need: VecDeque<u32>,
    flags: HashMap<u32, u8>,
    in_flight: HashMap<u32, InFlightEntry>,
    next_token: u64,
    /// `None` disables fairness aging and always drains need first.
    starvation_limit: Option<usize>,
    need_streak: usize,
    enqueued: u64,
    duplicates: u64,
    promotions: u64,
    started: u64,
    completed: u64,
    removed: u64,
    cancelled_in_flight: u64,
    failed: u64,
    stale_results: u64,
    need_served: u64,
    will_need_served: u64,
}

impl Default for HashCheckQueue {
    fn default() -> Self {
        Self {
            need: VecDeque::new(),
            will_need: VecDeque::new(),
            flags: HashMap::new(),
            in_flight: HashMap::new(),
            next_token: 0,
            starvation_limit: Some(DEFAULT_STARVATION_LIMIT),
            need_streak: 0,
            enqueued: 0,
            duplicates: 0,
            promotions: 0,
            started: 0,
            completed: 0,
            removed: 0,
            cancelled_in_flight: 0,
            failed: 0,
            stale_results: 0,
            need_served: 0,
            will_need_served: 0,
        }
    }
}

impl HashCheckQueue {
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of chunks waiting to be dequeued (in-flight chunks excluded).
    pub fn len(&self) -> usize {
        self.flags.len()
    }

    /// Alias for [`HashCheckQueue::len`], matching the legacy `size()`.
    pub fn size(&self) -> usize {
        self.len()
    }

    /// Alias for [`HashCheckQueue::size`], matching the statistics command.
    pub fn queue_size(&self) -> usize {
        self.len()
    }

    pub fn is_empty(&self) -> bool {
        self.flags.is_empty()
    }

    pub fn need_size(&self) -> usize {
        self.need.len()
    }

    pub fn will_need_size(&self) -> usize {
        self.will_need.len()
    }

    pub fn in_flight_size(&self) -> usize {
        self.in_flight.len()
    }

    pub fn need_count(&self) -> usize {
        self.need_size()
    }

    pub fn will_need_count(&self) -> usize {
        self.will_need_size()
    }

    pub fn in_flight_count(&self) -> usize {
        self.in_flight_size()
    }

    /// Queued plus in-flight chunks. This is the value suitable for
    /// `pieces.hash.queue_size`.
    pub fn pending_size(&self) -> usize {
        self.flags.len() + self.in_flight.len()
    }

    /// True when `index` is waiting in either priority tier.
    pub fn has(&self, index: u32) -> bool {
        self.flags.contains_key(&index)
    }

    /// True when `index` has been handed to the worker and not yet completed.
    pub fn is_in_flight(&self, index: u32) -> bool {
        self.in_flight.contains_key(&index)
    }

    pub fn flags(&self, index: u32) -> Option<u8> {
        self.flags.get(&index).copied()
    }

    /// Enqueues a chunk.
    ///
    /// Returns `true` when the queue changed. A duplicate insert with the same
    /// priority is a no-op; inserting `FLAG_NEED` for a will-need chunk
    /// promotes it. Inserting a chunk that is already in flight is also a
    /// no-op so a chunk can never be hashed twice concurrently.
    pub fn insert(&mut self, index: u32, flags: u8) -> bool {
        let flags = flags & PRIORITY_MASK;
        if flags == 0 {
            return false;
        }

        if let Some(existing) = self.flags.get_mut(&index) {
            let merged = *existing | flags;
            if merged == *existing {
                self.duplicates = self.duplicates.saturating_add(1);
                return false;
            }

            let was_will_need_only = *existing & FLAG_NEED == 0;
            *existing = merged;

            if merged & FLAG_NEED != 0 && was_will_need_only {
                self.will_need.retain(|&i| i != index);
                if !self.need.contains(&index) {
                    self.need.push_back(index);
                }
                self.promotions = self.promotions.saturating_add(1);
            }

            return true;
        }

        if self
            .in_flight
            .get(&index)
            .map(|entry| !entry.cancelled)
            .unwrap_or(false)
        {
            self.duplicates = self.duplicates.saturating_add(1);
            return false;
        }

        // A cancelled in-flight entry must not block a fresh request. Drop it
        // here; its later reply will not match any in-flight token.
        self.in_flight.remove(&index);

        self.flags.insert(index, flags);
        if flags & FLAG_NEED != 0 {
            self.need.push_back(index);
        } else {
            self.will_need.push_back(index);
        }
        self.enqueued = self.enqueued.saturating_add(1);
        true
    }

    /// Removes a chunk from the queue and/or cancels it if it is in flight.
    ///
    /// Returns `true` if the chunk was queued or was an active in-flight job.
    /// A cancelled in-flight job remains visible through
    /// [`HashCheckQueue::is_in_flight`] until the worker reports back, but its
    /// result is guaranteed to be rejected.
    pub fn remove(&mut self, index: u32) -> bool {
        let mut changed = false;

        if self.flags.remove(&index).is_some() {
            self.need.retain(|&i| i != index);
            self.will_need.retain(|&i| i != index);
            self.removed = self.removed.saturating_add(1);
            changed = true;
        }

        if let Some(entry) = self.in_flight.get_mut(&index) {
            if !entry.cancelled {
                entry.cancelled = true;
                self.cancelled_in_flight = self.cancelled_in_flight.saturating_add(1);
                changed = true;
            }
        }

        changed
    }

    /// Pops the highest-priority chunk: need before will-need, FIFO within a
    /// tier.
    ///
    /// Returns only the index. Callers that need stale-result protection
    /// should use [`HashCheckQueue::pop_next_job`].
    pub fn pop_next(&mut self) -> Option<u32> {
        self.pop_next_job().map(|job| job.index)
    }

    /// Pops the highest-priority chunk and returns its job token.
    ///
    /// A will-need chunk is promoted after
    /// [`HashCheckQueue::starvation_limit`] consecutive need-priority pops so
    /// an endless stream of needs cannot starve the lower tier. Within a tier
    /// order is FIFO.
    pub fn pop_next_job(&mut self) -> Option<HashCheckJob> {
        loop {
            let has_need = !self.need.is_empty();
            let has_will_need = !self.will_need.is_empty();
            if !has_need && !has_will_need {
                // A fully drained queue starts a fresh fairness window so a
                // later need insert is not preempted by an old streak.
                self.need_streak = 0;
                return None;
            }

            let serve_will_need = !has_need
                || (has_will_need
                    && self
                        .starvation_limit
                        .map(|limit| self.need_streak >= limit)
                        .unwrap_or(false));

            let job = if serve_will_need {
                self.pop_from(true)
            } else {
                self.pop_from(false)
            };

            if let Some(job) = job {
                if serve_will_need {
                    self.need_streak = 0;
                } else {
                    self.need_streak = self.need_streak.saturating_add(1);
                }
                return Some(job);
            }

            // The selected tier only contained stale bookkeeping entries.
            // Re-evaluate the other tier on the next iteration.
        }
    }

    fn pop_from(&mut self, will_need: bool) -> Option<HashCheckJob> {
        loop {
            let index = if will_need {
                self.will_need.pop_front()?
            } else {
                self.need.pop_front()?
            };

            let Some(flags) = self.flags.remove(&index) else {
                continue;
            };

            let token = self.next_token;
            self.next_token = self.next_token.wrapping_add(1);
            self.in_flight.insert(
                index,
                InFlightEntry {
                    token,
                    cancelled: false,
                },
            );
            self.started = self.started.saturating_add(1);
            if will_need {
                self.will_need_served = self.will_need_served.saturating_add(1);
            } else {
                self.need_served = self.need_served.saturating_add(1);
            }

            return Some(HashCheckJob {
                index,
                flags,
                token,
            });
        }
    }

    /// Returns a popped job to the front of its priority tier.
    ///
    /// Used when the worker cannot accept the command yet (full queue) so the
    /// caller can retry without reordering or losing the entry.
    pub fn requeue(&mut self, job: HashCheckJob) -> bool {
        let active = self
            .in_flight
            .get(&job.index)
            .map(|entry| entry.token == job.token && !entry.cancelled)
            .unwrap_or(false);
        if !active {
            return false;
        }
        self.in_flight.remove(&job.index);

        let flags = job.flags & PRIORITY_MASK;
        if flags == 0 || self.flags.contains_key(&job.index) {
            return false;
        }

        self.flags.insert(job.index, flags);
        self.started = self.started.saturating_sub(1);
        if flags & FLAG_NEED != 0 {
            self.need.push_front(job.index);
            self.need_served = self.need_served.saturating_sub(1);
            // The pop that produced this job counted towards the fairness
            // streak; undo it because the job did not actually run.
            self.need_streak = self.need_streak.saturating_sub(1);
        } else {
            self.will_need.push_front(job.index);
            self.will_need_served = self.will_need_served.saturating_sub(1);
        }
        true
    }

    /// Completes the current in-flight job for `index` regardless of token.
    pub fn complete(&mut self, index: u32) -> bool {
        match self.in_flight.get(&index) {
            Some(entry) => {
                let token = entry.token;
                self.complete_token(index, token)
            }
            None => {
                self.stale_results = self.stale_results.saturating_add(1);
                false
            }
        }
    }

    /// Completes an in-flight job.
    ///
    /// Returns `true` only when `token` matches the active, non-cancelled job.
    /// A removed job, or a result from before a remove/re-enqueue cycle, is
    /// counted as stale and returns `false`.
    pub fn complete_token(&mut self, index: u32, token: u64) -> bool {
        match self.in_flight.get(&index) {
            Some(entry) if entry.token == token => {
                let cancelled = entry.cancelled;
                self.in_flight.remove(&index);
                if cancelled {
                    self.stale_results = self.stale_results.saturating_add(1);
                    false
                } else {
                    self.completed = self.completed.saturating_add(1);
                    true
                }
            }
            // A result for an older job must not disturb the newer in-flight
            // entry for the same index.
            Some(_) | None => {
                self.stale_results = self.stale_results.saturating_add(1);
                false
            }
        }
    }

    /// Reports that the worker failed an in-flight job.
    pub fn fail(&mut self, index: u32) -> bool {
        match self.in_flight.get(&index) {
            Some(entry) => {
                let token = entry.token;
                self.fail_token(index, token)
            }
            None => false,
        }
    }

    /// Reports a worker failure for a specific job token.
    pub fn fail_token(&mut self, index: u32, token: u64) -> bool {
        match self.in_flight.get(&index) {
            Some(entry) if entry.token == token => {
                let cancelled = entry.cancelled;
                self.in_flight.remove(&index);
                if cancelled {
                    false
                } else {
                    self.failed = self.failed.saturating_add(1);
                    true
                }
            }
            // Leave a newer job for the same index untouched.
            Some(_) | None => false,
        }
    }

    /// Reclassifies a queued chunk. An in-flight chunk is left alone.
    pub fn set_flags(&mut self, index: u32, flags: u8) -> bool {
        let flags = flags & PRIORITY_MASK;
        if flags == 0 {
            return self.remove(index);
        }

        match self.flags.get(&index).copied() {
            Some(existing) if existing == flags => {
                self.duplicates = self.duplicates.saturating_add(1);
                false
            }
            Some(_) => {
                self.remove(index);
                self.insert(index, flags)
            }
            None => self.insert(index, flags),
        }
    }

    /// Cancels every queued and in-flight chunk.
    ///
    /// Lifetime counters are retained; call [`HashCheckQueue::reset_counters`]
    /// when starting a fresh session.
    pub fn clear(&mut self) {
        let active = self
            .in_flight
            .values()
            .filter(|entry| !entry.cancelled)
            .count() as u64;
        self.cancelled_in_flight = self.cancelled_in_flight.saturating_add(active);
        self.need.clear();
        self.will_need.clear();
        self.flags.clear();
        self.in_flight.clear();
        self.need_streak = 0;
    }

    /// Resets every lifetime counter without touching queued work.
    pub fn reset_counters(&mut self) {
        self.enqueued = 0;
        self.duplicates = 0;
        self.promotions = 0;
        self.started = 0;
        self.completed = 0;
        self.removed = 0;
        self.cancelled_in_flight = 0;
        self.failed = 0;
        self.stale_results = 0;
        self.need_served = 0;
        self.will_need_served = 0;
    }

    /// Number of consecutive need-tier pops before a will-need chunk is served.
    ///
    /// `None` disables aging (strict need-first ordering).
    pub fn starvation_limit(&self) -> Option<usize> {
        self.starvation_limit
    }

    pub fn set_starvation_limit(&mut self, limit: Option<usize>) {
        self.starvation_limit = limit;
        self.need_streak = 0;
    }

    /// Alias for [`HashCheckQueue::stats`].
    pub fn counters(&self) -> HashCheckQueueStats {
        self.stats()
    }

    pub fn stats(&self) -> HashCheckQueueStats {
        HashCheckQueueStats {
            queued: self.flags.len(),
            need: self.need.len(),
            will_need: self.will_need.len(),
            in_flight: self.in_flight.len(),
            enqueued: self.enqueued,
            duplicates: self.duplicates,
            promotions: self.promotions,
            started: self.started,
            completed: self.completed,
            removed: self.removed,
            cancelled_in_flight: self.cancelled_in_flight,
            failed: self.failed,
            stale_results: self.stale_results,
            need_served: self.need_served,
            will_need_served: self.will_need_served,
        }
    }

    pub fn total_enqueued(&self) -> u64 {
        self.enqueued
    }

    pub fn total_completed(&self) -> u64 {
        self.completed
    }

    pub fn total_removed(&self) -> u64 {
        self.removed
    }

    pub fn total_failed(&self) -> u64 {
        self.failed
    }

    pub fn duplicate_count(&self) -> u64 {
        self.duplicates
    }

    pub fn promotion_count(&self) -> u64 {
        self.promotions
    }

    pub fn stale_result_count(&self) -> u64 {
        self.stale_results
    }

    pub fn cancelled_in_flight_count(&self) -> u64 {
        self.cancelled_in_flight
    }

    /// Snapshot of currently queued indices, need-first.
    pub fn iter(&self) -> impl Iterator<Item = u32> + '_ {
        self.need
            .iter()
            .chain(self.will_need.iter())
            .copied()
            .filter(|i| self.flags.contains_key(i))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn need_precedes_will_need_fifo_within_tier() {
        let mut queue = HashCheckQueue::new();
        assert!(queue.insert(10, FLAG_WILL_NEED));
        assert!(queue.insert(11, FLAG_NEED));
        assert!(queue.insert(12, FLAG_NEED));
        assert!(queue.insert(13, FLAG_WILL_NEED));

        assert_eq!(queue.pop_next(), Some(11));
        assert_eq!(queue.pop_next(), Some(12));
        assert_eq!(queue.pop_next(), Some(10));
        assert_eq!(queue.pop_next(), Some(13));
        assert_eq!(queue.pop_next(), None);
    }

    #[test]
    fn duplicate_insert_is_idempotent_and_promotion_works() {
        let mut queue = HashCheckQueue::new();
        assert!(queue.insert(1, FLAG_WILL_NEED));
        assert!(!queue.insert(1, FLAG_WILL_NEED));
        assert_eq!(queue.len(), 1);

        assert!(queue.insert(1, FLAG_NEED));
        assert_eq!(queue.flags(1), Some(FLAG_NEED | FLAG_WILL_NEED));
        assert_eq!(queue.pop_next(), Some(1));
    }

    #[test]
    fn remove_and_clear() {
        let mut queue = HashCheckQueue::new();
        queue.insert(1, FLAG_NEED);
        queue.insert(2, FLAG_WILL_NEED);

        assert!(queue.remove(2));
        assert!(!queue.remove(2));
        assert_eq!(queue.len(), 1);
        assert_eq!(queue.pop_next(), Some(1));

        queue.insert(3, FLAG_NEED);
        queue.clear();
        assert!(queue.is_empty());
        assert_eq!(queue.pop_next(), None);
    }

    #[test]
    fn zero_flags_are_rejected() {
        let mut queue = HashCheckQueue::new();
        assert!(!queue.insert(1, 0));
        assert!(queue.is_empty());
    }

    #[test]
    fn in_flight_entries_deduplicate_and_removal_cancels_their_result() {
        let mut queue = HashCheckQueue::new();
        queue.insert(7, FLAG_NEED);

        let job = queue.pop_next_job().unwrap();
        assert_eq!(job.index, 7);
        assert!(queue.is_in_flight(7));
        assert!(!queue.has(7));
        assert!(!queue.insert(7, FLAG_NEED));

        assert!(queue.remove(7));
        assert!(!queue.complete_token(7, job.token));
        assert_eq!(queue.stats().stale_results, 1);
        assert_eq!(queue.stats().completed, 0);
        assert_eq!(queue.stats().cancelled_in_flight, 1);
    }

    #[test]
    fn stale_token_from_before_reenqueue_is_rejected() {
        let mut queue = HashCheckQueue::new();
        queue.insert(4, FLAG_NEED);
        let first = queue.pop_next_job().unwrap();
        assert!(queue.remove(4));

        // Re-enqueue the same index; this is a new job with a new token.
        assert!(queue.insert(4, FLAG_NEED));
        let second = queue.pop_next_job().unwrap();
        assert_ne!(first.token, second.token);

        assert!(!queue.complete_token(4, first.token));
        assert!(queue.complete_token(4, second.token));
        assert_eq!(queue.stats().completed, 1);
        assert_eq!(queue.stats().stale_results, 1);
    }

    #[test]
    fn starvation_limit_serves_will_need_without_breaking_priority() {
        let mut queue = HashCheckQueue::new();
        queue.set_starvation_limit(Some(2));

        queue.insert(100, FLAG_WILL_NEED);
        queue.insert(1, FLAG_NEED);
        queue.insert(2, FLAG_NEED);
        queue.insert(3, FLAG_NEED);

        // Need chunks are preferred, but after two consecutive need pops the
        // will-need chunk is served so it cannot starve.
        assert_eq!(queue.pop_next(), Some(1));
        assert_eq!(queue.pop_next(), Some(2));
        assert_eq!(queue.pop_next(), Some(100));
        assert_eq!(queue.pop_next(), Some(3));
    }

    #[test]
    fn will_need_never_preempts_need_before_the_fairness_limit() {
        let mut queue = HashCheckQueue::new();
        queue.set_starvation_limit(Some(4));

        for index in 0..100 {
            queue.insert(index, FLAG_WILL_NEED);
        }
        queue.insert(1000, FLAG_NEED);

        assert_eq!(queue.pop_next(), Some(1000));
    }
}
