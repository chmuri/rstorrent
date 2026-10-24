use flate2::write::GzEncoder;
use flate2::Compression;
use rt_rpc::{CommandMap, JsonRpc, XmlRpc};
use std::collections::HashMap;
use std::future::Future;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration;
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpListener, UnixListener};
use tokio::task::JoinSet;
use tokio::time::{timeout, timeout_at, Instant};
use tracing::{debug, error, info};

pub const DEFAULT_MAX_HEADER_SIZE: usize = 4096;
pub const DEFAULT_MAX_CONTENT_SIZE: usize = 64 << 20; // 64 MiB
pub const DEFAULT_MIN_COMPRESS_SIZE: usize = 1024;

/// Legacy `SCgiTask::timeout_request` is 60 seconds.
pub const DEFAULT_READ_TIMEOUT: Duration = Duration::from_secs(60);
pub const DEFAULT_WRITE_TIMEOUT: Duration = Duration::from_secs(60);
pub const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(60);
/// How long a graceful shutdown waits for in-flight connections before aborting them.
pub const DEFAULT_SHUTDOWN_GRACE_PERIOD: Duration = Duration::from_secs(1);
/// Legacy `SCgi::max_tasks` is 100. `0` disables the limit.
pub const DEFAULT_MAX_CONNECTIONS: usize = 100;

#[derive(Error, Debug)]
pub enum ScgiError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("SCGI protocol error: {0}")]
    Protocol(String),

    #[error("Header size exceeded: {0} > {1}")]
    HeaderTooLarge(usize, usize),

    #[error("Content size exceeded: {0} > {1}")]
    ContentTooLarge(usize, usize),

    #[error("SCGI {0} timeout")]
    Timeout(&'static str),

    #[error("No systemd socket available")]
    NoSystemdSocket,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContentType {
    Xml,
    Json,
}

#[derive(Debug)]
pub struct ScgiRequest {
    pub headers: HashMap<String, String>,
    pub body: Vec<u8>,
    pub content_length: usize,
    pub content_type: ContentType,
    pub trusted: bool,
    pub accepts_gzip: bool,
}

pub struct ScgiConfig {
    pub max_header_size: usize,
    pub max_content_size: usize,
    pub allow_compression: bool,
    pub min_compress_size: usize,
    /// Maximum time to wait for the first byte of a new request on an otherwise
    /// idle keep-alive connection.
    pub idle_timeout: Duration,
    /// Maximum time to receive the remainder of a request (header and body)
    /// once the first byte has arrived.
    pub read_timeout: Duration,
    /// Maximum time to write a single response.
    pub write_timeout: Duration,
    /// Whether a connection may be reused for subsequent requests.
    pub keep_alive: bool,
    /// Maximum number of concurrent connections. `0` disables the limit.
    pub max_connections: usize,
    /// Time allowed for in-flight connections to finish during shutdown.
    pub shutdown_grace_period: Duration,
}

impl Default for ScgiConfig {
    fn default() -> Self {
        Self {
            max_header_size: DEFAULT_MAX_HEADER_SIZE,
            max_content_size: DEFAULT_MAX_CONTENT_SIZE,
            allow_compression: true,
            min_compress_size: DEFAULT_MIN_COMPRESS_SIZE,
            idle_timeout: DEFAULT_IDLE_TIMEOUT,
            read_timeout: DEFAULT_READ_TIMEOUT,
            write_timeout: DEFAULT_WRITE_TIMEOUT,
            keep_alive: true,
            max_connections: DEFAULT_MAX_CONNECTIONS,
            shutdown_grace_period: DEFAULT_SHUTDOWN_GRACE_PERIOD,
        }
    }
}

/// Structured connection/request counters.
///
/// The counters are relaxed atomics: they are intended for observability and
/// tests, not for synchronisation.
#[derive(Debug, Default)]
pub struct ScgiMetrics {
    pub connections_accepted: AtomicU64,
    pub connections_closed: AtomicU64,
    pub connections_rejected: AtomicU64,
    pub active_connections: AtomicU64,
    pub accept_errors: AtomicU64,
    pub requests_received: AtomicU64,
    pub requests_completed: AtomicU64,
    pub requests_failed: AtomicU64,
    pub timeouts: AtomicU64,
    pub protocol_errors: AtomicU64,
    pub oversized_frames: AtomicU64,
    pub bytes_read: AtomicU64,
    pub bytes_written: AtomicU64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ScgiMetricsSnapshot {
    pub connections_accepted: u64,
    pub connections_closed: u64,
    pub connections_rejected: u64,
    pub active_connections: u64,
    pub accept_errors: u64,
    pub requests_received: u64,
    pub requests_completed: u64,
    pub requests_failed: u64,
    pub timeouts: u64,
    pub protocol_errors: u64,
    pub oversized_frames: u64,
    pub bytes_read: u64,
    pub bytes_written: u64,
}

impl ScgiMetrics {
    pub fn snapshot(&self) -> ScgiMetricsSnapshot {
        ScgiMetricsSnapshot {
            connections_accepted: self.connections_accepted.load(Ordering::Relaxed),
            connections_closed: self.connections_closed.load(Ordering::Relaxed),
            connections_rejected: self.connections_rejected.load(Ordering::Relaxed),
            active_connections: self.active_connections.load(Ordering::Relaxed),
            accept_errors: self.accept_errors.load(Ordering::Relaxed),
            requests_received: self.requests_received.load(Ordering::Relaxed),
            requests_completed: self.requests_completed.load(Ordering::Relaxed),
            requests_failed: self.requests_failed.load(Ordering::Relaxed),
            timeouts: self.timeouts.load(Ordering::Relaxed),
            protocol_errors: self.protocol_errors.load(Ordering::Relaxed),
            oversized_frames: self.oversized_frames.load(Ordering::Relaxed),
            bytes_read: self.bytes_read.load(Ordering::Relaxed),
            bytes_written: self.bytes_written.load(Ordering::Relaxed),
        }
    }
}

struct ConnectionGuard {
    metrics: Arc<ScgiMetrics>,
}

impl ConnectionGuard {
    fn new(metrics: Arc<ScgiMetrics>) -> Self {
        metrics.connections_accepted.fetch_add(1, Ordering::Relaxed);
        metrics.active_connections.fetch_add(1, Ordering::Relaxed);
        Self { metrics }
    }
}

impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        self.metrics
            .active_connections
            .fetch_sub(1, Ordering::Relaxed);
        self.metrics
            .connections_closed
            .fetch_add(1, Ordering::Relaxed);
    }
}

