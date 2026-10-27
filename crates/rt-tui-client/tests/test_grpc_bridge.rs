//! TDD contract for TASK-02 — gRPC client bridge.
//!
//! The bridge is exercised against in-process `tonic` servers that implement the
//! generated service traits, so no external daemon is required.

use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rt_grpc::command_service_server::{CommandService, CommandServiceServer};
use rt_grpc::system_service_server::{SystemService, SystemServiceServer};
use rt_grpc::torrent_service_server::{TorrentService, TorrentServiceServer};
use rt_grpc::{
    AddTorrentRequest, AddTorrentResponse, CommandRequest, CommandResponse, ControlTorrentRequest,
    ControlTorrentResponse, Empty, GetTorrentRequest, ListCommandsResponse, ListTorrentsRequest,
    ListTorrentsResponse, MulticallRequest, MulticallResponse, RemoveTorrentRequest,
    RemoveTorrentResponse, SessionStatsResponse, SetPriorityResponse, SetTorrentPriorityRequest,
    ShutdownRequest, ShutdownResponse, StreamEventsRequest, StreamStatsRequest, TorrentDetails,
    TorrentEvent, TorrentSummary, VersionResponse,
};
use rt_tui_client::bridge::{BridgeError, GrpcBridge};
use tokio_stream::wrappers::{TcpListenerStream, UnixListenerStream};
use tonic::transport::Server;
use tonic::{Request, Response, Status};

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
// In-process daemon
// ---------------------------------------------------------------------------

#[derive(Clone, Default)]
struct MockDaemon {
    summaries: Vec<TorrentSummary>,
    details: Option<TorrentDetails>,
    version: String,
    execute_response: Option<CommandResponse>,
    delay: Option<Duration>,
    error: Option<Status>,
}

impl MockDaemon {
    async fn prepare(&self) -> Result<(), Status> {
        if let Some(delay) = self.delay {
            tokio::time::sleep(delay).await;
        }
        match self.error.as_ref() {
            Some(status) => Err(status.clone()),
            None => Ok(()),
        }
    }

    fn summary(hash: &str, name: &str) -> TorrentSummary {
        TorrentSummary {
            hash: hash.to_string(),
            name: name.to_string(),
            size_bytes: 1024,
            ..TorrentSummary::default()
        }
    }
}

#[tonic::async_trait]
impl TorrentService for MockDaemon {
    async fn list_torrents(
        &self,
        _request: Request<ListTorrentsRequest>,
    ) -> Result<Response<ListTorrentsResponse>, Status> {
        self.prepare().await?;
        Ok(Response::new(ListTorrentsResponse {
            torrents: self.summaries.clone(),
        }))
    }

    async fn get_torrent(
        &self,
        _request: Request<GetTorrentRequest>,
    ) -> Result<Response<TorrentDetails>, Status> {
        self.prepare().await?;
        Ok(Response::new(self.details.clone().unwrap_or_default()))
    }

    async fn add_torrent(
        &self,
        _request: Request<AddTorrentRequest>,
    ) -> Result<Response<AddTorrentResponse>, Status> {
        Err(Status::unimplemented("not needed for the bridge tests"))
    }

    async fn control_torrent(
        &self,
        _request: Request<ControlTorrentRequest>,
    ) -> Result<Response<ControlTorrentResponse>, Status> {
        Err(Status::unimplemented("not needed for the bridge tests"))
    }

    async fn remove_torrent(
        &self,
        _request: Request<RemoveTorrentRequest>,
    ) -> Result<Response<RemoveTorrentResponse>, Status> {
        Err(Status::unimplemented("not needed for the bridge tests"))
    }

    async fn set_torrent_priority(
        &self,
        _request: Request<SetTorrentPriorityRequest>,
    ) -> Result<Response<SetPriorityResponse>, Status> {
        Err(Status::unimplemented("not needed for the bridge tests"))
    }

    type StreamEventsStream = tokio_stream::Empty<Result<TorrentEvent, Status>>;

