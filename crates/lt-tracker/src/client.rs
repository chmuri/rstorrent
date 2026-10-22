use lt_common::hash_string::HashString;
use lt_common::object::bencode;
use lt_net::addr::SocketAddress;
use std::net::SocketAddr;
use std::sync::OnceLock;
use std::time::{Duration, Instant};
use thiserror::Error;
use tokio::net::UdpSocket;
use tokio::time::{sleep, timeout};

use crate::http::{
    build_announce_url, uri_can_scrape, uri_generate_scrape_url, url_encode_bytes,
    HttpAnnounceResponse, HttpTrackerError,
};
use crate::udp::{
    AnnounceRequest, AnnounceResponse, ConnectRequest, ConnectResponse, ErrorResponse,
    ScrapeRequest, ScrapeResponse, UdpTrackerError, ACTION_ANNOUNCE, ACTION_CONNECT, ACTION_ERROR,
    ACTION_SCRAPE,
};
use crate::udp_router::UdpRouter;

/// Shared router used by the convenience UDP entry points. Every transaction
/// gets an ID reserved in this router, so concurrent tracker operations never
/// accept each other's responses.
fn global_udp_router() -> &'static tokio::sync::Mutex<UdpRouter> {
    static ROUTER: OnceLock<tokio::sync::Mutex<UdpRouter>> = OnceLock::new();
    ROUTER.get_or_init(|| tokio::sync::Mutex::new(UdpRouter::new()))
}

