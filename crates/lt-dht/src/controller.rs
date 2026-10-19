use std::collections::HashSet;
use std::path::Path;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU32, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::{watch, Mutex, Notify};
use tokio::time::Instant;
use tokio::task::JoinSet;
use tracing::{debug, info, warn};

use lt_common::hash_string::HashString;
use lt_common::object::{bencode, Object};
use lt_net::addr::SocketAddress;

use crate::error::DhtError;
use crate::node::DhtNode;
use crate::router::{DhtRouter, TIMEOUT_PEER_ANNOUNCE, TIMEOUT_REMOVE_NODE};
use crate::search::decode_nodes;
use crate::server::{DhtServer, ServerStatistics};
use crate::transaction::{KrpcQuery, KrpcResponse};

/// Number of nodes that marks the initial bootstrap process complete.
pub const BOOTSTRAP_COMPLETE_NODES: usize = 32;
/// Maximum number of configured bootstrap contacts queried in one pass.
pub const MAX_BOOTSTRAP_CONTACTS: usize = 16;
/// Default period between routing-table refreshes.
pub const DEFAULT_REFRESH_INTERVAL: Duration = Duration::from_secs(15 * 60);
/// Retry cadence while the table is still below `BOOTSTRAP_COMPLETE_NODES`.
pub const DEFAULT_BOOTSTRAP_RETRY_INTERVAL: Duration = Duration::from_secs(60);

/// Shareable handles needed to run lookups and announces.
#[derive(Clone)]
pub struct DhtHandle {
    pub server: Arc<DhtServer>,
    pub router: Arc<Mutex<DhtRouter>>,
}

/// `dht.mode` values.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DhtMode {
    /// DHT is completely disabled: no cache load and no socket.
    Disable,
    /// DHT is initialized but kept inactive.
    Off,
    /// DHT starts when a usable listen/override port is available.
    #[default]
    Auto,
    /// DHT is always started.
    On,
}

impl DhtMode {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim() {
            "disable" => Some(Self::Disable),
            "off" => Some(Self::Off),
            "auto" => Some(Self::Auto),
            "on" => Some(Self::On),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Disable => "disable",
            Self::Off => "off",
            Self::Auto => "auto",
            Self::On => "on",
        }
    }

    /// True for modes that may run the DHT server.
    pub fn is_enabled(&self) -> bool {
        matches!(self, Self::Auto | Self::On)
    }
}

impl FromStr for DhtMode {
    type Err = ();

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value).ok_or(())
    }
}

impl std::fmt::Display for DhtMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Lifecycle/interval configuration for a [`DhtController`].
#[derive(Clone, Debug)]
pub struct DhtLifecycleConfig {
    pub mode: DhtMode,
    /// Non-zero overrides the torrent listen port.
    pub port_override: u16,
    pub refresh_interval: Duration,
    pub bootstrap_retry_interval: Duration,
    /// Peers older than this are pruned during a refresh.
    pub peer_timeout: u32,
    /// Bad nodes older than this are removed during a refresh.
    pub remove_node_timeout: u32,
    pub bootstrap_nodes: Vec<SocketAddress>,
}

impl Default for DhtLifecycleConfig {
    fn default() -> Self {
        Self {
            mode: DhtMode::Auto,
            port_override: 0,
            refresh_interval: DEFAULT_REFRESH_INTERVAL,
            bootstrap_retry_interval: DEFAULT_BOOTSTRAP_RETRY_INTERVAL,
            peer_timeout: TIMEOUT_PEER_ANNOUNCE,
            remove_node_timeout: TIMEOUT_REMOVE_NODE,
            bootstrap_nodes: Vec::new(),
        }
    }
}

impl DhtLifecycleConfig {
    pub fn new() -> Self {
        Self::default()
    }
}

#[derive(Clone, Debug, Default)]
pub struct ControllerStatistics {
    /// 0 = inactive, 1 = bootstrapping, >= 2 = normal operation.
    pub cycle: u32,
    pub queries_sent: usize,
    pub queries_received: usize,
    pub replies_sent: usize,
    pub replies_received: usize,
    pub errors_sent: usize,
    pub errors_received: usize,
    pub errors_caught: usize,
    pub num_nodes: usize,
    pub num_buckets: usize,
    pub num_peers: usize,
    pub max_peers: usize,
    pub num_trackers: usize,
    pub active: bool,
}