pub struct ScgiHandler {
    command_map: Arc<RwLock<CommandMap>>,
    xmlrpc: Arc<XmlRpc>,
    jsonrpc: Arc<JsonRpc>,
    config: ScgiConfig,
    metrics: Arc<ScgiMetrics>,
}

impl ScgiHandler {
    pub fn new(
        command_map: Arc<RwLock<CommandMap>>,
        xmlrpc: Arc<XmlRpc>,
        jsonrpc: Arc<JsonRpc>,
        config: ScgiConfig,
    ) -> Self {
        Self {
            command_map,
            xmlrpc,
            jsonrpc,
            config,
            metrics: Arc::new(ScgiMetrics::default()),
        }
    }

    /// Shared metrics for this handler. Counters survive for the lifetime of
    /// the handler, including all SCGI servers that use it.
    pub fn metrics(&self) -> Arc<ScgiMetrics> {
        Arc::clone(&self.metrics)
    }

    /// Handle one client connection.
    ///
    /// Requests are processed strictly in order. After each response the
    /// connection is kept open for the next request when `keep_alive` is
    /// enabled; a clean EOF between requests is treated as a normal close.
    pub async fn handle_stream<S>(&self, mut stream: S) -> Result<(), ScgiError>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        let _guard = ConnectionGuard::new(Arc::clone(&self.metrics));
        let result = self.handle_stream_inner(&mut stream).await;

        if result.is_err() {
            // Best-effort graceful FIN. Avoid letting a stuck peer keep the
            // task alive after a timeout or protocol error.
            let _ = timeout(Duration::from_millis(100), stream.shutdown()).await;
        }