#[derive(Debug, Error)]
pub enum TrackerClientError {
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Bencode error: {0}")]
    Bencode(#[from] lt_common::object::BencodeError),
    #[error("Tracker HTTP protocol error: {0}")]
    TrackerHttp(#[from] HttpTrackerError),
    #[error("Tracker UDP protocol error: {0}")]
    TrackerUdp(#[from] UdpTrackerError),
    #[error("Timeout while contacting tracker")]
    Timeout,
    #[error("Unsupported tracker URL scheme: {0}")]
    UnsupportedScheme(String),
    #[error("Invalid tracker URL: {0}")]
    InvalidUrl(String),
    #[error("DNS resolution failed for tracker host: {0}")]
    DnsResolutionFailed(String),
    #[error("Malformed tracker response: {0}")]
    MalformedResponse(String),
}

impl TrackerClientError {
    /// True when the error means the tracker cannot serve scrapes rather than
    /// the announce endpoint being unhealthy. Such failures must not demote a
    /// tracker or mark it announce-failed.
    pub fn is_scrape_unsupported(&self) -> bool {
        match self {
            TrackerClientError::UnsupportedScheme(_) => true,
            TrackerClientError::InvalidUrl(message) => {
                message.contains("404") || message.contains("405")
            }
            TrackerClientError::TrackerHttp(HttpTrackerError::InvalidScrapeUrl) => true,
            TrackerClientError::TrackerHttp(HttpTrackerError::MissingKey("files")) => true,
            TrackerClientError::TrackerHttp(HttpTrackerError::MissingKey("files[info_hash]")) => {
                true
            }
            TrackerClientError::TrackerHttp(HttpTrackerError::FailureReason(message))
            | TrackerClientError::TrackerUdp(UdpTrackerError::TrackerError(message)) => {
                let message = message.to_ascii_lowercase();
                message.contains("scrape")
                    && (message.contains("not support")
                        || message.contains("unsupported")
                        || message.contains("disabled"))
            }
            _ => false,
        }
    }
}

/// Result of an announce, including the optional tracker statistics that are
/// used to refresh intervals and swarm counts.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TrackerAnnounceResult {
    pub peers: Vec<SocketAddr>,
    pub interval: Option<u64>,
    pub min_interval: Option<u64>,
    pub complete: Option<u32>,
    pub incomplete: Option<u32>,
    pub warning_message: Option<String>,
}

/// Scrape statistics for one torrent.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ScrapeStats {
    pub seeders: u32,
    pub completed: u32,
    pub leechers: u32,
}

#[derive(Clone, Debug)]
pub struct AnnounceParams {
    pub info_hash: HashString,
    pub peer_id: HashString,
    pub port: u16,
    pub uploaded: u64,
    pub downloaded: u64,
    pub left: u64,
    pub numwant: i32,
    pub event: Option<String>,
}

/// Dispatches an announce request to either an HTTP/HTTPS or UDP tracker.
pub async fn announce(
    url: &str,
    params: &AnnounceParams,
) -> Result<Vec<SocketAddr>, TrackerClientError> {
    announce_detailed(url, params)
        .await
        .map(|result| result.peers)
}

/// Like [`announce`], but also returns the tracker interval and swarm counts.
pub async fn announce_detailed(
    url: &str,
    params: &AnnounceParams,
) -> Result<TrackerAnnounceResult, TrackerClientError> {
    if url.starts_with("http://") || url.starts_with("https://") {
        announce_http_detailed(url, params).await
    } else if url.starts_with("udp://") {
        let router = global_udp_router();
        let mut router = router.lock().await;
        announce_udp_detailed_with_router(url, params, &mut *router).await
    } else {
        Err(TrackerClientError::UnsupportedScheme(url.to_string()))
    }
}

/// Announces to an HTTP or HTTPS tracker.
pub async fn announce_http(
    url: &str,
    params: &AnnounceParams,
) -> Result<Vec<SocketAddr>, TrackerClientError> {
    announce_http_detailed(url, params)
        .await
        .map(|result| result.peers)
}

/// Announces to an HTTP or HTTPS tracker and keeps the optional stats.
pub async fn announce_http_detailed(
    url: &str,
    params: &AnnounceParams,
) -> Result<TrackerAnnounceResult, TrackerClientError> {
    let announce_url = build_announce_url(
        url,
        params.info_hash.as_bytes(),
        params.peer_id.as_bytes(),
        params.port,
        params.uploaded,
        params.downloaded,
        params.left,
        params.numwant,
        params.event.as_deref(),
        0x12345678,
        true,
    );

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .user_agent(lt_net::http::DEFAULT_USER_AGENT)
        .build()?;

    let resp = client.get(&announce_url).send().await?;
    let status = resp.status();
    if !status.is_success() {
        return Err(TrackerClientError::InvalidUrl(format!(
            "Tracker responded with status HTTP {}",
            status
        )));
    }

    let bytes = resp.bytes().await?;
    let obj = bencode::decode(&bytes)?;
    let parsed = HttpAnnounceResponse::parse_bencode(&obj)?;

    Ok(TrackerAnnounceResult {
        peers: socket_addresses_to_socket_addrs(&parsed.peers),
        interval: Some(parsed.interval),
        min_interval: parsed.min_interval,
        complete: parsed.complete,
        incomplete: parsed.incomplete,
        warning_message: parsed.warning_message,
    })
}

fn socket_addresses_to_socket_addrs(addresses: &lt_net::addr::AddressList) -> Vec<SocketAddr> {
    let mut result = Vec::with_capacity(addresses.len());
    for sa in addresses.iter() {
        match sa {
            SocketAddress::Inet(v4) => result.push(SocketAddr::V4(*v4)),
            SocketAddress::Inet6(v6) => result.push(SocketAddr::V6(*v6)),
            _ => {}
        }
    }
    result
}

async fn resolve_udp_tracker(url: &str) -> Result<SocketAddr, TrackerClientError> {
    let stripped = url.strip_prefix("udp://").unwrap_or(url);
    let host_port_part = stripped.split('/').next().unwrap_or(stripped);

    let mut addrs = tokio::net::lookup_host(host_port_part)
        .await
        .map_err(|_| TrackerClientError::DnsResolutionFailed(host_port_part.to_string()))?;

    addrs
        .next()
        .ok_or_else(|| TrackerClientError::DnsResolutionFailed(host_port_part.to_string()))
}

async fn bind_udp_socket(tracker_addr: SocketAddr) -> Result<UdpSocket, TrackerClientError> {
    let bind_addr = if tracker_addr.is_ipv6() {
        "[::]:0"
    } else {
        "0.0.0.0:0"
    };

    let socket = UdpSocket::bind(bind_addr).await?;
    socket.connect(tracker_addr).await?;
    Ok(socket)
}

/// Sends one UDP packet and waits for a response carrying `expected_tx`.
///
/// Packets belonging to other transactions are ignored rather than treated as
/// errors. A tracker error packet is surfaced as a protocol failure. The
/// timeout grows with `attempt`, providing the retry/backoff described by BEP
/// 15.
async fn udp_exchange(
    socket: &UdpSocket,
    payload: &[u8],
    expected_tx: u32,
    expected_action: u32,
    attempt: u32,
) -> Result<Vec<u8>, TrackerClientError> {
    socket.send(payload).await?;

    let wait = UdpRouter::retry_timeout(attempt);
    let deadline = Instant::now() + wait;
    let mut buffer = vec![0u8; 65536];

    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(TrackerClientError::Timeout);
        }

