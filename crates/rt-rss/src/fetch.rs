//! HTTP fetching of feed documents and torrent payloads: conditional GET
//! (ETag/Last-Modified), timeouts, redirect limits, and response size caps.

use std::time::Duration;

use reqwest::header::{
    HeaderMap, HeaderName, ETAG, IF_MODIFIED_SINCE, IF_NONE_MATCH, LAST_MODIFIED,
};
use reqwest::StatusCode;

use crate::error::FetchError;

/// Result of a conditional feed GET.
#[derive(Debug, PartialEq, Eq)]
pub enum FeedResponse {
    /// Server answered 304: the cached copy is still current.
    NotModified,
    /// Fresh document plus the validators to send on the next request.
    Body {
        bytes: Vec<u8>,
        etag: Option<String>,
        last_modified: Option<String>,
    },
}

/// Build the shared RSS HTTP client.
///
/// `max_redirects` is the redirect budget (callers pass 5). The contract
/// signature returns a bare `Client`, but `ClientBuilder::build` can fail
/// (TLS backend init); instead of panicking we log and fall back to
/// `Client::new()`, which only fails in the same catastrophic situation.
pub fn build_client(timeout_secs: u64, max_redirects: usize) -> reqwest::Client {
    let builder = reqwest::Client::builder()
        .user_agent(format!("rstorrent/{} rss", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(timeout_secs))
        .redirect(reqwest::redirect::Policy::limited(max_redirects));
    match builder.build() {
        Ok(client) => client,
        Err(err) => {
            tracing::warn!(error = %err, "RSS client build failed; falling back to default client");
            reqwest::Client::new()
        }
    }
}

/// Accept only http/https URLs with a non-empty host.
///
/// Also enforced by the manager on `add_feed`/`update_feed`; the fetchers
/// re-check so direct callers cannot smuggle in e.g. `file://` URLs.
pub fn validate_url(url: &str) -> Result<(), FetchError> {
    let parsed = reqwest::Url::parse(url).map_err(|_| FetchError::InvalidUrl(url.to_string()))?;
    let scheme_ok = matches!(parsed.scheme(), "http" | "https");
    let host_ok = parsed.host_str().is_some_and(|host| !host.is_empty());
    if scheme_ok && host_ok {
        Ok(())
    } else {
        Err(FetchError::InvalidUrl(url.to_string()))
    }
}

/// Conditional GET of a feed document.
///
/// Sends `If-None-Match`/`If-Modified-Since` when validators are supplied,
/// maps 304 to [`FeedResponse::NotModified`], streams 2xx bodies with a hard
/// `max_bytes` cap, and maps any other status to [`FetchError::Http`].
pub async fn fetch_feed(
    client: &reqwest::Client,
    url: &str,
    etag: Option<&str>,
    last_modified: Option<&str>,
    max_bytes: u64,
) -> Result<FeedResponse, FetchError> {
    validate_url(url)?;
    let mut request = client.get(url);
    if let Some(etag) = etag {
        request = request.header(IF_NONE_MATCH, etag);
    }
    if let Some(last_modified) = last_modified {
        request = request.header(IF_MODIFIED_SINCE, last_modified);
    }
    let response = request.send().await.map_err(map_reqwest_error)?;
    let status = response.status();
    if status == StatusCode::NOT_MODIFIED {
        return Ok(FeedResponse::NotModified);
    }
    if !status.is_success() {
        return Err(FetchError::Http(status.as_u16()));
    }
    let etag = header_value(response.headers(), ETAG);
    let last_modified = header_value(response.headers(), LAST_MODIFIED);
    let bytes = read_body_capped(response, max_bytes).await?;
    Ok(FeedResponse::Body {
        bytes,
        etag,
        last_modified,
    })
}

/// GET a torrent payload, capped at `max_bytes`.
///
/// Bencode documents start with `d` (dictionary); anything else is rejected
/// as [`FetchError::NotTorrent`] before it reaches the parser.
pub async fn fetch_torrent_bytes(
    client: &reqwest::Client,
    url: &str,
    max_bytes: u64,
) -> Result<Vec<u8>, FetchError> {
    validate_url(url)?;
    let response = client.get(url).send().await.map_err(map_reqwest_error)?;
    let status = response.status();
    if !status.is_success() {
        return Err(FetchError::Http(status.as_u16()));
    }
    let bytes = read_body_capped(response, max_bytes).await?;
    if bytes.first() != Some(&b'd') {
        return Err(FetchError::NotTorrent);
    }
    Ok(bytes)
}

/// Map transport-level reqwest errors onto the fetch domain.
fn map_reqwest_error(err: reqwest::Error) -> FetchError {
    if err.is_timeout() {
        FetchError::Timeout
    } else {
        // Includes redirect-budget exhaustion and connection failures.
        FetchError::Io(err.to_string())
    }
}

fn header_value(headers: &HeaderMap, name: HeaderName) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
}

