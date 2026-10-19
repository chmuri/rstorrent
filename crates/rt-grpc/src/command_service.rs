use tonic::{Request, Response, Status};
use crate::proto::command_service_server::CommandService;
use crate::proto::*;
use crate::conversion::{object_to_proto, proto_to_object};
use crate::server::GrpcContext;
use rt_rpc::Target;

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
                        error_message: format!(
                            "Invalid target download hash {}",
                            req.target_hash
                        ),
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

        let map = self.ctx.command_map.read().unwrap();
        match map.call_command(&req.command, &arg, &target, req.trusted) {
            Ok(result_obj) => Ok(Response::new(CommandResponse {
                success: true,
                result: Some(object_to_proto(&result_obj)),
                error_message: String::new(),
            })),
            Err(e) => Ok(Response::new(CommandResponse {
                success: false,
                result: None,
                error_message: e.to_string(),
            })),
        }
    }

    async fn multicall(
        &self,
        request: Request<MulticallRequest>,
    ) -> Result<Response<MulticallResponse>, Status> {
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

            let map = self.ctx.command_map.read().unwrap();
            match map.call_command(&call.command, &arg, &target, call.trusted) {
                Ok(res) => {
                    results.push(CommandResponse {
                        success: true,
                        result: Some(object_to_proto(&res)),
                        error_message: String::new(),
                    });
                }
                Err(e) => {
                    results.push(CommandResponse {
                        success: false,
                        result: None,
                        error_message: e.to_string(),
                    });
                }
            }
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
