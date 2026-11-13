use lt_common::hash_string::HashString;
use lt_dht::controller::DhtController;
pub use lt_dht::controller::{DhtLifecycleConfig, DhtMode};
use lt_engine::{DownloadNetworkCoordinator, PeerPoolHandle};
use lt_net::addr::SocketAddress;
use lt_net::config::{ListenAddresses, NetworkConfig};
use lt_net::socket_manager::{shared_socket_manager, SocketCategory};
use lt_protocol::encryption::EncryptionPolicy;
use lt_protocol::handshake::{is_plaintext_handshake_prefix, HandshakeMessage};
use lt_protocol::mse::{accept as mse_accept, matches_obfuscated_info_hash};
use std::collections::{HashMap, HashSet};
#[cfg(target_os = "linux")]
use std::ffi::CString;
use std::io;
use std::os::fd::{FromRawFd, RawFd};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::net::TcpListener;
use tokio::sync::{watch, Semaphore};
use tokio::task::{JoinHandle, JoinSet};
use tokio::time::{sleep, timeout, timeout_at};
use tracing::{debug, info, warn};

use crate::commands::groups::ChokeGroupManager;
use crate::commands::throttle::ThrottleManager;
use crate::download_list::DownloadList;
use rt_rpc::parse_commands::parse_command_multiple;
use rt_rpc::{CommandMap, Target};

/// Cumulative number of per-torrent network coordinators started since the
/// process began, and the high-water mark of coordinators alive at once.
static COORDINATORS_SPAWNED: AtomicU64 = AtomicU64::new(0);
static COORDINATORS_PEAK: AtomicU64 = AtomicU64::new(0);

/// Returns `(spawned, peak_active)`. Diagnostic only; feeds startup and
/// shutdown metrics so an announce storm is visible without a debug build.
pub fn coordinator_stats() -> (u64, u64) {
    (
        COORDINATORS_SPAWNED.load(Ordering::Relaxed),
        COORDINATORS_PEAK.load(Ordering::Relaxed),
    )
}

fn record_coordinator_spawn(active: usize) {
    COORDINATORS_SPAWNED.fetch_add(1, Ordering::Relaxed);
    COORDINATORS_PEAK.fetch_max(active as u64, Ordering::Relaxed);
}

/// Default upper bound on concurrent inbound handshakes/connections, used
/// when no process-wide connection cap is configured.
const MAX_INBOUND_CONNECTIONS: usize = 256;
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// Inbound handshake budget for a process-wide connection cap. A cap of `0`
/// means unlimited, so the listener keeps its own bounded default rather than
/// accepting without limit.
fn inbound_connection_cap(global_cap: usize) -> usize {
    if global_cap == 0 {
        MAX_INBOUND_CONNECTIONS
    } else {
        global_cap
    }
}

/// Per-stage deadlines used while draining the network runtime.
#[derive(Debug, Clone, Copy)]
pub struct ShutdownTimeouts {
    pub listeners: Duration,
    pub dht: Duration,
    pub coordinators: Duration,
}

impl Default for ShutdownTimeouts {
    fn default() -> Self {
        Self {
            listeners: Duration::from_secs(2),
            dht: Duration::from_secs(3),
            coordinators: Duration::from_secs(3),
        }
    }
}

/// Result of a bounded network shutdown attempt.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ShutdownReport {
    pub listeners_completed: bool,
    pub dht_completed: bool,
    pub coordinators_completed: bool,
}

impl ShutdownReport {
    pub fn all_completed(&self) -> bool {
        self.listeners_completed && self.dht_completed && self.coordinators_completed
    }
}

/// Configuration for the inbound peer listener.
///
/// The legacy `Listen` object opens one or two sockets over an inclusive port
/// range. `listen_port` takes precedence when no explicit range is supplied;
/// `0` keeps the listener disabled.
#[derive(Clone, Debug)]
pub struct PeerListenerConfig {
    pub listen_port: u16,
    pub port_first: u16,
    pub port_last: u16,
    pub random: bool,
    pub backlog: i32,
    pub addresses: ListenAddresses,
    pub block_incoming: bool,
    pub handshake_timeout: Duration,
    pub max_connections: usize,
    pub enable_utp: bool,
    pub enable_upnp: bool,
}

impl PeerListenerConfig {
    pub fn new(listen_port: u16) -> Self {
        Self::from_network_config(listen_port, &NetworkConfig::new())
    }

    pub fn from_network_config(listen_port: u16, config: &NetworkConfig) -> Self {
        Self {
            listen_port,
            port_first: listen_port,
            port_last: listen_port,
            random: false,
            backlog: config.listen_backlog(),
            addresses: config.listen_addresses(),
            block_incoming: config.is_block_incoming(),
            handshake_timeout: HANDSHAKE_TIMEOUT,
            // Read the process-wide cap once at spawn, like enable_utp/enable_upnp.
            max_connections: inbound_connection_cap(lt_engine::network::global_max_connections()),
            enable_utp: config.is_enable_utp(),
            enable_upnp: config.is_enable_upnp(),
        }
    }

    /// Uses an inclusive port range. `random` selects a random starting point
    /// and scans the range from there; exhaustion is reported instead of
    /// silently binding an ephemeral port.
    pub fn with_port_range(mut self, first: u16, last: u16, random: bool) -> Self {
        self.listen_port = 0;
        self.port_first = first;
        self.port_last = last;
        self.random = random;
        self
    }

    pub fn with_backlog(mut self, backlog: i32) -> Self {
        self.backlog = backlog.max(1);
        self
    }

    fn effective_range(&self) -> Option<(u16, u16)> {
        if self.listen_port != 0 {
            return Some((self.listen_port, self.listen_port));
        }
        if self.port_first == 0 && self.port_last == 0 {
            return None;
        }
        if self.port_first == 0 || self.port_first > self.port_last {
            return None;
        }
        Some((self.port_first, self.port_last))
    }
}

/// Snapshot of the peer listener state.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ListenerStatistics {
    pub open: bool,
    pub ports: Vec<u16>,
    pub accepted: u64,
    pub handshakes: u64,
    pub active: u64,
}

struct ListenerState {
    ports: Vec<u16>,
    open: bool,
}

fn listener_state() -> &'static Mutex<ListenerState> {
    static STATE: OnceLock<Mutex<ListenerState>> = OnceLock::new();
    STATE.get_or_init(|| {
        Mutex::new(ListenerState {
            ports: Vec::new(),
            open: false,
        })
    })
}

fn listener_accepted() -> &'static AtomicU64 {
    static VALUE: AtomicU64 = AtomicU64::new(0);
    &VALUE
}

fn listener_handshakes() -> &'static AtomicU64 {
    static VALUE: AtomicU64 = AtomicU64::new(0);
    &VALUE
}

fn listener_active() -> &'static AtomicU64 {
    static VALUE: AtomicU64 = AtomicU64::new(0);
    &VALUE
}

/// Returns the live listener counters. `network.listen.port`,
/// `network.port_open` and `network.total_handshakes` can be derived from it.
pub fn listener_statistics() -> ListenerStatistics {
    let state = listener_state().lock().unwrap();
    ListenerStatistics {
        open: state.open,
        ports: state.ports.clone(),
        accepted: listener_accepted().load(Ordering::Relaxed),
        handshakes: listener_handshakes().load(Ordering::Relaxed),
        active: listener_active().load(Ordering::Relaxed),
    }
}

pub fn reset_listener_statistics() {
    let mut state = listener_state().lock().unwrap();
    state.ports.clear();
    state.open = false;
    listener_accepted().store(0, Ordering::Relaxed);
    listener_handshakes().store(0, Ordering::Relaxed);
    listener_active().store(0, Ordering::Relaxed);
}

fn mark_listener_open(port: u16) {
    let mut state = listener_state().lock().unwrap();
    state.open = true;
    if !state.ports.contains(&port) {
        state.ports.push(port);
    }
}

fn mark_listener_closed() {
    let mut state = listener_state().lock().unwrap();
    state.open = false;
    state.ports.clear();
}

struct ActiveConnectionGuard;

impl Drop for ActiveConnectionGuard {
    fn drop(&mut self) {
        listener_active().fetch_sub(1, Ordering::Relaxed);
    }
}

/// Process-wide DHT controller published by [`NetworkRuntime`] so the
/// `dht.*` command surface can inspect and control the live instance.
fn shared_dht_controller() -> &'static Mutex<Option<Arc<DhtController>>> {
    static CONTROLLER: OnceLock<Mutex<Option<Arc<DhtController>>>> = OnceLock::new();
    CONTROLLER.get_or_init(|| Mutex::new(None))
}

pub fn set_dht_controller(controller: Option<Arc<DhtController>>) {
    *shared_dht_controller().lock().unwrap() = controller;
}

pub fn dht_controller() -> Option<Arc<DhtController>> {
    shared_dht_controller().lock().unwrap().clone()
}

/// Command-layer DHT preferences (`dht.port`, `dht.mode`) registered by
/// [`initialize_builtin_commands`](crate::commands::initialize_builtin_commands).
fn shared_dht_preferences(
) -> &'static Mutex<Option<Arc<RwLock<crate::commands::tracker::DhtConfig>>>> {
    static PREFERENCES: OnceLock<Mutex<Option<Arc<RwLock<crate::commands::tracker::DhtConfig>>>>> =
        OnceLock::new();
    PREFERENCES.get_or_init(|| Mutex::new(None))
}

pub fn set_dht_config_handle(config: Option<Arc<RwLock<crate::commands::tracker::DhtConfig>>>) {
    *shared_dht_preferences().lock().unwrap() = config;
}

fn dht_config_snapshot() -> Option<(u16, String, u16)> {
    let handle = shared_dht_preferences().lock().unwrap().clone()?;
    let config = handle.read().unwrap();
    Some((config.port, config.mode.clone(), config.override_port))
}

fn log_dht_statistics(dht: &DhtController) {
    let stats = dht.get_statistics();
    debug!(
        active = stats.active,
        cycle = stats.cycle,
        nodes = stats.num_nodes,
        buckets = stats.num_buckets,
        trackers = stats.num_trackers,
        peers = stats.num_peers,
        queries_sent = stats.queries_sent,
        queries_received = stats.queries_received,
        "DHT statistics"
    );
}

/// Owns the join handles for every task spawned by [`NetworkRuntime::spawn`].
///
/// The caller must signal shutdown through the same `watch` channel that was
/// passed to `spawn`, then await [`NetworkRuntimeHandle::drain`]. Draining
/// happens in a fixed order (listeners, DHT, coordinators) and every stage is
/// bounded; tasks that overrun their deadline are aborted so no task is left
/// detached for the lifetime of the process.
pub struct NetworkRuntimeHandle {
    listeners: Mutex<Vec<JoinHandle<()>>>,
    dht: Mutex<Vec<JoinHandle<()>>>,
    coordinators: Mutex<Option<JoinHandle<()>>>,
}