        let read = timeout(remaining, socket.recv(&mut buffer))
            .await
            .map_err(|_| TrackerClientError::Timeout)??;

        if read < 8 {
            return Err(TrackerClientError::TrackerUdp(
                UdpTrackerError::MalformedResponse(format!(
                    "packet shorter than UDP tracker header: {read} bytes"
                )),
            ));
        }

        let action = u32::from_be_bytes(buffer[0..4].try_into().unwrap());
        let transaction_id = u32::from_be_bytes(buffer[4..8].try_into().unwrap());

        if action == ACTION_ERROR && transaction_id == expected_tx {
            let error = ErrorResponse::decode(&buffer[..read])?;
            return Err(TrackerClientError::TrackerUdp(
                UdpTrackerError::TrackerError(error.message),
            ));
        }

        // A stale or unrelated response from another concurrent request.
        if transaction_id != expected_tx {
            continue;
        }

        if action != expected_action {
            return Err(TrackerClientError::TrackerUdp(
                UdpTrackerError::InvalidAction(action),
            ));
        }

        buffer.truncate(read);
        return Ok(buffer);
    }
}

async fn ensure_udp_connection(
    socket: &UdpSocket,
    router: &mut UdpRouter,
    tracker_addr: &SocketAddress,
) -> Result<u64, TrackerClientError> {
    if let Some(connection_id) = router.get_connection_id(tracker_addr) {
        return Ok(connection_id);
    }

    let attempts = UdpRouter::DEFAULT_RETRIES + 1;
    let mut last_error = TrackerClientError::Timeout;

    for attempt in 0..attempts {
        let transaction_id = router.allocate_transaction_id();
        let request = ConnectRequest::new(transaction_id).encode();

        let result = udp_exchange(socket, &request, transaction_id, ACTION_CONNECT, attempt).await;

        router.release_transaction_id(transaction_id);

        match result {
            Ok(data) => {
                let response = ConnectResponse::decode(&data)?;
                if response.connection_id == 0 {
                    return Err(TrackerClientError::TrackerUdp(
                        UdpTrackerError::MalformedResponse(
                            "connect response contained connection id 0".into(),
                        ),
                    ));
                }
                router.cache_connection_id(tracker_addr.clone(), response.connection_id);
                return Ok(response.connection_id);
            }
            Err(TrackerClientError::Timeout) if attempt + 1 < attempts => {
                last_error = TrackerClientError::Timeout;
                sleep(UdpRouter::retry_timeout(attempt)).await;
            }
            Err(err) => return Err(err),
        }
    }

    Err(last_error)
}

/// Announces to a UDP tracker using BEP 15 binary protocol.
pub async fn announce_udp(
    url: &str,
    params: &AnnounceParams,
) -> Result<Vec<SocketAddr>, TrackerClientError> {
    let router = global_udp_router();
    let mut router = router.lock().await;
    announce_udp_with_router(url, params, &mut *router).await
}

/// Announces to a UDP tracker using a caller-supplied [`UdpRouter`].
pub async fn announce_udp_with_router(
    url: &str,
    params: &AnnounceParams,
    router: &mut UdpRouter,
) -> Result<Vec<SocketAddr>, TrackerClientError> {
    announce_udp_detailed_with_router(url, params, router)
        .await
        .map(|result| result.peers)
}

