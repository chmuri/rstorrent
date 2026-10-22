//! Unified peer discovery and connection scheduling.
//!
//! A [`PeerPool`] merges addresses learned from trackers, DHT lookups, PEX
//! messages and inbound connections.  It is the single source of truth for
//! connection attempts: addresses are deduplicated (after normalising
//! IPv4-mapped IPv6 addresses), tracked per source, and retried with
//! exponential backoff.  The coordinator consumes
//! [`PeerPool::next_connect_batch`] to drive a bounded number of concurrent
//! dial attempts.

use std::collections::{HashMap, HashSet};
use std::net::{SocketAddr, SocketAddrV4};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::client_config::ClientConfig;

/// Default upper bound on concurrent outbound connection attempts.
pub const DEFAULT_MAX_CONNECTING: usize = 16;
/// Default base delay before retrying a failed address.
pub const DEFAULT_RETRY_BASE: Duration = Duration::from_secs(5);
/// Default cap for the exponential retry delay.
pub const DEFAULT_RETRY_MAX: Duration = Duration::from_secs(15 * 60);
/// Default cooldown before redialing an address that disconnected.
pub const DEFAULT_DISCONNECT_COOLDOWN: Duration = Duration::from_secs(60);

/// Shareable handle used by the coordinator, inbound listeners and commands.
pub type PeerPoolHandle = Arc<Mutex<PeerPool>>;

/// Where an address was learned from.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum PeerSource {
    Tracker,
    Dht,
    Pex,
    Inbound,
}

impl PeerSource {
    pub const ALL: [PeerSource; 4] = [
        PeerSource::Tracker,
        PeerSource::Dht,
        PeerSource::Pex,
        PeerSource::Inbound,
    ];

    /// Sources that must be ignored for private torrents.
    pub fn is_external(self) -> bool {
        matches!(self, PeerSource::Dht | PeerSource::Pex)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            PeerSource::Tracker => "tracker",
            PeerSource::Dht => "dht",
            PeerSource::Pex => "pex",
            PeerSource::Inbound => "inbound",
        }
    }
}

/// Lifecycle of a pooled address.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PeerState {
    /// Known but not dialled yet.
    Available,
    /// A connection attempt is in flight.
    Connecting,
    /// A peer session is active.
    Connected,
    /// The last attempt failed; waiting for the retry deadline.
    Failed,
    /// A live session ended; waiting for the reconnect cooldown.
    Disconnected,
}

impl PeerState {
    pub fn is_connected(self) -> bool {
        matches!(self, PeerState::Connected)
    }
}

/// Per-address bookkeeping.
#[derive(Clone, Debug)]
pub struct PeerEntry {
    address: SocketAddr,
    sources: HashSet<PeerSource>,
    state: PeerState,
    last_attempt: Option<Instant>,
    connected_since: Option<Instant>,
    failure_count: u32,
    retry_at: Option<Instant>,
    attempts: u64,
    /// Number of live sessions for this address (normally 0 or 1, but an
    /// inbound and outbound connection can briefly overlap).
    active_sessions: u32,
}

impl PeerEntry {
    fn new(address: SocketAddr, source: PeerSource) -> Self {
        let mut sources = HashSet::new();
        sources.insert(source);
        Self {
            address,
            sources,
            state: PeerState::Available,
            last_attempt: None,
            connected_since: None,
            failure_count: 0,
            retry_at: None,
            attempts: 0,
            active_sessions: 0,
        }
    }

    pub fn address(&self) -> SocketAddr {
        self.address
    }

    pub fn state(&self) -> PeerState {
        self.state
    }

    pub fn sources(&self) -> impl Iterator<Item = PeerSource> + '_ {
        self.sources.iter().copied()
    }

    pub fn has_source(&self, source: PeerSource) -> bool {
        self.sources.contains(&source)
    }

    pub fn last_attempt(&self) -> Option<Instant> {
        self.last_attempt
    }

    pub fn connected_since(&self) -> Option<Instant> {
        self.connected_since
    }

    pub fn failure_count(&self) -> u32 {
        self.failure_count
    }

    pub fn retry_at(&self) -> Option<Instant> {
        self.retry_at
    }

    pub fn attempts(&self) -> u64 {
        self.attempts
    }

    pub fn active_sessions(&self) -> u32 {
        self.active_sessions
    }
}

