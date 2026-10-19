//! DHT runtime commands.
//!
//! `dht.mode`, `dht.port` and `dht.override_port` are persisted in
//! [`DhtConfig`] so they survive a restart. When a [`NetworkRuntime`] is live,
//! the same commands also drive the running [`DhtController`] (start/stop,
//! rebind, bootstrap and refresh) through the process-wide controller handle.
//!
//! [`NetworkRuntime`]: crate::network_runtime::NetworkRuntime
//! [`DhtController`]: lt_dht::controller::DhtController

use std::str::FromStr;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use lt_common::Object;
use lt_dht::controller::{DhtController, DhtMode};
use lt_net::addr::{AddressFamily, SocketAddress};
use rt_rpc::command_flags::*;
use rt_rpc::{convert_to_string, convert_to_value, CommandMap, RpcError};

use super::tracker::{checked_port_value, DhtConfig};
use crate::network_runtime::{dht_controller, DhtLifecycleConfig};

fn to_str(obj: &Object) -> Result<String, RpcError> {
    convert_to_string(obj)
}

fn parse_mode(value: &str) -> Result<DhtMode, RpcError> {
    DhtMode::parse(value.trim()).ok_or_else(|| {
        RpcError::InputError(format!(
            "dht.mode.set: invalid mode '{}', expected disable/off/auto/on",
            value.trim()
        ))
    })
}

fn spawn_detached<F>(future: F)
where
    F: std::future::Future<Output = ()> + Send + 'static,
{
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        handle.spawn(future);
    }
}

fn current_controller() -> Option<Arc<DhtController>> {
    dht_controller()
}

fn current_mode(controller: &Option<Arc<DhtController>>, config: &DhtConfig) -> String {
    controller
        .as_ref()
        .map(|controller| controller.mode().as_str().to_string())
        .unwrap_or_else(|| config.mode.clone())
}

fn effective_dht_port(config: &DhtConfig) -> u16 {
    if config.override_port != 0 {
        config.override_port
    } else {
        config.port
    }
}

fn statistics_object(controller: &Option<Arc<DhtController>>, config: &DhtConfig) -> Object {
    if let Some(controller) = controller {
        let mut map = controller.statistics_object();
        let port = controller.port();
        let _ = map.insert_key(
            "port",
            Object::create_value(if port == 0 { config.port } else { port } as i64),
        );
        return map;
    }

    let mut map = Object::create_map();
    let _ = map.insert_key(
        "dht",
        Object::create_string(config.mode.as_bytes().to_vec()),
    );
    for (key, value) in [
        ("active", 0i64),
        ("cycle", 0),
        ("queries_received", 0),
        ("queries_sent", 0),
        ("replies_received", 0),
        ("errors_received", 0),
        ("errors_caught", 0),
        ("nodes", 0),
        ("buckets", 0),
        ("peers", 0),
        ("peers_max", 0),
        ("torrents", 0),
    ] {
        let _ = map.insert_key(key, Object::create_value(value));
    }
    let _ = map.insert_key("port", Object::create_value(config.port as i64));
    map
}

fn parse_node(value: &str) -> Result<SocketAddress, RpcError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(RpcError::InputError(
            "dht.add_node: expected host:port".into(),
        ));
    }

    let (host, port) = if let Ok(address) = SocketAddress::from_str(value) {
        if address.port() != 0 {
            return Ok(address);
        }
        (value.to_string(), None)
    } else if let Some((host, port)) = value.rsplit_once(':') {
        let port = port.parse::<u16>().map_err(|_| {
            RpcError::InputError(format!("dht.add_node: invalid port in '{value}'"))
        })?;
        (host.trim_matches(['[', ']']).to_string(), Some(port))
    } else {
        (value.to_string(), None)
    };

    let port = port.unwrap_or(6881);
    if port == 0 {
        return Err(RpcError::InputError(
            "dht.add_node: port must not be zero".into(),
        ));
    }

    let host_port = if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    };
    SocketAddress::lookup_address(&host_port, AddressFamily::Unspec)
        .map_err(|err| RpcError::InputError(format!("dht.add_node: {err}")))
}

