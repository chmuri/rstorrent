use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// Standard BitTorrent block size. Peers may use a smaller block when the
/// piece itself is smaller; the actual length always comes from the
/// delegated [`BlockTransfer`].
pub const DEFAULT_BLOCK_SIZE: u32 = 16 * 1024;

/// Default number of outstanding block requests allowed per peer. This is
/// used until the remote advertises a different value through the LTEP
/// `reqq` field.
pub const DEFAULT_PEER_REQUEST_QUEUE: usize = 250;

/// Minimum and maximum dynamic queue depth under BDP pipelining.
pub const MIN_DYNAMIC_QUEUE: usize = 4;
pub const MAX_DYNAMIC_QUEUE: usize = 512;
pub const DEFAULT_BUFFER_TIME_SECS: f64 = 1.5;

/// Hard upper bound for a peer advertised request queue. Matches the legacy
/// protocol limit (2048) so a malicious/broken peer cannot make us allocate
/// an unbounded pipeline.
pub const MAX_PEER_REQUEST_QUEUE: usize = 2048;

/// A request is considered lost after this long without a matching block.
pub const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Maximum number of endgame copies of the same block that may be tracked by
/// one peer's request list. The delegator also caps overlaps globally; this
/// is the last line of defence against unbounded duplicate traffic.
pub const MAX_ENDGAME_DUPLICATES: usize = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct BlockTransfer {
    pub piece_index: u32,
    pub block_offset: u32,
    pub block_length: u32,
}

impl BlockTransfer {
    pub fn new(piece_index: u32, block_offset: u32, block_length: u32) -> Self {
        Self {
            piece_index,
            block_offset,
            block_length,
        }
    }
}

/// Bookkeeping for a block request that has been written to the wire (or is
/// about to be written) and is awaiting its `Piece` message.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RequestRecord {
    pub transfer: BlockTransfer,
    pub sent_at: Instant,
    pub attempts: u32,
    pub endgame: bool,
}

pub struct RequestList {
    // Legacy C++-style buckets. They are retained for the protocol-level
    // state machine and tests; the live peer worker uses `in_flight` below.
    pub queued: VecDeque<BlockTransfer>,
    pub unordered: VecDeque<BlockTransfer>,
    pub stalled: VecDeque<BlockTransfer>,
    pub choked: VecDeque<BlockTransfer>,
    pub current_transfer: Option<BlockTransfer>,

    /// Authoritative outgoing request pipeline for the peer.
    in_flight: VecDeque<RequestRecord>,
    max_queue: usize,
    block_size: u32,
    request_timeout: Duration,
    max_endgame_duplicates: usize,

    // Dynamic BDP pipelining state
    rate_ewma: f64,
    rtt_ewma: Duration,
    min_rtt: Duration,
    last_block_at: Option<Instant>,
    is_snubbed: bool,
    dynamic_pipelining: bool,
}

impl Default for RequestList {
    fn default() -> Self {
        Self::new()
    }
}

impl RequestList {
    pub const BUCKET_QUEUED: usize = 0;
    pub const BUCKET_UNORDERED: usize = 1;
    pub const BUCKET_STALLED: usize = 2;
    pub const BUCKET_CHOKED: usize = 3;

    pub fn new() -> Self {
        Self::with_queue_limit(DEFAULT_PEER_REQUEST_QUEUE)
    }

    /// Creates a request list with an explicit peer request-queue limit.
    pub fn with_queue_limit(max_queue: usize) -> Self {
        Self {
            queued: VecDeque::new(),
            unordered: VecDeque::new(),
            stalled: VecDeque::new(),
            choked: VecDeque::new(),
            current_transfer: None,
            in_flight: VecDeque::new(),
            max_queue: max_queue.max(1),
            block_size: DEFAULT_BLOCK_SIZE,
            request_timeout: DEFAULT_REQUEST_TIMEOUT,
            max_endgame_duplicates: MAX_ENDGAME_DUPLICATES,
            rate_ewma: 0.0,
            rtt_ewma: Duration::ZERO,
            min_rtt: Duration::ZERO,
            last_block_at: None,
            is_snubbed: false,
            dynamic_pipelining: true,
        }
    }

    pub fn clear(&mut self) {
        self.queued.clear();
        self.unordered.clear();
        self.stalled.clear();
        self.choked.clear();
        self.current_transfer = None;
        self.in_flight.clear();
    }

