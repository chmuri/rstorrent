use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Instant;
use tokio::sync::{broadcast, watch};
use tonic::transport::Server;
use tracing::info;

use crate::command_service::CommandServiceImpl;
use crate::conversion::download_to_summary;
use crate::proto::command_service_server::CommandServiceServer;
use crate::proto::system_service_server::SystemServiceServer;
use crate::proto::torrent_service_server::TorrentServiceServer;
use crate::proto::TorrentEvent;
use crate::system_service::SystemServiceImpl;
use crate::torrent_service::TorrentServiceImpl;

use rt_core::session::SessionManager;
use rt_core::{DownloadList, ViewManager};
use rt_rpc::CommandMap;

/// Large `CommandService.Multicall` batches and torrent payloads exceed the
/// tonic defaults (~4 MiB), so both directions are raised to 64 MiB.
const MAX_MESSAGE_SIZE: usize = 64 * 1024 * 1024;

fn build_services(
    ctx: &GrpcContext,
) -> (
    TorrentServiceServer<TorrentServiceImpl>,
    SystemServiceServer<SystemServiceImpl>,
    CommandServiceServer<CommandServiceImpl>,
) {
    (
        TorrentServiceServer::new(TorrentServiceImpl::new(ctx.clone()))
            .max_decoding_message_size(MAX_MESSAGE_SIZE)
            .max_encoding_message_size(MAX_MESSAGE_SIZE),
        SystemServiceServer::new(SystemServiceImpl::new(ctx.clone()))
            .max_decoding_message_size(MAX_MESSAGE_SIZE)
            .max_encoding_message_size(MAX_MESSAGE_SIZE),
        CommandServiceServer::new(CommandServiceImpl::new(ctx.clone()))
            .max_decoding_message_size(MAX_MESSAGE_SIZE)
            .max_encoding_message_size(MAX_MESSAGE_SIZE),
    )
}

#[derive(Clone)]
pub struct GrpcContext {
    pub command_map: Arc<RwLock<CommandMap>>,
    pub download_list: Arc<RwLock<DownloadList>>,
    pub view_manager: Arc<RwLock<ViewManager>>,
    pub session_manager: Arc<Mutex<SessionManager>>,
    pub shutdown_tx: watch::Sender<bool>,
    pub start_time: Instant,
    pub event_broadcaster: broadcast::Sender<TorrentEvent>,
}

impl GrpcContext {
    pub fn new(
        command_map: Arc<RwLock<CommandMap>>,
        download_list: Arc<RwLock<DownloadList>>,
        view_manager: Arc<RwLock<ViewManager>>,
        session_manager: Arc<Mutex<SessionManager>>,
        shutdown_tx: watch::Sender<bool>,
    ) -> Self {
        let (broadcast_tx, _) = broadcast::channel(256);

        // Hook a lifecycle event listener into download_list
        let event_tx = broadcast_tx.clone();
        download_list
            .write()
            .unwrap()
            .add_event_listener(Arc::new(move |event, download| {
                let guard = download.read().unwrap();
                let summary = download_to_summary(&guard);
                let proto_event = TorrentEvent {
                    event_name: event.to_string(),
                    hash: guard.info_hash_hex(),
                    summary: Some(summary),
                    timestamp: std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs(),
                };
                let _ = event_tx.send(proto_event);
            }));

        Self {
            command_map,
            download_list,
            view_manager,
            session_manager,
            shutdown_tx,
            start_time: Instant::now(),
            event_broadcaster: broadcast_tx,
        }
    }
}

pub struct GrpcServer {
    ctx: GrpcContext,
}

impl GrpcServer {
    pub fn new(ctx: GrpcContext) -> Self {
        Self { ctx }
    }

    pub async fn run_tcp(
        self,
        addr: SocketAddr,
        mut shutdown_rx: watch::Receiver<bool>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        info!("Starting gRPC TCP server on {}", addr);

        let (torrent_svc, system_svc, command_svc) = build_services(&self.ctx);

        Server::builder()
            .add_service(torrent_svc)
            .add_service(system_svc)
            .add_service(command_svc)
            .serve_with_shutdown(addr, async move {
                while !*shutdown_rx.borrow_and_update() {
                    if shutdown_rx.changed().await.is_err() {
                        break;
                    }
                }
                info!("gRPC TCP server received shutdown signal");
            })
            .await?;

        Ok(())
    }

    #[cfg(unix)]
    pub async fn run_unix(
        self,
        path: PathBuf,
        mut shutdown_rx: watch::Receiver<bool>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        use std::os::unix::fs::PermissionsExt;
        use tokio::net::UnixListener;
        use tokio_stream::wrappers::UnixListenerStream;

        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if path.exists() {
            let _ = std::fs::remove_file(&path);
        }

        let listener = UnixListener::bind(&path)?;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o666));
        info!("Starting gRPC Unix domain socket server on {:?}", path);

        let stream = UnixListenerStream::new(listener);

        let (torrent_svc, system_svc, command_svc) = build_services(&self.ctx);

        let path_cleanup = path.clone();
        Server::builder()
            .add_service(torrent_svc)
            .add_service(system_svc)
            .add_service(command_svc)
            .serve_with_incoming_shutdown(stream, async move {
                while !*shutdown_rx.borrow_and_update() {
                    if shutdown_rx.changed().await.is_err() {
                        break;
                    }
                }
                info!("gRPC Unix socket server received shutdown signal");
            })
            .await?;

        let _ = std::fs::remove_file(&path_cleanup);
        Ok(())
    }
}