impl From<ServerStatistics> for ControllerStatistics {
    fn from(stats: ServerStatistics) -> Self {
        Self {
            queries_sent: stats.queries_sent,
            queries_received: stats.queries_received,
            replies_sent: stats.replies_sent,
            replies_received: stats.replies_received,
            errors_sent: stats.errors_sent,
            errors_received: stats.errors_received,
            ..Self::default()
        }
    }
}

pub struct DhtController {
    router: Arc<Mutex<DhtRouter>>,
    server: Arc<DhtServer>,
    is_active: Arc<AtomicBool>,
    nodes_populated: Arc<AtomicBool>,
    refresh_cycles: Arc<AtomicU32>,
    requested_port: Arc<AtomicU16>,
    mode: Arc<RwLock<DhtMode>>,
    config: Arc<RwLock<DhtLifecycleConfig>>,
    bootstrap_notify: Arc<Notify>,
    bind_lock: Arc<Mutex<()>>,
}

impl DhtController {
    pub fn new(own_id: HashString, port: u16) -> Self {
        let mut config = DhtLifecycleConfig::default();
        config.port_override = port;
        Self::with_config(own_id, port, config)
    }

    pub fn with_config(own_id: HashString, port: u16, mut config: DhtLifecycleConfig) -> Self {
        let effective_port = if config.port_override != 0 {
            config.port_override
        } else {
            port
        };
        config.port_override = effective_port;
        let mode = config.mode;

        let router = Arc::new(Mutex::new(DhtRouter::new(own_id)));
        let server = Arc::new(DhtServer::new(router.clone(), effective_port));

        Self {
            router,
            server,
            is_active: Arc::new(AtomicBool::new(false)),
            nodes_populated: Arc::new(AtomicBool::new(false)),
            refresh_cycles: Arc::new(AtomicU32::new(0)),
            requested_port: Arc::new(AtomicU16::new(effective_port)),
            mode: Arc::new(RwLock::new(mode)),
            config: Arc::new(RwLock::new(config)),
            bootstrap_notify: Arc::new(Notify::new()),
            bind_lock: Arc::new(Mutex::new(())),
        }
    }

    pub fn router(&self) -> Arc<Mutex<DhtRouter>> {
        self.router.clone()
    }

    pub fn server(&self) -> Arc<DhtServer> {
        Arc::clone(&self.server)
    }

    pub fn handle(&self) -> DhtHandle {
        DhtHandle {
            server: self.server(),
            router: self.router(),
        }
    }

    pub fn mode(&self) -> DhtMode {
        *self.mode.read().unwrap()
    }

    pub fn config(&self) -> DhtLifecycleConfig {
        self.config.read().unwrap().clone()
    }

    /// Updates lifecycle configuration. Mode transitions apply start/stop
    /// immediately; a changed override port is applied by the next start or an
    /// explicit [`DhtController::restart_with_port`].
    pub fn configure(&self, config: DhtLifecycleConfig) {
        if config.port_override != 0 {
            self.requested_port
                .store(config.port_override, Ordering::Relaxed);
        }
        let mode = config.mode;
        *self.mode.write().unwrap() = mode;
        *self.config.write().unwrap() = config;
        if mode.is_enabled() {
            if !self.is_active() {
                self.start();
            }
        } else {
            self.stop();
        }
        self.bootstrap_notify.notify_one();
    }

    pub fn set_refresh_interval(&self, interval: Duration) {
        self.config.write().unwrap().refresh_interval = interval;
        self.bootstrap_notify.notify_one();
    }

    pub fn set_bootstrap_retry_interval(&self, interval: Duration) {
        self.config.write().unwrap().bootstrap_retry_interval = interval;
        self.bootstrap_notify.notify_one();
    }

    pub fn set_intervals(&self, refresh: Duration, bootstrap_retry: Duration) {
        {
            let mut config = self.config.write().unwrap();
            config.refresh_interval = refresh;
            config.bootstrap_retry_interval = bootstrap_retry;
        }
        self.bootstrap_notify.notify_one();
    }