        result
    }

    async fn handle_stream_inner<S>(&self, stream: &mut S) -> Result<(), ScgiError>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        loop {
            let request = match self.read_request_or_eof(stream).await {
                Ok(Some(request)) => request,
                Ok(None) => return Ok(()),
                Err(err) => {
                    self.record_error(&err);
                    return Err(err);
                }
            };

            self.metrics
                .requests_received
                .fetch_add(1, Ordering::Relaxed);

            let response = match self.process_request(&request) {
                Ok(response) => response,
                Err(err) => {
                    self.metrics.requests_failed.fetch_add(1, Ordering::Relaxed);
                    return Err(err);
                }
            };

            let write_result = timeout(self.config.write_timeout, async {
                stream.write_all(&response).await?;
                stream.flush().await?;
                Ok::<(), std::io::Error>(())
            })
            .await;

            match write_result {
                Ok(Ok(())) => {
                    self.metrics
                        .requests_completed
                        .fetch_add(1, Ordering::Relaxed);
                    self.metrics
                        .bytes_written
                        .fetch_add(response.len() as u64, Ordering::Relaxed);
                }
                Ok(Err(err)) => {
                    self.metrics.requests_failed.fetch_add(1, Ordering::Relaxed);
                    return Err(err.into());
                }
                Err(_) => {
                    self.metrics.requests_failed.fetch_add(1, Ordering::Relaxed);
                    self.metrics.timeouts.fetch_add(1, Ordering::Relaxed);
                    return Err(ScgiError::Timeout("write"));
                }
            }

            if !self.config.keep_alive {
                return Ok(());
            }
        }
    }

    fn record_error(&self, err: &ScgiError) {
        match err {
            ScgiError::Timeout(phase) => {
                self.metrics.timeouts.fetch_add(1, Ordering::Relaxed);
                // An idle keep-alive connection expiring is not a failed
                // request; a timeout while a request was in progress is.
                if *phase != "idle" {
                    self.metrics.requests_failed.fetch_add(1, Ordering::Relaxed);
                }
            }
            ScgiError::HeaderTooLarge(_, _) | ScgiError::ContentTooLarge(_, _) => {
                self.metrics.requests_failed.fetch_add(1, Ordering::Relaxed);
                self.metrics
                    .oversized_frames
                    .fetch_add(1, Ordering::Relaxed);
            }
            ScgiError::Protocol(_) => {
                self.metrics.requests_failed.fetch_add(1, Ordering::Relaxed);
                self.metrics.protocol_errors.fetch_add(1, Ordering::Relaxed);
            }
            ScgiError::Io(_) | ScgiError::NoSystemdSocket => {}
        }
    }

    /// Read a single request. A clean EOF before any byte of a request is
    /// reported as `Ok(None)`.
    async fn read_request_or_eof<R>(&self, reader: &mut R) -> Result<Option<ScgiRequest>, ScgiError>
    where
        R: AsyncRead + Unpin,
    {
        // Read until ':' to get header length. While no byte has arrived we are
        // "idle" and use the idle timeout; afterwards the whole request shares
        // a single read deadline so a slow peer cannot drip-feed forever.
        let idle_deadline = Instant::now() + self.config.idle_timeout;
        let mut deadline: Option<Instant> = None;
        let mut len_buf = Vec::with_capacity(10);

        loop {
            let is_idle = deadline.is_none();
            let byte_deadline = deadline.unwrap_or(idle_deadline);
            let phase = if is_idle { "idle" } else { "header length" };

            match self.read_byte(reader, byte_deadline, phase).await? {
                None => {
                    if is_idle {
                        return Ok(None);
                    }
                    return Err(ScgiError::Protocol(
                        "Unexpected EOF reading header length".to_string(),
                    ));
                }
                Some(byte) => {
                    if is_idle {
                        deadline = Some(Instant::now() + self.config.read_timeout);
                    }

                    if byte == b':' {
                        break;
                    }
                    if !byte.is_ascii_digit() {
                        return Err(ScgiError::Protocol(format!(
                            "Invalid character in header length: '{}'",
                            byte as char
                        )));
                    }
                    if len_buf.len() >= 10 {
                        return Err(ScgiError::Protocol(
                            "Header length integer too large".to_string(),
                        ));
                    }
                    len_buf.push(byte);
                }
            }
        }

        let request_deadline =
            deadline.unwrap_or_else(|| Instant::now() + self.config.read_timeout);

        let header_len_str = std::str::from_utf8(&len_buf)
            .map_err(|_| ScgiError::Protocol("Invalid UTF-8 in header length".to_string()))?;
        let header_len: usize = header_len_str
            .parse()
            .map_err(|_| ScgiError::Protocol("Failed to parse header length".to_string()))?;

        if header_len > self.config.max_header_size {
            return Err(ScgiError::HeaderTooLarge(
                header_len,
                self.config.max_header_size,
            ));
        }

        let header_buf_len = header_len.checked_add(1).ok_or(ScgiError::HeaderTooLarge(
            header_len,
            self.config.max_header_size,
        ))?;

        // Read header bytes + trailing ','. Bytes belonging to pipelined
        // requests stay in the stream and are consumed by the next iteration.
        let mut header_buf = vec![0u8; header_buf_len];
        self.read_exact(reader, &mut header_buf, request_deadline, "headers")
            .await?;

        if header_buf[header_len] != b',' {
            return Err(ScgiError::Protocol(
                "Missing comma after SCGI headers".to_string(),
            ));
        }

        let headers_data = &header_buf[..header_len];
        let headers = Self::parse_headers(headers_data)?;

        let content_length: usize = headers
            .get("CONTENT_LENGTH")
            .ok_or_else(|| ScgiError::Protocol("Missing CONTENT_LENGTH header".to_string()))?
            .parse()
            .map_err(|_| ScgiError::Protocol("Invalid CONTENT_LENGTH value".to_string()))?;

        if content_length > self.config.max_content_size {
            return Err(ScgiError::ContentTooLarge(
                content_length,
                self.config.max_content_size,
            ));
        }

        let mut body = vec![0u8; content_length];
        if content_length > 0 {
            self.read_exact(reader, &mut body, request_deadline, "body")
                .await?;
        }

        let trusted = headers
            .get("UNTRUSTED_CONNECTION")
            .map(|v| v != "1")
            .unwrap_or(true);

        let accepts_gzip = headers
            .get("ACCEPT_ENCODING")
            .map(|v| v.contains("gzip"))
            .unwrap_or(false);

        let content_type = match headers.get("CONTENT_TYPE").map(|s| s.as_str()) {
            Some(ct) if ct.contains("application/json") => ContentType::Json,
            Some(ct) if ct.contains("text/xml") => ContentType::Xml,
            _ => {
                // Peek body for JSON
                let trimmed = body.iter().find(|&&b| !b.is_ascii_whitespace()).copied();
                if trimmed == Some(b'{') || trimmed == Some(b'[') {
                    ContentType::Json
                } else {
                    ContentType::Xml
                }
            }
        };

        Ok(Some(ScgiRequest {
            headers,
            body,
            content_length,
            content_type,
            trusted,
            accepts_gzip,
        }))
    }

    async fn read_byte<R>(
        &self,
        reader: &mut R,
        deadline: Instant,
        phase: &'static str,
    ) -> Result<Option<u8>, ScgiError>
    where
        R: AsyncRead + Unpin,
    {
        let mut byte = [0u8; 1];
        match timeout_at(deadline, reader.read(&mut byte)).await {
            Ok(Ok(0)) => Ok(None),
            Ok(Ok(_)) => {
                self.metrics.bytes_read.fetch_add(1, Ordering::Relaxed);
                Ok(Some(byte[0]))
            }
            Ok(Err(err)) => Err(err.into()),
            Err(_) => Err(ScgiError::Timeout(phase)),
        }
    }

    async fn read_exact<R>(
        &self,
        reader: &mut R,
        buf: &mut [u8],
        deadline: Instant,
        phase: &'static str,
    ) -> Result<(), ScgiError>
    where
        R: AsyncRead + Unpin,
    {
        let expected = buf.len() as u64;
        match timeout_at(deadline, reader.read_exact(buf)).await {
            Ok(Ok(_)) => {
                self.metrics
                    .bytes_read
                    .fetch_add(expected, Ordering::Relaxed);
                Ok(())
            }
            Ok(Err(err)) => Err(err.into()),
            Err(_) => Err(ScgiError::Timeout(phase)),
        }
    }

    pub async fn read_request<R>(&self, reader: &mut R) -> Result<ScgiRequest, ScgiError>
    where
        R: AsyncRead + Unpin,
    {
        match self.read_request_or_eof(reader).await? {
            Some(request) => Ok(request),
            None => Err(ScgiError::Protocol(
                "Unexpected EOF reading header length".to_string(),
            )),
        }
    }

    fn parse_headers(data: &[u8]) -> Result<HashMap<String, String>, ScgiError> {
        let mut headers = HashMap::new();
        let mut iter = data.split(|&b| b == 0);

        while let Some(key_bytes) = iter.next() {
            if key_bytes.is_empty() {
                break;
            }
            let val_bytes = iter.next().ok_or_else(|| {
                ScgiError::Protocol("Mismatched header key/value pair".to_string())
            })?;

            let key = std::str::from_utf8(key_bytes)
                .map_err(|_| ScgiError::Protocol("Invalid UTF-8 in header key".to_string()))?
                .to_string();
            let val = std::str::from_utf8(val_bytes)
                .map_err(|_| ScgiError::Protocol("Invalid UTF-8 in header value".to_string()))?
                .to_string();

            if headers.insert(key.clone(), val).is_some() {
                return Err(ScgiError::Protocol(format!(
                    "Duplicate SCGI header: {}",
                    key
                )));
            }
        }

        Ok(headers)
    }

    pub fn process_request(&self, req: &ScgiRequest) -> Result<Vec<u8>, ScgiError> {
        let map = self
            .command_map
            .read()
            .map_err(|e| ScgiError::Protocol(format!("Lock error: {}", e)))?;

        let body_str = String::from_utf8_lossy(&req.body);

        let resp_payload = match req.content_type {
            ContentType::Xml => self.xmlrpc.process(&body_str, &map, req.trusted),
            ContentType::Json => self.jsonrpc.process(&body_str, &map, req.trusted),
        };

        let raw_bytes = resp_payload.into_bytes();
        let ct_header = match req.content_type {
            ContentType::Xml => "Content-Type: text/xml\r\n",
            ContentType::Json => "Content-Type: application/json\r\n",
        };

        let use_gzip = self.config.allow_compression
            && req.accepts_gzip
            && raw_bytes.len() >= self.config.min_compress_size;

        if use_gzip {
            let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
            encoder
                .write_all(&raw_bytes)
                .map_err(|e| ScgiError::Protocol(format!("Gzip compression error: {}", e)))?;
            let compressed = encoder
                .finish()
                .map_err(|e| ScgiError::Protocol(format!("Gzip finish error: {}", e)))?;

            let headers = format!(
                "Status: 200 OK\r\n{}Content-Encoding: gzip\r\nContent-Length: {}\r\n\r\n",
                ct_header,
                compressed.len()
            );

            let mut out = Vec::with_capacity(headers.len() + compressed.len());
            out.extend_from_slice(headers.as_bytes());
            out.extend_from_slice(&compressed);
            Ok(out)
        } else {
            let headers = format!(
                "Status: 200 OK\r\n{}Content-Length: {}\r\n\r\n",
                ct_header,
                raw_bytes.len()
            );

            let mut out = Vec::with_capacity(headers.len() + raw_bytes.len());
            out.extend_from_slice(headers.as_bytes());
            out.extend_from_slice(&raw_bytes);
            Ok(out)
        }
    }
}

