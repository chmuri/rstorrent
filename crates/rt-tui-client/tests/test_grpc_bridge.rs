//! TDD contract for TASK-02 — gRPC client bridge.
//!
//! The bridge is exercised against an in-process `tonic` daemon (see
//! `tests/common/mod.rs`), so no external process is required.

mod common;

use std::time::Duration;

use common::{spawn_tcp_daemon, spawn_unix_daemon, MockDaemon};
use rt_grpc::{CommandResponse, TorrentDetails};
use rt_tui_client::bridge::{BridgeError, GrpcBridge};

const SHORT_TIMEOUT: Duration = Duration::from_secs(1);

// ---------------------------------------------------------------------------
// Plan contract
// ---------------------------------------------------------------------------

#[tokio::test]
async fn bridge_returns_connection_error_when_server_unreachable() {
    let bridge = GrpcBridge::connect("http://127.0.0.1:1", Duration::from_millis(100)).await;
    assert!(matches!(bridge, Err(BridgeError::ConnectionLost(_))));
}

#[tokio::test]
async fn bridge_error_display_includes_endpoint() {
    let err = BridgeError::ConnectionLost("http://127.0.0.1:1".into());
    let msg = format!("{err}");
    assert!(msg.contains("127.0.0.1:1"));
}

#[test]
fn bridge_error_variants_are_exhaustive() {
    // Ensure all variants are matchable without wildcard
    let err = BridgeError::Timeout(Duration::from_secs(5));
    match err {
        BridgeError::ConnectionLost(_) => {}
        BridgeError::Timeout(_) => {}
        BridgeError::ServerError { .. } => {}
        BridgeError::InvalidResponse(_) => {}
    }
}

// ---------------------------------------------------------------------------
// Bridge behaviour
// ---------------------------------------------------------------------------

#[tokio::test]
async fn list_torrents_returns_summaries() {
    let endpoint = spawn_tcp_daemon(MockDaemon {
        summaries: vec![
            MockDaemon::summary("aaa", "first"),
            MockDaemon::summary("bbb", "second"),
        ],
        ..MockDaemon::default()
    })
    .await;
    let mut bridge = GrpcBridge::connect(&endpoint, SHORT_TIMEOUT)
        .await
        .expect("bridge connects");

    let torrents = bridge.list_torrents("main").await.expect("list succeeds");

    assert_eq!(torrents.len(), 2);
    assert_eq!(torrents[0].hash, "aaa");
    assert_eq!(torrents[1].name, "second");
}

#[tokio::test]
async fn get_torrent_returns_details() {
    let details = TorrentDetails {
        summary: Some(MockDaemon::summary("aaa", "first")),
        ..TorrentDetails::default()
    };
    let endpoint = spawn_tcp_daemon(MockDaemon {
        details: Some(details),
        ..MockDaemon::default()
    })
    .await;
    let mut bridge = GrpcBridge::connect(&endpoint, SHORT_TIMEOUT)
        .await
        .expect("bridge connects");

    let details = bridge.get_torrent("aaa").await.expect("details succeed");

    let summary = details.summary.expect("summary is validated");
    assert_eq!(summary.hash, "aaa");
}

#[tokio::test]
async fn get_torrent_rejects_details_without_summary() {
    let endpoint = spawn_tcp_daemon(MockDaemon::default()).await;
    let mut bridge = GrpcBridge::connect(&endpoint, SHORT_TIMEOUT)
        .await
        .expect("bridge connects");

    let result = bridge.get_torrent("aaa").await;

    assert!(matches!(result, Err(BridgeError::InvalidResponse(_))));
}

#[tokio::test]
async fn version_returns_server_version() {
    let endpoint = spawn_tcp_daemon(MockDaemon {
        version: "0.16.1".to_string(),
        ..MockDaemon::default()
    })
    .await;
    let mut bridge = GrpcBridge::connect(&endpoint, SHORT_TIMEOUT)
        .await
        .expect("bridge connects");

    let version = bridge.version().await.expect("version succeeds");

    assert_eq!(version.version, "0.16.1");
}