impl NetworkRuntimeHandle {
    /// Stops all network work in dependency order and returns which stages
    /// finished before their deadline.
    pub async fn drain(&self, timeouts: ShutdownTimeouts) -> ShutdownReport {
        let listeners = self.take(&self.listeners);
        let listeners_completed = join_all_bounded(listeners, timeouts.listeners).await;

        let dht = self.take(&self.dht);
        let dht_completed = join_all_bounded(dht, timeouts.dht).await;

        let coordinator = self.coordinators.lock().unwrap().take();
        let coordinators_completed = match coordinator {
            Some(task) => join_all_bounded(vec![task], timeouts.coordinators).await,
            None => true,
        };

        // Any task that outlived its stage deadline was already aborted by
        // `join_all_bounded`. Keep this as a final safety net for partially
        // constructed handles.
        self.abort_all();

        ShutdownReport {
            listeners_completed,
            dht_completed,
            coordinators_completed,
        }
    }

    /// Aborts every managed task without waiting. Used by the synchronous
    /// fallback shutdown path where blocking on Tokio is not possible.
    pub fn abort_all(&self) {
        for task in self.take(&self.listeners) {
            task.abort();
        }
        mark_listener_closed();
        for task in self.take(&self.dht) {
            task.abort();
        }
        if let Some(task) = self.coordinators.lock().unwrap().take() {
            task.abort();
        }
    }

    fn take(&self, tasks: &Mutex<Vec<JoinHandle<()>>>) -> Vec<JoinHandle<()>> {
        std::mem::take(&mut *tasks.lock().unwrap())
    }

    /// Test/diagnostic helper: true once every managed handle has been taken.
    pub fn is_drained(&self) -> bool {
        self.listeners.lock().unwrap().is_empty()
            && self.dht.lock().unwrap().is_empty()
            && self.coordinators.lock().unwrap().is_none()
    }
}

/// Joins all tasks using a single shared deadline. On timeout the task is
/// aborted and awaited, and the function reports that the stage was
/// incomplete.
async fn join_all_bounded(tasks: Vec<JoinHandle<()>>, budget: Duration) -> bool {
    if tasks.is_empty() {
        return true;
    }

    let deadline = tokio::time::Instant::now() + budget;
    let mut completed = true;

    for mut task in tasks {
        match tokio::time::timeout_at(deadline, &mut task).await {
            Ok(Ok(())) => {}
            Ok(Err(err)) => {
                if !err.is_cancelled() {
                    warn!(error = %err, "Background network task ended unexpectedly");
                }
                completed = false;
            }
            Err(_) => {
                warn!("Background network task exceeded shutdown deadline; aborting");
                task.abort();
                let _ = task.await;
                completed = false;
            }
        }
    }

    completed
}

pub struct NetworkRuntime;

/// Binds the peer listener and spawns one accept loop per bound socket.
///
/// IPv6 is attempted first. When `block_ipv4in6` is unset and both families
/// are configured as wildcards, a single dual-stack `[::]` socket is used so
/// the same port is never bound twice. If IPv6 is unavailable we fall back to
/// IPv4, and explicit per-family bind addresses are honoured independently.
fn spawn_peer_listeners(
    download_list: Arc<RwLock<DownloadList>>,
    our_peer_id: HashString,
    config: PeerListenerConfig,
    shutdown_rx: watch::Receiver<bool>,
    peer_pools: Arc<Mutex<HashMap<HashString, PeerPoolHandle>>>,
    encryption_policy: Arc<Mutex<EncryptionPolicy>>,
) -> Option<JoinHandle<()>> {
    if config.block_incoming {
        info!("Peer listener disabled: incoming connections are blocked");
        return None;
    }
    if config.effective_range().is_none() {
        info!("Peer listener disabled: listen port is 0");
        return None;
    }

    let downloads = Arc::clone(&download_list);
    let shutdown = shutdown_rx;
    let permits = Arc::new(Semaphore::new(config.max_connections.max(1)));

    Some(tokio::spawn(async move {
        let (listeners, port) = match bind_listener_set(&config).await {
            Ok(bound) => bound,
            Err(err) => {
                warn!(error = %err, "Could not bind peer listener");
                return;
            }
        };

        mark_listener_open(port);
        let mut tasks: JoinSet<()> = JoinSet::new();

        // One uTP socket on the primary listener address avoids binding the
        // same UDP port twice when IPv4 and IPv6 listeners are separate.
        let utp_addr = listeners.first().and_then(|l| l.local_addr().ok());

        for listener in listeners {
            if let Ok(addr) = listener.local_addr() {
                info!(%addr, "Peer listener bound");
            }
            tasks.spawn(accept_loop(
                listener,
                Arc::clone(&downloads),
                our_peer_id,
                shutdown.clone(),
                Arc::clone(&peer_pools),
                Arc::clone(&encryption_policy),
                config.handshake_timeout,
                Arc::clone(&permits),
            ));
        }

        if let Some(addr) = utp_addr {
            if config.enable_utp {
                tasks.spawn(utp_accept_loop(
                    addr,
                    Arc::clone(&downloads),
                    our_peer_id,
                    shutdown.clone(),
                    Arc::clone(&peer_pools),
                    Arc::clone(&encryption_policy),
                    config.handshake_timeout,
                    Arc::clone(&permits),
                ));
            } else {
                info!("uTP accept disabled by configuration");
            }
        }

        tasks.spawn(lsd_loop(
            port,
            Arc::clone(&downloads),
            shutdown.clone(),
            Arc::clone(&peer_pools),
        ));
        if config.enable_upnp {
            tasks.spawn(port_mapping_loop(port, shutdown.clone()));
        } else {
            info!("UPnP/NAT-PMP port mapping disabled by configuration");
        }
        tasks.spawn(choke_supervisor_loop(
            Arc::clone(&downloads),
            shutdown.clone(),
        ));
        tasks.spawn(web_seed_loop(Arc::clone(&downloads), shutdown.clone()));

        while tasks.join_next().await.is_some() {}
        mark_listener_closed();
        debug!("Peer accept loops stopped");
    }))
}

/// Candidates in the requested order. Random mode starts at a random offset
/// and wraps, but still scans every configured port before giving up.
fn candidate_ports(first: u16, last: u16, random: bool) -> Vec<u16> {
    if first == 0 || first > last {
        return Vec::new();
    }
    let count = last as u32 - first as u32 + 1;
    let start = if random {
        let mut bytes = [0u8; 2];
        if getrandom::fill(&mut bytes).is_ok() {
            (u16::from_be_bytes(bytes) as u32) % count
        } else {
            0
        }
    } else {
        0
    };

    (0..count)
        .map(|offset| first + ((start + offset) % count) as u16)
        .collect()
}

/// Uniform random index in `0..len`, used for the optimistic unchoke pick so
/// rotation is unpredictable rather than a fixed `round % len` cycle.
fn random_index(len: usize) -> usize {
    if len <= 1 {
        return 0;
    }
    let mut bytes = [0u8; 8];
    if getrandom::fill(&mut bytes).is_ok() {
        (u64::from_le_bytes(bytes) % len as u64) as usize
    } else {
        0
    }
}

fn socket_address_for(address: &SocketAddress, port: u16) -> Option<std::net::SocketAddr> {
    let ip = address.ip()?;
    Some(std::net::SocketAddr::new(ip, port))
}

fn bind_socket(
    address: std::net::SocketAddr,
    backlog: i32,
    device: &str,
    ipv6_only: bool,
) -> io::Result<TcpListener> {
    let family = match address {
        std::net::SocketAddr::V4(_) => libc::AF_INET,
        std::net::SocketAddr::V6(_) => libc::AF_INET6,
    };

    // SOCK_NONBLOCK/SOCK_CLOEXEC only exist in the Linux socket API; other
    // platforms set the same flags with fcntl() after creating the socket.
    #[cfg(target_os = "linux")]
    let socket_type = libc::SOCK_STREAM | libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC;
    #[cfg(not(target_os = "linux"))]
    let socket_type = libc::SOCK_STREAM;

    let fd = unsafe { libc::socket(family, socket_type, 0) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }

    #[cfg(not(target_os = "linux"))]
    {
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        if flags >= 0 {
            unsafe {
                libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK);
            }
        }
        let fd_flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
        if fd_flags >= 0 {
            unsafe {
                libc::fcntl(fd, libc::F_SETFD, fd_flags | libc::FD_CLOEXEC);
            }
        }
    }

    let result = (|| -> io::Result<()> {
        let enable: libc::c_int = 1;
        let rc = unsafe {
            libc::setsockopt(
                fd,
                libc::SOL_SOCKET,
                libc::SO_REUSEADDR,
                &enable as *const _ as *const libc::c_void,
                std::mem::size_of_val(&enable) as libc::socklen_t,
            )
        };
        if rc != 0 {
            return Err(io::Error::last_os_error());
        }

        if family == libc::AF_INET6 {
            let v6only: libc::c_int = if ipv6_only { 1 } else { 0 };
            let rc = unsafe {
                libc::setsockopt(
                    fd,
                    libc::IPPROTO_IPV6,
                    libc::IPV6_V6ONLY,
                    &v6only as *const _ as *const libc::c_void,
                    std::mem::size_of_val(&v6only) as libc::socklen_t,
                )
            };
            if rc != 0 {
                return Err(io::Error::last_os_error());
            }
        }

        #[cfg(target_os = "linux")]
        if !device.is_empty() {
            let name = CString::new(device).map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidInput, "bind device contains NUL")
            })?;
            let rc = unsafe {
                libc::setsockopt(
                    fd,
                    libc::SOL_SOCKET,
                    libc::SO_BINDTODEVICE,
                    name.as_ptr() as *const libc::c_void,
                    name.as_bytes_with_nul().len() as libc::socklen_t,
                )
            };
            if rc != 0 {
                // Binding to a device normally requires CAP_NET_RAW. Treat a
                // failure as non-fatal so an unprivileged daemon still starts,
                // but make it visible in the logs.
                debug!(
                    device,
                    error = %io::Error::last_os_error(),
                    "SO_BINDTODEVICE failed; using the default route"
                );
            }
        }

        #[cfg(not(target_os = "linux"))]
        if !device.is_empty() {
            debug!(
                device,
                "SO_BINDTODEVICE is not supported on this platform; using the default route"
            );
        }

        let mut storage: libc::sockaddr_storage = unsafe { std::mem::zeroed() };
        let (ptr, len) = match address {
            std::net::SocketAddr::V4(v4) => {
                let sin = &mut storage as *mut _ as *mut libc::sockaddr_in;
                unsafe {
                    (*sin).sin_family = libc::AF_INET as libc::sa_family_t;
                    (*sin).sin_port = v4.port().to_be();
                    (*sin).sin_addr = libc::in_addr {
                        s_addr: u32::from_ne_bytes(v4.ip().octets()),
                    };
                }
                (
                    &storage as *const _ as *const libc::sockaddr,
                    std::mem::size_of::<libc::sockaddr_in>() as libc::socklen_t,
                )
            }
            std::net::SocketAddr::V6(v6) => {
                let sin6 = &mut storage as *mut _ as *mut libc::sockaddr_in6;
                unsafe {
                    (*sin6).sin6_family = libc::AF_INET6 as libc::sa_family_t;
                    (*sin6).sin6_port = v6.port().to_be();
                    (*sin6).sin6_addr = libc::in6_addr {
                        s6_addr: v6.ip().octets(),
                    };
                    (*sin6).sin6_flowinfo = v6.flowinfo();
                    (*sin6).sin6_scope_id = v6.scope_id();
                }
                (
                    &storage as *const _ as *const libc::sockaddr,
                    std::mem::size_of::<libc::sockaddr_in6>() as libc::socklen_t,
                )
            }
        };

        let rc = unsafe { libc::bind(fd, ptr, len) };
        if rc != 0 {
            return Err(io::Error::last_os_error());
        }
        if unsafe { libc::listen(fd, backlog) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    })();

    if let Err(err) = result {
        unsafe { libc::close(fd) };
        return Err(err);
    }

    let std_listener = unsafe { std::net::TcpListener::from_raw_fd(fd as RawFd) };
    std_listener.set_nonblocking(true)?;
    TcpListener::from_std(std_listener)
}

