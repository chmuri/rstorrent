use crate::conversion::{object_to_proto, proto_to_object};
use crate::proto::command_service_server::CommandService;
use crate::proto::*;
use crate::server::GrpcContext;
use rt_rpc::Target;
use std::sync::Arc;
use tonic::{Request, Response, Status};

pub struct CommandServiceImpl {
    ctx: GrpcContext,
}

impl CommandServiceImpl {
    pub fn new(ctx: GrpcContext) -> Self {
        Self { ctx }
    }
}

#[tonic::async_trait]
impl CommandService for CommandServiceImpl {
    async fn execute(
        &self,
        request: Request<CommandRequest>,
    ) -> Result<Response<CommandResponse>, Status> {
        let trusted = request
            .extensions()
            .get::<crate::security::CallerTrusted>()
            .map(|t| t.0)
            .unwrap_or(false);
        let req = request.into_inner();
        let target = if req.target_hash.is_empty() {
            Target::None
        } else {
            match lt_common::HashString::from_hex(&req.target_hash) {
                Some(hash) => Target::Download(hash),
                None => {
                    return Ok(Response::new(CommandResponse {
                        success: false,
                        result: None,
                        error_message: format!("Invalid target download hash {}", req.target_hash),
                    }));
                }
            }
        };

        let arg = if req.args.is_empty() {
            lt_common::Object::create_empty()
        } else if req.args.len() == 1 {
            proto_to_object(&req.args[0])
        } else {
            let list: Vec<lt_common::Object> = req.args.iter().map(proto_to_object).collect();
            lt_common::Object::create_list_with(list)
        };

        // Command execution can block (fsync on pause/close, file I/O), so run
        // it on a blocking worker instead of the Tokio reactor.
        let map = Arc::clone(&self.ctx.command_map);
        let command = req.command;
        let response = tokio::task::spawn_blocking(move || {
            let map = map.read().unwrap();
            // Trust is the server-decided connection level (local Unix socket or
            // an authenticated TCP peer); never taken from the client payload.
            match map.call_command(&command, &arg, &target, trusted) {
                Ok(result_obj) => CommandResponse {
                    success: true,
                    result: Some(object_to_proto(&result_obj)),
                    error_message: String::new(),
                },
                Err(e) => CommandResponse {
                    success: false,
                    result: None,
                    error_message: e.to_string(),
                },
            }
        })
        .await
        .map_err(|e| Status::internal(format!("Command task failed: {}", e)))?;
        Ok(Response::new(response))
    }

    async fn multicall(
        &self,
        request: Request<MulticallRequest>,
    ) -> Result<Response<MulticallResponse>, Status> {
        let trusted = request
            .extensions()
            .get::<crate::security::CallerTrusted>()
            .map(|t| t.0)
            .unwrap_or(false);
        let req = request.into_inner();
        let mut results = Vec::with_capacity(req.calls.len());
        for call in req.calls {
            let target = if call.target_hash.is_empty() {
                Target::None
            } else {
                match lt_common::HashString::from_hex(&call.target_hash) {
                    Some(hash) => Target::Download(hash),
                    None => {
                        results.push(CommandResponse {
                            success: false,
                            result: None,
                            error_message: format!(
                                "Invalid target download hash {}",
                                call.target_hash
                            ),
                        });
                        continue;
                    }
                }
            };

            let arg = if call.args.is_empty() {
                lt_common::Object::create_empty()
            } else if call.args.len() == 1 {
                proto_to_object(&call.args[0])
            } else {
                let list: Vec<lt_common::Object> = call.args.iter().map(proto_to_object).collect();
                lt_common::Object::create_list_with(list)
            };

            let map = Arc::clone(&self.ctx.command_map);
            let command = call.command;
            let response = tokio::task::spawn_blocking(move || {
                let map = map.read().unwrap();
                match map.call_command(&command, &arg, &target, trusted) {
                    Ok(res) => CommandResponse {
                        success: true,
                        result: Some(object_to_proto(&res)),
                        error_message: String::new(),
                    },
                    Err(e) => CommandResponse {
                        success: false,
                        result: None,
                        error_message: e.to_string(),
                    },
                }
            })
            .await
            .map_err(|e| Status::internal(format!("Command task failed: {}", e)))?;
            results.push(response);
        }

        Ok(Response::new(MulticallResponse { results }))
    }

    async fn list_commands(
        &self,
        _request: Request<Empty>,
    ) -> Result<Response<ListCommandsResponse>, Status> {
        let map = self.ctx.command_map.read().unwrap();
        let commands = map.iter().map(|(name, _)| name.clone()).collect();
        Ok(Response::new(ListCommandsResponse { commands }))
    }
}
