use lt_net::addr::SocketAddress;
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

/// A random engine that satisfies the requirement that its internal state
/// size in bits is at least 256 (32 bytes), matching `test_random_engine_state`.
pub struct RandomEngine {
    state: [u32; 8], // 8 * 32 = 256 bits of state
    index: usize,
}

impl RandomEngine {
    pub const STATE_BITS: usize = 256;

    pub fn new() -> Self {
        let mut seed = [0u8; 32];
        getrandom::getrandom(&mut seed).expect("failed to get random seed");
        let mut state = [0u32; 8];
        for i in 0..8 {
            state[i] = u32::from_ne_bytes([
                seed[i * 4],
                seed[i * 4 + 1],
                seed[i * 4 + 2],
                seed[i * 4 + 3],
            ]);
        }
        Self { state, index: 0 }
    }

    pub fn next_u32(&mut self) -> u32 {
        // Simple Xorshift-based mixing on 256-bit state
        let s0 = self.state[self.index];
        self.index = (self.index + 1) % 8;
        let mut s1 = self.state[self.index];
        s1 ^= s1 << 11;
        s1 ^= s1 >> 8;
        let res = s0.wrapping_add(s1);
        self.state[self.index] = s1;
        res
    }
}

impl Default for RandomEngine {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Debug)]
pub struct CachedConnection {
    pub connection_id: u64,
    pub established: Instant,
    pub expires_at: Instant,
}

pub struct UdpRouter {
    random: RandomEngine,
    connections: HashMap<SocketAddress, CachedConnection>,
    /// Transaction IDs that belong to in-flight requests. Two concurrent
    /// announces/scrapes must never observe each other's replies, so an ID is
    /// reserved until the operation that allocated it completes.
    active_transactions: HashSet<u32>,
    ttl: Duration,
}

impl UdpRouter {
    pub const DEFAULT_CONNECTION_TTL: Duration = Duration::from_secs(120);

    /// Number of retries (in addition to the initial attempt) used for UDP
    /// connect/announce/scrape requests.
    pub const DEFAULT_RETRIES: u32 = 2;

    /// First retry timeout. Subsequent retries double this value.
    pub const DEFAULT_RETRY_TIMEOUT: Duration = Duration::from_secs(1);

    /// Returns the timeout to use for a given retry attempt. Attempt 0 is the
    /// initial request, attempt 1 the first retry, and so on.
    pub fn retry_timeout(attempt: u32) -> Duration {
        let shift = attempt.min(6);
        Self::DEFAULT_RETRY_TIMEOUT * (1u32 << shift)
    }

    pub fn new() -> Self {
        Self {
            random: RandomEngine::new(),
            connections: HashMap::new(),
            active_transactions: HashSet::new(),
            ttl: Self::DEFAULT_CONNECTION_TTL,
        }
    }

    /// Allocates a non-zero transaction ID that is not currently in use by
    /// another request routed through this router.
    pub fn allocate_transaction_id(&mut self) -> u32 {
        // The PRNG has a full 256-bit state, so a collision is extremely
        // unlikely. Bound the loop anyway so a defective engine cannot hang
        // the tracker thread.
        for _ in 0..1_000_000 {
            let mut tid = self.random.next_u32();
            while tid == 0 {
                tid = self.random.next_u32();
            }
            if self.active_transactions.insert(tid) {
                return tid;
            }
        }
        // Practically unreachable fallback that still guarantees uniqueness
        // for the IDs currently known to the router.
        let mut tid = 1u32;
        while self.active_transactions.contains(&tid) {
            tid = tid.wrapping_add(1);
            if tid == 0 {
                tid = 1;
            }
        }
        self.active_transactions.insert(tid);
        tid
    }

    /// Releases a transaction ID previously returned by
    /// [`Self::allocate_transaction_id`].
    pub fn release_transaction_id(&mut self, transaction_id: u32) {
        self.active_transactions.remove(&transaction_id);
    }

    pub fn is_transaction_active(&self, transaction_id: u32) -> bool {
        self.active_transactions.contains(&transaction_id)
    }

    pub fn active_transaction_count(&self) -> usize {
        self.active_transactions.len()
    }

    /// Backwards-compatible name for [`Self::allocate_transaction_id`].
    ///
    /// The returned ID is reserved in the router; call
    /// [`Self::release_transaction_id`] once the request completes.
    pub fn generate_transaction_id(&mut self) -> u32 {
        self.allocate_transaction_id()
    }

    /// Alias used by callers that model transaction allocation as a queue.
    pub fn next_transaction_id(&mut self) -> u32 {
        self.allocate_transaction_id()
    }

    pub fn get_connection_id(&mut self, sa: &SocketAddress) -> Option<u64> {
        self.cleanup_expired();
        self.connections.get(sa).map(|c| c.connection_id)
    }

    pub fn cache_connection_id(&mut self, sa: SocketAddress, connection_id: u64) {
        let now = Instant::now();
        self.connections.insert(
            sa,
            CachedConnection {
                connection_id,
                established: now,
                expires_at: now + self.ttl,
            },
        );
    }

    pub fn remove_connection(&mut self, sa: &SocketAddress) {
        self.connections.remove(sa);
    }

    pub fn cleanup_expired(&mut self) {
        let now = Instant::now();
        self.connections.retain(|_, c| c.expires_at > now);
    }
}

impl Default for UdpRouter {
    fn default() -> Self {
        Self::new()
    }
}