async fn bind_port(port: u16, config: &PeerListenerConfig) -> io::Result<Vec<TcpListener>> {
    let addresses = &config.addresses;
    let inet = addresses
        .inet_address
        .as_ref()
        .and_then(|address| socket_address_for(address, port));
    let inet6 = addresses
        .inet6_address
        .as_ref()
        .and_then(|address| socket_address_for(address, port));

    let wildcard_v4 = inet
        .map(|address| address.ip().is_unspecified())
        .unwrap_or(false);
    let wildcard_v6 = inet6
        .map(|address| address.ip().is_unspecified())
        .unwrap_or(false);
    // When IPv4 is disabled (or explicitly blocked inside IPv6), the IPv6
    // socket must not accept mapped IPv4 peers.
    let must_v6only = addresses.block_ipv4in6 || inet.is_none();

    let mut listeners = Vec::new();

    // Both families configured as wildcards: prefer one dual-stack socket
    // unless an interface binding forces per-family sockets.
    let use_dual_stack = !must_v6only
        && wildcard_v4
        && wildcard_v6
        && inet.is_some()
        && addresses.inet_device.is_empty()
        && addresses.inet6_device.is_empty();
    if use_dual_stack {
        if let Some(v6) = inet6 {
            match bind_socket(v6, config.backlog, "", false) {
                Ok(listener) => return Ok(vec![listener]),
                Err(err) => {
                    debug!(error = %err, "IPv6 dual-stack bind failed; falling back to IPv4");
                    if let Some(v4) = inet {
                        let listener =
                            bind_socket(v4, config.backlog, &addresses.inet_device, false)?;
                        return Ok(vec![listener]);
                    }
                    return Err(err);
                }
            }
        }
    }

    let v6only = must_v6only;
    if let Some(v6) = inet6 {
        match bind_socket(v6, config.backlog, &addresses.inet6_device, v6only) {
            Ok(listener) => listeners.push(listener),
            Err(err) => {
                debug!(error = %err, "IPv6 peer bind failed");
                if inet.is_none() {
                    return Err(err);
                }
            }
        }
    }

    if let Some(v4) = inet {
        match bind_socket(v4, config.backlog, &addresses.inet_device, false) {
            Ok(listener) => listeners.push(listener),
            Err(err) => {
                debug!(error = %err, "IPv4 peer bind failed");
                if listeners.is_empty() {
                    return Err(err);
                }
            }
        }
    }

    if listeners.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::AddrNotAvailable,
            "no listen address is enabled",
        ));
    }
    Ok(listeners)
}

async fn bind_listener_set(config: &PeerListenerConfig) -> io::Result<(Vec<TcpListener>, u16)> {
    let Some((first, last)) = config.effective_range() else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "listen port is disabled",
        ));
    };

    let mut last_error = None;
    for port in candidate_ports(first, last, config.random) {
        match bind_port(port, config).await {
            Ok(listeners) => return Ok((listeners, port)),
            Err(err) => last_error = Some(err),
        }
    }

    Err(last_error.unwrap_or_else(|| {
        io::Error::new(
            io::ErrorKind::AddrInUse,
            format!("listen port range {first}-{last} is exhausted"),
        )
    }))
}

#[allow(clippy::too_many_arguments)]
async fn accept_loop(
    listener: TcpListener,
    download_list: Arc<RwLock<DownloadList>>,
    our_peer_id: HashString,
    mut shutdown_rx: watch::Receiver<bool>,
    peer_pools: Arc<Mutex<HashMap<HashString, PeerPoolHandle>>>,
    encryption_policy: Arc<Mutex<EncryptionPolicy>>,
    handshake_timeout: Duration,
    permits: Arc<Semaphore>,
) {
    let mut connections: JoinSet<()> = JoinSet::new();

    loop {
        if *shutdown_rx.borrow() {
            break;
        }

        // Reap finished connection tasks so the set cannot grow unbounded.
        while connections.try_join_next().is_some() {}

        tokio::select! {
            _ = shutdown_rx.changed() => break,
            accepted = listener.accept() => {
                let (mut stream, peer_addr) = match accepted {
                    Ok(pair) => pair,
                    Err(err) => {
                        debug!(error = %err, "Peer accept failed");
                        continue;
                    }
                };

                listener_accepted().fetch_add(1, Ordering::Relaxed);

                let permit = match Arc::clone(&permits).try_acquire_owned() {
                    Ok(permit) => permit,
                    Err(_) => {
                        debug!(peer = %peer_addr, "Inbound connection limit reached");
                        continue;
                    }
                };

                if !shared_socket_manager().can_open_socket(SocketCategory::Generic) {
                    debug!(peer = %peer_addr, "Socket budget exhausted; dropping inbound connection");
                    continue;
                }
                listener_active().fetch_add(1, Ordering::Relaxed);

                let downloads = Arc::clone(&download_list);
                let shutdown = shutdown_rx.clone();
                let peer_pools = Arc::clone(&peer_pools);
                let policy_state = Arc::clone(&encryption_policy);

                connections.spawn(async move {
                    let _active = ActiveConnectionGuard;
                    let _permit = permit;

                    // Classify the first 20 bytes exactly like the legacy
                    // `read_encryption_key`: a complete plaintext handshake
                    // preamble means classic BT, anything else means MSE.
                    // Waiting for 20 bytes avoids misclassifying a DH key
                    // whose first byte happens to be 19.
                    let policy = *policy_state.lock().unwrap();
                    let mut first = [0u8; 20];
                    let deadline = tokio::time::Instant::now() + handshake_timeout;

                    loop {
                        match timeout_at(deadline, stream.peek(&mut first)).await {
                            Ok(Ok(0)) => {
                                debug!(peer = %peer_addr, "Inbound peer closed before handshake");
                                return;
                            }
                            Ok(Ok(n)) if n >= first.len() => break,
                            Ok(Ok(_)) => {
                                sleep(Duration::from_millis(2)).await;
                            }
                            Ok(Err(err)) => {
                                debug!(peer = %peer_addr, error = %err, "Inbound peek failed");
                                return;
                            }
                            Err(_) => {
                                debug!(peer = %peer_addr, "Inbound handshake timed out");
                                return;
                            }
                        }
                    }

                    let plaintext_prefix = is_plaintext_handshake_prefix(&first);

                    if !plaintext_prefix && policy.allow_encrypted_handshake() {
                        let resolver_downloads = Arc::clone(&downloads);
                        let resolve = move |obfuscated: &HashString| -> Option<HashString> {
                            let list = resolver_downloads.read().unwrap();
                            for download in list.all() {
                                let guard = download.read().unwrap();
                                // Constant-time req2 comparison: do not leak a
                                // matching prefix through the lookup.
                                if matches_obfuscated_info_hash(obfuscated, guard.info_hash()) {
                                    return Some(*guard.info_hash());
                                }
                            }
                            None
                        };

                        let session =
                            match mse_accept(&mut stream, our_peer_id, &policy, resolve).await {
                                Ok(session) => session,
                                Err(err) => {
                                    debug!(peer = %peer_addr, error = %err, "Inbound MSE handshake failed");
                                    return;
                                }
                            };
                        listener_handshakes().fetch_add(1, Ordering::Relaxed);

                        let download = {
                            let list = downloads.read().unwrap();
                            list.find(&session.info_hash)
                        };

                        let Some(download) = download else {
                            debug!(peer = %peer_addr, "MSE peer for unknown info-hash");
                            return;
                        };

                        let pool = peer_pools
                            .lock()
                            .unwrap()
                            .get(&session.info_hash)
                            .cloned();
                        let engine = download.read().unwrap().engine_download();
                        lt_engine::run_inbound_mse_peer_with_pool(
                            stream, session, engine, shutdown, pool,
                        )
                        .await;
                        return;
                    }

                    // Plaintext handshake (or a peer that sent non-MSE bytes).
                    // Never accept it when policy requires encryption.
                    if !policy.allow_plaintext_handshake() {
                        debug!(peer = %peer_addr, "Rejected unencrypted inbound handshake");
                        return;
                    }

                    let mut handshake_buf = [0u8; 68];

                    match timeout(handshake_timeout, stream.read_exact(&mut handshake_buf)).await {
                        Ok(Ok(_)) => {}
                        Ok(Err(err)) => {
                            debug!(peer = %peer_addr, error = %err, "Inbound handshake read failed");
                            return;
                        }
                        Err(_) => {
                            debug!(peer = %peer_addr, "Inbound handshake timed out");
                            return;
                        }
                    }

                    let handshake = match HandshakeMessage::decode(&handshake_buf) {
                        Ok(handshake) => handshake,
                        Err(err) => {
                            debug!(peer = %peer_addr, error = %err, "Invalid inbound handshake");
                            return;
                        }
                    };
                    listener_handshakes().fetch_add(1, Ordering::Relaxed);

                    let download = {
                        let list = downloads.read().unwrap();
                        list.find(&handshake.info_hash)
                    };

                    let Some(download) = download else {
                        debug!(
                            peer = %peer_addr,
                            info_hash = %handshake.info_hash.to_hex(),
                            "Inbound peer for unknown info-hash"
                        );
                        return;
                    };

                    let pool = peer_pools
                        .lock()
                        .unwrap()
                        .get(&handshake.info_hash)
                        .cloned();
                    let engine = download.read().unwrap().engine_download();
                    lt_engine::run_inbound_peer_with_pool(
                        stream,
                        handshake,
                        our_peer_id,
                        engine,
                        shutdown,
                        pool,
                    )
                    .await;
                });
            }
        }
    }

    // Inbound peers are children of the listener: cancel them and wait for
    // their tasks to finish so none survive the listener task itself.
    connections.abort_all();
    while connections.join_next().await.is_some() {}

    debug!("Peer accept loop stopped");
}

