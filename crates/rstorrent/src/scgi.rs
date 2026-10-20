use lt_common::Object;
use rt_rpc::command::flags::{FLAG_IS_REDIRECT, FLAG_PUBLIC_RPC, FLAG_UNTRUSTED_SAFE};
use rt_rpc::{
    convert_to_string, convert_to_value, CommandMap, JsonRpc, RpcError, XmlRpc, MAX_CONTENT_SIZE,
    MIN_SIZE_LIMIT,
};
use rt_scgi::{ScgiConfig, ScgiHandler, ScgiServer};
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
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

pub fn register_scgi_commands(
    map: &mut CommandMap,
    command_map_arc: Arc<RwLock<CommandMap>>,
) -> Result<(), RpcError> {
    let xmlrpc = Arc::new(XmlRpc::new());
    let xmlrpc_tcp = Arc::clone(&xmlrpc);
    let cmd_map_for_tcp = Arc::clone(&command_map_arc);
    map.insert_func(
        "network.scgi.open_port",
        move |_target, obj| {
            let addr_input = extract_string_param(obj)
                .map_err(|_| RpcError::InputError("Invalid SCGI port parameter".into()))?;

            let addr_str = if addr_input.starts_with(':') {
                format!("0.0.0.0{}", addr_input)
            } else if addr_input.parse::<u16>().is_ok() {
                format!("0.0.0.0:{}", addr_input)
            } else {
                addr_input
            };

            let std_listener = std::net::TcpListener::bind(&addr_str).map_err(|e| {
                RpcError::InputError(format!(
                    "Could not bind SCGI TCP address {}: {}",
                    addr_str, e
                ))
            })?;

            let handler = Arc::new(ScgiHandler::new(
                Arc::clone(&cmd_map_for_tcp),
                Arc::clone(&xmlrpc_tcp),
                Arc::new(JsonRpc::new()),
                ScgiConfig::default(),
            ));

            let server = ScgiServer::from_std_tcp(std_listener, handler).map_err(|e| {
                RpcError::InputError(format!("Failed to initialize SCGI TCP listener: {}", e))
            })?;

            tokio::spawn(async move {
                info!("Starting SCGI TCP server on {}", addr_str);
                if let Err(e) = server.run().await {
                    error!("SCGI TCP server error: {}", e);
                }
            });

            Ok(Object::from(0i64))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE | FLAG_IS_REDIRECT,
        "",
        "Open SCGI TCP port.",
    )?;

    map.create_redirect("scgi_port", "network.scgi.open_port", 0)?;

    let xmlrpc_unix = Arc::clone(&xmlrpc);
    let cmd_map_for_unix = Arc::clone(&command_map_arc);
    map.insert_func(
        "network.scgi.open_local",
        move |_target, obj| {
            let path_str = extract_string_param(obj)
                .map_err(|_| RpcError::InputError("Invalid SCGI socket path parameter".into()))?;

            if path_str.is_empty() {
                return Err(RpcError::InputError("Empty SCGI socket path".into()));
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

            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if path.exists() {
                let _ = std::fs::remove_file(&path);
            }

            let std_listener = std::os::unix::net::UnixListener::bind(&path).map_err(|e| {
                RpcError::InputError(format!("Could not bind SCGI Unix socket {:?}: {}", path, e))
            })?;

            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o666));
            }

            let handler = Arc::new(ScgiHandler::new(
                Arc::clone(&cmd_map_for_unix),
                Arc::clone(&xmlrpc_unix),
                Arc::new(JsonRpc::new()),
                ScgiConfig::default(),
            ));

            let server =
                ScgiServer::from_std_unix(std_listener, path.clone(), handler).map_err(|e| {
                    RpcError::InputError(format!("Failed to initialize SCGI Unix listener: {}", e))
                })?;

            tokio::spawn(async move {
                info!("Starting SCGI Unix server on {:?}", path);
                if let Err(e) = server.run().await {
                    error!("SCGI Unix server error: {}", e);
                }
            });

            Ok(Object::from(0i64))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE | FLAG_IS_REDIRECT,
        "",
        "Open SCGI Unix socket.",
    )?;

    map.create_redirect("scgi_local", "network.scgi.open_local", 0)?;

    let xmlrpc_systemd = Arc::clone(&xmlrpc);
    let cmd_map_for_systemd = Arc::clone(&command_map_arc);
    map.insert_func(
        "network.scgi.open_systemd",
        move |_target, _obj| {
            let handler = Arc::new(ScgiHandler::new(
                Arc::clone(&cmd_map_for_systemd),
                Arc::clone(&xmlrpc_systemd),
                Arc::new(JsonRpc::new()),
                ScgiConfig::default(),
            ));

            let server = ScgiServer::from_systemd(handler).map_err(|e| {
                RpcError::InputError(format!("Failed to get systemd socket: {}", e))
            })?;

            tokio::spawn(async move {
                info!("Starting SCGI systemd server");
                if let Err(e) = server.run().await {
                    error!("SCGI systemd server error: {}", e);
                }
            });

            Ok(Object::from(0i64))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "Open SCGI systemd socket.",
    )?;

    let xmlrpc_limit = Arc::clone(&xmlrpc);
    map.insert_func(
        "network.xmlrpc.size_limit",
        move |_target, _obj| Ok(Object::from(xmlrpc_limit.size_limit() as i64)),
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "Maximum XML-RPC request size.",
    )?;

    let xmlrpc_limit_set = Arc::clone(&xmlrpc);
    map.insert_func(
        "network.xmlrpc.size_limit.set",
        move |_target, obj| {
            let value = convert_to_value(obj, 10, 1)
                .map_err(|_| RpcError::InputError("Invalid XML-RPC size limit.".into()))?;
            if value < MIN_SIZE_LIMIT as i64 || value > MAX_CONTENT_SIZE as i64 {
                return Err(RpcError::InputError(
                    "XML-RPC size limit is outside the allowed range.".into(),
                ));
            }
            xmlrpc_limit_set.set_size_limit(value as u64);
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "<bytes>",
        "Set the maximum XML-RPC request size.",
    )?;

    let xmlrpc_dialect = Arc::clone(&xmlrpc);
    map.insert_func(
        "network.xmlrpc.dialect",
        move |_target, _obj| {
            Ok(Object::from(match xmlrpc_dialect.dialect() {
                1 => "i8",
                2 => "apache",
                _ => "generic",
            }))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "Current XML-RPC dialect.",
    )?;

    let xmlrpc_dialect_set = Arc::clone(&xmlrpc);
    map.insert_func(
        "network.xmlrpc.dialect.set",
        move |_target, obj| {
            let value = convert_to_string(obj)?;
            let dialect = match value.trim() {
                "i8" => 1,
                "apache" => 2,
                "generic" => 0,
                _ => {
                    return Err(RpcError::InputError(format!(
                        "Unsupported XML-RPC dialect '{value}'."
                    )))
                }
            };
            xmlrpc_dialect_set.set_dialect(dialect);
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "<i8|apache|generic>",
        "Select the XML-RPC dialect.",
    )?;

    map.insert_func(
        "network.scgi.dont_route",
        |_target, _obj| Ok(Object::from(0i64)),
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "SCGI don't route flag.",
    )?;

    Ok(())
}