/// Announces to a UDP tracker and returns the optional interval/swarm stats.
pub async fn announce_udp_detailed_with_router(
    url: &str,
    params: &AnnounceParams,
    router: &mut UdpRouter,
) -> Result<TrackerAnnounceResult, TrackerClientError> {
    let tracker_addr = resolve_udp_tracker(url).await?;
    let tracker_address = SocketAddress::from(tracker_addr);
    let socket = bind_udp_socket(tracker_addr).await?;

    let event_code = match params.event.as_deref() {
        Some("completed") => 1,
        Some("started") => 2,
        Some("stopped") => 3,
        _ => 0,
    };

    let attempts = UdpRouter::DEFAULT_RETRIES + 1;
    let mut last_error = TrackerClientError::Timeout;

    for attempt in 0..attempts {
        let connection_id = match ensure_udp_connection(&socket, router, &tracker_address).await {
            Ok(id) => id,
            Err(err) => return Err(err),
        };

        let transaction_id = router.allocate_transaction_id();
        let request = AnnounceRequest {
            connection_id,
            transaction_id,
            info_hash: *params.info_hash.as_bytes(),
            peer_id: *params.peer_id.as_bytes(),
            downloaded: params.downloaded,
            left: params.left,
            uploaded: params.uploaded,
            event: event_code,
            ip_address: 0,
            key: 0x12345678,
            num_want: params.numwant,
            port: params.port,
        }
        .encode();

        let result =
            udp_exchange(&socket, &request, transaction_id, ACTION_ANNOUNCE, attempt).await;

        router.release_transaction_id(transaction_id);

        match result {
            Ok(data) => {
                let response = AnnounceResponse::decode(&data)?;
                return Ok(TrackerAnnounceResult {
                    peers: socket_addresses_to_socket_addrs(&response.peers),
                    interval: Some(response.interval as u64),
                    min_interval: None,
                    complete: Some(response.seeders),
                    incomplete: Some(response.leechers),
                    warning_message: None,
                });
            }
            Err(TrackerClientError::Timeout) if attempt + 1 < attempts => {
                // Keep the connection id for the retry. It is still valid for
                // the connection TTL and avoids an unnecessary reconnect.
                last_error = TrackerClientError::Timeout;
                sleep(UdpRouter::retry_timeout(attempt)).await;
            }
            Err(err) => return Err(err),
        }
    }

    router.remove_connection(&tracker_address);
    Err(last_error)
}

/// Dispatches a scrape request to an HTTP or UDP tracker.
pub async fn scrape(url: &str, info_hash: HashString) -> Result<ScrapeStats, TrackerClientError> {
    if url.starts_with("http://") || url.starts_with("https://") {
        scrape_http(url, info_hash).await
    } else if url.starts_with("udp://") {
        scrape_udp(url, info_hash).await
    } else {
        Err(TrackerClientError::UnsupportedScheme(url.to_string()))
    }
}

/// Like [`scrape`], but uses a caller-supplied [`UdpRouter`] for UDP trackers.
pub async fn scrape_with_router(
    url: &str,
    info_hash: HashString,
    router: &mut UdpRouter,
) -> Result<ScrapeStats, TrackerClientError> {
    if url.starts_with("http://") || url.starts_with("https://") {
        scrape_http(url, info_hash).await
    } else if url.starts_with("udp://") {
        scrape_udp_with_router(url, info_hash, router).await
    } else {
        Err(TrackerClientError::UnsupportedScheme(url.to_string()))
    }
}