/// Snapshot of pool state for commands and the UI.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PeerPoolStats {
    pub total: usize,
    pub available: usize,
    pub connecting: usize,
    pub connected: usize,
    pub failed: usize,
    pub tracker: usize,
    pub dht: usize,
    pub pex: usize,
    pub inbound: usize,
}

impl PeerPoolStats {
    pub fn source_count(&self, source: PeerSource) -> usize {
        match source {
            PeerSource::Tracker => self.tracker,
            PeerSource::Dht => self.dht,
            PeerSource::Pex => self.pex,
            PeerSource::Inbound => self.inbound,
        }
    }
}

/// Scheduling policy for a pool.
#[derive(Clone, Copy, Debug)]
pub struct PeerPoolConfig {
    pub max_peers: usize,
    pub max_connecting: usize,
    pub retry_base: Duration,
    pub retry_max: Duration,
    pub disconnect_cooldown: Duration,
}

impl Default for PeerPoolConfig {
    fn default() -> Self {
        Self {
            max_peers: 200,
            max_connecting: DEFAULT_MAX_CONNECTING,
            retry_base: DEFAULT_RETRY_BASE,
            retry_max: DEFAULT_RETRY_MAX,
            disconnect_cooldown: DEFAULT_DISCONNECT_COOLDOWN,
        }
    }
}

impl PeerPoolConfig {
    pub fn new(max_peers: usize) -> Self {
        let max_peers = max_peers.max(1);
        Self {
            max_peers,
            max_connecting: max_connecting_for(max_peers),
            ..Self::default()
        }
    }

    pub fn from_client_config(config: &ClientConfig) -> Self {
        Self::new(config.max_peers() as usize)
    }
}

/// The default number of concurrent dials for a given peer limit.
pub fn max_connecting_for(max_peers: usize) -> usize {
    max_peers.clamp(1, DEFAULT_MAX_CONNECTING)
}

/// Converts IPv4-mapped IPv6 addresses to their IPv4 form so that the same
/// peer is only represented once.
pub fn normalize_socket_addr(address: SocketAddr) -> SocketAddr {
    match address {
        SocketAddr::V6(v6) => match v6.ip().to_ipv4_mapped() {
            Some(v4) => SocketAddr::V4(SocketAddrV4::new(v4, v6.port())),
            None => address,
        },
        SocketAddr::V4(_) => address,
    }
}

fn is_usable(address: &SocketAddr) -> bool {
    address.port() != 0 && !address.ip().is_unspecified()
}

/// Merged peer set for one download.
pub struct PeerPool {
    config: PeerPoolConfig,
    entries: HashMap<SocketAddr, PeerEntry>,
}

impl PeerPool {
    pub fn new(max_peers: usize) -> Self {
        Self::with_config(PeerPoolConfig::new(max_peers))
    }

    pub fn with_config(config: PeerPoolConfig) -> Self {
        let max_peers = config.max_peers.max(1);
        let config = PeerPoolConfig {
            max_peers,
            max_connecting: config.max_connecting.max(1).min(max_peers),
            ..config
        };
        Self {
            config,
            entries: HashMap::new(),
        }
    }

    pub fn config(&self) -> &PeerPoolConfig {
        &self.config
    }

    pub fn set_limits(&mut self, max_peers: usize, max_connecting: usize) {
        self.config.max_peers = max_peers.max(1);
        self.config.max_connecting = max_connecting.max(1).min(self.config.max_peers);
    }

    /// Whether a source is allowed for this torrent.
    pub fn accepts_source(&self, source: PeerSource, is_private: bool) -> bool {
        !(is_private && source.is_external())
    }

    /// Drops external-only entries when a torrent turns out to be private
    /// (for example after metadata arrives for a magnet link).
    ///
    /// Returns the number of removed addresses.
    pub fn enforce_private(&mut self, is_private: bool) -> usize {
        if !is_private {
            return 0;
        }

        let before = self.entries.len();
        self.entries.retain(|_, entry| {
            entry.sources.remove(&PeerSource::Dht);
            entry.sources.remove(&PeerSource::Pex);
            !entry.sources.is_empty()
        });
        before - self.entries.len()
    }