/// Accepts inbound uTP (BEP 29) peers over UDP on the same port as the TCP
/// listener. uTP carries plaintext BitTorrent handshakes; MSE-over-uTP is not
/// negotiated, so encrypted-only policies reject these connections.
#[allow(clippy::too_many_arguments)]
async fn utp_accept_loop(
    bind_addr: std::net::SocketAddr,
    download_list: Arc<RwLock<DownloadList>>,
    our_peer_id: HashString,
    mut shutdown_rx: watch::Receiver<bool>,
    peer_pools: Arc<Mutex<HashMap<HashString, PeerPoolHandle>>>,
    encryption_policy: Arc<Mutex<EncryptionPolicy>>,
    handshake_timeout: Duration,
    permits: Arc<Semaphore>,
) {
    let mut socket = match lt_net::UtpSocket::bind(bind_addr).await {
        Ok(socket) => socket,
        Err(err) => {
            warn!(error = %err, "Could not bind uTP listener");
            return;
        }
    };
    info!(addr = %socket.local_addr(), "uTP listener bound");

    let mut connections: JoinSet<()> = JoinSet::new();

    loop {
        if *shutdown_rx.borrow() {
            break;
        }
        while connections.try_join_next().is_some() {}

        tokio::select! {
            _ = shutdown_rx.changed() => break,
            accepted = socket.accept() => {
                let mut stream = match accepted {
                    Ok(stream) => stream,
                    Err(err) => {
                        debug!(error = %err, "uTP accept failed");
                        continue;
                    }
                };
                let peer_addr = stream.peer_addr();

                let permit = match Arc::clone(&permits).try_acquire_owned() {
                    Ok(permit) => permit,
                    Err(_) => {
                        debug!(peer = %peer_addr, "uTP inbound connection limit reached");
                        continue;
                    }
                };

                let downloads = Arc::clone(&download_list);
                let shutdown = shutdown_rx.clone();
                let peer_pools = Arc::clone(&peer_pools);
                let policy_state = Arc::clone(&encryption_policy);

                connections.spawn(async move {
                    let _permit = permit;
                    let policy = *policy_state.lock().unwrap_or_else(|e| e.into_inner());
                    if !policy.allow_plaintext_handshake() {
                        debug!(peer = %peer_addr, "Rejected unencrypted inbound uTP handshake");
                        return;
                    }

                    let mut handshake_buf = [0u8; 68];
                    match timeout(handshake_timeout, stream.read_exact(&mut handshake_buf)).await {
                        Ok(Ok(_)) => {}
                        Ok(Err(err)) => {
                            debug!(peer = %peer_addr, error = %err, "uTP handshake read failed");
                            return;
                        }
                        Err(_) => {
                            debug!(peer = %peer_addr, "uTP handshake timed out");
                            return;
                        }
                    }

                    let handshake = match HandshakeMessage::decode(&handshake_buf) {
                        Ok(handshake) => handshake,
                        Err(err) => {
                            debug!(peer = %peer_addr, error = %err, "Invalid inbound uTP handshake");
                            return;
                        }
                    };

                    let download = {
                        let list = downloads.read().unwrap_or_else(|e| e.into_inner());
                        list.find(&handshake.info_hash)
                    };
                    let Some(download) = download else {
                        debug!(
                            peer = %peer_addr,
                            info_hash = %handshake.info_hash.to_hex(),
                            "uTP peer for unknown info-hash"
                        );
                        return;
                    };

                    let pool = peer_pools
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .get(&handshake.info_hash)
                        .cloned();
                    let engine = download
                        .read()
                        .unwrap_or_else(|e| e.into_inner())
                        .engine_download();
                    lt_engine::run_inbound_utp_peer(
                        stream,
                        handshake,
                        our_peer_id,
                        engine,
                        shutdown,
                        pool,
                        Some(handshake_timeout),
                    )
                    .await;
                });
            }
        }
    }

    connections.abort_all();
    while connections.join_next().await.is_some() {}
    debug!("uTP accept loop stopped");
}

fn rand_u32() -> u32 {
    let mut bytes = [0u8; 4];
    if getrandom::fill(&mut bytes).is_ok() {
        u32::from_be_bytes(bytes)
    } else {
        0
    }
}

async fn lsd_loop(
    listen_port: u16,
    download_list: Arc<RwLock<DownloadList>>,
    mut shutdown_rx: watch::Receiver<bool>,
    peer_pools: Arc<Mutex<HashMap<HashString, PeerPoolHandle>>>,
) {
    let socket = match lt_net::create_lsd_socket() {
        Ok(s) => s,
        Err(err) => {
            debug!(error = %err, "Could not initialize LSD multicast socket");
            return;
        }
    };

    let cookie = format!("{:08x}", rand_u32());
    let mut announce_interval = tokio::time::interval(Duration::from_secs(300));
    let mut buf = [0u8; 2048];
    let multicast_target =
        std::net::SocketAddr::from((lt_net::LSD_MULTICAST_IPV4, lt_net::LSD_PORT));

    loop {
        if *shutdown_rx.borrow() {
            break;
        }

        tokio::select! {
            _ = shutdown_rx.changed() => break,
            _ = announce_interval.tick() => {
                let active_hashes: Vec<HashString> = {
                    let dl = download_list.read().unwrap();
                    dl.all()
                        .iter()
                        .filter_map(|d| {
                            let item = d.read().unwrap();
                            if item.is_active() && !item.metadata().is_private {
                                Some(*item.info_hash())
                            } else {
                                None
                            }
                        })
                        .collect()
                };

                if !active_hashes.is_empty() {
                    let packet = lt_net::format_lsd_packet(listen_port, &active_hashes, Some(&cookie));
                    let _ = socket.send_to(&packet, multicast_target).await;
                }
            }
            recv_res = socket.recv_from(&mut buf) => {
                if let Ok((len, sender)) = recv_res {
                    if let Some(packet) = lt_net::parse_lsd_packet(&buf[..len]) {
                        if packet.cookie.as_deref() == Some(&cookie) {
                            continue;
                        }
                        let peer_addr = std::net::SocketAddr::new(sender.ip(), packet.port);
                        let pools = peer_pools.lock().unwrap();
                        for hash in packet.info_hashes {
                            if let Some(pool_handle) = pools.get(&hash) {
                                let mut pool = pool_handle.lock().unwrap();
                                pool.add_peer(peer_addr, lt_engine::PeerSource::Lsd, false);
                            }
                        }
                    }
                }
            }
        }
    }
    debug!("LSD multicast loop stopped");
}

/// How a NAT port mapping was established, so it can be renewed and removed
/// through the same mechanism.
#[derive(Clone, Copy)]
enum MappingMethod {
    Upnp,
    NatPmp(std::net::Ipv4Addr),
}

/// A successfully established NAT port mapping.
#[derive(Clone, Copy)]
struct ActiveMapping {
    internal_port: u16,
    external_port: u16,
    protocol: lt_net::PortMappingProtocol,
    method: MappingMethod,
}

/// Establishes one mapping, preferring UPnP IGD and falling back to NAT-PMP on
/// the discovered gateway (then well-known candidates).
async fn establish_mapping(
    internal_port: u16,
    protocol: lt_net::PortMappingProtocol,
    gateways: &[std::net::Ipv4Addr],
    lease: u32,
) -> Option<ActiveMapping> {
    if let Ok(ext) =
        lt_net::try_map_upnp(internal_port, internal_port, protocol, "rstorrent", lease).await
    {
        return Some(ActiveMapping {
            internal_port,
            external_port: ext,
            protocol,
            method: MappingMethod::Upnp,
        });
    }
    for &gw in gateways {
        if let Ok(ext) =
            lt_net::try_map_nat_pmp(gw, internal_port, internal_port, protocol, lease).await
        {
            return Some(ActiveMapping {
                internal_port,
                external_port: ext,
                protocol,
                method: MappingMethod::NatPmp(gw),
            });
        }
    }
    None
}

/// Refreshes the lease on an existing mapping, keeping its external port.
async fn renew_mapping(mapping: &ActiveMapping, lease: u32) -> bool {
    match mapping.method {
        MappingMethod::Upnp => lt_net::try_map_upnp(
            mapping.internal_port,
            mapping.external_port,
            mapping.protocol,
            "rstorrent",
            lease,
        )
        .await
        .is_ok(),
        MappingMethod::NatPmp(gw) => lt_net::try_map_nat_pmp(
            gw,
            mapping.internal_port,
            mapping.external_port,
            mapping.protocol,
            lease,
        )
        .await
        .is_ok(),
    }
}

/// Removes a mapping on shutdown so the router does not keep a stale hole.
async fn delete_mapping(mapping: &ActiveMapping) {
    let result = match mapping.method {
        MappingMethod::Upnp => {
            lt_net::try_delete_upnp(mapping.external_port, mapping.protocol).await
        }
        MappingMethod::NatPmp(gw) => {
            lt_net::try_delete_nat_pmp(
                gw,
                mapping.internal_port,
                mapping.external_port,
                mapping.protocol,
            )
            .await
        }
    };
    if let Err(err) = result {
        debug!(error = %err, external = mapping.external_port, "port mapping delete failed");
    }
}