    pub fn len(&self) -> usize {
        self.queued.len()
            + self.unordered.len()
            + self.stalled.len()
            + self.choked.len()
            + self.in_flight.len()
            + usize::from(self.current_transfer.is_some())
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn contains(&self, transfer: BlockTransfer) -> bool {
        self.queued.contains(&transfer)
            || self.unordered.contains(&transfer)
            || self.stalled.contains(&transfer)
            || self.choked.contains(&transfer)
            || self.current_transfer == Some(transfer)
            || self.contains_outstanding(transfer)
    }

    /// Removes a request from any bucket or the outstanding pipeline. Returns
    /// `true` when it was present.
    pub fn remove(&mut self, transfer: BlockTransfer) -> bool {
        let before = self.len();

        self.queued.retain(|item| *item != transfer);
        self.unordered.retain(|item| *item != transfer);
        self.stalled.retain(|item| *item != transfer);
        self.choked.retain(|item| *item != transfer);
        self.in_flight.retain(|record| record.transfer != transfer);

        if self.current_transfer == Some(transfer) {
            self.current_transfer = None;
        }

        before != self.len()
    }

    pub fn is_downloading(&self) -> bool {
        self.current_transfer.is_some()
    }

    pub fn queued_size(&self) -> usize {
        self.queued.len()
    }

    pub fn queued_empty(&self) -> bool {
        self.queued.is_empty()
    }

    pub fn unordered_size(&self) -> usize {
        self.unordered.len()
    }

    pub fn unordered_empty(&self) -> bool {
        self.unordered.is_empty()
    }

    pub fn stalled_size(&self) -> usize {
        self.stalled.len()
    }

    pub fn stalled_empty(&self) -> bool {
        self.stalled.is_empty()
    }

    pub fn choked_size(&self) -> usize {
        self.choked.len()
    }

    pub fn choked_empty(&self) -> bool {
        self.choked.is_empty()
    }

    // ------------------------------------------------------------------
    // Per-peer outgoing request pipeline
    // ------------------------------------------------------------------

    /// Maximum number of requests that may be outstanding on the wire.
    pub fn queue_limit(&self) -> usize {
        self.max_queue
    }

    /// Updates the peer request-queue limit (for example from LTEP `reqq`).
    /// Existing outstanding requests are never dropped.
    pub fn set_queue_limit(&mut self, max_queue: usize) {
        self.max_queue = max_queue.clamp(1, MAX_PEER_REQUEST_QUEUE);
    }

    /// Number of requests written to the peer and not yet satisfied.
    pub fn queue_depth(&self) -> usize {
        self.in_flight.len()
    }

    /// Free slots in the peer's request queue.
    pub fn available_slots(&self) -> usize {
        self.max_queue.saturating_sub(self.in_flight.len())
    }

    pub fn is_full(&self) -> bool {
        self.in_flight.len() >= self.max_queue
    }

    /// Number of outstanding requests that are endgame duplicates.
    pub fn endgame_count(&self) -> usize {
        self.in_flight
            .iter()
            .filter(|record| record.endgame)
            .count()
    }

    pub fn outstanding(&self) -> impl Iterator<Item = &RequestRecord> {
        self.in_flight.iter()
    }

    /// Block size used when a caller asks this list to synthesise transfers.
    /// Real peers still use the length returned by the delegator.
    pub fn block_size(&self) -> u32 {
        self.block_size
    }

    pub fn set_block_size(&mut self, block_size: u32) {
        if block_size > 0 {
            self.block_size = block_size;
        }
    }

    pub fn request_timeout(&self) -> Duration {
        self.request_timeout
    }

    pub fn set_request_timeout(&mut self, timeout: Duration) {
        self.request_timeout = timeout;
    }

    pub fn enable_dynamic_pipelining(&mut self, enabled: bool) {
        self.dynamic_pipelining = enabled;
    }

    pub fn is_dynamic_pipelining(&self) -> bool {
        self.dynamic_pipelining
    }

    pub fn is_snubbed(&self) -> bool {
        self.is_snubbed
    }

    pub fn set_snubbed(&mut self, snubbed: bool) {
        self.is_snubbed = snubbed;
        if snubbed {
            self.max_queue = 2;
        }
    }

    pub fn rate_ewma(&self) -> f64 {
        self.rate_ewma
    }

    pub fn rtt_ewma(&self) -> Duration {
        self.rtt_ewma
    }

    pub fn min_rtt(&self) -> Duration {
        self.min_rtt
    }

    /// Updates download rate and RTT measurement upon receiving a block.
    /// Recalculates `max_queue` dynamically according to BDP:
    /// Q_target = clamp(ceil((R_ewma * 1.5) / 16384), 4, 512)
    pub fn on_block_received(
        &mut self,
        block_bytes: usize,
        sent_at: Option<Instant>,
        now: Instant,
    ) {
        let prev_block_at = self.last_block_at;
        self.last_block_at = Some(now);
        self.is_snubbed = false;

        let arrival_dt = if let Some(prev) = prev_block_at {
            now.saturating_duration_since(prev).as_secs_f64()
        } else if let Some(sent) = sent_at {
            now.saturating_duration_since(sent).as_secs_f64()
        } else {
            0.0
        };

        if arrival_dt > 0.000_01 {
            let inst_rate = block_bytes as f64 / arrival_dt;
            if self.rate_ewma <= 0.0 {
                self.rate_ewma = inst_rate;
            } else {
                self.rate_ewma = 0.2 * inst_rate + 0.8 * self.rate_ewma;
            }

            if self.dynamic_pipelining {
                let target = ((self.rate_ewma * DEFAULT_BUFFER_TIME_SECS)
                    / DEFAULT_BLOCK_SIZE as f64)
                    .ceil() as usize;
                self.max_queue = target.clamp(MIN_DYNAMIC_QUEUE, MAX_DYNAMIC_QUEUE);
            }
        }

        if let Some(sent) = sent_at {
            if let Some(raw_rtt) = now.checked_duration_since(sent) {
                let rtt = if self.in_flight.len() <= 2 {
                    raw_rtt
                } else if !self.min_rtt.is_zero() {
                    raw_rtt.min(self.min_rtt.saturating_mul(2))
                } else {
                    raw_rtt
                };

                if self.min_rtt.is_zero() || rtt < self.min_rtt {
                    self.min_rtt = rtt;
                }

                if self.rtt_ewma.is_zero() {
                    self.rtt_ewma = rtt;
                } else {
                    let rtt_nanos =
                        0.2 * rtt.as_nanos() as f64 + 0.8 * self.rtt_ewma.as_nanos() as f64;
                    self.rtt_ewma = Duration::from_nanos(rtt_nanos as u64);
                }
            }
        }
    }

    /// Checks if the peer has timed out without delivering any block.
    /// T_snub = max(15s, 4 * RTT_ewma).
    /// If snubbed, sets max_queue = 2 and returns true.
    pub fn check_snubbing(&mut self, now: Instant) -> bool {
        if self.in_flight.is_empty() {
            return false;
        }

        let min_snub = Duration::from_secs(15);
        let rtt_snub = self.rtt_ewma.saturating_mul(4);
        let t_snub = min_snub.max(rtt_snub);

        let last_activity = self
            .last_block_at
            .unwrap_or_else(|| self.in_flight.front().map(|r| r.sent_at).unwrap_or(now));

        if now.duration_since(last_activity) >= t_snub {
            self.set_snubbed(true);
            true
        } else {
            false
        }
    }

    pub fn max_endgame_duplicates(&self) -> usize {
        self.max_endgame_duplicates
    }

    pub fn set_max_endgame_duplicates(&mut self, duplicates: usize) {
        self.max_endgame_duplicates = duplicates.max(1);
    }

    pub fn outstanding_count(&self, transfer: BlockTransfer) -> usize {
        self.in_flight
            .iter()
            .filter(|record| record.transfer == transfer)
            .count()
    }

    pub fn contains_outstanding(&self, transfer: BlockTransfer) -> bool {
        self.outstanding_count(transfer) != 0
    }

    /// Registers a fresh (non-endgame) request. Returns `false` when the
    /// block is already outstanding or the peer queue is full.
    pub fn register(&mut self, transfer: BlockTransfer, now: Instant) -> bool {
        self.register_inner(transfer, now, false)
    }

    /// Registers an endgame duplicate. The total number of copies for one
    /// block is capped by [`Self::max_endgame_duplicates`].
    pub fn register_endgame(&mut self, transfer: BlockTransfer, now: Instant) -> bool {
        self.register_inner(transfer, now, true)
    }

    fn register_inner(&mut self, transfer: BlockTransfer, now: Instant, endgame: bool) -> bool {
        if self.is_full() {
            return false;
        }

        let copies = self.outstanding_count(transfer);
        if endgame {
            if copies >= self.max_endgame_duplicates {
                return false;
            }
        } else if copies != 0 {
            return false;
        }

        self.in_flight.push_back(RequestRecord {
            transfer,
            sent_at: now,
            attempts: 0,
            endgame: endgame || copies != 0,
        });
        true
    }

    /// Removes every outstanding copy of a block, for example when a `Piece`
    /// arrives or the peer rejects the request. The returned records are in
    /// send order.
    pub fn take_block(&mut self, transfer: BlockTransfer) -> Vec<RequestRecord> {
        let mut taken = Vec::new();
        let mut kept = VecDeque::with_capacity(self.in_flight.len());

        while let Some(record) = self.in_flight.pop_front() {
            if record.transfer == transfer {
                taken.push(record);
            } else {
                kept.push_back(record);
            }
        }

        self.in_flight = kept;
        taken
    }

    /// Removes every outstanding request for a piece/offset pair, regardless
    /// of the recorded block length. Useful for tolerant handling of a peer
    /// that echoes a slightly different length in `cancel`/`reject`/`piece`.
    pub fn take_block_at(&mut self, piece_index: u32, block_offset: u32) -> Vec<RequestRecord> {
        let mut taken = Vec::new();
        let mut kept = VecDeque::with_capacity(self.in_flight.len());

        while let Some(record) = self.in_flight.pop_front() {
            if record.transfer.piece_index == piece_index
                && record.transfer.block_offset == block_offset
            {
                taken.push(record);
            } else {
                kept.push_back(record);
            }
        }

        self.in_flight = kept;
        taken
    }

    /// Removes and returns every outstanding request. Used on choke and
    /// disconnect so the blocks can be returned to the delegator.
    pub fn take_outstanding(&mut self) -> Vec<RequestRecord> {
        self.in_flight.drain(..).collect()
    }

    /// Returns requests whose deadline has elapsed. Their `attempts` count is
    /// incremented so callers can apply retry policy.
    pub fn expire(&mut self, now: Instant) -> Vec<RequestRecord> {
        let timeout = self.request_timeout;
        let mut expired = Vec::new();
        let mut kept = VecDeque::with_capacity(self.in_flight.len());

        while let Some(mut record) = self.in_flight.pop_front() {
            if now.saturating_duration_since(record.sent_at) >= timeout {
                record.attempts = record.attempts.saturating_add(1);
                expired.push(record);
            } else {
                kept.push_back(record);
            }
        }

        self.in_flight = kept;
        expired
    }

    pub fn delegate(&mut self, max_pieces: usize) -> Vec<BlockTransfer> {
        let mut result = Vec::new();
        for _ in 0..max_pieces {
            let next_index = self.queued.len() as u32;
            let transfer = BlockTransfer::new(next_index, 0, self.block_size);
            self.queued.push_back(transfer);
            result.push(transfer);
        }
        result
    }

    pub fn push_queued(&mut self, transfer: BlockTransfer) {
        self.queued.push_back(transfer);
    }

    pub fn pop_queued(&mut self) -> Option<BlockTransfer> {
        let t = self.queued.pop_front();
        self.current_transfer = t;
        t
    }

    pub fn choked(&mut self) {
        while let Some(t) = self.queued.pop_front() {
            self.choked.push_back(t);
        }
        while let Some(record) = self.in_flight.pop_front() {
            self.choked.push_back(record.transfer);
        }
        if let Some(cur) = self.current_transfer.take() {
            self.choked.push_back(cur);
        }
    }

    pub fn unchoked(&mut self) {
        while let Some(t) = self.choked.pop_front() {
            self.queued.push_back(t);
        }
    }

    pub fn stall_initial(&mut self) {
        if let Some(cur) = self.current_transfer.take() {
            self.stalled.push_back(cur);
        }
    }

    pub fn stall_prolonged(&mut self) {
        self.stall_initial();
        while let Some(t) = self.queued.pop_front() {
            self.stalled.push_back(t);
        }
    }

    pub fn push_unordered(&mut self, transfer: BlockTransfer) {
        self.unordered.push_back(transfer);
    }

    pub fn finished(&mut self) {
        self.current_transfer = None;
    }

    pub fn skipped(&mut self) {
        self.current_transfer = None;
    }

    pub fn calculate_pipe_size(rate: u32, max_size: u32) -> u32 {
        let pipe = rate / DEFAULT_BLOCK_SIZE;
        pipe.clamp(4, max_size)
    }

    /// Rate-based pipe size using this peer's negotiated block size.
    pub fn calculate_pipe_size_for(&self, rate: u32, max_size: u32) -> u32 {
        let pipe = rate / self.block_size.max(1);
        pipe.clamp(4, max_size)
    }
}