/// Stream the body chunk by chunk, aborting as soon as the cap is exceeded.
async fn read_body_capped(
    mut response: reqwest::Response,
    max_bytes: u64,
) -> Result<Vec<u8>, FetchError> {
    // Reject advertised oversized bodies before downloading anything; the
    // streaming check below remains authoritative for lying senders.
    if response.content_length().is_some_and(|len| len > max_bytes) {
        return Err(FetchError::TooLarge);
    }
    let mut bytes: Vec<u8> = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(map_reqwest_error)? {
        if bytes.len() as u64 + chunk.len() as u64 > max_bytes {
            return Err(FetchError::TooLarge);
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};
    use tokio::task::JoinHandle;

    /// Read until the end of the request head (GET requests carry no body).
    async fn read_request_head(socket: &mut TcpStream) -> String {
        let mut buf = Vec::new();
        let mut chunk = [0u8; 1024];
        loop {
            let read = socket.read(&mut chunk).await.unwrap();
            if read == 0 {
                break;
            }
            buf.extend_from_slice(&chunk[..read]);
            if buf.windows(4).any(|window| window == b"\r\n\r\n") {
                break;
            }
        }
        String::from_utf8_lossy(&buf).into_owned()
    }

    fn response(status: &str, extra_headers: &str, body: &str) -> String {
        format!(
            "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n{extra_headers}\r\n{body}",
            body.len()
        )
    }

    /// One-shot mock server; returns the feed URL and a handle yielding the
    /// received request head. Write errors are ignored: size-capped clients
    /// hang up mid-response, which is expected.
    async fn spawn_server(reply: String) -> (String, JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/feed", listener.local_addr().unwrap());
        let handle = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let head = read_request_head(&mut socket).await;
            let _ = socket.write_all(reply.as_bytes()).await;
            head
        });
        (url, handle)
    }

    #[tokio::test]
    async fn feed_200_returns_body_and_validators() {
        let reply = response(
            "200 OK",
            "ETag: \"feed-1\"\r\nLast-Modified: Wed, 21 Oct 2026 07:28:00 GMT\r\n",
            "<rss/>",
        );
        let (url, server) = spawn_server(reply).await;
        let client = build_client(5, 5);
        let result = fetch_feed(&client, &url, None, None, 1024).await.unwrap();
        assert_eq!(
            result,
            FeedResponse::Body {
                bytes: b"<rss/>".to_vec(),
                etag: Some("\"feed-1\"".to_string()),
                last_modified: Some("Wed, 21 Oct 2026 07:28:00 GMT".to_string()),
            }
        );
        server.await.unwrap();
    }

    #[tokio::test]
    async fn feed_304_returns_not_modified() {
        let (url, server) = spawn_server(response("304 Not Modified", "", "")).await;
        let client = build_client(5, 5);
        let result = fetch_feed(&client, &url, Some("\"feed-1\""), None, 1024)
            .await
            .unwrap();
        assert_eq!(result, FeedResponse::NotModified);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn feed_404_maps_to_http_status() {
        let (url, server) = spawn_server(response("404 Not Found", "", "gone")).await;
        let client = build_client(5, 5);
        let err = fetch_feed(&client, &url, None, None, 1024)
            .await
            .unwrap_err();
        assert!(matches!(err, FetchError::Http(404)), "got {err:?}");
        server.await.unwrap();
    }

    #[tokio::test]
    async fn feed_stream_over_cap_aborts_with_too_large() {
        // No Content-Length: the cap must trip while streaming chunks.
        let body = "x".repeat(64);
        let reply = format!("HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n{body}");
        let (url, server) = spawn_server(reply).await;
        let client = build_client(5, 5);
        let err = fetch_feed(&client, &url, None, None, 16).await.unwrap_err();
        assert!(matches!(err, FetchError::TooLarge), "got {err:?}");
        let _ = server.await;
    }

    #[tokio::test]
    async fn feed_advertised_over_cap_rejected_upfront() {
        let reply = response("200 OK", "", &"y".repeat(100));
        let (url, server) = spawn_server(reply).await;
        let client = build_client(5, 5);
        let err = fetch_feed(&client, &url, None, None, 10).await.unwrap_err();
        assert!(matches!(err, FetchError::TooLarge), "got {err:?}");
        let _ = server.await;
    }

    #[tokio::test]
    async fn feed_slow_server_times_out() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/feed", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            read_request_head(&mut socket).await;
            tokio::time::sleep(Duration::from_secs(3)).await;
            let _ = socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")
                .await;
        });
        let client = build_client(1, 5);
        let err = fetch_feed(&client, &url, None, None, 1024)
            .await
            .unwrap_err();
        assert!(matches!(err, FetchError::Timeout), "got {err:?}");
        server.abort();
    }

    #[tokio::test]
    async fn redirects_within_limit_are_followed() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            for hop in 0..6 {
                let (mut socket, _) = listener.accept().await.unwrap();
                read_request_head(&mut socket).await;
                let reply = if hop < 5 {
                    format!(
                        "HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:{port}/hop{hop}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    )
                } else {
                    response("200 OK", "", "done")
                };
                let _ = socket.write_all(reply.as_bytes()).await;
            }
        });
        let client = build_client(5, 5);
        let url = format!("http://127.0.0.1:{port}/start");
        let result = fetch_feed(&client, &url, None, None, 1024).await.unwrap();
        assert_eq!(
            result,
            FeedResponse::Body {
                bytes: b"done".to_vec(),
                etag: None,
                last_modified: None,
            }
        );
        tokio::time::timeout(Duration::from_secs(5), server)
            .await
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn redirects_over_limit_error() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        // Six 302s: the client gives up after the sixth response, so the
        // server sees exactly six connections.
        let server = tokio::spawn(async move {
            for hop in 0..6 {
                let (mut socket, _) = listener.accept().await.unwrap();
                read_request_head(&mut socket).await;
                let reply = format!(
                    "HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:{port}/hop{hop}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                );
                let _ = socket.write_all(reply.as_bytes()).await;
            }
        });
        let client = build_client(5, 5);
        let url = format!("http://127.0.0.1:{port}/start");
        let err = fetch_feed(&client, &url, None, None, 1024)
            .await
            .unwrap_err();
        assert!(matches!(err, FetchError::Io(_)), "got {err:?}");
        tokio::time::timeout(Duration::from_secs(5), server)
            .await
            .unwrap()
            .unwrap();
    }

    #[test]
    fn validate_url_rejects_non_http_and_empty_hosts() {
        for bad in [
            "file:///tmp/feed.xml",
            "ftp://example.com/feed",
            // WHATWG normalizes "http:///nohost" to host "nohost", so only
            // truly empty authorities like "http://" are rejected.
            "http://",
            "not a url",
            "",
        ] {
            assert!(
                matches!(validate_url(bad), Err(FetchError::InvalidUrl(_))),
                "expected rejection of {bad:?}"
            );
        }
    }

    #[test]
    fn validate_url_accepts_http_and_https() {
        assert!(validate_url("http://example.com/feed").is_ok());
        assert!(validate_url("https://example.com:8443/rss?x=1").is_ok());
    }

    #[tokio::test]
    async fn fetch_feed_rejects_file_scheme() {
        let client = build_client(5, 5);
        let err = fetch_feed(&client, "file:///tmp/feed.xml", None, None, 1024)
            .await
            .unwrap_err();
        assert!(matches!(err, FetchError::InvalidUrl(_)), "got {err:?}");
    }

    #[tokio::test]
    async fn conditional_headers_and_user_agent_are_sent() {
        let (url, server) = spawn_server(response("304 Not Modified", "", "")).await;
        let client = build_client(5, 5);
        let result = fetch_feed(
            &client,
            &url,
            Some("\"abc\""),
            Some("Wed, 21 Oct 2026 07:28:00 GMT"),
            1024,
        )
        .await
        .unwrap();
        assert_eq!(result, FeedResponse::NotModified);
        let head = server.await.unwrap().to_lowercase();
        assert!(
            head.contains("if-none-match: \"abc\""),
            "missing If-None-Match: {head}"
        );
        assert!(
            head.contains("if-modified-since: wed, 21 oct 2026 07:28:00 gmt"),
            "missing If-Modified-Since: {head}"
        );
        let expected_ua = format!("user-agent: rstorrent/{} rss", env!("CARGO_PKG_VERSION"));
        assert!(head.contains(expected_ua.as_str()), "missing UA: {head}");
    }

    #[tokio::test]
    async fn torrent_with_bencode_magic_is_accepted() {
        let payload = "d4:name4:teste";
        let (url, server) = spawn_server(response("200 OK", "", payload)).await;
        let client = build_client(5, 5);
        let bytes = fetch_torrent_bytes(&client, &url, 1024).await.unwrap();
        assert_eq!(bytes, payload.as_bytes());
        server.await.unwrap();
    }

    #[tokio::test]
    async fn torrent_without_bencode_magic_is_rejected() {
        let (url, server) =
            spawn_server(response("200 OK", "", "<html>not a torrent</html>")).await;
        let client = build_client(5, 5);
        let err = fetch_torrent_bytes(&client, &url, 1024).await.unwrap_err();
        assert!(matches!(err, FetchError::NotTorrent), "got {err:?}");
        server.await.unwrap();
    }

    #[tokio::test]
    async fn torrent_empty_body_is_rejected() {
        let (url, server) = spawn_server(response("200 OK", "", "")).await;
        let client = build_client(5, 5);
        let err = fetch_torrent_bytes(&client, &url, 1024).await.unwrap_err();
        assert!(matches!(err, FetchError::NotTorrent), "got {err:?}");
        server.await.unwrap();
    }

    #[tokio::test]
    async fn torrent_over_cap_aborts_with_too_large() {
        let reply = response("200 OK", "", &format!("d{}", "x".repeat(100)));
        let (url, server) = spawn_server(reply).await;
        let client = build_client(5, 5);
        let err = fetch_torrent_bytes(&client, &url, 10).await.unwrap_err();
        assert!(matches!(err, FetchError::TooLarge), "got {err:?}");
        let _ = server.await;
    }

    #[tokio::test]
    async fn torrent_404_maps_to_http_status() {
        let (url, server) = spawn_server(response("404 Not Found", "", "gone")).await;
        let client = build_client(5, 5);
        let err = fetch_torrent_bytes(&client, &url, 1024).await.unwrap_err();
        assert!(matches!(err, FetchError::Http(404)), "got {err:?}");
        server.await.unwrap();
    }
}