    pub fn set_peer_timeout(&self, timeout: u32) {
        self.config.write().unwrap().peer_timeout = timeout;
    }

    pub fn set_remove_node_timeout(&self, timeout: u32) {
        self.config.write().unwrap().remove_node_timeout = timeout;
    }

    pub fn refresh_interval(&self) -> Duration {
        self.config.read().unwrap().refresh_interval
    }

    pub fn set_bootstrap_nodes(&self, nodes: Vec<SocketAddress>) {
        self.config.write().unwrap().bootstrap_nodes = nodes;
        self.bootstrap_notify.notify_one();
    }

    pub fn add_bootstrap_node(&self, node: SocketAddress) {
        let mut config = self.config.write().unwrap();
        if !config.bootstrap_nodes.contains(&node) {
            config.bootstrap_nodes.push(node);
        }
        drop(config);
        self.bootstrap_notify.notify_one();
    }

    pub fn bootstrap_nodes(&self) -> Vec<SocketAddress> {
        self.config.read().unwrap().bootstrap_nodes.clone()
    }

    /// Binds the UDP socket, runs the KRPC receive loop and a periodic
    /// bootstrap/refresh task until shutdown.
    ///
    /// When the controller is stopped (or its mode is `disable`/`off`) this
    /// returns immediately without binding, so session startup is never
    /// blocked.
    pub async fn run(&self, shutdown: watch::Receiver<bool>) -> Result<(), DhtError> {
        if !self.mode.read().unwrap().is_enabled() {
            info!(mode = self.mode().as_str(), "DHT run skipped by mode");
            return Ok(());
        }
        if !self.is_active.load(Ordering::Relaxed) {
            info!("DHT run skipped: controller is stopped");
            return Ok(());
        }

        {
            let _bind_guard = self.bind_lock.lock().await;
            let port = self.requested_port.load(Ordering::Relaxed);
            if let Err(err) = self.server.bind_port(port).await {
                self.is_active.store(false, Ordering::Relaxed);
                self.nodes_populated.store(false, Ordering::Relaxed);
                warn!(port, error = %err, "DHT failed to bind; disabling DHT");
                return Err(err);
            }
        }

        info!(port = self.server.port(), mode = self.mode().as_str(), "DHT started");

        let maintenance = tokio::spawn(maintenance_loop(
            self.server(),
            self.router(),
            Arc::clone(&self.config),
            Arc::clone(&self.is_active),
            Arc::clone(&self.nodes_populated),
            Arc::clone(&self.refresh_cycles),
            Arc::clone(&self.bootstrap_notify),
            Arc::clone(&self.requested_port),
            shutdown.clone(),
        ));

        let result = self.server.run(shutdown).await;
        maintenance.abort();

        self.is_active.store(false, Ordering::Relaxed);
        self.server.unbind();

        match &result {
            Ok(()) => info!("DHT server stopped"),
            Err(err) => warn!(error = %err, "DHT server stopped with an error"),
        }

        result
    }

    /// Contacts bootstrap nodes with `find_node(self_id)` and merges the
    /// discovered nodes into the routing table.
    ///
    /// Failures are counted but never retried here: a single call is bounded
    /// by the per-query timeout and returns success with `0` replies.
    pub async fn bootstrap(&self, nodes: &[SocketAddress]) -> Result<usize, DhtError> {
        let targets: Vec<SocketAddress> = nodes
            .iter()
            .take(MAX_BOOTSTRAP_CONTACTS)
            .cloned()
            .collect();
        self.bootstrap_targets(targets).await
    }

    /// Bootstraps using the configured router list.
    pub async fn bootstrap_configured(&self) -> Result<usize, DhtError> {
        let nodes = self.bootstrap_nodes();
        self.bootstrap(&nodes).await
    }

    async fn bootstrap_targets(&self, targets: Vec<SocketAddress>) -> Result<usize, DhtError> {
        if targets.is_empty() {
            self.update_population_state();
            return Ok(0);
        }

        let own_id = self.router.lock().await.own_id;
        let replied = query_find_node(
            self.server(),
            self.router(),
            targets,
            own_id,
            true,
        )
        .await;

        self.update_population_state();
        Ok(replied)
    }

