//! `network.*` configuration commands.
//!
//! Bind addresses, ports, buffers, blocking and encryption-adjacent settings
//! are backed by [`lt_net::config::NetworkConfig`]. Values that have no
//! counterpart in the Rust networking layer yet (HTTP stack policy, proxy
//! strings, RPC protocol toggles and the listen-port range) are kept in this
//! module's configuration state. They are deliberately accepted at runtime but
//! are **not** applied to an already running listener; changing a bind address,
//! local port or listen port takes effect for newly created sockets and the
//! listener itself requires a restart (the daemon supervisor owns the sockets).
//!
//! The HTTP/RPC/SCGI values mirror the legacy command surface. They are exposed
//! for rc compatibility and inspection; the `rt-core` command layer does not
//! own the HTTP stack or SCGI listeners, so a change is a configuration
//! snapshot rather than a live reconfiguration. Commands that have no runtime
//! source are documented as such and never expose a setter.

use std::sync::{Arc, RwLock};

use lt_common::Object;
use lt_net::config::NetworkConfig;
use lt_net::socket_manager::calculate_alloc_limit;
use lt_net::SocketCategory;
use lt_storage::disk_thread::global_file_manager_stats;
use rt_rpc::command_flags::*;
use rt_rpc::{convert_to_string, convert_to_value, CommandMap, RpcError};

use super::pieces::parse_byte_size;
use super::system::socket_manager;

#[derive(Debug, Clone)]
struct NetworkExtConfig {
    listen_port: i64,
    listen_port_first: u16,
    listen_port_last: u16,
    listen_port_random: bool,

    proxy_global: String,
    proxy_http: String,

    http_cacert: String,
    http_capath: String,
    http_dns_cache_timeout: i64,
    http_max_cache_connections: i64,
    http_max_host_connections: i64,
    http_max_total_connections: i64,
    http_ssl_verify_host: bool,
    http_ssl_verify_peer: bool,

    rpc_use_xmlrpc: bool,
    rpc_use_jsonrpc: bool,

    scgi_use_gzip: bool,
    scgi_gzip_min_size: i64,

    max_open_files: i64,
}

impl Default for NetworkExtConfig {
    fn default() -> Self {
        Self {
            listen_port: 6881,
            listen_port_first: 6881,
            listen_port_last: 6999,
            listen_port_random: true,

            proxy_global: String::new(),
            proxy_http: String::new(),

            http_cacert: String::new(),
            http_capath: String::new(),
            http_dns_cache_timeout: 60,
            http_max_cache_connections: 0,
            http_max_host_connections: 0,
            http_max_total_connections: 32,
            http_ssl_verify_host: true,
            http_ssl_verify_peer: true,

            rpc_use_xmlrpc: true,
            rpc_use_jsonrpc: true,

            scgi_use_gzip: true,
            scgi_gzip_min_size: 1000,

            max_open_files: 500,
        }
    }
}

