use lt_common::hash_string::HashString;
use lt_dht::controller::DhtController;
pub use lt_dht::controller::{DhtLifecycleConfig, DhtMode};
use lt_engine::{DownloadNetworkCoordinator, PeerPoolHandle};
use lt_net::addr::SocketAddress;
use lt_net::config::{ListenAddresses, NetworkConfig};
use lt_protocol::encryption::EncryptionPolicy;
use lt_protocol::handshake::{is_plaintext_handshake_prefix, HandshakeMessage};
use lt_protocol::mse::{accept as mse_accept, matches_obfuscated_info_hash};
use std::collections::{HashMap, HashSet};
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

use crate::download_list::DownloadList;

/// Upper bound on concurrent inbound handshakes/connections.
const MAX_INBOUND_CONNECTIONS: usize = 256;
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

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
            max_connections: MAX_INBOUND_CONNECTIONS,
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
    let count = (last as u32 - first as u32 + 1) as u32;
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
        Self::spawn_internal(
            download_list,
            our_peer_id,
            listen_port,
            shutdown_rx,
            dht_cache_path,
            EncryptionPolicy::allow(),
            None,
            DhtLifecycleConfig::default(),
            PeerListenerConfig::new(listen_port),
        )
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
        Self::spawn_internal(
            download_list,
            our_peer_id,
            listen_port,
            shutdown_rx,
            dht_cache_path,
            policy,
            None,
            DhtLifecycleConfig::default(),
            PeerListenerConfig::new(listen_port),
        )
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
        Self::spawn_internal(
            download_list,
            our_peer_id,
            listen_port,
            shutdown_rx,
            dht_cache_path,
            policy,
            Some(network_config),
            dht_config,
            listener_config,
        )
    }

    /// Full-control variant with an explicit listener configuration
    /// (port range, bind addresses/devices, backlog and connection limits).
    pub fn spawn_with_listener_config(
        download_list: Arc<RwLock<DownloadList>>,
        our_peer_id: HashString,
        listen_port: u16,
        shutdown_rx: watch::Receiver<bool>,
        dht_cache_path: Option<std::path::PathBuf>,
        network_config: NetworkConfig,
        dht_config: DhtLifecycleConfig,
        listener_config: PeerListenerConfig,
    ) -> NetworkRuntimeHandle {
        let policy = EncryptionPolicy::from(network_config.encryption_modes());
        Self::spawn_internal(
            download_list,
            our_peer_id,
            listen_port,
            shutdown_rx,
            dht_cache_path,
            policy,
            Some(network_config),
            dht_config,
            listener_config,
        )
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
        Self::spawn_internal(
            download_list,
            our_peer_id,
            listen_port,
            shutdown_rx,
            dht_cache_path,
            EncryptionPolicy::allow(),
            None,
            dht_config,
            PeerListenerConfig::new(listen_port),
        )
    }

    fn spawn_internal(
        download_list: Arc<RwLock<DownloadList>>,
        our_peer_id: HashString,
        listen_port: u16,
        mut shutdown_rx: watch::Receiver<bool>,
        dht_cache_path: Option<std::path::PathBuf>,
        initial_policy: EncryptionPolicy,
        live_config: Option<NetworkConfig>,
        mut dht_config: DhtLifecycleConfig,
        listener_config: PeerListenerConfig,
    ) -> NetworkRuntimeHandle {
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
                    let metadata_updated = if dl_arc.read().unwrap().is_metadata_download() {
                        dl_arc.write().unwrap().sync_metadata_from_engine()
                    } else {
                        false
                    };
                    if metadata_updated {
                        download_list
                            .read()
                            .unwrap()
                            .notify_metadata_updated(dl_arc);
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
                        if !active_coordinators.contains_key(&hash) {
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
                            active_coordinators.insert(hash, (stop_tx, pool));
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
