use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU16, AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration;
use tokio::net::UdpSocket;
use tokio::sync::{oneshot, watch, Mutex, Notify};
use tokio::time::timeout;
use tracing::debug;

use lt_net::addr::SocketAddress;

use crate::error::DhtError;
use crate::router::DhtRouter;
use crate::transaction::{KrpcMessage, KrpcQuery, KrpcResponse};

/// Timeout for a single KRPC query.
pub const QUERY_TIMEOUT: Duration = Duration::from_secs(2);

/// Counters maintained by the server. The controller reports these through
/// `dht.statistics`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ServerStatistics {
    pub queries_sent: usize,
    pub queries_received: usize,
    pub replies_sent: usize,
    pub replies_received: usize,
    pub errors_sent: usize,
    pub errors_received: usize,
}

pub struct DhtServer {
    socket: RwLock<Option<Arc<UdpSocket>>>,
    bind_lock: Mutex<()>,
    socket_changed: Notify,
    router: Arc<Mutex<DhtRouter>>,
    pending_transactions: Arc<Mutex<HashMap<Vec<u8>, oneshot::Sender<KrpcResponse>>>>,
    next_tid: AtomicU32,
    port: AtomicU16,
    queries_sent: AtomicUsize,
    queries_received: AtomicUsize,
    replies_sent: AtomicUsize,
    replies_received: AtomicUsize,
    errors_sent: AtomicUsize,
    errors_received: AtomicUsize,
}

impl DhtServer {
    pub fn new(router: Arc<Mutex<DhtRouter>>, port: u16) -> Self {
        Self {
            socket: RwLock::new(None),
            bind_lock: Mutex::new(()),
            socket_changed: Notify::new(),
            router,
            pending_transactions: Arc::new(Mutex::new(HashMap::new())),
            next_tid: AtomicU32::new(1),
            port: AtomicU16::new(port),
            queries_sent: AtomicUsize::new(0),
            queries_received: AtomicUsize::new(0),
            replies_sent: AtomicUsize::new(0),
            replies_received: AtomicUsize::new(0),
            errors_sent: AtomicUsize::new(0),
            errors_received: AtomicUsize::new(0),
        }
    }

    /// Currently requested/bound UDP port. Returns 0 while stopped.
    pub fn port(&self) -> u16 {
        self.port.load(Ordering::Relaxed)
    }

    pub fn socket(&self) -> Option<Arc<UdpSocket>> {
        self.socket
            .read()
            .ok()
            .and_then(|guard| guard.as_ref().cloned())
    }

    /// Binds the currently configured port (no-op when already bound).
    pub async fn bind(&self) -> Result<(), DhtError> {
        self.bind_port(self.port()).await
    }

    /// Binds `port`, closing any previous socket first.
    ///
    /// Rebinding wakes [`DhtServer::run`] so it picks up the new socket. When
    /// the requested port is unchanged the old loop may still hold a clone of
    /// the socket for a moment, so binding is retried briefly instead of
    /// failing the whole restart.
    pub async fn bind_port(&self, port: u16) -> Result<(), DhtError> {
        let _bind_guard = self.bind_lock.lock().await;

        {
            let guard = self.socket.read().unwrap();
            if let Some(socket) = guard.as_ref() {
                let current = socket.local_addr().ok().map(|addr| addr.port());
                if port == 0 || current == Some(port) {
                    if port != 0 {
                        self.port.store(port, Ordering::Relaxed);
                    }
                    return Ok(());
                }
            }
        }

        // Close the previous socket before trying to bind the new one. Give
        // the receive loop a moment to drop its clone so a same-port restart
        // can bind immediately.
        let stale = self.socket.write().unwrap().take();
        self.port.store(0, Ordering::Relaxed);
        self.socket_changed.notify_one();
        if let Some(stale) = stale {
            for _ in 0..10 {
                if Arc::strong_count(&stale) <= 1 {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            drop(stale);
        }

        let addr = format!("0.0.0.0:{port}");
        let mut last_error = None;

        for attempt in 0..10 {
            match UdpSocket::bind(&addr).await {
                Ok(socket) => {
                    let actual_port = socket.local_addr()?.port();
                    self.port.store(actual_port, Ordering::Relaxed);
                    *self.socket.write().unwrap() = Some(Arc::new(socket));
                    self.socket_changed.notify_one();
                    return Ok(());
                }
                Err(err) => {
                    last_error = Some(err);
                    if attempt < 9 {
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                }
            }
        }

        Err(DhtError::Io(last_error.unwrap_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::AddrInUse, "could not bind DHT socket")
        })))
    }