    /// Refreshes the routing table: prunes stale peers/trackers, ages and
    /// evicts bad nodes and queries the closest known nodes.
    pub async fn refresh(&self) -> Result<usize, DhtError> {
        let config = self.config();
        let replied = refresh_handles(
            self.server(),
            self.router(),
            config.peer_timeout,
            config.remove_node_timeout,
        )
        .await;
        self.update_population_state();
        Ok(replied)
    }

    fn update_population_state(&self) -> bool {
        let populated = match self.router.try_lock() {
            Ok(router) => router.get_statistics().num_nodes >= BOOTSTRAP_COMPLETE_NODES,
            Err(_) => self.nodes_populated.load(Ordering::Relaxed),
        };

        let was_populated = self.nodes_populated.swap(populated, Ordering::Relaxed);
        if populated && !was_populated {
            info!(
                nodes = self.get_statistics().num_nodes,
                "DHT bootstrap complete"
            );
        }
        populated
    }

    /// Serializes the routing table, tracker cache and self id into a
    /// bencode-friendly object. Returns an empty map if the router is busy.
    pub fn save_cache(&self) -> Object {
        self.try_save_cache().unwrap_or_else(Object::create_map)
    }

    /// Non-blocking cache snapshot. `None` means the router was locked by a
    /// lookup and the caller should retry later rather than persist an empty
    /// cache.
    pub fn try_save_cache(&self) -> Option<Object> {
        let router = self.router.try_lock().ok()?;
        Some(build_cache(&router))
    }

    /// Async cache snapshot that always waits for the router lock.
    pub async fn save_cache_async(&self) -> Object {
        let router = self.router.lock().await;
        build_cache(&router)
    }

    /// Merges the cache into `container` (C++ `store_cache(Object*)`).
    /// Returns false when the router could not be locked.
    pub fn store_cache_into(&self, container: &mut Object) -> bool {
        let Some(cache) = self.try_save_cache() else {
            return false;
        };
        if let Ok(map) = cache.as_map() {
            for (key, value) in map {
                let _ = container.insert_key(key.clone(), value.clone());
            }
        }
        true
    }

    /// Alias matching the legacy `store_cache(Object*)` API.
    pub fn store_cache(&self, container: &mut Object) -> bool {
        self.store_cache_into(container)
    }

    /// Atomically writes the DHT cache to `path`.
    pub fn save_cache_to(&self, path: impl AsRef<Path>) -> Result<(), DhtError> {
        let cache = self
            .try_save_cache()
            .ok_or_else(|| DhtError::Internal("DHT router is busy; cache not saved".into()))?;
        write_cache(path.as_ref(), &cache)
    }

    /// Async variant used by the periodic saver so a busy router never
    /// results in an empty cache file being written.
    pub async fn save_cache_to_async(&self, path: impl AsRef<Path>) -> Result<(), DhtError> {
        let cache = self.save_cache_async().await;
        write_cache(path.as_ref(), &cache)
    }

