//! TDD contract for TASK-01 — endpoint validation and reachability probe.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use rt_tui_client::endpoint::{Endpoint, EndpointError, ProbeError};

/// Unique temporary socket path so parallel test runs never collide.
fn unique_socket_path(name: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or_default();
    std::env::temp_dir().join(format!(
        "rt-tui-client-{name}-{}-{nanos}.sock",
        std::process::id()
    ))
}

fn tcp(host: &str, port: u16) -> Endpoint {
    Endpoint::Tcp {
        host: host.to_string(),
        port,
    }
}

#[test]
fn endpoint_parses_host_and_port() {
    let endpoint: Endpoint = "localhost:5000".parse().expect("valid endpoint");
    assert_eq!(endpoint, tcp("localhost", 5000));
}

#[test]
fn endpoint_parses_tcp_scheme() {
    let endpoint: Endpoint = "tcp://127.0.0.1:8080".parse().expect("valid endpoint");
    assert_eq!(endpoint, tcp("127.0.0.1", 8080));
}

#[test]
fn endpoint_parses_unix_socket() {
    let endpoint: Endpoint = "unix:///tmp/rstorrent.sock"
        .parse()
        .expect("valid endpoint");
    assert_eq!(
        endpoint,
        Endpoint::Unix {
            path: PathBuf::from("/tmp/rstorrent.sock"),
        }
    );
}

#[test]
fn endpoint_rejects_empty_string() {
    let result: Result<Endpoint, EndpointError> = "".parse();
    assert!(matches!(result, Err(EndpointError::Empty)));
}

#[test]
fn endpoint_rejects_missing_port() {
    let result: Result<Endpoint, EndpointError> = "localhost".parse();
    assert!(matches!(result, Err(EndpointError::MissingPort(_))));
}

#[test]
fn endpoint_rejects_missing_host() {
    let result: Result<Endpoint, EndpointError> = ":5000".parse();
    assert!(matches!(result, Err(EndpointError::MissingHost(_))));
}

#[test]
fn endpoint_rejects_unknown_scheme() {
    let result: Result<Endpoint, EndpointError> = "http://localhost:5000".parse();
    assert!(matches!(result, Err(EndpointError::UnsupportedScheme(_))));
}

#[test]
fn endpoint_rejects_invalid_port() {
    let result: Result<Endpoint, EndpointError> = "localhost:not-a-port".parse();
    assert!(matches!(result, Err(EndpointError::InvalidPort(_))));
}

#[test]
fn endpoint_rejects_empty_unix_path() {
    let result: Result<Endpoint, EndpointError> = "unix://".parse();
    assert!(matches!(result, Err(EndpointError::EmptyUnixPath)));
}

#[test]
fn endpoint_display_round_trips() {
    for raw in [
        "localhost:5000",
        "tcp://127.0.0.1:8080",
        "unix:///tmp/rstorrent.sock",
    ] {
        let endpoint: Endpoint = raw.parse().expect("valid endpoint");
        let reparsed: Endpoint = endpoint
            .to_string()
            .parse()
            .expect("valid rendered endpoint");
        assert_eq!(endpoint, reparsed, "round trip failed for {raw}");
    }
}

#[tokio::test]
async fn probe_succeeds_on_listening_tcp_socket() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind loopback listener");
    let port = listener.local_addr().expect("listener address").port();
    let endpoint = tcp("127.0.0.1", port);

    assert!(endpoint.probe().await.is_ok());
}

#[tokio::test]
async fn probe_reports_refused_on_closed_port() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback listener");
    let port = listener.local_addr().expect("listener address").port();
    drop(listener); // the port is now free, so connecting must be refused

    let endpoint = tcp("127.0.0.1", port);

    assert!(matches!(
        endpoint.probe().await,
        Err(ProbeError::Refused(_))
    ));
}

#[tokio::test]
async fn probe_succeeds_on_listening_unix_socket() {
    let path = unique_socket_path("probe");
    let listener = tokio::net::UnixListener::bind(&path).expect("bind unix listener");
    let endpoint = Endpoint::Unix { path: path.clone() };

    let result = endpoint.probe().await;

    drop(listener);
    let _ = std::fs::remove_file(&path);

    assert!(result.is_ok(), "probe failed: {result:?}");
}
