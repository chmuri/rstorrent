use crate::proto::system_service_server::SystemService;
use crate::proto::*;
use crate::server::GrpcContext;
use futures_util::Stream;
use std::pin::Pin;
use std::time::Duration;
use tokio_stream::wrappers::IntervalStream;
use tokio_stream::StreamExt;
use tonic::{Request, Response, Status};

pub struct SystemServiceImpl {
    ctx: GrpcContext,
}

impl SystemServiceImpl {
    pub fn new(ctx: GrpcContext) -> Self {
        Self { ctx }
    }

    fn calculate_stats(&self) -> SessionStatsResponse {
        let dl_list = self.ctx.download_list.read().unwrap();
        let mut up_rate: i64 = 0;
        let mut down_rate: i64 = 0;
        let mut up_total: i64 = 0;
        let mut down_total: i64 = 0;
        let mut active_count: u32 = 0;
        let total_count = dl_list.len() as u32;

        for dl in dl_list.all() {
            let guard = dl.read().unwrap();
            up_rate += guard.rate_up() as i64;
            down_rate += guard.rate_down() as i64;
            up_total += guard.uploaded_bytes() as i64;
            down_total += guard.downloaded_bytes() as i64;
            if guard.is_active() {
                active_count += 1;
            }
        }

        SessionStatsResponse {
            up_rate_bytes_per_sec: up_rate,
            down_rate_bytes_per_sec: down_rate,
            up_total_bytes: up_total,
            down_total_bytes: down_total,
            active_torrents: active_count,
            total_torrents: total_count,
            listen_port: 0,
            throttle_up_limit: 0,
            throttle_down_limit: 0,
        }
    }
}

#[tonic::async_trait]
impl SystemService for SystemServiceImpl {
    async fn get_version(
        &self,
        _request: Request<Empty>,
    ) -> Result<Response<VersionResponse>, Status> {
        let uptime = self.ctx.start_time.elapsed().as_secs();
        Ok(Response::new(VersionResponse {
            version: env!("CARGO_PKG_VERSION").to_string(),
            api_version: lt_common::version::VERSION.to_string(),
            build_target: std::env::consts::ARCH.to_string(),
            uptime_seconds: uptime,
        }))
    }

    async fn get_session_stats(
        &self,
        _request: Request<Empty>,
    ) -> Result<Response<SessionStatsResponse>, Status> {
        Ok(Response::new(self.calculate_stats()))
    }

    type StreamStatsStream =
        Pin<Box<dyn Stream<Item = Result<SessionStatsResponse, Status>> + Send + 'static>>;

    async fn stream_stats(
        &self,
        request: Request<StreamStatsRequest>,
    ) -> Result<Response<Self::StreamStatsStream>, Status> {
        let interval_ms = request.into_inner().interval_ms;
        let interval_ms = if interval_ms == 0 { 1000 } else { interval_ms };
        let interval = tokio::time::interval(Duration::from_millis(interval_ms as u64));
        let ctx = self.ctx.clone();

        let stream = IntervalStream::new(interval).map(move |_| {
            let service = SystemServiceImpl::new(ctx.clone());
            Ok(service.calculate_stats())
        });

        Ok(Response::new(Box::pin(stream)))
    }

    async fn shutdown(
        &self,
        _request: Request<ShutdownRequest>,
    ) -> Result<Response<ShutdownResponse>, Status> {
        let _ = self.ctx.shutdown_tx.send(true);
        Ok(Response::new(ShutdownResponse {
            acknowledged: true,
            message: "Shutdown sequence initiated".to_string(),
        }))
    }
}