/// Scrapes an HTTP tracker (`/scrape` endpoint).
pub async fn scrape_http(
    url: &str,
    info_hash: HashString,
) -> Result<ScrapeStats, TrackerClientError> {
    if !uri_can_scrape(url) {
        return Err(TrackerClientError::TrackerHttp(
            HttpTrackerError::InvalidScrapeUrl,
        ));
    }

    let mut scrape_url = uri_generate_scrape_url(url)?;
    scrape_url.push(if scrape_url.contains('?') { '&' } else { '?' });
    scrape_url.push_str("info_hash=");
    scrape_url.push_str(&url_encode_bytes(info_hash.as_bytes()));

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .user_agent(lt_net::http::DEFAULT_USER_AGENT)
        .build()?;

    let response = client.get(&scrape_url).send().await?;
    if !response.status().is_success() {
        return Err(TrackerClientError::InvalidUrl(format!(
            "Tracker responded with status HTTP {}",
            response.status()
        )));
    }

    let bytes = response.bytes().await?;
    let object = bencode::decode(&bytes)?;

    if let Ok(reason) = object.get_key("failure reason").and_then(|v| v.as_string()) {
        return Err(TrackerClientError::TrackerHttp(
            HttpTrackerError::FailureReason(String::from_utf8_lossy(reason).to_string()),
        ));
    }

    let files = object
        .get_key_map("files")
        .map_err(|_| TrackerClientError::TrackerHttp(HttpTrackerError::MissingKey("files")))?;

    let entry = files
        .get(info_hash.as_slice())
        .ok_or(TrackerClientError::TrackerHttp(
            HttpTrackerError::MissingKey("files[info_hash]"),
        ))?;

    fn scrape_count(entry: &lt_common::object::Object, key: &str) -> u32 {
        entry
            .get_key_value(key)
            .unwrap_or(0)
            .max(0)
            .min(u32::MAX as i64) as u32
    }

    Ok(ScrapeStats {
        seeders: scrape_count(entry, "complete"),
        completed: scrape_count(entry, "downloaded"),
        leechers: scrape_count(entry, "incomplete"),
    })
}

/// Scrapes a UDP tracker (BEP 15).
pub async fn scrape_udp(
    url: &str,
    info_hash: HashString,
) -> Result<ScrapeStats, TrackerClientError> {
    let router = global_udp_router();
    let mut router = router.lock().await;
    scrape_udp_with_router(url, info_hash, &mut *router).await
}

/// Scrapes a UDP tracker using a caller-supplied [`UdpRouter`].
pub async fn scrape_udp_with_router(
    url: &str,
    info_hash: HashString,
    router: &mut UdpRouter,
) -> Result<ScrapeStats, TrackerClientError> {
    let tracker_addr = resolve_udp_tracker(url).await?;
    let tracker_address = SocketAddress::from(tracker_addr);
    let socket = bind_udp_socket(tracker_addr).await?;

    let attempts = UdpRouter::DEFAULT_RETRIES + 1;
    let mut last_error = TrackerClientError::Timeout;

    for attempt in 0..attempts {
        let connection_id = match ensure_udp_connection(&socket, router, &tracker_address).await {
            Ok(id) => id,
            Err(err) => return Err(err),
        };

        let transaction_id = router.allocate_transaction_id();
        let request = ScrapeRequest {
            connection_id,
            transaction_id,
            info_hashes: vec![*info_hash.as_bytes()],
        }
        .encode();

        let result = udp_exchange(&socket, &request, transaction_id, ACTION_SCRAPE, attempt).await;

        router.release_transaction_id(transaction_id);

        match result {
            Ok(data) => {
                let response = ScrapeResponse::decode(&data)?;
                let stat = response.stats.first().copied().ok_or_else(|| {
                    TrackerClientError::MalformedResponse(
                        "UDP scrape response contained no stats".into(),
                    )
                })?;

                return Ok(ScrapeStats {
                    seeders: stat.seeders,
                    completed: stat.completed,
                    leechers: stat.leechers,
                });
            }
            Err(TrackerClientError::Timeout) if attempt + 1 < attempts => {
                last_error = TrackerClientError::Timeout;
                sleep(UdpRouter::retry_timeout(attempt)).await;
            }
            Err(err) => return Err(err),
        }
    }

    router.remove_connection(&tracker_address);
    Err(last_error)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_unsupported_scheme() {
        let params = AnnounceParams {
            info_hash: HashString::new([0u8; 20]),
            peer_id: HashString::new([1u8; 20]),
            port: 6881,
            uploaded: 0,
            downloaded: 0,
            left: 1000,
            numwant: 50,
            event: None,
        };
        let err = announce("ftp://tracker.example.com", &params)
            .await
            .unwrap_err();
        assert!(matches!(err, TrackerClientError::UnsupportedScheme(_)));
    }
}
