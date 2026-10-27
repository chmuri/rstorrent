//! Shared in-process gRPC daemon used by the `rt-tui-client` integration tests.
//!
//! Each test binary compiles its own copy, so items not needed by every test
//! file are allowed to be dead there.

#![allow(dead_code)]

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
use tokio_stream::wrappers::{TcpListenerStream, UnixListenerStream};
use tonic::transport::server::Router;
use tonic::transport::Server;
use tonic::{Request, Response, Status};

/// Configurable fake daemon implementing all three generated services.
#[derive(Clone, Default)]
pub struct MockDaemon {
    pub summaries: Vec<TorrentSummary>,
    pub details: Option<TorrentDetails>,
    pub version: String,
    pub stats: SessionStatsResponse,
    pub execute_response: Option<CommandResponse>,
    pub delay: Option<Duration>,
    pub error: Option<Status>,
}

impl MockDaemon {
    /// Builds a summary with the given hash and name.
    pub fn summary(hash: &str, name: &str) -> TorrentSummary {
        TorrentSummary {
            hash: hash.to_string(),
            name: name.to_string(),
            size_bytes: 1024,
            ..TorrentSummary::default()
        }
    }

    async fn prepare(&self) -> Result<(), Status> {
        if let Some(delay) = self.delay {
            tokio::time::sleep(delay).await;
        }
        match self.error.as_ref() {
            Some(status) => Err(status.clone()),
            None => Ok(()),
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
        Err(Status::unimplemented("not needed by the rt-tui-client tests"))
    }

    async fn control_torrent(
        &self,
        _request: Request<ControlTorrentRequest>,
    ) -> Result<Response<ControlTorrentResponse>, Status> {
        Err(Status::unimplemented("not needed by the rt-tui-client tests"))
    }

    async fn remove_torrent(
        &self,
        _request: Request<RemoveTorrentRequest>,
    ) -> Result<Response<RemoveTorrentResponse>, Status> {
        Err(Status::unimplemented("not needed by the rt-tui-client tests"))
    }

    async fn set_torrent_priority(
        &self,
        _request: Request<SetTorrentPriorityRequest>,
    ) -> Result<Response<SetPriorityResponse>, Status> {
        Err(Status::unimplemented("not needed by the rt-tui-client tests"))
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
        self.prepare().await?;
        Ok(Response::new(self.stats))
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
        Err(Status::unimplemented("not needed by the rt-tui-client tests"))
    }
}

#[tonic::async_trait]
impl CommandService for MockDaemon {
    async fn execute(
        &self,
        _request: Request<CommandRequest>,
    ) -> Result<Response<CommandResponse>, Status> {
        self.prepare().await?;
        Ok(Response::new(
            self.execute_response.clone().unwrap_or_default(),
        ))
    }

    async fn multicall(
        &self,
        _request: Request<MulticallRequest>,
    ) -> Result<Response<MulticallResponse>, Status> {
        Err(Status::unimplemented("not needed by the rt-tui-client tests"))
    }

    async fn list_commands(
        &self,
        _request: Request<Empty>,
    ) -> Result<Response<ListCommandsResponse>, Status> {
        Err(Status::unimplemented("not needed by the rt-tui-client tests"))
    }
}

fn services(daemon: MockDaemon) -> Router {
    Server::builder()
        .add_service(TorrentServiceServer::new(daemon.clone()))
        .add_service(SystemServiceServer::new(daemon.clone()))
        .add_service(CommandServiceServer::new(daemon))
}

/// Starts the mock daemon on an ephemeral loopback port and returns its endpoint.
pub async fn spawn_tcp_daemon(daemon: MockDaemon) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind loopback listener");
    let port = listener.local_addr().expect("listener address").port();
    tokio::spawn(services(daemon).serve_with_incoming(TcpListenerStream::new(listener)));
    format!("127.0.0.1:{port}")
}

/// Starts the mock daemon on a Unix socket, returning its endpoint and path.
pub async fn spawn_unix_daemon(daemon: MockDaemon) -> (String, PathBuf) {
    let path = unique_socket_path();
    let listener = tokio::net::UnixListener::bind(&path).expect("bind unix listener");
    tokio::spawn(services(daemon).serve_with_incoming(UnixListenerStream::new(listener)));
    (format!("unix://{}", path.display()), path)
}

fn unique_socket_path() -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or_default();
    std::env::temp_dir().join(format!(
        "rt-tui-client-bridge-{}-{nanos}.sock",
        std::process::id()
    ))
}