async fn port_mapping_loop(listen_port: u16, mut shutdown_rx: watch::Receiver<bool>) {
    if *shutdown_rx.borrow() || listen_port == 0 {
        let _ = shutdown_rx.changed().await;
        return;
    }

    let lease: u32 = 3600;
    let renew_interval = std::time::Duration::from_secs(u64::from(lease / 2));

    // DHT may listen on a different UDP port than the peer listener; honour the
    // command-layer `dht.port`/`dht.override_port` preferences the same way the
    // DHT lifecycle does.
    let dht_port = match dht_config_snapshot() {
        Some((port, _mode, override_port)) => {
            let preferred = if override_port != 0 {
                override_port
            } else {
                port
            };
            if preferred != 0 {
                preferred
            } else {
                listen_port
            }
        }
        None => listen_port,
    };

    // TCP for incoming peer connections, UDP for uTP on the same port, and UDP
    // for DHT when it uses a distinct port.
    let mut desired: Vec<(u16, lt_net::PortMappingProtocol)> = vec![
        (listen_port, lt_net::PortMappingProtocol::Tcp),
        (listen_port, lt_net::PortMappingProtocol::Udp),
    ];
    if dht_port != listen_port {
        desired.push((dht_port, lt_net::PortMappingProtocol::Udp));
    }

    // Discover the real default gateway first, then fall back to common router
    // addresses for hosts where the routing table is unavailable.
    let mut gateways: Vec<std::net::Ipv4Addr> = Vec::new();
    if let Some(gw) = lt_net::discover_default_gateway() {
        gateways.push(gw);
    }
    for gw in [
        std::net::Ipv4Addr::new(192, 168, 1, 1),
        std::net::Ipv4Addr::new(192, 168, 0, 1),
        std::net::Ipv4Addr::new(10, 0, 0, 1),
    ] {
        if !gateways.contains(&gw) {
            gateways.push(gw);
        }
    }

    let mut active: Vec<ActiveMapping> = Vec::new();
    for &(port, protocol) in &desired {
        // Cancel discovery on shutdown so a slow/absent gateway can't stall drain.
        let mapping = tokio::select! {
            mapping = establish_mapping(port, protocol, &gateways, lease) => mapping,
            _ = shutdown_rx.changed() => break,
        };
        if let Some(mapping) = mapping {
            info!(
                internal = mapping.internal_port,
                external = mapping.external_port,
                protocol = mapping.protocol.as_str(),
                "NAT port mapped successfully"
            );
            active.push(mapping);
        }
    }
    if *shutdown_rx.borrow() {
        for mapping in &active {
            delete_mapping(mapping).await;
        }
        return;
    }
    if active.is_empty() {
        debug!("Automatic port mapping: neither UPnP nor NAT-PMP gateway available");
    }

    loop {
        tokio::select! {
            _ = shutdown_rx.changed() => {
                for mapping in &active {
                    delete_mapping(mapping).await;
                }
                return;
            }
            _ = tokio::time::sleep(renew_interval) => {
                if *shutdown_rx.borrow() {
                    for mapping in &active {
                        delete_mapping(mapping).await;
                    }
                    return;
                }
                // Renew existing leases, dropping any that no longer refresh.
                let mut renewed = Vec::new();
                for mapping in &active {
                    if renew_mapping(mapping, lease).await {
                        renewed.push(*mapping);
                    }
                }
                active = renewed;
                // Re-establish any desired mapping that is not currently active
                // (covers both initial failures and leases that dropped out).
                for &(port, protocol) in &desired {
                    if active
                        .iter()
                        .any(|m| m.internal_port == port && m.protocol == protocol)
                    {
                        continue;
                    }
                    if let Some(mapping) =
                        establish_mapping(port, protocol, &gateways, lease).await
                    {
                        active.push(mapping);
                    }
                }
            }
        }
    }
}

/// Handles the choke supervisor needs to enforce stop-on-ratio.
///
/// The authoritative per-group ratio config lives in the choke group manager
/// (the daemon's `ratio.*` commands are group-aware), and the command map runs
/// the group's `ratio.command` against the target torrent.
struct RatioContext {
    group_manager: Arc<RwLock<ChokeGroupManager>>,
    command_map: Arc<RwLock<CommandMap>>,
}

static RATIO_CONTEXT: OnceLock<RatioContext> = OnceLock::new();

/// Publish the ratio-enforcement handles. Called once by the daemon control
/// plane after the group manager and command map are created.
pub fn install_ratio_context(
    group_manager: Arc<RwLock<ChokeGroupManager>>,
    command_map: Arc<RwLock<CommandMap>>,
) {
    let _ = RATIO_CONTEXT.set(RatioContext {
        group_manager,
        command_map,
    });
}

/// Handle to the daemon's throttle command state, published once so the network
/// runtime can mirror the configured rates into the shared peer throttle
/// registry and refill its quotas.
static THROTTLE_CONTEXT: OnceLock<Arc<RwLock<ThrottleManager>>> = OnceLock::new();

/// Publish the throttle manager. Called once by the daemon control plane after
/// the command map is created, mirroring [`install_ratio_context`].
pub fn install_throttle_manager(manager: Arc<RwLock<ThrottleManager>>) {
    let _ = THROTTLE_CONTEXT.set(manager);
}

/// Periodically mirror the configured global and per-group rates into the shared
/// [`lt_net::ThrottleRegistry`] and refill its byte quotas. Peer streams wrapped
/// in `ThrottledReader`/`ThrottledWriter` consume that quota, so this loop is what
/// actually paces transfers to the configured limits.
///
/// Runs independently of the peer listeners so outbound transfers stay paced
/// even when incoming connections are blocked.
async fn throttle_driver_loop(mut shutdown_rx: watch::Receiver<bool>) {
    let registry = lt_net::throttle_registry();
    let interval = Duration::from_millis(100);
    let mut tick = tokio::time::interval(interval);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    tick.tick().await;

    let mut known_names: HashSet<String> = HashSet::new();

    loop {
        tokio::select! {
            _ = shutdown_rx.changed() => break,
            _ = tick.tick() => {
                if let Some(manager) = THROTTLE_CONTEXT.get() {
                    let mut current_names: HashSet<String> = HashSet::new();
                    {
                        let mgr = manager.read().unwrap();
                        registry.set_global_rates(mgr.global_up, mgr.global_down);
                        for (name, (up, down)) in &mgr.named_throttles {
                            registry.set_named_rate(name, *up, *down);
                            current_names.insert(name.clone());
                        }
                    }

                    // A group removed from the config must stop limiting any
                    // session still holding its throttle: reset it to unlimited
                    // before dropping it from the registry so orphaned nodes
                    // bypass gating instead of stalling without quota.
                    for name in known_names.difference(&current_names) {
                        registry.set_named_rate(name, 0, 0);
                        registry.remove_named(name);
                    }
                    known_names = current_names;
                }
                registry.refill(interval);
            }
        }
    }
}

/// Stop-on-ratio: for every active, seeding torrent whose group ratio limit is
/// reached, run the group's `ratio.command` against the torrent and stop it.
///
/// Reads the same authoritative config the user sets via `group.seeding.ratio.*`
/// so the supervisor and the RPC surface never disagree. Collection and action
/// are split into two phases: no download lock is held while the command runs,
/// because the command re-enters the download list (`d.try_close`).
fn enforce_stop_on_ratio(downloads: &Arc<RwLock<DownloadList>>) {
    let Some(ctx) = RATIO_CONTEXT.get() else {
        return;
    };
    enforce_stop_on_ratio_with(ctx, downloads);
}

fn enforce_stop_on_ratio_with(ctx: &RatioContext, downloads: &Arc<RwLock<DownloadList>>) {
    let mut reached: Vec<(HashString, String)> = Vec::new();
    {
        let list = downloads.read().unwrap();
        let groups = ctx.group_manager.read().unwrap();
        for item in list.all() {
            let dl = item.read().unwrap();
            if !dl.is_seeding() {
                continue;
            }
            let group = dl.group() as usize;
            let Some(config) = groups.resource_manager().ratio_config(group) else {
                continue;
            };
            if config.ratio_reached(dl.uploaded_bytes(), dl.downloaded_bytes()) {
                reached.push((*dl.info_hash(), config.command.clone()));
            }
        }
    }

    for (hash, command) in reached {
        let target = Target::Download(hash);
        if !command.trim().is_empty() {
            let map = ctx.command_map.read().unwrap();
            if let Err(e) = parse_command_multiple(&command, &target, &map, true) {
                warn!(target = %hash, error = %e, "stop-on-ratio: ratio.command failed");
            }
        }
        let list = downloads.read().unwrap();
        if let Some(item) = list.find(&hash) {
            list.stop(&item);
        }
    }
}

async fn choke_supervisor_loop(
    downloads: Arc<RwLock<DownloadList>>,
    mut shutdown_rx: watch::Receiver<bool>,
) {
    let mut round: u64 = 0;
    let mut interval = tokio::time::interval(Duration::from_secs(10));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    interval.tick().await;

    loop {
        tokio::select! {
            _ = shutdown_rx.changed() => break,
            _ = interval.tick() => {
                round = round.wrapping_add(1);
                let is_optimistic_round = round.is_multiple_of(3);

                enforce_stop_on_ratio(&downloads);

                let engines: Vec<Arc<Mutex<lt_engine::download::Download>>> = {
                    let dl = downloads.read().unwrap();
                    dl.all()
                        .iter()
                        .filter_map(|d| {
                            let item = d.read().unwrap();
                            if item.is_active() {
                                Some(item.engine_download())
                            } else {
                                None
                            }
                        })
                        .collect()
                };

                for engine_arc in engines {
                    if let Ok(mut dl) = engine_arc.try_lock() {
                        if dl.is_initial_seeding() {
                            continue;
                        }
                        let max_slots = dl.client_config().max_uploads().max(1);
                        let is_seeding = dl.is_done();

                        // (peer_id, rate_down, rate_up, downloaded, is_snubbed)
                        let mut interested_peers = Vec::new();
                        for peer in dl.connection_list_mut().iter_mut() {
                            if peer.peer_interested {
                                interested_peers.push((
                                    peer.peer_id,
                                    peer.rate_down.rate(),
                                    peer.rate_up.rate(),
                                    peer.downloaded,
                                    peer.is_snubbed,
                                ));
                            } else {
                                peer.is_choked = true;
                            }
                        }

                        if interested_peers.is_empty() {
                            continue;
                        }

                        // Tit-for-tat ranking: snubbed peers (those not sending
                        // us data) lose regular slots first; the rest are ranked
                        // by the rate that matters for our current role.
                        interested_peers.sort_by(|a, b| {
                            a.4.cmp(&b.4).then_with(|| {
                                if is_seeding {
                                    b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal)
                                } else {
                                    b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal)
                                }
                            })
                        });

                        let regular_slots = if is_optimistic_round && max_slots > 1 {
                            (max_slots - 1) as usize
                        } else {
                            max_slots as usize
                        };

                        let mut unchoked_ids: std::collections::HashSet<HashString> = interested_peers
                            .iter()
                            .take(regular_slots)
                            .map(|p| p.0)
                            .collect();

                        if is_optimistic_round && unchoked_ids.len() < interested_peers.len() {
                            let remaining: Vec<(HashString, bool)> = interested_peers
                                .iter()
                                .skip(regular_slots)
                                .map(|p| (p.0, p.4))
                                .collect();
                            // Prefer a non-snubbed peer for the optimistic slot
                            // so we probe peers that might actually reciprocate.
                            let candidates: Vec<HashString> =
                                if remaining.iter().any(|(_, snubbed)| !snubbed) {
                                    remaining
                                        .iter()
                                        .filter(|(_, s)| !s)
                                        .map(|(id, _)| *id)
                                        .collect()
                                } else {
                                    remaining.iter().map(|(id, _)| *id).collect()
                                };
                            if !candidates.is_empty() {
                                let opt_idx = random_index(candidates.len());
                                unchoked_ids.insert(candidates[opt_idx]);
                            }
                        }

                        for peer in dl.connection_list_mut().iter_mut() {
                            peer.is_choked = !unchoked_ids.contains(&peer.peer_id);
                        }
                    }
                }
            }
        }
    }
    debug!("Choke supervisor loop stopped");
}