    /// Closes the UDP socket and marks the server inactive.
    pub fn unbind(&self) {
        *self.socket.write().unwrap() = None;
        self.port.store(0, Ordering::Relaxed);
        self.socket_changed.notify_one();
    }

    pub fn is_active(&self) -> bool {
        self.socket().is_some()
    }

    pub fn statistics(&self) -> ServerStatistics {
        ServerStatistics {
            queries_sent: self.queries_sent.load(Ordering::Relaxed),
            queries_received: self.queries_received.load(Ordering::Relaxed),
            replies_sent: self.replies_sent.load(Ordering::Relaxed),
            replies_received: self.replies_received.load(Ordering::Relaxed),
            errors_sent: self.errors_sent.load(Ordering::Relaxed),
            errors_received: self.errors_received.load(Ordering::Relaxed),
        }
    }

    pub fn get_statistics(&self) -> ServerStatistics {
        self.statistics()
    }

    pub fn reset_statistics(&self) {
        self.queries_sent.store(0, Ordering::Relaxed);
        self.queries_received.store(0, Ordering::Relaxed);
        self.replies_sent.store(0, Ordering::Relaxed);
        self.replies_received.store(0, Ordering::Relaxed);
        self.errors_sent.store(0, Ordering::Relaxed);
        self.errors_received.store(0, Ordering::Relaxed);
    }

    /// Records a query that will be sent outside [`DhtServer::send_query`]
    /// (used by `DhtController::add_peer_node` so the counter is visible
    /// synchronously).
    pub(crate) fn note_query_sent(&self) {
        self.queries_sent.fetch_add(1, Ordering::Relaxed);
    }

    /// Sends a KRPC query and waits for the matching response.
    pub async fn send_query(
        &self,
        target_addr: SocketAddr,
        query: KrpcQuery,
    ) -> Result<KrpcResponse, DhtError> {
        self.note_query_sent();
        self.send_query_raw(target_addr, query).await
    }

    /// Like [`DhtServer::send_query`] but assumes the caller already counted
    /// the query.
    pub(crate) async fn send_query_raw(
        &self,
        target_addr: SocketAddr,
        query: KrpcQuery,
    ) -> Result<KrpcResponse, DhtError> {
        let socket = self
            .socket()
            .ok_or_else(|| DhtError::Internal("DhtServer socket is not active".into()))?;

        let tid = self
            .next_tid
            .fetch_add(1, Ordering::Relaxed)
            .to_be_bytes()
            .to_vec();
        let msg = KrpcMessage::Query(query);
        let packet = msg.encode(&tid)?;

        let (tx, rx) = oneshot::channel();
        self.pending_transactions
            .lock()
            .await
            .insert(tid.clone(), tx);

        if let Err(err) = socket.send_to(&packet, target_addr).await {
            self.pending_transactions.lock().await.remove(&tid);
            self.errors_sent.fetch_add(1, Ordering::Relaxed);
            return Err(err.into());
        }

        match timeout(QUERY_TIMEOUT, rx).await {
            Ok(Ok(response)) => Ok(response),
            Ok(Err(_)) => {
                self.pending_transactions.lock().await.remove(&tid);
                Err(DhtError::Input(
                    "KRPC query was cancelled before a response arrived".into(),
                ))
            }
            Err(_) => {
                self.pending_transactions.lock().await.remove(&tid);
                Err(DhtError::Input("KRPC query timed out".into()))
            }
        }
    }