    async fn stream_events(
        &self,
        _request: Request<StreamEventsRequest>,
    ) -> Result<Response<Self::StreamEventsStream>, Status> {
        Err(Status::unimplemented("streaming is TASK-14"))
    }
}

#[tonic::async_trait]
impl SystemService for MockDaemon {
    async fn get_version(
        &self,
        _request: Request<Empty>,
    ) -> Result<Response<VersionResponse>, Status> {
        self.prepare().await?;
        Ok(Response::new(VersionResponse {
            version: self.version.clone(),
            ..VersionResponse::default()
        }))
    }

    async fn get_session_stats(
        &self,
        _request: Request<Empty>,
    ) -> Result<Response<SessionStatsResponse>, Status> {
        Err(Status::unimplemented("not needed for the bridge tests"))
    }

    type StreamStatsStream = tokio_stream::Empty<Result<SessionStatsResponse, Status>>;

    async fn stream_stats(
        &self,
        _request: Request<StreamStatsRequest>,
    ) -> Result<Response<Self::StreamStatsStream>, Status> {
        Err(Status::unimplemented("streaming is TASK-14"))
    }

    async fn shutdown(
        &self,
        _request: Request<ShutdownRequest>,
    ) -> Result<Response<ShutdownResponse>, Status> {
        Err(Status::unimplemented("not needed for the bridge tests"))
    }
}

#[tonic::async_trait]
impl CommandService for MockDaemon {
    async fn execute(
        &self,
        _request: Request<CommandRequest>,
    ) -> Result<Response<CommandResponse>, Status> {
        self.prepare().await?;
        Ok(Response::new(self.execute_response.clone().unwrap_or_default()))
    }

    async fn multicall(
        &self,
        _request: Request<MulticallRequest>,
    ) -> Result<Response<MulticallResponse>, Status> {
        Err(Status::unimplemented("not needed for the bridge tests"))
    }

    async fn list_commands(
        &self,
        _request: Request<Empty>,
    ) -> Result<Response<ListCommandsResponse>, Status> {
        Err(Status::unimplemented("not needed for the bridge tests"))
    }
}

fn services(daemon: MockDaemon) -> tonic::transport::server::Router {
    Server::builder()
        .add_service(TorrentServiceServer::new(daemon.clone()))
        .add_service(SystemServiceServer::new(daemon.clone()))
        .add_service(CommandServiceServer::new(daemon))
}

/// Starts the mock daemon on an ephemeral loopback port and returns its endpoint.
async fn spawn_tcp_daemon(daemon: MockDaemon) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind loopback listener");
    let port = listener.local_addr().expect("listener address").port();
    tokio::spawn(services(daemon).serve_with_incoming(TcpListenerStream::new(listener)));
    format!("127.0.0.1:{port}")
}

fn unique_socket_path() -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or_default();
    std::env::temp_dir().join(format!("rt-tui-client-bridge-{}-{nanos}.sock", std::process::id()))
}

/// Starts the mock daemon on a Unix socket and returns its endpoint plus path.
async fn spawn_unix_daemon(daemon: MockDaemon) -> (String, PathBuf) {
    let path = unique_socket_path();
    let listener = tokio::net::UnixListener::bind(&path).expect("bind unix listener");
    tokio::spawn(services(daemon).serve_with_incoming(UnixListenerStream::new(listener)));
    (format!("unix://{}", path.display()), path)
}

const SHORT_TIMEOUT: Duration = Duration::from_secs(1);

// ---------------------------------------------------------------------------
// Bridge behaviour
// ---------------------------------------------------------------------------

#[tokio::test]
async fn list_torrents_returns_summaries() {
    let endpoint =
        spawn_tcp_daemon(MockDaemon {
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
    let endpoint =
        spawn_tcp_daemon(MockDaemon {
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
        error: Some(Status::internal("boom")),
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
async fn connects_over_unix_socket() {
    let (endpoint, path) =
        spawn_unix_daemon(MockDaemon {
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