/// Fallback HTTP seeding (BEP 19/17): fetches missing pieces from `url-list` /
/// `httpseeds` mirrors for active downloads that have no connected peers.
async fn web_seed_loop(
    downloads: Arc<RwLock<DownloadList>>,
    mut shutdown_rx: watch::Receiver<bool>,
) {
    let client = match lt_net::HttpClient::new() {
        Ok(client) => client,
        Err(err) => {
            warn!(error = %err, "WebSeed loop disabled: HTTP client init failed");
            return;
        }
    };
    let mut interval = tokio::time::interval(Duration::from_secs(5));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    interval.tick().await;

    loop {
        tokio::select! {
            _ = shutdown_rx.changed() => break,
            _ = interval.tick() => {
                let engines: Vec<Arc<Mutex<lt_engine::download::Download>>> = {
                    let dl = downloads.read().unwrap();
                    dl.all()
                        .iter()
                        .filter_map(|d| {
                            let item = d.read().unwrap();
                            if item.is_active() {
                                Some(item.engine_download())
                            } else {
                                None
                            }
                        })
                        .collect()
                };

                for engine_arc in engines {
                    // Reserve pieces and collect their v2 verification params
                    // under the same lock so hashing can run off the reactor.
                    let Some((plan, pieces, v2_checks)) = ({
                        let Ok(mut dl) = engine_arc.try_lock() else {
                            continue;
                        };
                        dl.reserve_web_seed_pieces(4).map(|(plan, pieces)| {
                            let v2_checks = pieces
                                .iter()
                                .map(|&piece| dl.v2_piece_check(piece).ok().flatten())
                                .collect::<Vec<_>>();
                            (plan, pieces, v2_checks)
                        })
                    }) else {
                        continue;
                    };

                    let mut fetches = JoinSet::new();
                    for (piece, v2_check) in pieces.iter().copied().zip(v2_checks) {
                        let plan = plan.clone();
                        let client = client.clone();
                        fetches.spawn(async move {
                            let result = plan.fetch_piece(&client, piece).await;
                            (piece, v2_check, result)
                        });
                    }

                    let mut results = Vec::new();
                    let mut fetch_pending = pieces.clone();
                    while let Some(joined) = fetches.join_next().await {
                        let Ok((piece, v2_check, result)) = joined else {
                            continue;
                        };
                        fetch_pending.retain(|&p| p != piece);
                        results.push((piece, v2_check, result));
                    }
                    // Fetch tasks that panicked never reported their piece.
                    let mut failed = fetch_pending;

                    // Hash every fetched piece off the reactor thread before
                    // taking the download lock again.
                    let mut hashes = JoinSet::new();
                    let mut hash_pending = Vec::new();
                    for (piece, v2_check, result) in results {
                        match result {
                            Ok(data) => {
                                hash_pending.push(piece);
                                hashes.spawn(async move {
                                    let hashed = tokio::task::spawn_blocking(move || {
                                        let hash = lt_crypto::sha1::sha1_hash(&data);
                                        let v2_root = v2_check.map(|check| {
                                            lt_crypto::merkle::piece_root(
                                                &data,
                                                check.piece_length,
                                                check.single_piece_file,
                                            )
                                        });
                                        (hash, v2_root, data)
                                    })
                                    .await;
                                    (piece, hashed)
                                });
                            }
                            Err(err) => {
                                debug!(piece, error = %err, "WebSeed fetch failed");
                                failed.push(piece);
                            }
                        }
                    }

                    let mut completed = Vec::new();
                    while let Some(joined) = hashes.join_next().await {
                        let Ok((piece, hashed)) = joined else {
                            continue;
                        };
                        hash_pending.retain(|&p| p != piece);
                        match hashed {
                            Ok((hash, v2_root, data)) => {
                                completed.push((piece, data, hash, v2_root));
                            }
                            Err(_) => failed.push(piece),
                        }
                    }
                    failed.extend(hash_pending);

                    // Lock (not try_lock) so reserved pieces are always completed
                    // or released; skipping completion would leak the reservation
                    // and stall the piece.
                    let mut dl = engine_arc.lock().unwrap_or_else(|e| e.into_inner());
                    for piece in failed {
                        dl.release_web_seed_piece(piece);
                    }
                    for (piece, data, hash, v2_root) in completed {
                        if dl
                            .receive_web_seed_piece_prehashed(piece, data, hash, v2_root)
                            .is_err()
                        {
                            dl.release_web_seed_piece(piece);
                        }
                    }
                }
            }
        }
    }
    debug!("WebSeed loop stopped");
}

/// Configuration for [`NetworkRuntime::spawn_with_listener_config`].
pub struct NetworkRuntimeConfig {
    pub download_list: Arc<RwLock<DownloadList>>,
    pub our_peer_id: HashString,
    pub listen_port: u16,
    pub shutdown_rx: watch::Receiver<bool>,
    pub dht_cache_path: Option<std::path::PathBuf>,
    pub network_config: NetworkConfig,
    pub dht_config: DhtLifecycleConfig,
    pub listener_config: PeerListenerConfig,
}

struct SpawnParams {
    download_list: Arc<RwLock<DownloadList>>,
    our_peer_id: HashString,
    listen_port: u16,
    shutdown_rx: watch::Receiver<bool>,
    dht_cache_path: Option<std::path::PathBuf>,
    initial_policy: EncryptionPolicy,
    live_config: Option<NetworkConfig>,
    dht_config: DhtLifecycleConfig,
    listener_config: PeerListenerConfig,
}

impl NetworkRuntime {
    /// Spawns the listener, DHT and supervisor tasks that monitor active
    /// torrents in `download_list` and start/stop their
    /// `DownloadNetworkCoordinator` network tasks.
    ///
    /// The returned handle owns every task spawned here. Signal shutdown via
    /// the supplied `watch` channel, then await
    /// [`NetworkRuntimeHandle::drain`] to stop them deterministically.
    ///
    /// This entry point uses `EncryptionPolicy::allow()`. Callers that own a
    /// [`NetworkConfig`] should use [`NetworkRuntime::spawn_with_network_config`]
    /// so `protocol.encryption.*` settings are enforced for inbound and
    /// outbound peers.
    pub fn spawn(
        download_list: Arc<RwLock<DownloadList>>,
        our_peer_id: HashString,
        listen_port: u16,
        shutdown_rx: watch::Receiver<bool>,
        dht_cache_path: Option<std::path::PathBuf>,
    ) -> NetworkRuntimeHandle {
        Self::spawn_internal(SpawnParams {
            download_list,
            our_peer_id,
            listen_port,
            shutdown_rx,
            dht_cache_path,
            initial_policy: EncryptionPolicy::allow(),
            live_config: None,
            dht_config: DhtLifecycleConfig::default(),
            listener_config: PeerListenerConfig::new(listen_port),
        })
    }

    /// Like [`NetworkRuntime::spawn`], but enforces an explicit encryption
    /// policy for both inbound and outbound connections.
    pub fn spawn_with_policy(
        download_list: Arc<RwLock<DownloadList>>,
        our_peer_id: HashString,
        listen_port: u16,
        shutdown_rx: watch::Receiver<bool>,
        dht_cache_path: Option<std::path::PathBuf>,
        policy: EncryptionPolicy,
    ) -> NetworkRuntimeHandle {
        Self::spawn_internal(SpawnParams {
            download_list,
            our_peer_id,
            listen_port,
            shutdown_rx,
            dht_cache_path,
            initial_policy: policy,
            live_config: None,
            dht_config: DhtLifecycleConfig::default(),
            listener_config: PeerListenerConfig::new(listen_port),
        })
    }

    /// Like [`NetworkRuntime::spawn`], but reads the handshake/stream modes
    /// from `network_config` and keeps them up to date while running.
    pub fn spawn_with_network_config(
        download_list: Arc<RwLock<DownloadList>>,
        our_peer_id: HashString,
        listen_port: u16,
        shutdown_rx: watch::Receiver<bool>,
        dht_cache_path: Option<std::path::PathBuf>,
        network_config: NetworkConfig,
    ) -> NetworkRuntimeHandle {
        Self::spawn_with_network_config_and_dht(
            download_list,
            our_peer_id,
            listen_port,
            shutdown_rx,
            dht_cache_path,
            network_config,
            DhtLifecycleConfig::default(),
        )
    }

    /// Full-control variant: additionally selects `dht.mode`, the DHT port
    /// override and refresh/bootstrap intervals.
    pub fn spawn_with_network_config_and_dht(
        download_list: Arc<RwLock<DownloadList>>,
        our_peer_id: HashString,
        listen_port: u16,
        shutdown_rx: watch::Receiver<bool>,
        dht_cache_path: Option<std::path::PathBuf>,
        network_config: NetworkConfig,
        dht_config: DhtLifecycleConfig,
    ) -> NetworkRuntimeHandle {
        let policy = EncryptionPolicy::from(network_config.encryption_modes());
        let listener_config = PeerListenerConfig::from_network_config(listen_port, &network_config);
        Self::spawn_internal(SpawnParams {
            download_list,
            our_peer_id,
            listen_port,
            shutdown_rx,
            dht_cache_path,
            initial_policy: policy,
            live_config: Some(network_config),
            dht_config,
            listener_config,
        })
    }

