use std::pin::Pin;
use std::sync::Arc;
use futures_util::Stream;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt;
use tonic::{Request, Response, Status};
use crate::proto::torrent_service_server::TorrentService;
use crate::proto::*;
use crate::conversion::{download_to_details, download_to_summary, download_state_to_proto};
use crate::server::GrpcContext;
use rt_core::DownloadFactory;
use lt_common::HashString;

pub struct TorrentServiceImpl {
    ctx: GrpcContext,
}

impl TorrentServiceImpl {
    pub fn new(ctx: GrpcContext) -> Self {
        Self { ctx }
    }
}

#[tonic::async_trait]
impl TorrentService for TorrentServiceImpl {
    async fn list_torrents(
        &self,
        request: Request<ListTorrentsRequest>,
    ) -> Result<Response<ListTorrentsResponse>, Status> {
        let req = request.into_inner();
        let dl_list = self.ctx.download_list.read().unwrap();

        let downloads = if !req.view.is_empty() {
            let vm = self.ctx.view_manager.read().unwrap();
            if let Some(view) = vm.find(&req.view) {
                view.all_downloads().to_vec()
            } else {
                dl_list.all()
            }
        } else {
            dl_list.all()
        };

        let filter_hashes: Option<std::collections::HashSet<String>> = if !req.hashes.is_empty() {
            Some(req.hashes.into_iter().map(|h| h.to_lowercase()).collect())
        } else {
            None
        };

        let mut torrents = Vec::new();
        for dl in downloads {
            let guard = dl.read().unwrap();
            let hash = guard.info_hash_hex().to_lowercase();
            if let Some(ref filter) = filter_hashes {
                if !filter.contains(&hash) {
                    continue;
                }
            }
            torrents.push(download_to_summary(&guard));
        }

        Ok(Response::new(ListTorrentsResponse { torrents }))
    }

    async fn get_torrent(
        &self,
        request: Request<GetTorrentRequest>,
    ) -> Result<Response<TorrentDetails>, Status> {
        let req = request.into_inner();
        let dl_list = self.ctx.download_list.read().unwrap();
        let dl = dl_list
            .find_hex(&req.hash)
            .ok_or_else(|| Status::not_found(format!("Torrent with hash {} not found", req.hash)))?;

        let guard = dl.read().unwrap();
        Ok(Response::new(download_to_details(&guard)))
    }

    async fn add_torrent(
        &self,
        request: Request<AddTorrentRequest>,
    ) -> Result<Response<AddTorrentResponse>, Status> {
        let req = request.into_inner();
        let mut factory = DownloadFactory::new();
        factory.set_start(req.start_now);

        let load_res = match req.source {
            Some(add_torrent_request::Source::TorrentFile(bytes)) => {
                factory.load_raw_data(&bytes)
            }
            Some(add_torrent_request::Source::FilePath(path)) => {
                factory.load_file(&path)
            }
            Some(add_torrent_request::Source::Url(url)) => {
                // If URL is provided, try fetching or fail if not supported inline
                return Ok(Response::new(AddTorrentResponse {
                    hash: String::new(),
                    name: String::new(),
                    success: false,
                    error_message: format!("Loading directly from HTTP URL '{}' is not supported; pass torrent_file bytes instead", url),
                }));
            }
            Some(add_torrent_request::Source::MagnetUri(uri)) => {
                return Ok(Response::new(AddTorrentResponse {
                    hash: String::new(),
                    name: String::new(),
                    success: false,
                    error_message: format!("Direct magnet link '{}' parsing without metadata requires dht/pex lookup", uri),
                }));
            }
            None => {
                return Err(Status::invalid_argument("Missing torrent source"));
            }
        };

        if let Err(e) = load_res {
            return Ok(Response::new(AddTorrentResponse {
                hash: String::new(),
                name: String::new(),
                success: false,
                error_message: format!("Failed to parse torrent data: {}", e),
            }));
        }

        let dl_arc = {
            let mut dl_list = self.ctx.download_list.write().unwrap();
            match factory.commit(&mut dl_list) {
                Ok(dl) => dl,
                Err(e) => {
                    return Ok(Response::new(AddTorrentResponse {
                        hash: String::new(),
                        name: String::new(),
                        success: false,
                        error_message: format!("Failed to commit torrent: {}", e),
                    }));
                }
            }
        };

        if !req.destination_directory.is_empty() {
            dl_arc.write().unwrap().set_directory(req.destination_directory);
        }

        {
            let mut vm = self.ctx.view_manager.write().unwrap();
            vm.insert_download_to_all(Arc::clone(&dl_arc));
        }

        let (hash, name) = {
            let guard = dl_arc.read().unwrap();
            (guard.info_hash_hex(), guard.name().to_string())
        };

        Ok(Response::new(AddTorrentResponse {
            hash,
            name,
            success: true,
            error_message: String::new(),
        }))
    }