    /// Loads a cache previously written by [`save_cache_to`].
    ///
    /// A corrupt or empty file is logged and ignored; it never aborts
    /// startup and never replaces the current in-memory state.
    pub fn load_cache_from(&self, path: impl AsRef<Path>) -> Result<(), DhtError> {
        let path = path.as_ref();
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                debug!(path = %path.display(), "DHT cache does not exist yet");
                return Ok(());
            }
            Err(err) => return Err(err.into()),
        };
        let cache = match bencode::decode(&bytes) {
            Ok(cache) => cache,
            Err(err) => {
                warn!(path = %path.display(), error = %err, "Ignoring corrupt DHT cache");
                return Ok(());
            }
        };

        if cache.as_map().is_err() {
            warn!(path = %path.display(), "Ignoring DHT cache with invalid root object");
            return Ok(());
        }

        self.initialize(&cache);
        debug!(path = %path.display(), "Loaded DHT cache");
        Ok(())
    }

    /// Restores self id, routing nodes and tracker cache from a session
    /// `Object`. Invalid entries are skipped individually.
    pub fn initialize(&self, cache: &Object) {
        if self.mode() == DhtMode::Disable {
            debug!("DHT cache ignored: mode is disable");
            return;
        }

        let Ok(mut router) = self.router.try_lock() else {
            warn!("DHT cache load skipped: router is busy");
            return;
        };

        if let Ok(self_id_val) = cache.get_key("self_id") {
            if let Ok(bytes) = self_id_val.as_string() {
                if bytes.len() == HashString::size_data() {
                    let mut self_id = HashString::default();
                    self_id.as_mut_slice().copy_from_slice(bytes);
                    router.own_id = self_id;
                }
            }
        }

        router.load_cache(cache);
        let populated = router.get_statistics().num_nodes >= BOOTSTRAP_COMPLETE_NODES;
        self.nodes_populated.store(populated, Ordering::Relaxed);

        debug!(
            nodes = router.get_statistics().num_nodes,
            trackers = router.get_statistics().num_trackers,
            populated,
            "DHT cache restored"
        );
    }

    /// Alias for [`DhtController::initialize`].
    pub fn load_cache(&self, cache: &Object) {
        self.initialize(cache);
    }

    /// Generates a fresh self id, e.g. after an explicit reset.
    pub fn regenerate_self_id(&self) -> bool {
        let Ok(mut router) = self.router.try_lock() else {
            return false;
        };

        let mut seed = Vec::new();
        seed.extend_from_slice(&now_seconds().to_be_bytes());
        seed.extend_from_slice(&std::process::id().to_be_bytes());
        if let Ok(now) = SystemTime::now().duration_since(UNIX_EPOCH) {
            seed.extend_from_slice(&now.subsec_nanos().to_be_bytes());
        }
        let digest = lt_crypto::sha1::sha1_hash(&seed);
        router.own_id = HashString::new(digest);
        self.nodes_populated.store(false, Ordering::Relaxed);
        info!("DHT self id regenerated");
        true
    }

    pub fn start(&self) -> bool {
        if !self.mode.read().unwrap().is_enabled() {
            info!(mode = self.mode().as_str(), "DHT start refused by mode");
            return false;
        }

        let was_active = self.is_active.swap(true, Ordering::Relaxed);
        if !was_active {
            info!(
                port = self.requested_port.load(Ordering::Relaxed),
                mode = self.mode().as_str(),
                "DHT start requested"
            );
            self.bootstrap_notify.notify_one();
        }
        true
    }

    pub fn stop(&self) {
        let was_active = self.is_active.swap(false, Ordering::Relaxed);
        self.server.unbind();
        self.nodes_populated.store(false, Ordering::Relaxed);
        self.bootstrap_notify.notify_one();
        if was_active {
            info!("DHT stopped");
        }
    }

    pub fn is_active(&self) -> bool {
        self.is_active.load(Ordering::Relaxed)
    }

    pub fn is_nodes_populated(&self) -> bool {
        self.update_population_state()
    }

    /// Applies a `dht.mode` change. `disable`/`off` immediately close the
    /// socket; `auto`/`on` start the controller (the socket is bound by
    /// [`DhtController::run`]).
    pub fn set_mode(&self, mode: DhtMode) -> bool {
        *self.mode.write().unwrap() = mode;
        self.config.write().unwrap().mode = mode;

        match mode {
            DhtMode::Disable | DhtMode::Off => {
                self.stop();
                info!(mode = mode.as_str(), "DHT mode applied");
                false
            }
            DhtMode::Auto | DhtMode::On => {
                self.start();
                info!(mode = mode.as_str(), "DHT mode applied");
                true
            }
        }
    }

    /// Async mode transition that also opens/closes the socket immediately.
    pub async fn apply_mode(&self, mode: DhtMode) -> Result<(), DhtError> {
        let enabled = self.set_mode(mode);
        if enabled {
            let _bind_guard = self.bind_lock.lock().await;
            let port = self.requested_port.load(Ordering::Relaxed);
            self.server.bind_port(port).await?;
        }
        Ok(())
    }

    /// Active UDP port (0 while stopped). Before the async receive loop has
    /// bound the socket this reports the requested port.
    pub fn port(&self) -> u16 {
        let bound = self.server.port();
        if bound != 0 {
            bound
        } else if self.is_active() {
            self.requested_port.load(Ordering::Relaxed)
        } else {
            0
        }
    }

    /// Port that will be used on the next start.
    pub fn requested_port(&self) -> u16 {
        self.requested_port.load(Ordering::Relaxed)
    }

    pub fn set_port(&self, port: u16) {
        self.requested_port.store(port, Ordering::Relaxed);
        self.config.write().unwrap().port_override = port;
    }

    /// Changes the listen port. The old socket is closed before the new one
    /// is bound and the running receive loop is woken up.
    pub async fn restart_with_port(&self, port: u16) -> Result<(), DhtError> {
        self.set_port(port);
        if self.is_active() {
            let _bind_guard = self.bind_lock.lock().await;
            let requested = self.requested_port.load(Ordering::Relaxed);
            info!(new_port = requested, "Restarting DHT on new port");
            self.server.bind_port(requested).await?;
        }
        Ok(())
    }

    /// Restarts on the currently requested port.
    pub async fn restart(&self) -> Result<(), DhtError> {
        self.restart_with_port(self.requested_port()).await
    }

    pub fn set_private_torrent(&self, info_hash: HashString, is_private: bool) {
        if let Ok(mut router) = self.router.try_lock() {
            router.set_private_info_hash(info_hash, is_private);
        }
    }

    pub async fn set_private_torrent_async(&self, info_hash: HashString, is_private: bool) {
        self.router
            .lock()
            .await
            .set_private_info_hash(info_hash, is_private);
    }

    /// Contacts an externally supplied peer (BT `PORT` message / explicit
    /// add_node) while the routing table is still being populated.
    pub fn add_peer_node(&self, sa: &SocketAddress, port: u16) {
        if self.is_nodes_populated() {
            return;
        }

        let mut node_sa = sa.clone();
        if node_sa.set_port(port).is_err() {
            return;
        }

        // Count synchronously so callers observe the contact immediately, even
        // when no Tokio runtime is available.
        self.server.note_query_sent();

        if !self.server.is_active() {
            // No socket yet: remember the peer so the periodic bootstrap can
            // retry it once the server is running.
            self.add_bootstrap_node(node_sa);
            debug!("DHT add_peer_node deferred until the socket is active");
            return;
        }

        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            debug!("DHT add_peer_node called without a runtime; contact deferred");
            return;
        };

        let server = self.server();
        let router = self.router();
        handle.spawn(async move {
            let own_id = router.lock().await.own_id;
            query_find_node(server, router, vec![node_sa], own_id, false).await;
        });
    }

    pub fn get_statistics(&self) -> ControllerStatistics {
        let (num_nodes, num_buckets, num_trackers, num_peers, max_peers) =
            if let Ok(router) = self.router.try_lock() {
                let stats = router.get_statistics();
                (
                    stats.num_nodes,
                    stats.num_buckets,
                    stats.num_trackers,
                    stats.num_peers,
                    stats.max_peers,
                )
            } else {
                (0, 0, 0, 0, 0)
            };

        let mut stats = ControllerStatistics::from(self.server.statistics());
        let active = self.is_active();
        let server_active = self.server.is_active();
        let populated = self.nodes_populated.load(Ordering::Relaxed)
            || num_nodes >= BOOTSTRAP_COMPLETE_NODES;

        stats.cycle = if !active || !server_active {
            0
        } else if !populated {
            1
        } else {
            self.refresh_cycles
                .load(Ordering::Relaxed)
                .saturating_add(2)
        };
        stats.num_nodes = num_nodes;
        stats.num_buckets = num_buckets;
        stats.num_trackers = num_trackers;
        stats.num_peers = num_peers;
        stats.max_peers = max_peers;
        stats.active = active && server_active;
        stats
    }

    pub fn reset_statistics(&self) {
        self.server.reset_statistics();
        self.refresh_cycles.store(0, Ordering::Relaxed);
    }

    /// Map form of [`DhtController::get_statistics`] suitable for a
    /// `dht.statistics` RPC response.
    pub fn statistics_object(&self) -> Object {
        let stats = self.get_statistics();
        let mut map = Object::create_map();
        let _ = map.insert_key(
            "dht",
            Object::create_string(self.mode().as_str().as_bytes().to_vec()),
        );
        let _ = map.insert_key("active", Object::create_value(stats.active as i64));
        let _ = map.insert_key("cycle", Object::create_value(stats.cycle as i64));
        let _ = map.insert_key(
            "queries_received",
            Object::create_value(stats.queries_received as i64),
        );
        let _ = map.insert_key(
            "queries_sent",
            Object::create_value(stats.queries_sent as i64),
        );
        let _ = map.insert_key(
            "replies_received",
            Object::create_value(stats.replies_received as i64),
        );
        let _ = map.insert_key(
            "errors_received",
            Object::create_value(stats.errors_received as i64),
        );
        let _ = map.insert_key(
            "errors_caught",
            Object::create_value(stats.errors_caught as i64),
        );
        let _ = map.insert_key("nodes", Object::create_value(stats.num_nodes as i64));
        let _ = map.insert_key(
            "buckets",
            Object::create_value(stats.num_buckets as i64),
        );
        let _ = map.insert_key("peers", Object::create_value(stats.num_peers as i64));
        let _ = map.insert_key(
            "peers_max",
            Object::create_value(stats.max_peers as i64),
        );
        let _ = map.insert_key(
            "torrents",
            Object::create_value(stats.num_trackers as i64),
        );
        map
    }
}

