use lt_common::Object;
use rt_grpc::{GrpcContext, GrpcServer};
use rt_rpc::command::flags::{FLAG_IS_REDIRECT, FLAG_PUBLIC_RPC, FLAG_UNTRUSTED_SAFE};
use rt_rpc::{CommandMap, RpcError};
use std::net::SocketAddr;
use std::path::PathBuf;
use tokio::sync::watch;
use tracing::{error, info};

fn extract_string_param(obj: &Object) -> Result<String, RpcError> {
    if let Ok(s) = obj.as_str() {
        return Ok(s.trim().to_string());
    }
    if let Ok(v) = obj.as_value() {
        return Ok(v.to_string());
    }
    if let Ok(list) = obj.as_list() {
        if !list.is_empty() {
            return extract_string_param(&list[0]);
        }
    }
    Err(RpcError::InputError("Invalid parameter".into()))
}

pub fn register_grpc_commands(
    map: &mut CommandMap,
    grpc_context: GrpcContext,
    shutdown_rx: watch::Receiver<bool>,
) -> Result<(), RpcError> {
    let ctx_tcp = grpc_context.clone();
    let rx_tcp = shutdown_rx.clone();
    map.insert_func(
        "network.grpc.open_port",
        move |_target, obj| {
            let addr_input = extract_string_param(obj)
                .map_err(|_| RpcError::InputError("Invalid gRPC port parameter".into()))?;

            let addr_str = if addr_input.starts_with(':') {
                format!("0.0.0.0{}", addr_input)
            } else if addr_input.parse::<u16>().is_ok() {
                format!("0.0.0.0:{}", addr_input)
            } else {
                addr_input
            };

            let addr: SocketAddr = addr_str.parse().map_err(|e| {
                RpcError::InputError(format!("Invalid socket address {}: {}", addr_str, e))
            })?;

            let server = GrpcServer::new(ctx_tcp.clone());
            let rx = rx_tcp.clone();

            tokio::spawn(async move {
                info!("Starting gRPC TCP server on {}", addr);
                if let Err(e) = server.run_tcp(addr, rx).await {
                    error!("gRPC TCP server error: {}", e);
                }
            });

            Ok(Object::from(0i64))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE | FLAG_IS_REDIRECT,
        "<port|ip:port>",
        "Open gRPC TCP port.",
    )?;

    map.create_redirect("grpc_port", "network.grpc.open_port", 0)?;

    let ctx_unix = grpc_context.clone();
    let rx_unix = shutdown_rx;
    map.insert_func(
        "network.grpc.open_local",
        move |_target, obj| {
            let path_str = extract_string_param(obj)
                .map_err(|_| RpcError::InputError("Invalid gRPC socket path parameter".into()))?;

            if path_str.is_empty() {
                return Err(RpcError::InputError("Empty gRPC socket path".into()));
            }

            let path = if let Some(stripped) = path_str.strip_prefix("~/") {
                if let Ok(home) = std::env::var("HOME") {
                    PathBuf::from(home).join(stripped)
                } else {
                    PathBuf::from(path_str)
                }
            } else {
                PathBuf::from(path_str)
            };

            let server = GrpcServer::new(ctx_unix.clone());
            let rx = rx_unix.clone();

            tokio::spawn(async move {
                #[cfg(unix)]
                {
                    info!("Starting gRPC Unix socket server on {}", path.display());
                    if let Err(e) = server.run_unix(path, rx).await {
                        error!("gRPC Unix socket server error: {}", e);
                    }
                }
                #[cfg(not(unix))]
                error!("Unix domain sockets are not supported on this platform");
            });

            Ok(Object::from(0i64))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE | FLAG_IS_REDIRECT,
        "<socket_path>",
        "Open gRPC Unix domain socket.",
    )?;

    map.create_redirect("grpc_local", "network.grpc.open_local", 0)?;

    Ok(())
}