    async fn control_torrent(
        &self,
        request: Request<ControlTorrentRequest>,
    ) -> Result<Response<ControlTorrentResponse>, Status> {
        let req = request.into_inner();
        let dl = {
            let dl_list = self.ctx.download_list.read().unwrap();
            dl_list.find_hex(&req.hash)
        };

        let dl = match dl {
            Some(d) => d,
            None => {
                return Ok(Response::new(ControlTorrentResponse {
                    success: false,
                    hash: req.hash,
                    current_state: TorrentState::Unspecified.into(),
                    error_message: "Torrent not found".to_string(),
                }));
            }
        };

        let action = req.action();
        let mut err_msg = String::new();
        let success = match action {
            TorrentAction::Start => {
                let mut guard = dl.write().unwrap();
                let _ = guard.open();
                match guard.try_start() {
                    Ok(_) => true,
                    Err(e) => {
                        err_msg = e;
                        false
                    }
                }
            }
            TorrentAction::Stop => {
                let mut guard = dl.write().unwrap();
                guard.stop();
                true
            }
            TorrentAction::Pause => {
                let mut guard = dl.write().unwrap();
                guard.pause();
                true
            }
            TorrentAction::Resume => {
                let mut guard = dl.write().unwrap();
                guard.resume();
                true
            }
            TorrentAction::Close => {
                let mut guard = dl.write().unwrap();
                guard.close();
                true
            }
            TorrentAction::CheckHash => {
                let mut guard = dl.write().unwrap();
                guard.check_hash();
                true
            }
            TorrentAction::Unspecified => {
                err_msg = "Unspecified torrent action".to_string();
                false
            }
        };

        let current_state = {
            let guard = dl.read().unwrap();
            let (proto_state, _) = download_state_to_proto(&guard.state());
            proto_state.into()
        };

        Ok(Response::new(ControlTorrentResponse {
            success,
            hash: req.hash,
            current_state,
            error_message: err_msg,
        }))
    }

    async fn remove_torrent(
        &self,
        request: Request<RemoveTorrentRequest>,
    ) -> Result<Response<RemoveTorrentResponse>, Status> {
        let req = request.into_inner();
        let hash = match HashString::from_hex(&req.hash) {
            Some(h) => h,
            None => {
                return Ok(Response::new(RemoveTorrentResponse {
                    success: false,
                    hash: req.hash,
                    error_message: "Invalid info hash".to_string(),
                }));
            }
        };

        let erased = {
            let mut dl_list = self.ctx.download_list.write().unwrap();
            dl_list.erase(&hash)
        };

        if erased.is_some() {
            let _ = self.ctx.session_manager.lock().unwrap().erase_entry(&req.hash);
            Ok(Response::new(RemoveTorrentResponse {
                success: true,
                hash: req.hash,
                error_message: String::new(),
            }))
        } else {
            Ok(Response::new(RemoveTorrentResponse {
                success: false,
                hash: req.hash,
                error_message: "Torrent not found or already erasing".to_string(),
            }))
        }
    }

    async fn set_torrent_priority(
        &self,
        request: Request<SetTorrentPriorityRequest>,
    ) -> Result<Response<SetPriorityResponse>, Status> {
        let req = request.into_inner();
        let dl = {
            let dl_list = self.ctx.download_list.read().unwrap();
            dl_list.find_hex(&req.hash)
        };

        if let Some(dl) = dl {
            dl.write().unwrap().set_priority(req.priority);
            Ok(Response::new(SetPriorityResponse {
                success: true,
                error_message: String::new(),
            }))
        } else {
            Ok(Response::new(SetPriorityResponse {
                success: false,
                error_message: "Torrent not found".to_string(),
            }))
        }
    }

    type StreamEventsStream = Pin<
        Box<dyn Stream<Item = Result<TorrentEvent, Status>> + Send + 'static>,
    >;

    async fn stream_events(
        &self,
        request: Request<StreamEventsRequest>,
    ) -> Result<Response<Self::StreamEventsStream>, Status> {
        let req = request.into_inner();
        let filter_set: Option<std::collections::HashSet<String>> = if !req.event_filter.is_empty() {
            Some(req.event_filter.into_iter().collect())
        } else {
            None
        };

        let rx = self.ctx.event_broadcaster.subscribe();
        let stream = BroadcastStream::new(rx).filter_map(move |item| {
            match item {
                Ok(event) => {
                    if let Some(ref filter) = filter_set {
                        if !filter.contains(&event.event_name) {
                            return None;
                        }
                    }
                    Some(Ok(event))
                }
                Err(_) => None, // skipped message
            }
        });

        Ok(Response::new(Box::pin(stream)))
    }
}