fn build_cache(router: &DhtRouter) -> Object {
    let mut cache = router.store_cache();
    let _ = cache.insert_key(
        "self_id",
        Object::create_string(router.own_id.as_slice().to_vec()),
    );
    cache
}

fn write_cache(path: &Path, cache: &Object) -> Result<(), DhtError> {
    let bytes = bencode::encode(cache).map_err(|err| DhtError::Input(err.to_string()))?;

    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }

    let temporary = path.with_extension("tmp");
    std::fs::write(&temporary, bytes)?;
    std::fs::rename(&temporary, path)?;
    Ok(())
}

fn now_seconds() -> u32 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as u32)
        .unwrap_or(0)
}

/// Periodic lifecycle task. Sleeps until the next bootstrap retry or refresh
/// deadline, so an unreachable bootstrap set never spins.
#[allow(clippy::too_many_arguments)]
async fn maintenance_loop(
    server: Arc<DhtServer>,
    router: Arc<Mutex<DhtRouter>>,
    config: Arc<RwLock<DhtLifecycleConfig>>,
    active: Arc<AtomicBool>,
    populated: Arc<AtomicBool>,
    refresh_cycles: Arc<AtomicU32>,
    bootstrap_notify: Arc<Notify>,
    requested_port: Arc<AtomicU16>,
    mut shutdown: watch::Receiver<bool>,
) {
    let mut next = Instant::now();

    loop {
        tokio::select! {
            _ = shutdown.changed() => break,
            _ = bootstrap_notify.notified() => {
                next = Instant::now();
                continue;
            }
            _ = tokio::time::sleep_until(next) => {}
        }

        if !active.load(Ordering::Relaxed) {
            next = Instant::now() + Duration::from_secs(1);
            continue;
        }

        // A stop/start cycle may leave the server without a socket while the
        // receive loop is still alive; bind it again here rather than
        // spawning a detached task.
        if server.socket().is_none() {
            let port = requested_port.load(Ordering::Relaxed);
            if let Err(err) = server.bind_port(port).await {
                warn!(port, error = %err, "DHT could not re-bind after start");
                next = Instant::now() + Duration::from_secs(5);
                continue;
            }
            info!(port = server.port(), "DHT socket re-bound after start");
        }

        let cfg = config.read().unwrap().clone();
        let mut is_populated = populated.load(Ordering::Relaxed);

        if !is_populated {
            let node_count = router.lock().await.get_statistics().num_nodes;
            if node_count < BOOTSTRAP_COMPLETE_NODES && !cfg.bootstrap_nodes.is_empty() {
                let targets: Vec<SocketAddress> = cfg
                    .bootstrap_nodes
                    .iter()
                    .take(MAX_BOOTSTRAP_CONTACTS)
                    .cloned()
                    .collect();
                let own_id = router.lock().await.own_id;
                let replied =
                    query_find_node(Arc::clone(&server), Arc::clone(&router), targets, own_id, true)
                        .await;
                debug!(replied, "DHT bootstrap attempt finished");

                let node_count = router.lock().await.get_statistics().num_nodes;
                if node_count >= BOOTSTRAP_COMPLETE_NODES {
                    populated.store(true, Ordering::Relaxed);
                    is_populated = true;
                    info!(nodes = node_count, "DHT bootstrap complete");
                } else {
                    debug!(
                        nodes = node_count,
                        retry_secs = cfg.bootstrap_retry_interval.as_secs(),
                        "DHT bootstrap incomplete; will retry"
                    );
                }
            } else if node_count >= BOOTSTRAP_COMPLETE_NODES {
                populated.store(true, Ordering::Relaxed);
                is_populated = true;
                info!(nodes = node_count, "DHT bootstrap complete");
            }
        }

        refresh_handles(
            Arc::clone(&server),
            Arc::clone(&router),
            cfg.peer_timeout,
            cfg.remove_node_timeout,
        )
        .await;
        refresh_cycles.fetch_add(1, Ordering::Relaxed);

        let node_count = router.lock().await.get_statistics().num_nodes;
        if node_count >= BOOTSTRAP_COMPLETE_NODES && !populated.swap(true, Ordering::Relaxed) {
            is_populated = true;
            info!(nodes = node_count, "DHT bootstrap complete");
        }

        let delay = if !is_populated && !cfg.bootstrap_nodes.is_empty() {
            cfg.bootstrap_retry_interval
        } else {
            cfg.refresh_interval
        };
        next = Instant::now() + delay.max(Duration::from_millis(10));
    }
}