pub enum ScgiListener {
    Tcp(TcpListener),
    Unix(UnixListener, PathBuf),
}

pub struct ScgiServer {
    pub handler: Arc<ScgiHandler>,
    pub listener: ScgiListener,
}

impl ScgiServer {
    pub fn from_std_tcp(
        listener: std::net::TcpListener,
        handler: Arc<ScgiHandler>,
    ) -> Result<Self, ScgiError> {
        listener.set_nonblocking(true)?;
        let tokio_listener = TcpListener::from_std(listener)?;
        info!("SCGI listening on TCP (from_std)");
        Ok(Self {
            handler,
            listener: ScgiListener::Tcp(tokio_listener),
        })
    }

    pub fn from_std_unix(
        listener: std::os::unix::net::UnixListener,
        path: PathBuf,
        handler: Arc<ScgiHandler>,
    ) -> Result<Self, ScgiError> {
        listener.set_nonblocking(true)?;
        let tokio_listener = UnixListener::from_std(listener)?;
        info!("SCGI listening on Unix socket {:?}", path);
        Ok(Self {
            handler,
            listener: ScgiListener::Unix(tokio_listener, path),
        })
    }

    pub async fn bind_tcp(addr: &str, handler: Arc<ScgiHandler>) -> Result<Self, ScgiError> {
        let listener = TcpListener::bind(addr).await?;
        info!("SCGI listening on TCP {}", addr);
        Ok(Self {
            handler,
            listener: ScgiListener::Tcp(listener),
        })
    }