/// Registers the canonical DHT command surface.
///
/// The legacy `dht.*` commands are still registered by
/// [`register_tracker_commands`](super::tracker::register_tracker_commands) for
/// backwards compatibility with tests and embedders that only install the
/// tracker domain. This function replaces them in the full daemon command
/// map, so it must run *after* the tracker commands.
pub fn register_dht_commands(
    map: &mut CommandMap,
    config: Arc<RwLock<DhtConfig>>,
) -> Result<(), RpcError> {
    // Publish the shared preferences so a later `NetworkRuntime` start uses
    // `dht.port`/`dht.mode` even though the daemon only passes a
    // `DhtLifecycleConfig` at spawn time.
    crate::network_runtime::set_dht_config_handle(Some(Arc::clone(&config)));

    for key in [
        "dht.override_port",
        "dht.override_port.set",
        "dht.port",
        "dht.port.set",
        "dht.mode",
        "dht.mode.set",
    ] {
        let _ = map.erase(key);
    }

    // ---- mode ---------------------------------------------------------
    let cfg_get = Arc::clone(&config);
    map.insert_func(
        "dht.mode",
        move |_target, _obj| {
            let cfg = cfg_get.read().unwrap();
            Ok(Object::from(current_mode(&current_controller(), &cfg)))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "Get DHT mode (disable/off/auto/on).",
    )?;

    let cfg_set = Arc::clone(&config);
    map.insert_func(
        "dht.mode.set",
        move |_target, obj| {
            let mode = parse_mode(&to_str(obj)?)?;
            cfg_set.write().unwrap().mode = mode.as_str().to_string();
            if let Some(controller) = current_controller() {
                controller.set_mode(mode);
            }
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "<disable|off|auto|on>",
        "Set DHT mode.",
    )?;

    // ---- port / override port ----------------------------------------
    let cfg_get = Arc::clone(&config);
    map.insert_func(
        "dht.port",
        move |_target, _obj| {
            let cfg = cfg_get.read().unwrap();
            let port = current_controller()
                .as_ref()
                .map(|controller| controller.port())
                .filter(|port| *port != 0)
                .unwrap_or(cfg.port);
            Ok(Object::from(port as i64))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "Get the active DHT port.",
    )?;

    let cfg_set = Arc::clone(&config);
    map.insert_func(
        "dht.port.set",
        move |_target, obj| {
            let value = convert_to_value(obj, 10, 1)?;
            let port = checked_port_value(value, "dht.port.set")?;
            {
                let mut cfg = cfg_set.write().unwrap();
                cfg.port = port;
            }
            if let Some(controller) = current_controller() {
                spawn_detached(async move {
                    if let Err(err) = controller.restart_with_port(port).await {
                        tracing::warn!(port, error = %err, "Could not restart DHT on new port");
                    }
                });
            }
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "<port>",
        "Set the DHT port.",
    )?;

    let cfg_get = Arc::clone(&config);
    map.insert_func(
        "dht.override_port",
        move |_target, _obj| {
            Ok(Object::from(cfg_get.read().unwrap().override_port as i64))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "Get DHT override port.",
    )?;

    let cfg_set = Arc::clone(&config);
    map.insert_func(
        "dht.override_port.set",
        move |_target, obj| {
            let value = convert_to_value(obj, 10, 1)?;
            let port = checked_port_value(value, "dht.override_port.set")?;
            let effective = {
                let mut cfg = cfg_set.write().unwrap();
                cfg.override_port = port;
                effective_dht_port(&cfg)
            };
            if let Some(controller) = current_controller() {
                controller.set_port(effective);
                spawn_detached(async move {
                    if let Err(err) = controller.restart_with_port(effective).await {
                        tracing::warn!(
                            port = effective,
                            error = %err,
                            "Could not apply DHT override port"
                        );
                    }
                });
            }
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "<port>",
        "Set DHT override port.",
    )?;

    // ---- runtime state / statistics ----------------------------------
    let cfg_stats = Arc::clone(&config);
    map.insert_func(
        "dht.statistics",
        move |_target, _obj| {
            let cfg = cfg_stats.read().unwrap();
            Ok(statistics_object(&current_controller(), &cfg))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "DHT statistics.",
    )?;

    map.insert_func(
        "dht.is_active",
        move |_target, _obj| {
            Ok(Object::from(
                current_controller()
                    .map(|controller| controller.is_active() && controller.get_statistics().active)
                    .unwrap_or(false) as i64,
            ))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "Whether the DHT server is running.",
    )?;

    // ---- node/bootstrap/refresh controls -----------------------------
    map.insert_func(
        "dht.add_node",
        move |_target, obj| {
            let node = parse_node(&to_str(obj)?)?;
            if let Some(controller) = current_controller() {
                controller.add_peer_node(&node, node.port());
            }
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "<host:port>",
        "Add a DHT node/bootstrap contact.",
    )?;

    map.insert_func(
        "dht.bootstrap",
        move |_target, _obj| {
            if let Some(controller) = current_controller() {
                spawn_detached(async move {
                    match controller.bootstrap_configured().await {
                        Ok(replied) => {
                            tracing::debug!(replied, "DHT bootstrap completed");
                        }
                        Err(err) => tracing::debug!(error = %err, "DHT bootstrap failed"),
                    }
                });
            }
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "",
        "Contact the configured bootstrap nodes.",
    )?;

    map.insert_func(
        "dht.refresh",
        move |_target, _obj| {
            if let Some(controller) = current_controller() {
                spawn_detached(async move {
                    if let Err(err) = controller.refresh().await {
                        tracing::debug!(error = %err, "DHT refresh failed");
                    }
                });
            }
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "",
        "Refresh the DHT routing table.",
    )?;

    map.insert_func(
        "dht.bootstrap_nodes",
        move |_target, _obj| {
            let nodes = current_controller()
                .map(|controller| controller.bootstrap_nodes())
                .unwrap_or_default();
            let mut list = Object::create_list();
            for node in nodes {
                list.insert_back(Object::from(node.addr_str()))
                    .map_err(|_| RpcError::InternalError("could not build list".into()))?;
            }
            Ok(list)
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "Configured DHT bootstrap nodes.",
    )?;

    map.insert_func(
        "dht.bootstrap_node.add",
        move |_target, obj| {
            let node = parse_node(&to_str(obj)?)?;
            if let Some(controller) = current_controller() {
                controller.add_bootstrap_node(node);
            }
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "<host:port>",
        "Add a DHT bootstrap contact.",
    )?;

    add_interval_commands(map)?;

    map.insert_func(
        "dht.reset_statistics",
        move |_target, _obj| {
            if let Some(controller) = current_controller() {
                controller.reset_statistics();
            }
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "",
        "Reset DHT statistics.",
    )?;

    Ok(())
}

fn add_interval_commands(map: &mut CommandMap) -> Result<(), RpcError> {
    map.insert_func(
        "dht.refresh_interval",
        move |_target, _obj| {
            let seconds = current_controller()
                .map(|controller| controller.refresh_interval().as_secs())
                .unwrap_or_else(|| DhtLifecycleConfig::default().refresh_interval.as_secs());
            Ok(Object::from(seconds as i64))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "DHT refresh interval in seconds.",
    )?;

    map.insert_func(
        "dht.refresh_interval.set",
        move |_target, obj| {
            let value = convert_to_value(obj, 10, 1)?;
            if value <= 0 {
                return Err(RpcError::InputError(
                    "dht.refresh_interval: must be positive".into(),
                ));
            }
            if let Some(controller) = current_controller() {
                controller.set_refresh_interval(Duration::from_secs(value as u64));
            }
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "<seconds>",
        "Set the DHT refresh interval.",
    )?;

    map.insert_func(
        "dht.bootstrap_retry_interval",
        move |_target, _obj| {
            let seconds = current_controller()
                .map(|controller| controller.config().bootstrap_retry_interval.as_secs())
                .unwrap_or_else(|| {
                    DhtLifecycleConfig::default()
                        .bootstrap_retry_interval
                        .as_secs()
                });
            Ok(Object::from(seconds as i64))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "DHT bootstrap retry interval in seconds.",
    )?;

    map.insert_func(
        "dht.bootstrap_retry_interval.set",
        move |_target, obj| {
            let value = convert_to_value(obj, 10, 1)?;
            if value <= 0 {
                return Err(RpcError::InputError(
                    "dht.bootstrap_retry_interval: must be positive".into(),
                ));
            }
            if let Some(controller) = current_controller() {
                controller.set_bootstrap_retry_interval(Duration::from_secs(value as u64));
            }
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "<seconds>",
        "Set the DHT bootstrap retry interval.",
    )?;

    Ok(())
}