fn add_bool<G, S>(
    map: &mut CommandMap,
    config: &NetworkConfig,
    name: &str,
    get: G,
    set: S,
) -> Result<(), RpcError>
where
    G: Fn(&NetworkConfig) -> bool + Send + Sync + 'static,
    S: Fn(&NetworkConfig, bool) + Send + Sync + 'static,
{
    let get_config = config.clone();
    map.insert_func(
        name,
        move |_target, _obj| Ok(Object::from(if get(&get_config) { 1i64 } else { 0 })),
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        name,
    )?;

    let set_config = config.clone();
    map.insert_func(
        &format!("{name}.set"),
        move |_target, obj| {
            let value = convert_to_value(obj, 0, 1)
                .map_err(|_| RpcError::InputError("Not a value.".into()))?;
            set(&set_config, value != 0);
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "<0|1>",
        name,
    )
}

fn add_int<G, S>(
    map: &mut CommandMap,
    config: &NetworkConfig,
    name: &str,
    get: G,
    set: S,
) -> Result<(), RpcError>
where
    G: Fn(&NetworkConfig) -> i64 + Send + Sync + 'static,
    S: Fn(&NetworkConfig, i64) -> Result<(), String> + Send + Sync + 'static,
{
    let get_config = config.clone();
    map.insert_func(
        name,
        move |_target, _obj| Ok(Object::from(get(&get_config))),
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        name,
    )?;

    let set_config = config.clone();
    map.insert_func(
        &format!("{name}.set"),
        move |_target, obj| {
            let value = convert_to_value(obj, 0, 1)
                .map_err(|_| RpcError::InputError("Not a value.".into()))?;
            set(&set_config, value).map_err(RpcError::InputError)?;
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "<value>",
        name,
    )
}

fn add_byte_size<G, S>(
    map: &mut CommandMap,
    config: &NetworkConfig,
    name: &str,
    get: G,
    set: S,
) -> Result<(), RpcError>
where
    G: Fn(&NetworkConfig) -> i64 + Send + Sync + 'static,
    S: Fn(&NetworkConfig, u32) -> Result<(), String> + Send + Sync + 'static,
{
    let get_config = config.clone();
    map.insert_func(
        name,
        move |_target, _obj| Ok(Object::from(get(&get_config))),
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        name,
    )?;

    let set_config = config.clone();
    map.insert_func(
        &format!("{name}.set"),
        move |_target, obj| {
            let value = parse_byte_size(obj)?;
            if value > u32::MAX as u64 {
                return Err(RpcError::InputError(
                    "Buffer size is too large for this platform.".into(),
                ));
            }
            set(&set_config, value as u32).map_err(RpcError::InputError)?;
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "<bytes>",
        name,
    )
}

fn add_address<G, S>(
    map: &mut CommandMap,
    config: &NetworkConfig,
    name: &str,
    get: G,
    set: S,
) -> Result<(), RpcError>
where
    G: Fn(&NetworkConfig) -> String + Send + Sync + 'static,
    S: Fn(&NetworkConfig, &str) -> Result<(), String> + Send + Sync + 'static,
{
    let get_config = config.clone();
    map.insert_func(
        name,
        move |_target, _obj| Ok(Object::from(get(&get_config))),
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        name,
    )?;

    let set_config = config.clone();
    map.insert_func(
        &format!("{name}.set"),
        move |_target, obj| {
            let value = convert_to_string(obj)?;
            set(&set_config, &value).map_err(RpcError::InputError)?;
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "<address>",
        name,
    )
}

fn add_ext_bool<G, S>(
    map: &mut CommandMap,
    state: &Arc<RwLock<NetworkExtConfig>>,
    name: &str,
    get: G,
    set: S,
) -> Result<(), RpcError>
where
    G: Fn(&NetworkExtConfig) -> bool + Send + Sync + 'static,
    S: Fn(&mut NetworkExtConfig, bool) + Send + Sync + 'static,
{
    let get_state = Arc::clone(state);
    map.insert_func(
        name,
        move |_target, _obj| {
            Ok(Object::from(if get(&get_state.read().unwrap()) {
                1i64
            } else {
                0
            }))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        name,
    )?;

    let set_state = Arc::clone(state);
    map.insert_func(
        &format!("{name}.set"),
        move |_target, obj| {
            let value = convert_to_value(obj, 0, 1)
                .map_err(|_| RpcError::InputError("Not a value.".into()))?;
            set(&mut set_state.write().unwrap(), value != 0);
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "<0|1>",
        name,
    )
}

fn add_ext_int<G, S>(
    map: &mut CommandMap,
    state: &Arc<RwLock<NetworkExtConfig>>,
    name: &str,
    get: G,
    set: S,
) -> Result<(), RpcError>
where
    G: Fn(&NetworkExtConfig) -> i64 + Send + Sync + 'static,
    S: Fn(&mut NetworkExtConfig, i64) -> Result<(), String> + Send + Sync + 'static,
{
    let get_state = Arc::clone(state);
    map.insert_func(
        name,
        move |_target, _obj| Ok(Object::from(get(&get_state.read().unwrap()))),
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        name,
    )?;

    let set_state = Arc::clone(state);
    map.insert_func(
        &format!("{name}.set"),
        move |_target, obj| {
            let value = convert_to_value(obj, 0, 1)
                .map_err(|_| RpcError::InputError("Not a value.".into()))?;
            set(&mut set_state.write().unwrap(), value).map_err(RpcError::InputError)?;
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "<value>",
        name,
    )
}

fn add_ext_string<G, S>(
    map: &mut CommandMap,
    state: &Arc<RwLock<NetworkExtConfig>>,
    name: &str,
    get: G,
    set: S,
) -> Result<(), RpcError>
where
    G: Fn(&NetworkExtConfig) -> String + Send + Sync + 'static,
    S: Fn(&mut NetworkExtConfig, String) -> Result<(), String> + Send + Sync + 'static,
{
    let get_state = Arc::clone(state);
    map.insert_func(
        name,
        move |_target, _obj| Ok(Object::from(get(&get_state.read().unwrap()))),
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        name,
    )?;

    let set_state = Arc::clone(state);
    map.insert_func(
        &format!("{name}.set"),
        move |_target, obj| {
            let value = convert_to_string(obj)?;
            set(&mut set_state.write().unwrap(), value).map_err(RpcError::InputError)?;
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "<string>",
        name,
    )
}

fn checked_port(value: i64, label: &str) -> Result<u16, String> {
    if !(0..=u16::MAX as i64).contains(&value) {
        return Err(format!("Invalid {label} port number."));
    }
    Ok(value as u16)
}

fn parse_port_range(value: &str) -> Result<(u16, u16), String> {
    let mut parts = value.trim().split('-');
    let first = parts
        .next()
        .ok_or_else(|| "Invalid port_range argument.".to_string())?;
    let last = parts
        .next()
        .ok_or_else(|| "Invalid port_range argument.".to_string())?;
    if parts.next().is_some() {
        return Err("Invalid port_range argument.".to_string());
    }

    let first: u64 = first
        .trim()
        .parse()
        .map_err(|_| "Invalid port_range argument.".to_string())?;
    let last: u64 = last
        .trim()
        .parse()
        .map_err(|_| "Invalid port_range argument.".to_string())?;

    if first > u16::MAX as u64 || last > u16::MAX as u64 {
        return Err("Port range out-of-bounds.".to_string());
    }

    // `0-0` disables the listener (used by `-p 0-0`); any other range must
    // use valid TCP ports and be ordered.
    if first == 0 && last == 0 {
        return Ok((0, 0));
    }
    if first == 0 || first > last {
        return Err("Invalid listen port range.".to_string());
    }

    Ok((first as u16, last as u16))
}

fn validate_proxy_url(value: &str) -> Result<(), String> {
    if value.trim().is_empty() {
        return Ok(());
    }
    if value.chars().any(char::is_whitespace) {
        return Err("Proxy address must not contain whitespace.".into());
    }

    let (scheme, rest) = value
        .split_once("://")
        .ok_or_else(|| "Proxy address must include a scheme.".to_string())?;
    if !matches!(scheme, "http" | "https" | "socks5" | "socks5h") {
        return Err(format!("Unsupported proxy scheme: {}", scheme));
    }
    if rest.is_empty() || rest.starts_with('/') {
        return Err("Proxy address must include a host.".into());
    }
    if rest.contains('/') || rest.contains('?') || rest.contains('#') {
        return Err("Proxy address must not include a path, query, or fragment.".into());
    }

    let host_port = rest.rsplit_once('@').map(|(_, hp)| hp).unwrap_or(rest);
    let (host, port) = host_port
        .rsplit_once(':')
        .ok_or_else(|| "Proxy address must include a port.".to_string())?;
    if host.is_empty() {
        return Err("Proxy address must include a host.".into());
    }
    let port: u16 = port
        .parse()
        .map_err(|_| "Proxy address must include a valid port.".to_string())?;
    if port == 0 {
        return Err("Proxy address must include a port.".into());
    }
    Ok(())
}

fn register_listen_port_commands(
    map: &mut CommandMap,
    state: &Arc<RwLock<NetworkExtConfig>>,
) -> Result<(), RpcError> {
    add_ext_int(
        map,
        state,
        "network.listen.port",
        |s| s.listen_port,
        |s, value| {
            s.listen_port = checked_port(value, "listen")? as i64;
            Ok(())
        },
    )?;

    add_ext_bool(
        map,
        state,
        "network.listen.port.random",
        |s| s.listen_port_random,
        |s, value| s.listen_port_random = value,
    )?;

    let get_state = Arc::clone(state);
    map.insert_func(
        "network.listen.port.range",
        move |_target, _obj| {
            let s = get_state.read().unwrap();
            Ok(Object::from(format!(
                "{}-{}",
                s.listen_port_first, s.listen_port_last
            )))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "Listen port range.",
    )?;

    let set_state = Arc::clone(state);
    map.insert_func(
        "network.listen.port.range.set",
        move |_target, obj| {
            let raw = convert_to_string(obj)?;
            let (first, last) = parse_port_range(&raw).map_err(RpcError::InputError)?;
            let mut s = set_state.write().unwrap();
            s.listen_port_first = first;
            s.listen_port_last = last;
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "<first>-<last>",
        "Set the listen port range.",
    )?;

    // Legacy aliases from rtorrent's rc compatibility layer.
    let get_alias = Arc::clone(state);
    map.insert_func(
        "network.port_random",
        move |_target, _obj| {
            Ok(Object::from(
                if get_alias.read().unwrap().listen_port_random {
                    1i64
                } else {
                    0
                },
            ))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "Legacy alias for network.listen.port.random.",
    )?;

    let set_alias = Arc::clone(state);
    map.insert_func(
        "network.port_random.set",
        move |_target, obj| {
            let value = convert_to_value(obj, 0, 1)
                .map_err(|_| RpcError::InputError("Not a value.".into()))?;
            set_alias.write().unwrap().listen_port_random = value != 0;
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "<0|1>",
        "Legacy alias for network.listen.port.random.set.",
    )?;

    let set_alias = Arc::clone(state);
    map.insert_func(
        "port_random",
        move |_target, obj| {
            let value = convert_to_value(obj, 0, 1)
                .map_err(|_| RpcError::InputError("Not a value.".into()))?;
            set_alias.write().unwrap().listen_port_random = value != 0;
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "<0|1>",
        "Legacy alias for network.listen.port.random.set.",
    )?;

    let get_alias = Arc::clone(state);
    map.insert_func(
        "network.port_range",
        move |_target, _obj| {
            let s = get_alias.read().unwrap();
            Ok(Object::from(format!(
                "{}-{}",
                s.listen_port_first, s.listen_port_last
            )))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "Legacy alias for network.listen.port.range.",
    )?;

    let set_alias = Arc::clone(state);
    map.insert_func(
        "network.port_range.set",
        move |_target, obj| {
            let raw = convert_to_string(obj)?;
            let (first, last) = parse_port_range(&raw).map_err(RpcError::InputError)?;
            let mut s = set_alias.write().unwrap();
            s.listen_port_first = first;
            s.listen_port_last = last;
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "<first>-<last>",
        "Legacy alias for network.listen.port.range.set.",
    )?;

    let set_alias = Arc::clone(state);
    map.insert_func(
        "port_range",
        move |_target, obj| {
            let raw = convert_to_string(obj)?;
            let (first, last) = parse_port_range(&raw).map_err(RpcError::InputError)?;
            let mut s = set_alias.write().unwrap();
            s.listen_port_first = first;
            s.listen_port_last = last;
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "<first>-<last>",
        "Legacy alias for network.listen.port.range.set.",
    )?;

    for name in ["network.port_open", "network.port_open.set"] {
        map.insert_func(
            name,
            move |_target, _obj| {
                Ok(if name.ends_with(".set") {
                    Object::create_empty()
                } else {
                    Object::from(0i64)
                })
            },
            if name.ends_with(".set") {
                FLAG_PUBLIC_RPC
            } else {
                FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE
            },
            "",
            "Deprecated command; does nothing.",
        )?;
    }

    Ok(())
}

fn register_proxy_http_commands(
    map: &mut CommandMap,
    state: &Arc<RwLock<NetworkExtConfig>>,
) -> Result<(), RpcError> {
    add_ext_string(
        map,
        state,
        "network.proxy.global",
        |s| s.proxy_global.clone(),
        |s, value| {
            validate_proxy_url(&value)?;
            s.proxy_global = value;
            Ok(())
        },
    )?;

    add_ext_string(
        map,
        state,
        "network.proxy.http",
        |s| s.proxy_http.clone(),
        |s, value| {
            validate_proxy_url(&value)?;
            s.proxy_http = value;
            Ok(())
        },
    )?;

    add_ext_string(
        map,
        state,
        "network.http.cacert",
        |s| s.http_cacert.clone(),
        |s, value| {
            s.http_cacert = value;
            Ok(())
        },
    )?;
    add_ext_string(
        map,
        state,
        "network.http.capath",
        |s| s.http_capath.clone(),
        |s, value| {
            s.http_capath = value;
            Ok(())
        },
    )?;

    add_ext_int(
        map,
        state,
        "network.http.dns_cache_timeout",
        |s| s.http_dns_cache_timeout,
        |s, value| {
            if value < 0 {
                return Err("network.http.dns_cache_timeout: value must be non-negative.".into());
            }
            s.http_dns_cache_timeout = value;
            Ok(())
        },
    )?;
    add_ext_int(
        map,
        state,
        "network.http.max_cache_connections",
        |s| s.http_max_cache_connections,
        |s, value| {
            // The C++ CurlStack rejects values above 1024.
            if !(0..=1024).contains(&value) {
                return Err(
                    "network.http.max_cache_connections: value must be between 0 and 1024.".into(),
                );
            }
            s.http_max_cache_connections = value;
            Ok(())
        },
    )?;
    add_ext_int(
        map,
        state,
        "network.http.max_host_connections",
        |s| s.http_max_host_connections,
        |s, value| {
            // The C++ CurlStack rejects values above 1024.
            if !(0..=1024).contains(&value) {
                return Err(
                    "network.http.max_host_connections: value must be between 0 and 1024.".into(),
                );
            }
            s.http_max_host_connections = value;
            Ok(())
        },
    )?;
    add_ext_int(
        map,
        state,
        "network.http.max_total_connections",
        |s| s.http_max_total_connections,
        |s, value| {
            let limit = calculate_alloc_limit(SocketCategory::Http) as i64;
            if !(0..=limit).contains(&value) {
                return Err(format!(
                    "network.http.max_total_connections: value must be between 0 and {}.",
                    limit
                ));
            }
            s.http_max_total_connections = value;
            Ok(())
        },
    )?;
    add_ext_bool(
        map,
        state,
        "network.http.ssl_verify_host",
        |s| s.http_ssl_verify_host,
        |s, value| s.http_ssl_verify_host = value,
    )?;
    add_ext_bool(
        map,
        state,
        "network.http.ssl_verify_peer",
        |s| s.http_ssl_verify_peer,
        |s, value| s.http_ssl_verify_peer = value,
    )?;

    map.insert_func(
        "network.http.current_open",
        |_target, _obj| {
            Ok(Object::from(
                socket_manager().category_managed_size(SocketCategory::Http) as i64,
            ))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "Current number of open HTTP connections.",
    )?;

    // Legacy aliases.
    let get_alias = Arc::clone(state);
    map.insert_func(
        "network.http.proxy_address",
        move |_target, _obj| Ok(Object::from(get_alias.read().unwrap().proxy_http.clone())),
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "Legacy alias for network.proxy.http.",
    )?;
    let set_alias = Arc::clone(state);
    map.insert_func(
        "network.http.proxy_address.set",
        move |_target, obj| {
            let value = convert_to_string(obj)?;
            validate_proxy_url(&value).map_err(RpcError::InputError)?;
            set_alias.write().unwrap().proxy_http = value;
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "<url>",
        "Legacy alias for network.proxy.http.set.",
    )?;
    let set_alias = Arc::clone(state);
    map.insert_func(
        "proxy_address",
        move |_target, obj| {
            let value = convert_to_string(obj)?;
            validate_proxy_url(&value).map_err(RpcError::InputError)?;
            set_alias.write().unwrap().proxy_http = value;
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "<url>",
        "Legacy alias for network.proxy.http.set.",
    )?;

    let get_alias = Arc::clone(state);
    map.insert_func(
        "network.http.max_open",
        move |_target, _obj| {
            Ok(Object::from(
                get_alias.read().unwrap().http_max_total_connections,
            ))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "Legacy alias for network.http.max_total_connections.",
    )?;
    let set_alias = Arc::clone(state);
    map.insert_func(
        "network.http.max_open.set",
        move |_target, obj| {
            let value = convert_to_value(obj, 0, 1)
                .map_err(|_| RpcError::InputError("Not a value.".into()))?;
            let limit = calculate_alloc_limit(SocketCategory::Http) as i64;
            if !(0..=limit).contains(&value) {
                return Err(RpcError::InputError(format!(
                    "network.http.max_open: value must be between 0 and {}.",
                    limit
                )));
            }
            set_alias.write().unwrap().http_max_total_connections = value;
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "<value>",
        "Legacy alias for network.http.max_total_connections.set.",
    )?;

    Ok(())
}

fn register_socket_aliases(
    map: &mut CommandMap,
    state: &Arc<RwLock<NetworkExtConfig>>,
) -> Result<(), RpcError> {
    // C++ reports `file_manager->open_files()`, i.e. descriptors owned by the
    // disk worker, not the socket-manager file category. The global accessor
    // does not spawn a worker just to read the gauge, so it is safe here.
    map.insert_func(
        "network.open_files",
        |_target, _obj| {
            let open = global_file_manager_stats()
                .map(|stats| stats.open_files)
                .unwrap_or(0);
            Ok(Object::from(open.min(i64::MAX as usize) as i64))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "Number of files currently open in the disk worker.",
    )?;

    add_ext_int(
        map,
        state,
        "network.max_open_files",
        |s| s.max_open_files,
        |s, value| {
            if !(4..=65536).contains(&value) {
                return Err("Max open files must be between 4 and 65536.".into());
            }
            s.max_open_files = value;
            Ok(())
        },
    )?;

    map.insert_func(
        "network.open_sockets",
        |_target, _obj| Ok(Object::from(socket_manager().size() as i64)),
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "Legacy alias for system.sockets.size.",
    )?;
    map.insert_func(
        "network.max_open_sockets",
        |_target, _obj| Ok(Object::from(socket_manager().max_size() as i64)),
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "Legacy alias for system.sockets.max_size.",
    )?;
    map.insert_func(
        "network.max_open_sockets.set",
        |_target, obj| {
            let value = convert_to_value(obj, 0, 1)
                .map_err(|_| RpcError::InputError("Not a value.".into()))?;
            if value < 0 || value > u32::MAX as i64 {
                return Err(RpcError::InputError(
                    "Invalid socket max size value.".into(),
                ));
            }
            socket_manager()
                .set_max_size_and_adjust(value as u32)
                .map_err(|err| RpcError::InputError(err.to_string()))?;
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "<value>",
        "Legacy alias for system.sockets.max_size.set.",
    )?;

    // The network runtime does not publish a handshake counter yet, so this
    // stays a read-only zero rather than maintaining a second, never-updated
    // counter here.
    map.insert_func(
        "network.total_handshakes",
        |_target, _obj| Ok(Object::from(0i64)),
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "Total number of completed handshakes (zero; not published by the runtime).",
    )?;

    Ok(())
}

fn register_rpc_toggles(
    map: &mut CommandMap,
    state: &Arc<RwLock<NetworkExtConfig>>,
) -> Result<(), RpcError> {
    add_ext_bool(
        map,
        state,
        "network.rpc.use_xmlrpc",
        |s| s.rpc_use_xmlrpc,
        |s, value| s.rpc_use_xmlrpc = value,
    )?;
    add_ext_bool(
        map,
        state,
        "network.rpc.use_jsonrpc",
        |s| s.rpc_use_jsonrpc,
        |s, value| s.rpc_use_jsonrpc = value,
    )?;
    add_ext_bool(
        map,
        state,
        "network.scgi.use_gzip",
        |s| s.scgi_use_gzip,
        |s, value| s.scgi_use_gzip = value,
    )?;
    add_ext_int(
        map,
        state,
        "network.scgi.gzip.min_size",
        |s| s.scgi_gzip_min_size,
        |s, value| {
            if !(0..=u32::MAX as i64).contains(&value) {
                return Err("network.scgi.gzip.min_size: value is out of range.".into());
            }
            s.scgi_gzip_min_size = value;
            Ok(())
        },
    )?;

    Ok(())
}

pub fn register_network_commands(
    map: &mut CommandMap,
    config: NetworkConfig,
) -> Result<(), RpcError> {
    let state = Arc::new(RwLock::new(NetworkExtConfig::default()));

    add_bool(
        map,
        &config,
        "network.block.ipv4",
        |c| c.is_block_ipv4(),
        |c, v| c.set_block_ipv4(v),
    )?;
    add_bool(
        map,
        &config,
        "network.block.ipv6",
        |c| c.is_block_ipv6(),
        |c, v| c.set_block_ipv6(v),
    )?;
    add_bool(
        map,
        &config,
        "network.block.ipv4in6",
        |c| c.is_block_ipv4in6(),
        |c, v| c.set_block_ipv4in6(v),
    )?;
    add_bool(
        map,
        &config,
        "network.block.outgoing",
        |c| c.is_block_outgoing(),
        |c, v| c.set_block_outgoing(v),
    )?;
    add_bool(
        map,
        &config,
        "network.block.udp",
        |c| c.is_block_udp(),
        |c, v| c.set_block_udp(v),
    )?;
    add_bool(
        map,
        &config,
        "network.block.incoming",
        |c| c.is_block_incoming(),
        |c, v| c.set_block_incoming(v),
    )?;
    add_bool(
        map,
        &config,
        "network.prefer.ipv6",
        |c| c.is_prefer_ipv6(),
        |c, v| c.set_prefer_ipv6(v),
    )?;

    add_address(
        map,
        &config,
        "network.bind_address",
        |c| c.bind_address_best_match().addr_str(),
        |c, v| c.set_bind_address(v).map_err(|e| e.to_string()),
    )?;
    add_address(
        map,
        &config,
        "network.bind_address.ipv4",
        |c| c.bind_inet_address().addr_str(),
        |c, v| c.set_bind_inet_address(v).map_err(|e| e.to_string()),
    )?;
    add_address(
        map,
        &config,
        "network.bind_address.ipv6",
        |c| c.bind_inet6_address().addr_str(),
        |c, v| c.set_bind_inet6_address(v).map_err(|e| e.to_string()),
    )?;
    add_address(
        map,
        &config,
        "network.local_address",
        |c| c.local_address_best_match().addr_str(),
        |c, v| c.set_local_address(v).map_err(|e| e.to_string()),
    )?;
    add_address(
        map,
        &config,
        "network.local_address.ipv4",
        |c| c.local_inet_address().addr_str(),
        |c, v| c.set_local_inet_address(v).map_err(|e| e.to_string()),
    )?;
    add_address(
        map,
        &config,
        "network.local_address.ipv6",
        |c| c.local_inet6_address().addr_str(),
        |c, v| c.set_local_inet6_address(v).map_err(|e| e.to_string()),
    )?;

    // Bind/local addresses only take effect for new sockets; the listener
    // itself owns the bound socket and requires a restart on change.
    for (alias, target) in [
        ("bind", "network.bind_address.set"),
        ("ip", "network.local_address.set"),
    ] {
        let dest = config.clone();
        map.insert_func(
            alias,
            move |_target, obj| {
                let value = convert_to_string(obj)?;
                match target {
                    "network.bind_address.set" => dest
                        .set_bind_address(&value)
                        .map_err(|err| RpcError::InputError(err.to_string()))?,
                    _ => dest
                        .set_local_address(&value)
                        .map_err(|err| RpcError::InputError(err.to_string()))?,
                }
                Ok(Object::create_empty())
            },
            FLAG_PUBLIC_RPC,
            "<address>",
            "Legacy address setter alias.",
        )?;
    }

    add_int(
        map,
        &config,
        "network.local_port",
        |c| c.local_port_best_match() as i64,
        |c, v| {
            let port = checked_port(v, "local")?;
            c.set_local_port(port);
            Ok(())
        },
    )?;
    add_int(
        map,
        &config,
        "network.local_port.ipv4",
        |c| c.local_inet_port() as i64,
        |c, v| {
            let port = checked_port(v, "local ipv4")?;
            c.set_local_inet_port(port);
            Ok(())
        },
    )?;
    add_int(
        map,
        &config,
        "network.local_port.ipv6",
        |c| c.local_inet6_port() as i64,
        |c, v| {
            let port = checked_port(v, "local ipv6")?;
            c.set_local_inet6_port(port);
            Ok(())
        },
    )?;
    add_int(
        map,
        &config,
        "network.listen.backlog",
        |c| c.listen_backlog() as i64,
        |c, v| {
            if v < 1 || v > 65536 {
                return Err("Listen backlog must be between 1 and 65536.".to_string());
            }
            c.set_listen_backlog(v as i32).map_err(|e| e.to_string())
        },
    )?;
    add_byte_size(
        map,
        &config,
        "network.send_buffer.size",
        |c| c.send_buffer_size() as i64,
        |c, v| {
            c.set_send_buffer_size(v);
            Ok(())
        },
    )?;
    add_byte_size(
        map,
        &config,
        "network.receive_buffer.size",
        |c| c.receive_buffer_size() as i64,
        |c, v| {
            c.set_receive_buffer_size(v);
            Ok(())
        },
    )?;
    let get_tos = config.clone();
    map.insert_func(
        "network.tos",
        move |_target, _obj| Ok(Object::from(get_tos.priority() as i64)),
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "IP type-of-service value.",
    )?;

    for (name, get, set) in [
        (
            "network.bind_device.ipv4",
            NetworkConfig::bind_inet_device_name as fn(&NetworkConfig) -> String,
            NetworkConfig::set_bind_inet_device_name as fn(&NetworkConfig, String),
        ),
        (
            "network.bind_device.ipv6",
            NetworkConfig::bind_inet6_device_name as fn(&NetworkConfig) -> String,
            NetworkConfig::set_bind_inet6_device_name as fn(&NetworkConfig, String),
        ),
    ] {
        let get_config = config.clone();
        map.insert_func(
            name,
            move |_target, _obj| Ok(Object::from(get(&get_config))),
            FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
            "",
            name,
        )?;

        let set_config = config.clone();
        map.insert_func(
            &format!("{name}.set"),
            move |_target, obj| {
                let value = convert_to_string(obj)?;
                set(&set_config, value);
                Ok(Object::create_empty())
            },
            FLAG_PUBLIC_RPC,
            "<device>",
            name,
        )?;
    }

    let set_device = config.clone();
    map.insert_func(
        "network.bind_device.set",
        move |_target, obj| {
            let value = convert_to_string(obj)?;
            set_device.set_bind_device_name(value);
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "<device>",
        "Bind sockets to a network device.",
    )?;

    register_listen_port_commands(map, &state)?;
    register_proxy_http_commands(map, &state)?;
    register_socket_aliases(map, &state)?;
    register_rpc_toggles(map, &state)?;

    // `network.tos.set` in the C++ daemon accepts both symbolic names and
    // hexadecimal values (optionally prefixed with `0x`).
    let tos = config.clone();
    map.insert_func(
        "network.tos.set",
        move |_target, obj| {
            let raw = convert_to_string(obj)?;
            let trimmed = raw.trim();
            let lowered = trimmed.to_ascii_lowercase();
            let value = match lowered.as_str() {
                "default" => 0,
                "lowdelay" => 0x10,
                "throughput" => 0x08,
                "reliability" => 0x04,
                "mincost" => 0x02,
                _ => {
                    // C++ parses the numeric form with base 16 (`strtoll`),
                    // which accepts both `10` and `0x10`. Keep that behaviour
                    // instead of interpreting `10` as decimal.
                    let digits = lowered.strip_prefix("0x").unwrap_or(lowered.as_str());
                    i64::from_str_radix(digits, 16)
                        .map_err(|_| RpcError::InputError("Invalid TOS value.".into()))?
                }
            };
            if !(0..=255).contains(&value) {
                return Err(RpcError::InputError(
                    "TOS value must be between 0 and 255.".into(),
                ));
            }
            tos.set_priority(value as i32);
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "<tos>",
        "Set the IP type-of-service value (symbolic name or hexadecimal).",
    )?;

    Ok(())
}