    /// Adds one address, returning `true` when it was not already present.
    ///
    /// Duplicate addresses keep their existing retry/backoff state; only the
    /// set of sources grows.
    pub fn add_peer(&mut self, address: SocketAddr, source: PeerSource, is_private: bool) -> bool {
        if !self.accepts_source(source, is_private) {
            return false;
        }

        let address = normalize_socket_addr(address);
        if !is_usable(&address) {
            return false;
        }

        match self.entries.get_mut(&address) {
            Some(entry) => {
                entry.sources.insert(source);
                false
            }
            None => {
                self.entries
                    .insert(address, PeerEntry::new(address, source));
                true
            }
        }
    }

    /// Merges an address batch from one discovery source.
    ///
    /// Returns the number of newly inserted addresses.
    pub fn merge<I>(&mut self, addresses: I, source: PeerSource, is_private: bool) -> usize
    where
        I: IntoIterator<Item = SocketAddr>,
    {
        let mut added = 0;
        for address in addresses {
            if self.add_peer(address, source, is_private) {
                added += 1;
            }
        }
        added
    }

    /// Records a live inbound connection.  Inbound peers are per definition
    /// connected and bypass the outbound scheduler.
    pub fn record_inbound(&mut self, address: SocketAddr, now: Instant) -> bool {
        let address = normalize_socket_addr(address);
        if !is_usable(&address) {
            return false;
        }

        let inserted = match self.entries.get_mut(&address) {
            Some(entry) => {
                entry.sources.insert(PeerSource::Inbound);
                false
            }
            None => {
                self.entries
                    .insert(address, PeerEntry::new(address, PeerSource::Inbound));
                true
            }
        };

        if let Some(entry) = self.entries.get_mut(&address) {
            entry.state = PeerState::Connected;
            entry.connected_since = Some(now);
            entry.failure_count = 0;
            entry.retry_at = None;
            entry.active_sessions = entry.active_sessions.saturating_add(1);
        }

        inserted
    }

    /// Selects ready addresses and marks them as connecting.
    ///
    /// At most `limit` addresses are returned.  The result is additionally
    /// bounded by the configured concurrency cap and the `max_peers` limit
    /// (which counts both connecting and connected peers).
    pub fn next_connect_batch(&mut self, now: Instant, limit: usize) -> Vec<SocketAddr> {
        let connecting = self.connecting_count();
        let connected = self.connected_count();

        let free_peer_slots = self
            .config
            .max_peers
            .saturating_sub(connecting.saturating_add(connected));
        let free_dial_slots = self.config.max_connecting.saturating_sub(connecting);
        let slots = limit.min(free_peer_slots).min(free_dial_slots);
        if slots == 0 {
            return Vec::new();
        }

        let mut candidates: Vec<(u32, Option<Instant>, SocketAddr)> = self
            .entries
            .values()
            .filter(|entry| {
                if entry.sources.is_empty() {
                    return false;
                }
                match entry.state {
                    PeerState::Available | PeerState::Failed | PeerState::Disconnected => entry
                        .retry_at
                        .map(|deadline| deadline <= now)
                        .unwrap_or(true),
                    PeerState::Connecting | PeerState::Connected => false,
                }
            })
            .map(|entry| (entry.failure_count, entry.retry_at, entry.address))
            .collect();

        candidates.sort_by(|a, b| {
            a.0.cmp(&b.0)
                .then_with(|| a.1.cmp(&b.1))
                .then_with(|| a.2.cmp(&b.2))
        });

        let mut batch = Vec::with_capacity(slots.min(self.entries.len()));
        for (_, _, address) in candidates.into_iter().take(slots) {
            if let Some(entry) = self.entries.get_mut(&address) {
                entry.state = PeerState::Connecting;
                entry.last_attempt = Some(now);
                entry.attempts = entry.attempts.saturating_add(1);
                batch.push(address);
            }
        }
        batch
    }

    /// Marks a dial as successful and resets its backoff state.
    pub fn record_success(&mut self, address: SocketAddr, now: Instant) {
        let address = normalize_socket_addr(address);
        if let Some(entry) = self.entries.get_mut(&address) {
            entry.state = PeerState::Connected;
            entry.failure_count = 0;
            entry.retry_at = None;
            entry.connected_since = Some(now);
            entry.active_sessions = entry.active_sessions.saturating_add(1);
        }
    }