    pub async fn bind_unix<P: AsRef<Path>>(
        path: P,
        handler: Arc<ScgiHandler>,
    ) -> Result<Self, ScgiError> {
        let p = path.as_ref().to_path_buf();
        if p.exists() {
            let _ = std::fs::remove_file(&p);
        }
        let listener = UnixListener::bind(&p)?;
        info!("SCGI listening on Unix socket {:?}", p);
        Ok(Self {
            handler,
            listener: ScgiListener::Unix(listener, p),
        })
    }

    pub fn from_systemd(handler: Arc<ScgiHandler>) -> Result<Self, ScgiError> {
        // Systemd passes listening sockets starting at file descriptor 3
        let pid_str = std::env::var("LISTEN_PID").ok();
        let fds_str = std::env::var("LISTEN_FDS").ok();

        if let (Some(pid), Some(fds)) = (pid_str, fds_str) {
            let my_pid = std::process::id().to_string();
            if pid == my_pid {
                let count: i32 = fds.parse().unwrap_or(0);
                if count > 0 {
                    use std::os::unix::io::FromRawFd;
                    // First socket activated is FD 3
                    let std_listener = unsafe { std::net::TcpListener::from_raw_fd(3) };
                    std_listener.set_nonblocking(true)?;
                    let tokio_listener = TcpListener::from_std(std_listener)?;
                    info!("SCGI acquired systemd activated socket (FD 3)");
                    return Ok(Self {
                        handler,
                        listener: ScgiListener::Tcp(tokio_listener),
                    });
                }
            }
        }
        Err(ScgiError::NoSystemdSocket)
    }