#[tokio::test]
async fn version_rejects_empty_version_string() {
    let endpoint = spawn_tcp_daemon(MockDaemon::default()).await;
    let mut bridge = GrpcBridge::connect(&endpoint, SHORT_TIMEOUT)
        .await
        .expect("bridge connects");

    let result = bridge.version().await;

    assert!(matches!(result, Err(BridgeError::InvalidResponse(_))));
}

#[tokio::test]
async fn execute_returns_command_response() {
    let endpoint = spawn_tcp_daemon(MockDaemon {
        execute_response: Some(CommandResponse {
            success: true,
            result: None,
            error_message: String::new(),
        }),
        ..MockDaemon::default()
    })
    .await;
    let mut bridge = GrpcBridge::connect(&endpoint, SHORT_TIMEOUT)
        .await
        .expect("bridge connects");

    let response = bridge
        .execute("get_name", Vec::new(), Some("aaa"))
        .await
        .expect("execute succeeds");

    assert!(response.success);
}

#[tokio::test]
async fn server_error_is_mapped_to_server_error() {
    let endpoint = spawn_tcp_daemon(MockDaemon {
        error: Some(tonic::Status::internal("boom")),
        ..MockDaemon::default()
    })
    .await;
    let mut bridge = GrpcBridge::connect(&endpoint, SHORT_TIMEOUT)
        .await
        .expect("bridge connects");

    let result = bridge.list_torrents("main").await;

    match result {
        Err(BridgeError::ServerError { code, message }) => {
            assert_eq!(code, tonic::Code::Internal);
            assert!(message.contains("boom"), "message was {message}");
        }
        other => panic!("expected ServerError, got {other:?}"),
    }
}

#[tokio::test]
async fn slow_rpc_is_reported_as_timeout() {
    let endpoint = spawn_tcp_daemon(MockDaemon {
        delay: Some(Duration::from_millis(500)),
        ..MockDaemon::default()
    })
    .await;
    let mut bridge = GrpcBridge::connect(&endpoint, Duration::from_millis(50))
        .await
        .expect("bridge connects");

    let result = bridge.list_torrents("main").await;

    assert!(matches!(result, Err(BridgeError::Timeout(_))));
}

#[tokio::test]
async fn connects_with_tcp_scheme_prefix() {
    let endpoint = spawn_tcp_daemon(MockDaemon {
        summaries: vec![MockDaemon::summary("ddd", "fourth")],
        ..MockDaemon::default()
    })
    .await;
    let mut bridge = GrpcBridge::connect(&format!("tcp://{endpoint}"), SHORT_TIMEOUT)
        .await
        .expect("bridge connects");

    let torrents = bridge.list_torrents("main").await.expect("list succeeds");
    assert_eq!(torrents.len(), 1);
}

#[tokio::test]
async fn empty_endpoint_is_a_connection_error() {
    let result = GrpcBridge::connect("   ", Duration::from_millis(50)).await;

    assert!(matches!(result, Err(BridgeError::ConnectionLost(_))));
}

#[tokio::test]
async fn empty_unix_path_is_a_connection_error() {
    let result = GrpcBridge::connect("unix://", Duration::from_millis(50)).await;

    assert!(matches!(result, Err(BridgeError::ConnectionLost(_))));
}

#[tokio::test]
async fn connects_over_unix_socket() {
    let (endpoint, path) = spawn_unix_daemon(MockDaemon {
        summaries: vec![MockDaemon::summary("ccc", "third")],
        ..MockDaemon::default()
    })
    .await;
    let mut bridge = GrpcBridge::connect(&endpoint, SHORT_TIMEOUT)
        .await
        .expect("bridge connects over unix socket");

    let result = bridge.list_torrents("main").await;

    let _ = std::fs::remove_file(&path);

    let torrents = result.expect("list succeeds over unix socket");
    assert_eq!(torrents.len(), 1);
    assert_eq!(torrents[0].hash, "ccc");
}