    /// Marks a dial as failed and schedules an exponential retry.
    ///
    /// A failure never overrides an already established session (for example
    /// an inbound connection from the same address).
    pub fn record_failure(&mut self, address: SocketAddr, now: Instant) {
        let address = normalize_socket_addr(address);
        let retry_base = self.config.retry_base;
        let retry_max = self.config.retry_max;
        if let Some(entry) = self.entries.get_mut(&address) {
            if entry.state == PeerState::Connected {
                return;
            }
            entry.failure_count = entry.failure_count.saturating_add(1);
            entry.state = PeerState::Failed;
            let delay = backoff_delay(retry_base, retry_max, entry.failure_count);
            entry.retry_at = Some(now + delay);
        }
    }

    /// Recovers slots from dials that never reported success or failure
    /// (for example because their task was lost).  Such an address is treated
    /// like a failed attempt and enters the normal backoff schedule.
    pub fn reap_stale(&mut self, now: Instant, timeout: Duration) {
        let retry_base = self.config.retry_base;
        let retry_max = self.config.retry_max;

        for entry in self.entries.values_mut() {
            if entry.state != PeerState::Connecting {
                continue;
            }
            let Some(last_attempt) = entry.last_attempt else {
                continue;
            };
            if now.saturating_duration_since(last_attempt) < timeout {
                continue;
            }

            entry.failure_count = entry.failure_count.saturating_add(1);
            entry.state = PeerState::Failed;
            let delay = backoff_delay(retry_base, retry_max, entry.failure_count);
            entry.retry_at = Some(now + delay);
        }
    }

    /// Removes a peer from the connected set and applies a reconnect cooldown.
    pub fn record_disconnected(&mut self, address: SocketAddr, now: Instant) {
        let address = normalize_socket_addr(address);
        let cooldown = self.config.disconnect_cooldown;
        if let Some(entry) = self.entries.get_mut(&address) {
            if !matches!(entry.state, PeerState::Connected | PeerState::Connecting) {
                return;
            }

            entry.active_sessions = entry.active_sessions.saturating_sub(1);
            if entry.active_sessions > 0 {
                entry.state = PeerState::Connected;
                return;
            }

            entry.state = PeerState::Disconnected;
            entry.retry_at = Some(now + cooldown);
        }
    }

    pub fn get(&self, address: SocketAddr) -> Option<&PeerEntry> {
        self.entries.get(&normalize_socket_addr(address))
    }

    pub fn entries(&self) -> impl Iterator<Item = &PeerEntry> {
        self.entries.values()
    }

    pub fn connected_count(&self) -> usize {
        self.entries
            .values()
            .filter(|entry| entry.state == PeerState::Connected)
            .count()
    }

    pub fn connecting_count(&self) -> usize {
        self.entries
            .values()
            .filter(|entry| entry.state == PeerState::Connecting)
            .count()
    }

    /// Known addresses that are neither connected nor currently dialing.
    pub fn available_count(&self) -> usize {
        self.entries
            .len()
            .saturating_sub(self.connected_count() + self.connecting_count())
    }

    pub fn stats(&self) -> PeerPoolStats {
        let mut stats = PeerPoolStats {
            total: self.entries.len(),
            ..PeerPoolStats::default()
        };

        for entry in self.entries.values() {
            match entry.state {
                PeerState::Available => {}
                PeerState::Connecting => stats.connecting += 1,
                PeerState::Connected => stats.connected += 1,
                PeerState::Failed | PeerState::Disconnected => stats.failed += 1,
            }

            if entry.sources.contains(&PeerSource::Tracker) {
                stats.tracker += 1;
            }
            if entry.sources.contains(&PeerSource::Dht) {
                stats.dht += 1;
            }
            if entry.sources.contains(&PeerSource::Pex) {
                stats.pex += 1;
            }
            if entry.sources.contains(&PeerSource::Inbound) {
                stats.inbound += 1;
            }
        }

        stats.available = stats
            .total
            .saturating_sub(stats.connected + stats.connecting);
        stats
    }
}