    /// Full-control variant with an explicit listener configuration
    /// (port range, bind addresses/devices, backlog and connection limits).
    pub fn spawn_with_listener_config(config: NetworkRuntimeConfig) -> NetworkRuntimeHandle {
        let NetworkRuntimeConfig {
            download_list,
            our_peer_id,
            listen_port,
            shutdown_rx,
            dht_cache_path,
            network_config,
            dht_config,
            listener_config,
        } = config;
        let policy = EncryptionPolicy::from(network_config.encryption_modes());
        Self::spawn_internal(SpawnParams {
            download_list,
            our_peer_id,
            listen_port,
            shutdown_rx,
            dht_cache_path,
            initial_policy: policy,
            live_config: Some(network_config),
            dht_config,
            listener_config,
        })
    }

    /// Like [`NetworkRuntime::spawn`], but with explicit DHT lifecycle
    /// options and the default (permissive) encryption policy.
    pub fn spawn_with_dht_config(
        download_list: Arc<RwLock<DownloadList>>,
        our_peer_id: HashString,
        listen_port: u16,
        shutdown_rx: watch::Receiver<bool>,
        dht_cache_path: Option<std::path::PathBuf>,
        dht_config: DhtLifecycleConfig,
    ) -> NetworkRuntimeHandle {
        Self::spawn_internal(SpawnParams {
            download_list,
            our_peer_id,
            listen_port,
            shutdown_rx,
            dht_cache_path,
            initial_policy: EncryptionPolicy::allow(),
            live_config: None,
            dht_config,
            listener_config: PeerListenerConfig::new(listen_port),
        })
    }

    fn spawn_internal(params: SpawnParams) -> NetworkRuntimeHandle {
        let SpawnParams {
            download_list,
            our_peer_id,
            listen_port,
            mut shutdown_rx,
            dht_cache_path,
            initial_policy,
            live_config,
            mut dht_config,
            listener_config,
        } = params;
        let encryption_policy = Arc::new(Mutex::new(initial_policy));
        reset_listener_statistics();

        // Pools are registered while a coordinator is alive so inbound
        // connections can join the same deduplicated peer set.
        let peer_pools: Arc<Mutex<HashMap<HashString, PeerPoolHandle>>> =
            Arc::new(Mutex::new(HashMap::new()));

        let mut listener_tasks = Vec::new();
        if let Some(task) = spawn_peer_listeners(
            Arc::clone(&download_list),
            our_peer_id,
            listener_config,
            shutdown_rx.clone(),
            Arc::clone(&peer_pools),
            Arc::clone(&encryption_policy),
        ) {
            listener_tasks.push(task);
        }

        let mut dht_tasks = Vec::new();
        // `dht.port`/`dht.mode` are command-layer preferences. The daemon
        // installs the same `DhtConfig` before the network runtime starts, so
        // honour it here without requiring the caller to copy every value into
        // `DhtLifecycleConfig`.
        if let Some((port, mode, override_port)) = dht_config_snapshot() {
            if dht_config.port_override == 0 {
                dht_config.port_override = if override_port != 0 {
                    override_port
                } else {
                    port
                };
            }
            if let Some(mode) = DhtMode::parse(&mode) {
                dht_config.mode = mode;
            }
        }
        let dht_port = if dht_config.port_override != 0 {
            dht_config.port_override
        } else {
            listen_port
        };
        let dht_handle = if dht_port != 0 || dht_cache_path.is_some() {
            let mut id_bytes = [0u8; 20];
            let own_id = match getrandom::fill(&mut id_bytes) {
                Ok(()) => HashString::new(id_bytes),
                Err(err) => {
                    warn!(error = %err, "Could not generate DHT node id");
                    HashString::default()
                }
            };

            let mut dht_config = dht_config.clone();
            dht_config.port_override = dht_port;
            let dht = Arc::new(DhtController::with_config(
                own_id,
                dht_port,
                dht_config.clone(),
            ));

            if dht_config.mode != DhtMode::Disable {
                if let Some(path) = &dht_cache_path {
                    if let Err(err) = dht.load_cache_from(path) {
                        debug!(path = %path.display(), error = %err, "Could not load DHT cache");
                    } else {
                        info!(path = %path.display(), "Loaded DHT cache");
                    }
                }
            }

            if dht_config.mode.is_enabled() {
                dht.start();
            } else {
                info!(
                    mode = dht_config.mode.as_str(),
                    "DHT initialized but not started"
                );
            }

            // Publish the live controller for the `dht.*` command surface.
            set_dht_controller(Some(Arc::clone(&dht)));

            // A supervisor keeps the receive loop running across stop/start
            // transitions: an `off -> on` mode change has no task to wake in
            // the controller itself, so this loop starts `run` as soon as the
            // controller becomes active.
            let run_dht = Arc::clone(&dht);
            let mut run_shutdown = shutdown_rx.clone();
            dht_tasks.push(tokio::spawn(async move {
                loop {
                    if *run_shutdown.borrow() {
                        break;
                    }

                    if run_dht.is_active() {
                        if let Err(err) = run_dht.run(run_shutdown.clone()).await {
                            warn!(error = %err, "DHT server stopped");
                        }
                    }

                    tokio::select! {
                        _ = run_shutdown.changed() => break,
                        _ = sleep(Duration::from_millis(500)) => {}
                    }
                }

                if let Some(current) = dht_controller() {
                    if Arc::ptr_eq(&current, &run_dht) {
                        set_dht_controller(None);
                    }
                }
            }));

            if dht_config.mode != DhtMode::Disable {
                if let Some(path) = dht_cache_path.clone() {
                    let save_dht = Arc::clone(&dht);
                    let mut save_shutdown = shutdown_rx.clone();

                    dht_tasks.push(tokio::spawn(async move {
                    let mut tick = tokio::time::interval(Duration::from_secs(300));
                    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

                    loop {
                        tokio::select! {
                            _ = save_shutdown.changed() => break,
                            _ = tick.tick() => {
                                log_dht_statistics(&save_dht);
                                if let Err(err) = save_dht.save_cache_to_async(&path).await {
                                    debug!(path = %path.display(), error = %err, "Could not save DHT cache");
                                }
                            }
                        }
                    }

                    if let Err(err) = save_dht.save_cache_to_async(&path).await {
                        debug!(path = %path.display(), error = %err, "Could not save DHT cache on shutdown");
                    }
                }));
                }

                // Resolve bootstrap routers asynchronously so DNS or unreachable
                // routers never delay session startup. The controller wakes its
                // maintenance task as soon as the list is installed and retries
                // with a bounded backoff afterwards.
                if dht_config.mode != DhtMode::Disable {
                    let bootstrap_dht = Arc::clone(&dht);
                    dht_tasks.push(tokio::spawn(async move {
                        let hosts = [
                            "router.bittorrent.com:6881",
                            "dht.transmissionbt.com:6881",
                            "router.utorrent.com:6881",
                        ];

                        let resolve = async {
                            let mut nodes = Vec::new();
                            for host in hosts {
                                match tokio::net::lookup_host(host).await {
                                    Ok(addresses) => {
                                        nodes.extend(addresses.map(SocketAddress::from));
                                    }
                                    Err(err) => {
                                        debug!(host, error = %err, "DHT bootstrap DNS failed")
                                    }
                                }
                            }
                            nodes
                        };

                        // Keep the resolver bounded so shutdown never waits on DNS.
                        let nodes =
                            match tokio::time::timeout(Duration::from_secs(2), resolve).await {
                                Ok(nodes) => nodes,
                                Err(_) => {
                                    debug!("DHT bootstrap DNS resolution timed out");
                                    return;
                                }
                            };

                        if nodes.is_empty() {
                            debug!("No DHT bootstrap addresses resolved");
                            return;
                        }

                        info!(count = nodes.len(), "DHT bootstrap routers resolved");
                        bootstrap_dht.set_bootstrap_nodes(nodes);
                    }));
                }
            }

            Some(dht.handle())
        } else {
            if !dht_config.mode.is_enabled() {
                info!(mode = dht_config.mode.as_str(), "DHT disabled by mode");
            }
            set_dht_controller(None);
            None
        };

        let throttle_task = tokio::spawn(throttle_driver_loop(shutdown_rx.clone()));
        listener_tasks.push(throttle_task);

        let coordinator_task = tokio::spawn(async move {
            info!(
                "NetworkRuntime supervisor started (listen_port: {})",
                listen_port
            );

            let mut active_coordinators: HashMap<
                HashString,
                (watch::Sender<bool>, PeerPoolHandle),
            > = HashMap::new();
            // Torrents loaded from a `.torrent` already know their raw info
            // dictionary; publish it once so `ut_metadata` can serve it.
            let mut registered_metadata: HashSet<HashString> = HashSet::new();

            while !*shutdown_rx.borrow() {
                // Apply live `protocol.encryption.*` changes to new dials and
                // inbound classifications. Existing sessions keep the policy
                // they negotiated with.
                if let Some(config) = &live_config {
                    *encryption_policy.lock().unwrap() =
                        EncryptionPolicy::from(config.encryption_modes());
                }

                // Collect current active and inactive torrents
                let downloads = {
                    let dl_list = download_list.read().unwrap();
                    dl_list.all()
                };

                let mut seen = HashSet::new();

                let now_secs = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|duration| duration.as_secs())
                    .unwrap_or_default();

                for dl_arc in &downloads {
                    // Magnet downloads start without an info dictionary; the
                    // coordinator applies the fetched metadata to the engine
                    // download, so the command-layer snapshot must follow.
                    // Persist immediately once the static torrent can be built.
                    let (metadata_updated, metadata_recheck_pending) = {
                        let needs_sync = {
                            let dl = dl_arc.read().unwrap();
                            dl.is_metadata_download() || dl.metadata_recheck_pending()
                        };
                        if !needs_sync {
                            (false, false)
                        } else {
                            let mut dl = dl_arc.write().unwrap();
                            let metadata_updated = if dl.is_metadata_download() {
                                dl.sync_metadata_from_engine()
                            } else {
                                false
                            };
                            let recheck_pending = dl.take_metadata_recheck_pending();
                            (metadata_updated, recheck_pending)
                        }
                    };
                    if metadata_updated {
                        download_list
                            .read()
                            .unwrap()
                            .notify_metadata_updated(dl_arc);
                    }
                    if metadata_recheck_pending {
                        // Metadata is only known now. Verify an existing
                        // payload asynchronously instead of overwriting it.
                        download_list.read().unwrap().begin_metadata_recheck(dl_arc);
                    }

                    let (hash, is_active, _is_done, engine_dl, raw_info) = {
                        let dl = dl_arc.read().unwrap();
                        let raw_info = dl.bencode().and_then(|bencode| {
                            bencode
                                .get_key("info")
                                .ok()
                                .and_then(|info| lt_common::object::bencode::encode(info).ok())
                        });
                        (
                            *dl.info_hash(),
                            dl.is_active(),
                            dl.is_done(),
                            dl.engine_download(),
                            raw_info,
                        )
                    };
                    seen.insert(hash);

                    if is_active {
                        dl_arc.write().unwrap().set_last_active(now_secs);
                    }

                    if !registered_metadata.contains(&hash) {
                        if let Some(bytes) = raw_info {
                            if lt_engine::network::register_local_metadata(hash, bytes) {
                                registered_metadata.insert(hash);
                            }
                        }
                    }

                    if is_active {
                        let created = !active_coordinators.contains_key(&hash);
                        active_coordinators.entry(hash).or_insert_with(|| {
                            debug!(hash = %hash.to_hex(), "Spawning network coordinator for torrent");
                            let coordinator = DownloadNetworkCoordinator::new_with_shared_policy(
                                engine_dl,
                                our_peer_id,
                                listen_port,
                                dht_handle.clone(),
                                Arc::clone(&encryption_policy),
                            );
                            let pool = coordinator.peer_pool();
                            peer_pools.lock().unwrap().insert(hash, Arc::clone(&pool));
                            let stop_tx = coordinator.spawn();
                            (stop_tx, pool)
                        });
                        if created {
                            record_coordinator_spawn(active_coordinators.len());
                        }
                    } else if let Some((stop_tx, _)) = active_coordinators.remove(&hash) {
                        debug!(hash = %hash.to_hex(), "Stopping network coordinator for torrent");
                        let _ = stop_tx.send(true);
                        peer_pools.lock().unwrap().remove(&hash);
                    }
                }

                // An erased download disappears from the list entirely; stop
                // its coordinator and all of its connect tasks as well.
                let stale: Vec<HashString> = active_coordinators
                    .keys()
                    .filter(|hash| !seen.contains(*hash))
                    .copied()
                    .collect();
                for hash in stale {
                    if let Some((stop_tx, _)) = active_coordinators.remove(&hash) {
                        debug!(hash = %hash.to_hex(), "Stopping coordinator for removed torrent");
                        let _ = stop_tx.send(true);
                        peer_pools.lock().unwrap().remove(&hash);
                    }
                }

                tokio::select! {
                    _ = shutdown_rx.changed() => {
                        break;
                    }
                    _ = sleep(Duration::from_millis(500)) => {}
                }
            }

            // Cleanup all coordinators on shutdown
            for (hash, (stop_tx, _)) in active_coordinators.drain() {
                debug!(hash = %hash.to_hex(), "Shutting down network coordinator");
                let _ = stop_tx.send(true);
                peer_pools.lock().unwrap().remove(&hash);
            }

            info!("NetworkRuntime supervisor stopped cleanly.");
        });