    /// Runs the receive loop until `shutdown` changes.
    ///
    /// The loop re-reads the socket whenever `bind_port`/`unbind` is called,
    /// which allows a live port change without restarting the task.
    pub async fn run(&self, mut shutdown: watch::Receiver<bool>) -> Result<(), DhtError> {
        self.bind().await?;

        let mut buffer = vec![0u8; 65535];

        loop {
            let socket = match self.socket() {
                Some(socket) => socket,
                None => {
                    tokio::select! {
                        _ = shutdown.changed() => break,
                        _ = self.socket_changed.notified() => continue,
                    }
                }
            };

            tokio::select! {
                _ = shutdown.changed() => break,
                _ = self.socket_changed.notified() => continue,
                received = socket.recv_from(&mut buffer) => {
                    let (size, from) = match received {
                        Ok(value) => value,
                        Err(err) => {
                            debug!(error = %err, "DHT socket receive failed");
                            continue;
                        }
                    };

                    match self.handle_packet(&buffer[..size], from).await {
                        Ok(Some(reply)) => {
                            match socket.send_to(&reply, from).await {
                                Ok(_) => {
                                    self.replies_sent.fetch_add(1, Ordering::Relaxed);
                                }
                                Err(err) => {
                                    self.errors_sent.fetch_add(1, Ordering::Relaxed);
                                    debug!(error = %err, "DHT reply send failed");
                                }
                            }
                        }
                        Ok(None) => {}
                        Err(err) => {
                            self.errors_received.fetch_add(1, Ordering::Relaxed);
                            debug!(peer = %from, error = %err, "Invalid DHT packet");
                        }
                    }
                }
            }
        }

        Ok(())
    }

    pub async fn handle_packet(
        &self,
        buf: &[u8],
        from_addr: SocketAddr,
    ) -> Result<Option<Vec<u8>>, DhtError> {
        let (tid, msg) = KrpcMessage::decode(buf)?;

        match &msg {
            KrpcMessage::Query(_) => {
                self.queries_received.fetch_add(1, Ordering::Relaxed);
            }
            KrpcMessage::Response(_) => {
                self.replies_received.fetch_add(1, Ordering::Relaxed);
            }
            KrpcMessage::Error(_) => {
                self.errors_received.fetch_add(1, Ordering::Relaxed);
            }
        }

        match msg {
            KrpcMessage::Query(query) => {
                let from_sa = SocketAddress::from(from_addr);
                let response = self.process_query(query, &from_sa).await;
                let reply_msg = KrpcMessage::Response(response);
                let reply_bytes = reply_msg.encode(&tid)?;
                Ok(Some(reply_bytes))
            }
            KrpcMessage::Response(resp) => {
                if let Some(tx) = self.pending_transactions.lock().await.remove(&tid) {
                    let _ = tx.send(resp);
                }
                Ok(None)
            }
            KrpcMessage::Error(_) => {
                self.pending_transactions.lock().await.remove(&tid);
                Ok(None)
            }
        }
    }

    async fn process_query(&self, query: KrpcQuery, from_sa: &SocketAddress) -> KrpcResponse {
        let mut router = self.router.lock().await;
        let own_id = router.own_id;

        match query {
            KrpcQuery::Ping { .. } => KrpcResponse::Ping { id: own_id },
            KrpcQuery::FindNode { target, .. } => {
                let nodes = router.get_closest_nodes(&target);
                KrpcResponse::FindNode { id: own_id, nodes }
            }
            KrpcQuery::GetPeers { info_hash, .. } => {
                let token = router.make_token(from_sa).to_vec();
                if let Some(tracker) = router.get_tracker(&info_hash, false) {
                    let peers = tracker.get_peers(32);
                    if !peers.is_empty() {
                        return KrpcResponse::GetPeers {
                            id: own_id,
                            token,
                            values: peers,
                            nodes: None,
                        };
                    }
                }
                let nodes = Some(router.get_closest_nodes(&info_hash));
                KrpcResponse::GetPeers {
                    id: own_id,
                    token,
                    values: Vec::new(),
                    nodes,
                }
            }
            KrpcQuery::AnnouncePeer {
                info_hash,
                port,
                token,
                ..
            } => {
                if router.token_valid(&token, from_sa) {
                    if let Some(std::net::IpAddr::V4(v4)) = from_sa.ip() {
                        let tracker = router.get_tracker(&info_hash, true);
                        if let Some(t) = tracker {
                            t.add_peer(u32::from(v4), port, 0);
                        }
                    }
                }
                KrpcResponse::AnnouncePeer { id: own_id }
            }
        }
    }
}