    pub fn metrics(&self) -> Arc<ScgiMetrics> {
        self.handler.metrics()
    }

    /// Run the accept loop until the listener fails or the process ends.
    pub async fn run(self) -> Result<(), ScgiError> {
        self.run_until(std::future::pending::<()>()).await
    }

    /// Run the accept loop until `shutdown` completes, then drain in-flight
    /// connections for at most `shutdown_grace_period` before aborting them.
    pub async fn run_with_shutdown<F>(self, shutdown: F) -> Result<(), ScgiError>
    where
        F: Future<Output = ()> + Send,
    {
        self.run_until(shutdown).await
    }

    async fn run_until<F>(self, shutdown: F) -> Result<(), ScgiError>
    where
        F: Future<Output = ()> + Send,
    {
        let handler = Arc::clone(&self.handler);
        let metrics = handler.metrics();
        let mut tasks: JoinSet<()> = JoinSet::new();
        tokio::pin!(shutdown);

        let mut unix_path: Option<PathBuf> = None;

        let result = match self.listener {
            ScgiListener::Tcp(listener) => loop {
                while tasks.try_join_next().is_some() {}
                tokio::select! {
                    _ = &mut shutdown => break Ok(()),
                    accepted = listener.accept() => {
                        match accepted {
                            Ok((stream, _addr)) => {
                                Self::spawn_connection(&handler, &mut tasks, stream);
                            }
                            Err(err) => {
                                metrics.accept_errors.fetch_add(1, Ordering::Relaxed);
                                error!("SCGI TCP accept error: {}", err);
                                // Avoid a tight accept-error loop (e.g. EMFILE).
                                tokio::time::sleep(Duration::from_millis(100)).await;
                            }
                        }
                    }
                }
            },
            ScgiListener::Unix(listener, path) => {
                unix_path = Some(path);
                loop {
                    while tasks.try_join_next().is_some() {}
                    tokio::select! {
                        _ = &mut shutdown => break Ok(()),
                        accepted = listener.accept() => {
                            match accepted {
                                Ok((stream, _addr)) => {
                                    Self::spawn_connection(&handler, &mut tasks, stream);
                                }
                                Err(err) => {
                                    metrics.accept_errors.fetch_add(1, Ordering::Relaxed);
                                    error!("SCGI Unix accept error: {}", err);
                                    break Err(ScgiError::Io(err));
                                }
                            }
                        }
                    }
                }
            }
        };

        // Let in-flight requests finish, then abort whatever remains. This
        // guarantees no connection task outlives the server.
        if !tasks.is_empty() {
            let drained = timeout(handler.config.shutdown_grace_period, async {
                while tasks.join_next().await.is_some() {}
            })
            .await;

            if drained.is_err() {
                tasks.shutdown().await;
            }
        }

        if let Some(path) = unix_path {
            let _ = std::fs::remove_file(path);
        }

        result
    }

    fn spawn_connection<S>(handler: &Arc<ScgiHandler>, tasks: &mut JoinSet<()>, stream: S)
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        while tasks.try_join_next().is_some() {}

        let metrics = handler.metrics();
        let limit = handler.config.max_connections;
        if limit > 0 && tasks.len() >= limit {
            metrics.connections_rejected.fetch_add(1, Ordering::Relaxed);
            debug!(
                "SCGI connection limit reached ({}), rejecting connection",
                limit
            );
            // Dropping the stream closes it immediately.
            return;
        }

        let handler = Arc::clone(handler);
        tasks.spawn(async move {
            if let Err(err) = handler.handle_stream(stream).await {
                debug!("SCGI connection error: {}", err);
            }
        });
    }
}