        NetworkRuntimeHandle {
            listeners: Mutex::new(listener_tasks),
            dht: Mutex::new(dht_tasks),
            coordinators: Mutex::new(Some(coordinator_task)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Builds a real single-piece torrent that is seeding (its only piece has
    /// arrived) with the given transfer counters, inserted into a fresh list.
    fn seeding_fixture(
        uploaded: u64,
        downloaded: u64,
    ) -> (
        Arc<RwLock<DownloadList>>,
        Arc<RwLock<crate::download::Download>>,
    ) {
        use crate::download::Download;
        use lt_crypto::sha1::sha1_hash;
        use lt_engine::constructor::TorrentMetadata;

        let payload = b"stop-on-ratio";
        let hash = sha1_hash(payload);
        let meta = TorrentMetadata {
            info_hash: HashString::new(hash),
            name: "stop_on_ratio".into(),
            piece_length: payload.len() as u32,
            pieces: hash.to_vec(),
            num_pieces: 1,
            total_size: payload.len() as u64,
            is_multi_file: false,
            files: Vec::new(),
            announce: None,
            announce_list: vec![],
            is_private: false,
            is_meta_download: false,
            v2: None,
        };

        let mut dl = Download::from_metadata(meta);
        {
            let mut engine = dl.download_mut();
            engine.start();
            assert!(engine.receive_block(0, 0, payload).unwrap());
        }
        dl.set_downloaded_bytes(downloaded);
        dl.set_uploaded_bytes(uploaded);
        assert!(dl.is_seeding());
        let item = Arc::new(RwLock::new(dl));

        let list = Arc::new(RwLock::new(DownloadList::new()));
        list.write().unwrap().insert(Arc::clone(&item));
        (list, item)
    }

    /// Group 0 with ratio enabled at `min` percent and no upload threshold, so
    /// the ratio alone decides. Empty command isolates the stop action.
    fn ratio_context(min: f64) -> RatioContext {
        let groups = Arc::new(RwLock::new(ChokeGroupManager::new()));
        {
            let mut g = groups.write().unwrap();
            let rm = g.resource_manager_mut();
            rm.set_ratio_enabled(0, true);
            let cfg = rm.ratio_mut(0).unwrap();
            cfg.min = min;
            cfg.max = 0.0;
            cfg.upload = 0;
            cfg.command = String::new();
        }
        RatioContext {
            group_manager: groups,
            command_map: Arc::new(RwLock::new(CommandMap::new())),
        }
    }

    #[test]
    fn stop_on_ratio_stops_seeding_torrent_when_limit_reached() {
        // uploaded/downloaded = 1000/100 -> 1000% >= min 100%.
        let (list, item) = seeding_fixture(1000, 100);
        let ctx = ratio_context(100.0);

        enforce_stop_on_ratio_with(&ctx, &list);

        assert!(item.read().unwrap().is_stopped());
    }

    #[test]
    fn stop_on_ratio_leaves_torrent_below_limit_seeding() {
        // 10/100 -> 10% < min 100%: the limit is not reached, keep seeding.
        let (list, item) = seeding_fixture(10, 100);
        let ctx = ratio_context(100.0);

        enforce_stop_on_ratio_with(&ctx, &list);

        assert!(item.read().unwrap().is_seeding());
        assert!(!item.read().unwrap().is_stopped());
    }

    #[tokio::test]
    async fn drain_waits_for_stages_in_dependency_order() {
        let stages: Arc<Mutex<Vec<&'static str>>> = Arc::new(Mutex::new(Vec::new()));

        let record = |name: &'static str| {
            let stages = Arc::clone(&stages);
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(10)).await;
                stages.lock().unwrap().push(name);
            })
        };

        let handle = NetworkRuntimeHandle {
            listeners: Mutex::new(vec![record("listeners")]),
            dht: Mutex::new(vec![record("dht")]),
            coordinators: Mutex::new(Some(record("coordinators"))),
        };

        let report = handle
            .drain(ShutdownTimeouts {
                listeners: Duration::from_secs(1),
                dht: Duration::from_secs(1),
                coordinators: Duration::from_secs(1),
            })
            .await;

        assert!(report.all_completed());
        assert_eq!(
            *stages.lock().unwrap(),
            vec!["listeners", "dht", "coordinators"]
        );
        assert!(handle.is_drained());
    }

    #[tokio::test]
    async fn drain_aborts_tasks_that_exceed_their_deadline() {
        let task = tokio::spawn(async {
            std::future::pending::<()>().await;
        });

        let handle = NetworkRuntimeHandle {
            listeners: Mutex::new(vec![task]),
            dht: Mutex::new(Vec::new()),
            coordinators: Mutex::new(None),
        };

        let report = handle
            .drain(ShutdownTimeouts {
                listeners: Duration::from_millis(20),
                dht: Duration::from_millis(20),
                coordinators: Duration::from_millis(20),
            })
            .await;

        assert!(!report.listeners_completed);
        assert!(report.dht_completed);
        assert!(report.coordinators_completed);
        assert!(handle.is_drained());
    }

    #[test]
    fn listener_port_range_candidates_cover_the_whole_range() {
        let ports = candidate_ports(7000, 7003, false);
        assert_eq!(ports, vec![7000, 7001, 7002, 7003]);

        let random = candidate_ports(7000, 7003, true);
        assert_eq!(random.len(), 4);
        let mut sorted = random.clone();
        sorted.sort_unstable();
        assert_eq!(sorted, vec![7000, 7001, 7002, 7003]);

        assert!(candidate_ports(0, 0, false).is_empty());
        assert!(candidate_ports(10, 5, false).is_empty());
    }

    #[test]
    fn listener_config_honours_explicit_port_and_range() {
        let config = PeerListenerConfig::new(7000);
        assert_eq!(config.effective_range(), Some((7000, 7000)));

        let ranged = PeerListenerConfig::new(0).with_port_range(7001, 7010, true);
        assert_eq!(ranged.effective_range(), Some((7001, 7010)));

        let disabled = PeerListenerConfig::new(0);
        assert_eq!(disabled.effective_range(), None);
    }

    #[test]
    fn listener_config_reads_utp_and_upnp_flags() {
        let network_config = NetworkConfig::new();
        let default = PeerListenerConfig::from_network_config(7000, &network_config);
        assert!(default.enable_utp);
        assert!(default.enable_upnp);

        network_config.set_enable_utp(false);
        network_config.set_enable_upnp(false);
        let disabled = PeerListenerConfig::from_network_config(7000, &network_config);
        assert!(!disabled.enable_utp);
        assert!(!disabled.enable_upnp);
    }

    #[test]
    fn inbound_cap_falls_back_to_default_when_unlimited() {
        // A cap of 0 (unlimited/unset) keeps the bounded default; a set cap is
        // used verbatim. Pure decision, independent of the global budget.
        assert_eq!(inbound_connection_cap(0), MAX_INBOUND_CONNECTIONS);
        assert_eq!(inbound_connection_cap(64), 64);
        assert_eq!(inbound_connection_cap(1), 1);
    }

    #[test]
    fn listener_config_sources_inbound_cap_from_global_limit() {
        // Sole installer in this binary: `.get()` reads never pre-initialize
        // the budget, so this install lands regardless of test ordering.
        lt_engine::network::install_connection_limits(64, 0);
        let config = PeerListenerConfig::from_network_config(7000, &NetworkConfig::new());
        assert_eq!(config.max_connections, 64);
    }

    #[tokio::test]
    async fn bounded_join_reports_completed_tasks() {
        let started = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&started);
        let task = tokio::spawn(async move {
            counter.fetch_add(1, Ordering::SeqCst);
        });

        assert!(join_all_bounded(vec![task], Duration::from_secs(1)).await);
        assert_eq!(started.load(Ordering::SeqCst), 1);
    }
}