async fn query_find_node(
    server: Arc<DhtServer>,
    router: Arc<Mutex<DhtRouter>>,
    targets: Vec<SocketAddress>,
    own_id: HashString,
    count_queries: bool,
) -> usize {
    let mut tasks = JoinSet::new();

    for address in targets {
        let server = Arc::clone(&server);
        tasks.spawn(async move {
            let Some(target) = address.as_socket_addr() else {
                return (address, None);
            };

            let query = KrpcQuery::FindNode {
                id: own_id,
                target: own_id,
            };
            let response = if count_queries {
                server.send_query(target, query).await
            } else {
                server.send_query_raw(target, query).await
            };

            match response {
                Ok(KrpcResponse::FindNode { id, nodes }) => {
                    (address, Some((id, nodes)))
                }
                _ => (address, None),
            }
        });
    }

    let mut replied = 0usize;
    while let Some(joined) = tasks.join_next().await {
        let (address, result) = match joined {
            Ok(value) => value,
            Err(_) => continue,
        };

        let Some((id, nodes)) = result else {
            router.lock().await.mark_inactive_by_address(&address);
            continue;
        };

        replied += 1;
        let mut router = router.lock().await;
        router.mark_replied(&id, now_seconds());
        router.add_node(DhtNode::new(id, address));
        for node in decode_nodes(&nodes) {
            router.add_node(node);
        }
    }

    replied
}

async fn refresh_handles(
    server: Arc<DhtServer>,
    router: Arc<Mutex<DhtRouter>>,
    peer_timeout: u32,
    remove_node_timeout: u32,
) -> usize {
    let (own_id, targets) = {
        let mut router = router.lock().await;
        let now = now_seconds();

        // Drop peers that have not re-announced and empty tracker entries.
        let _ = router.prune_trackers(peer_timeout, now);

        // Age nodes, evict bad ones and collect questionable nodes to ping.
        let questionable = router.update_nodes(now, remove_node_timeout);
        let own_id = router.own_id;

        let mut seen = HashSet::new();
        let mut targets = Vec::new();

        for node in questionable.into_iter().take(8) {
            if seen.insert(node.address.clone()) {
                targets.push(node.address);
            }
        }
        for node in router.closest_nodes(&own_id, 8) {
            if seen.insert(node.address.clone()) {
                targets.push(node.address);
            }
        }

        targets.truncate(MAX_BOOTSTRAP_CONTACTS);
        (own_id, targets)
    };

    if targets.is_empty() {
        return 0;
    }

    query_find_node(server, router, targets, own_id, true).await
}