fn backoff_delay(base: Duration, max: Duration, failure_count: u32) -> Duration {
    let exponent = failure_count.saturating_sub(1).min(31);
    let multiplier = 1u128 << exponent;
    let millis = base.as_millis().saturating_mul(multiplier);
    let capped = millis.min(max.as_millis());
    Duration::from_millis(capped as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    fn addr(port: u16) -> SocketAddr {
        SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(127, 0, 0, 1), port))
    }

    fn mapped_addr(port: u16) -> SocketAddr {
        let mapped = Ipv4Addr::new(127, 0, 0, 1).to_ipv6_mapped();
        SocketAddr::V6(std::net::SocketAddrV6::new(mapped, port, 0, 0))
    }

    fn test_config() -> PeerPoolConfig {
        PeerPoolConfig {
            max_peers: 100,
            max_connecting: 2,
            retry_base: Duration::from_millis(100),
            retry_max: Duration::from_secs(10),
            disconnect_cooldown: Duration::from_secs(1),
        }
    }

    #[test]
    fn deduplicates_across_sources_and_merges_sources() {
        let peer = addr(6881);
        let mut pool = PeerPool::new(10);

        assert_eq!(pool.merge([peer], PeerSource::Tracker, false), 1);
        assert_eq!(pool.merge([peer], PeerSource::Dht, false), 0);
        assert_eq!(pool.merge([peer], PeerSource::Pex, false), 0);

        assert_eq!(pool.stats().total, 1);
        let entry = pool.get(peer).expect("peer should be pooled");
        assert!(entry.has_source(PeerSource::Tracker));
        assert!(entry.has_source(PeerSource::Dht));
        assert!(entry.has_source(PeerSource::Pex));
        assert_eq!(entry.sources().count(), 3);
    }

    #[test]
    fn duplicate_sources_produce_a_single_connect_attempt() {
        let peer = addr(6881);
        let start = Instant::now();
        let mut pool = PeerPool::new(10);

        pool.merge([peer], PeerSource::Tracker, false);
        pool.merge([peer], PeerSource::Dht, false);
        pool.merge([peer], PeerSource::Pex, false);

        assert_eq!(pool.next_connect_batch(start, 10), vec![peer]);
        assert!(pool.next_connect_batch(start, 10).is_empty());
        assert_eq!(pool.get(peer).unwrap().attempts(), 1);
    }

    #[test]
    fn normalises_ipv4_mapped_ipv6_before_dedupe() {
        let peer = addr(6881);
        let mapped = mapped_addr(6881);
        let mut pool = PeerPool::new(10);

        assert_eq!(pool.merge([mapped], PeerSource::Dht, false), 1);
        assert_eq!(pool.merge([peer], PeerSource::Tracker, false), 0);
        assert_eq!(pool.stats().total, 1);
        assert!(pool.get(mapped).is_some());
        assert!(pool.get(peer).is_some());
    }

    #[test]
    fn private_torrents_ignore_dht_and_pex() {
        let mut pool = PeerPool::new(10);

        assert_eq!(pool.merge([addr(6881)], PeerSource::Dht, true), 0);
        assert_eq!(pool.merge([addr(6882)], PeerSource::Pex, true), 0);
        assert_eq!(pool.merge([addr(6883)], PeerSource::Tracker, true), 1);
        assert_eq!(pool.stats().total, 1);
    }

    #[test]
    fn enforcing_private_drops_external_only_peers() {
        let tracker_peer = addr(6881);
        let dht_peer = addr(6882);
        let pex_peer = addr(6883);
        let mut pool = PeerPool::new(10);

        pool.merge([tracker_peer], PeerSource::Tracker, false);
        pool.merge([dht_peer], PeerSource::Dht, false);
        pool.merge([pex_peer], PeerSource::Pex, false);
        pool.merge([dht_peer], PeerSource::Tracker, false);

        assert_eq!(pool.enforce_private(true), 1);
        assert_eq!(pool.stats().total, 2);
        assert!(pool.get(tracker_peer).is_some());
        assert!(pool.get(dht_peer).is_some());
        assert!(pool.get(pex_peer).is_none());
        assert_eq!(pool.stats().pex, 0);
        assert_eq!(pool.stats().dht, 0);
    }

    #[test]
    fn failed_dials_back_off_exponentially() {
        let peer = addr(6881);
        let start = Instant::now();
        let mut pool = PeerPool::with_config(test_config());
        pool.add_peer(peer, PeerSource::Tracker, false);

        assert_eq!(pool.next_connect_batch(start, 10), vec![peer]);
        pool.record_failure(peer, start);

        assert!(pool.next_connect_batch(start, 10).is_empty());
        assert_eq!(
            pool.next_connect_batch(start + Duration::from_millis(100), 10),
            vec![peer]
        );

        pool.record_failure(peer, start + Duration::from_millis(100));
        let entry = pool.get(peer).expect("peer should be pooled");
        assert_eq!(entry.failure_count(), 2);
        assert_eq!(entry.retry_at(), Some(start + Duration::from_millis(300)));

        assert!(pool
            .next_connect_batch(start + Duration::from_millis(299), 10)
            .is_empty());
        assert_eq!(
            pool.next_connect_batch(start + Duration::from_millis(300), 10),
            vec![peer]
        );
    }

    #[test]
    fn honours_concurrency_cap() {
        let start = Instant::now();
        let mut pool = PeerPool::with_config(PeerPoolConfig {
            max_peers: 100,
            max_connecting: 2,
            ..test_config()
        });

        let peers: Vec<_> = (1..=5).map(addr).collect();
        assert_eq!(
            pool.merge(peers.iter().copied(), PeerSource::Tracker, false),
            5
        );

        let first = pool.next_connect_batch(start, 10);
        assert_eq!(first.len(), 2);
        assert!(pool.next_connect_batch(start, 10).is_empty());

        pool.record_success(first[0], start);
        let second = pool.next_connect_batch(start, 10);
        assert_eq!(second.len(), 1);
        assert!(pool.next_connect_batch(start, 10).is_empty());
    }

    #[test]
    fn honours_max_peers() {
        let start = Instant::now();
        let mut pool = PeerPool::with_config(PeerPoolConfig {
            max_peers: 2,
            max_connecting: 2,
            ..test_config()
        });

        let peers: Vec<_> = (1..=5).map(addr).collect();
        assert_eq!(
            pool.merge(peers.iter().copied(), PeerSource::Tracker, false),
            5
        );

        let first = pool.next_connect_batch(start, 10);
        assert_eq!(first.len(), 2);
        assert!(pool.next_connect_batch(start, 10).is_empty());

        // One connecting plus one connected peer already fills `max_peers`.
        pool.record_success(first[0], start);
        assert!(pool.next_connect_batch(start, 10).is_empty());

        pool.record_success(first[1], start);
        assert!(pool.next_connect_batch(start, 10).is_empty());
        assert_eq!(pool.stats().connected, 2);
        assert_eq!(pool.stats().available, 3);
    }

    #[test]
    fn stale_dials_release_concurrency_slots() {
        let peer = addr(6881);
        let start = Instant::now();
        let mut pool = PeerPool::with_config(PeerPoolConfig {
            max_connecting: 1,
            ..test_config()
        });
        pool.add_peer(peer, PeerSource::Tracker, false);

        assert_eq!(pool.next_connect_batch(start, 10), vec![peer]);
        // The slot is still held before the timeout expires.
        assert!(pool
            .next_connect_batch(start + Duration::from_secs(1), 10)
            .is_empty());

        pool.reap_stale(start + Duration::from_secs(2), Duration::from_secs(2));
        let entry = pool.get(peer).expect("peer should be pooled");
        assert_eq!(entry.state(), PeerState::Failed);
        assert_eq!(entry.failure_count(), 1);
    }

    #[test]
    fn disconnect_applies_cooldown() {
        let peer = addr(6881);
        let start = Instant::now();
        let mut pool = PeerPool::with_config(test_config());
        pool.add_peer(peer, PeerSource::Tracker, false);

        let batch = pool.next_connect_batch(start, 10);
        assert_eq!(batch, vec![peer]);
        pool.record_success(peer, start);
        pool.record_disconnected(peer, start);

        let entry = pool.get(peer).expect("peer should be pooled");
        assert_eq!(entry.state(), PeerState::Disconnected);
        assert_eq!(entry.retry_at(), Some(start + Duration::from_secs(1)));
        assert!(pool.next_connect_batch(start, 10).is_empty());
        assert_eq!(
            pool.next_connect_batch(start + Duration::from_secs(1), 10),
            vec![peer]
        );
    }

    #[test]
    fn inbound_peers_are_connected_and_deduplicated() {
        let peer = addr(6881);
        let mut pool = PeerPool::new(10);
        assert!(pool.record_inbound(peer, Instant::now()));
        assert!(!pool.record_inbound(mapped_addr(6881), Instant::now()));

        assert_eq!(pool.stats().total, 1);
        assert_eq!(pool.stats().connected, 1);
        assert_eq!(pool.stats().inbound, 1);
        assert_eq!(pool.get(peer).unwrap().state(), PeerState::Connected);
    }
}
