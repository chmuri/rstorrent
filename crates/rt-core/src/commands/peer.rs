use std::sync::{Arc, RwLock};

use lt_common::{HashString, Object};
use lt_net::addr::SocketAddress;
use lt_protocol::connection::{PeerConnection, PeerInfo};
use rt_rpc::command_flags::*;
use rt_rpc::parse_commands::parse_command;
use rt_rpc::{convert_to_string, convert_to_value, CommandMap, RpcError, Target};

use crate::download::Download;
use crate::download_list::DownloadList;

/// Mirrors libtorrent's `copy_escape_html_str`: keep ASCII alphanumerics and
/// `-`, and render every other byte as `%XX` (uppercase hex).
fn escape_html_peer_id(peer_id: &HashString) -> String {
    let mut escaped = String::with_capacity(peer_id.as_bytes().len() * 3);
    for &byte in peer_id.as_bytes() {
        if byte.is_ascii_alphanumeric() || byte == b'-' {
            escaped.push(byte as char);
        } else {
            use std::fmt::Write;
            write!(&mut escaped, "%{:02X}", byte).unwrap();
        }
    }
    escaped
}

fn get_peer_target(
    target: &Target,
    download_list: &Arc<RwLock<DownloadList>>,
) -> Result<(Arc<RwLock<Download>>, SocketAddress), RpcError> {
    let (hash, address) = match target {
        Target::Peer(hash, address) => (*hash, address.clone()),
        _ => return Err(RpcError::InputError("Command requires Peer target".into())),
    };

    let download = download_list
        .read()
        .unwrap()
        .find(&hash)
        .ok_or_else(|| RpcError::Custom {
            code: -500,
            message: format!("Download '{}' not found", hash.to_hex()),
        })?;

    Ok((download, address))
}

fn add_peer_getter<F>(
    map: &mut CommandMap,
    download_list: &Arc<RwLock<DownloadList>>,
    name: &str,
    f: F,
) -> Result<(), RpcError>
where
    F: Fn(&PeerInfo) -> Object + Send + Sync + 'static,
{
    let list = Arc::clone(download_list);
    map.insert_func(
        name,
        move |target, _obj| {
            let (download, address) = get_peer_target(target, &list)?;
            let guard = download.read().unwrap();
            let peer = guard
                .download()
                .peer_info_by_address(&address)
                .ok_or_else(|| RpcError::InputError("peer not found".into()))?;
            Ok(f(&peer))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        name,
    )
}

fn add_peer_setter<F>(
    map: &mut CommandMap,
    download_list: &Arc<RwLock<DownloadList>>,
    name: &str,
    f: F,
) -> Result<(), RpcError>
where
    F: Fn(&mut PeerConnection, &Object) -> Result<(), RpcError> + Send + Sync + 'static,
{
    let list = Arc::clone(download_list);
    map.insert_func(
        name,
        move |target, obj| {
            let (download, address) = get_peer_target(target, &list)?;
            let mut guard = download.write().unwrap();
            let mut engine = guard.download_mut();
            let peer = engine
                .connection_list_mut()
                .find_by_address_mut(&address)
                .ok_or_else(|| RpcError::InputError("peer not found".into()))?;
            f(peer, obj)?;
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "<value>",
        name,
    )
}

fn add_peer_disconnect(
    map: &mut CommandMap,
    download_list: &Arc<RwLock<DownloadList>>,
    name: &str,
    delayed: bool,
) -> Result<(), RpcError> {
    let list = Arc::clone(download_list);
    let doc = if delayed {
        "Disconnect a peer. The engine has no disconnect scheduler, so the delayed form is immediate."
    } else {
        "Disconnect a peer immediately."
    };

    map.insert_func(
        name,
        move |target, _obj| {
            let (download, address) = get_peer_target(target, &list)?;
            let mut guard = download.write().unwrap();
            let mut engine = guard.download_mut();

            let peer_id = engine
                .connection_list()
                .find_by_address(&address)
                .filter(|peer| peer.is_connected())
                .map(|peer| peer.peer_id)
                .ok_or_else(|| RpcError::InputError("peer is not connected".into()))?;

            engine.remove_peer(&peer_id);
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        doc,
    )
}

/// Accept both the legacy multicall form (leading download hash) and the
/// target-oriented form (commands only).
fn multicall_commands(list: &[Object]) -> &[Object] {
    let mut start = 0;

    if let Some(first) = list.first() {
        if let Ok(first) = convert_to_string(first) {
            if first.is_empty() || (first.len() == 40 && HashString::from_hex(&first).is_some()) {
                start = 1;
            }
        }
    }

    if start < list.len()
        && convert_to_string(&list[start])
            .map(|s| s.is_empty())
            .unwrap_or(false)
    {
        start += 1;
    }

    &list[start..]
}

pub fn register_peer_commands(
    map: &mut CommandMap,
    download_list: Arc<RwLock<DownloadList>>,
) -> Result<(), RpcError> {
    let p_address_list = Arc::clone(&download_list);
    map.insert_func(
        "p.address",
        move |target, _obj| {
            let (download, address) = get_peer_target(target, &p_address_list)?;
            let guard = download.read().unwrap();
            let peer = guard
                .download()
                .peer_info_by_address(&address)
                .ok_or_else(|| RpcError::InputError("peer not found".into()))?;
            let addr = peer.socket_address.addr_str();
            if matches!(&peer.socket_address, SocketAddress::Inet6(_)) {
                Ok(Object::from(format!("[{addr}]")))
            } else {
                Ok(Object::from(addr))
            }
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "Get peer IP address.",
    )?;

    let p_port_list = Arc::clone(&download_list);
    map.insert_func(
        "p.port",
        move |target, _obj| {
            let (download, address) = get_peer_target(target, &p_port_list)?;
            let guard = download.read().unwrap();
            let peer = guard
                .download()
                .peer_info_by_address(&address)
                .ok_or_else(|| RpcError::InputError("peer not found".into()))?;
            Ok(Object::from(peer.socket_address.port() as i64))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "Get peer port.",
    )?;

    add_peer_getter(map, &download_list, "p.client_version", |peer| {
        Object::from(peer.client_version())
    })?;
    add_peer_getter(map, &download_list, "p.id", |peer| {
        Object::from(peer.peer_id.to_hex())
    })?;
    add_peer_getter(map, &download_list, "p.id_html", |peer| {
        Object::from(escape_html_peer_id(&peer.peer_id))
    })?;
    // NET-10 retains the eight reserved handshake bytes on PeerInfo, so this
    // exposes the negotiated DHT/extension bits instead of a placeholder mask.
    add_peer_getter(map, &download_list, "p.options_str", |peer| {
        Object::from(peer.options_str())
    })?;
    add_peer_getter(map, &download_list, "p.listen_port", |peer| {
        Object::from(peer.listen_port as i64)
    })?;
    add_peer_getter(map, &download_list, "p.queue_depth", |peer| {
        Object::from(peer.queue_depth as i64)
    })?;

    add_peer_getter(map, &download_list, "p.down_rate", |peer| {
        Object::from(peer.rate_down as i64)
    })?;
    add_peer_getter(map, &download_list, "p.up_rate", |peer| {
        Object::from(peer.rate_up as i64)
    })?;
    add_peer_getter(map, &download_list, "p.down_total", |peer| {
        Object::from(peer.downloaded as i64)
    })?;
    add_peer_getter(map, &download_list, "p.up_total", |peer| {
        Object::from(peer.uploaded as i64)
    })?;
    // libtorrent keeps a separate peer-reported rate meter. PeerInfo does not
    // retain that meter yet, so both commands expose the measured connection
    // counters rather than fabricating remote values.
    add_peer_getter(map, &download_list, "p.peer_rate", |peer| {
        Object::from(peer.rate_down as i64)
    })?;
    add_peer_getter(map, &download_list, "p.peer_total", |peer| {
        Object::from(peer.downloaded as i64)
    })?;
    add_peer_getter(map, &download_list, "p.completed_percent", |peer| {
        Object::from(peer.completed_percent() as i64)
    })?;

    add_peer_getter(map, &download_list, "p.is_connected", |peer| {
        Object::from(if peer.is_connected { 1i64 } else { 0 })
    })?;
    add_peer_getter(map, &download_list, "p.is_handshake", |peer| {
        Object::from(if peer.is_handshake { 1i64 } else { 0 })
    })?;
    add_peer_getter(map, &download_list, "p.is_blocked", |peer| {
        Object::from(if peer.is_blocked { 1i64 } else { 0 })
    })?;
    add_peer_getter(map, &download_list, "p.is_restart", |peer| {
        Object::from(if peer.is_restart { 1i64 } else { 0 })
    })?;
    add_peer_getter(map, &download_list, "p.is_encrypted", |peer| {
        Object::from(if peer.is_encrypted { 1i64 } else { 0 })
    })?;
    add_peer_getter(map, &download_list, "p.is_obfuscated", |peer| {
        Object::from(if peer.is_obfuscated { 1i64 } else { 0 })
    })?;
    add_peer_getter(map, &download_list, "p.is_incoming", |peer| {
        Object::from(if peer.is_incoming() { 1i64 } else { 0 })
    })?;
    add_peer_getter(map, &download_list, "p.is_outgoing", |peer| {
        Object::from(if peer.is_outgoing() { 1i64 } else { 0 })
    })?;
    add_peer_getter(map, &download_list, "p.is_choked", |peer| {
        Object::from(if peer.is_choked { 1i64 } else { 0 })
    })?;
    add_peer_getter(map, &download_list, "p.is_interested", |peer| {
        Object::from(if peer.is_interested { 1i64 } else { 0 })
    })?;
    add_peer_getter(map, &download_list, "p.is_preferred", |peer| {
        Object::from(if peer.is_preferred { 1i64 } else { 0 })
    })?;
    add_peer_getter(map, &download_list, "p.is_snubbed", |peer| {
        Object::from(if peer.is_snubbed { 1i64 } else { 0 })
    })?;
    add_peer_getter(map, &download_list, "p.snubbed", |peer| {
        Object::from(if peer.is_snubbed { 1i64 } else { 0 })
    })?;
    add_peer_getter(map, &download_list, "p.is_banned", |peer| {
        Object::from(if peer.is_banned { 1i64 } else { 0 })
    })?;
    add_peer_getter(map, &download_list, "p.banned", |peer| {
        Object::from(if peer.is_banned { 1i64 } else { 0 })
    })?;
    add_peer_getter(map, &download_list, "p.is_unwanted", |peer| {
        Object::from(if peer.is_unwanted { 1i64 } else { 0 })
    })?;
    add_peer_getter(map, &download_list, "p.is_seed", |peer| {
        Object::from(if peer.is_seed() { 1i64 } else { 0 })
    })?;
    add_peer_getter(map, &download_list, "p.failed_counter", |peer| {
        Object::from(peer.failed_counter as i64)
    })?;
    add_peer_getter(map, &download_list, "p.transfer_counter", |peer| {
        Object::from(peer.transfer_counter as i64)
    })?;
    add_peer_getter(map, &download_list, "p.last_connection", |peer| {
        Object::from(peer.last_connection as i64)
    })?;
    add_peer_getter(map, &download_list, "p.last_handshake", |peer| {
        Object::from(peer.last_handshake as i64)
    })?;

    add_peer_setter(map, &download_list, "p.snubbed.set", |peer, obj| {
        peer.set_snubbed(convert_to_value(obj, 10, 1)? != 0);
        Ok(())
    })?;
    add_peer_setter(map, &download_list, "p.preferred.set", |peer, obj| {
        peer.set_preferred(convert_to_value(obj, 10, 1)? != 0);
        Ok(())
    })?;
    add_peer_setter(map, &download_list, "p.banned.set", |peer, obj| {
        peer.set_banned(convert_to_value(obj, 10, 1)? != 0);
        Ok(())
    })?;
    add_peer_setter(map, &download_list, "p.unwanted.set", |peer, obj| {
        peer.set_unwanted(convert_to_value(obj, 10, 1)? != 0);
        Ok(())
    })?;

    // The engine does not keep a disconnect queue or a scheduler callback for
    // peer removal. `p.disconnect_delayed` is therefore an alias for the
    // immediate form; both commands document that behaviour instead of
    // pretending the removal was deferred.
    add_peer_disconnect(map, &download_list, "p.disconnect", false)?;
    add_peer_disconnect(map, &download_list, "p.disconnect_delayed", true)?;

    // p.multicall over all peers of a download.
    let mc_list = Arc::clone(&download_list);
    let map_clone = Arc::new(RwLock::new(map.clone()));
    let mc_map = Arc::clone(&map_clone);

    map.insert_func(
        "p.multicall",
        move |target, obj| {
            let hash = match target {
                Target::Download(hash) | Target::Peer(hash, _) => *hash,
                _ => {
                    return Err(RpcError::InputError(
                        "p.multicall requires a download or peer target".into(),
                    ))
                }
            };

            let list = obj.as_list()?;
            if list.is_empty() {
                return Err(RpcError::InputError(
                    "p.multicall: missing arguments".into(),
                ));
            }
            let commands = multicall_commands(list);

            let addresses: Vec<SocketAddress> = {
                let download =
                    mc_list
                        .read()
                        .unwrap()
                        .find(&hash)
                        .ok_or_else(|| RpcError::Custom {
                            code: -500,
                            message: "Download not found".into(),
                        })?;
                let guard = download.read().unwrap();
                let found: Vec<SocketAddress> = guard
                    .download()
                    .peer_infos()
                    .into_iter()
                    .map(|peer| peer.socket_address)
                    .collect();
                found
            };

            let map_guard = mc_map.read().unwrap();
            let mut rows = Vec::with_capacity(addresses.len());

            for address in addresses {
                let peer_target = Target::Peer(hash, address);
                let mut row = Vec::with_capacity(commands.len());
                for command in commands {
                    let cmd_str = rt_rpc::convert_to_string(command)?;
                    let cmd = if cmd_str.contains('=') {
                        cmd_str
                    } else {
                        format!("{cmd_str}=")
                    };
                    match parse_command(&cmd, &peer_target, &map_guard, true) {
                        Ok((value, _)) => row.push(value),
                        Err(_) => row.push(Object::create_empty()),
                    }
                }
                rows.push(Object::create_list_with(row));
            }

            Ok(Object::create_list_with(rows))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "<commands...>",
        "Perform multicall over peers.",
    )?;

    // p.call_target is a peer-id based dispatcher: the C++ surface takes the
    // download hash, the 20-byte peer id and a command key, then calls that
    // command with the peer as target. Keep a snapshot of the command map
    // registered so far (the same approach as p.multicall).
    let call_map = Arc::new(RwLock::new(map.clone()));
    let call_map_guard = Arc::clone(&call_map);
    let call_list = Arc::clone(&download_list);
    map.insert_func(
        "p.call_target",
        move |_target, obj| {
            let list = obj.as_list()?;
            if list.len() < 3 {
                return Err(RpcError::InputError(
                    "p.call_target: too few arguments".into(),
                ));
            }

            let hash_str = convert_to_string(&list[0])?;
            let hash = HashString::from_hex(&hash_str).ok_or_else(|| {
                RpcError::InputError("p.call_target: invalid download hash".into())
            })?;

            let peer_id_str = convert_to_string(&list[1])?;
            if peer_id_str.len() != 40 {
                return Err(RpcError::InputError(
                    "p.call_target: peer id target is not 40 bytes long".into(),
                ));
            }
            let peer_id = HashString::from_hex(&peer_id_str).ok_or_else(|| {
                RpcError::InputError("p.call_target: peer id target is not a hex string".into())
            })?;

            let command_key = convert_to_string(&list[2])?;
            if command_key.is_empty() {
                return Err(RpcError::InputError(
                    "p.call_target: empty command name".into(),
                ));
            }

            let address = {
                let download =
                    call_list
                        .read()
                        .unwrap()
                        .find(&hash)
                        .ok_or_else(|| RpcError::Custom {
                            code: -500,
                            message: "Download not found".into(),
                        })?;
                let guard = download.read().unwrap();
                let found = guard
                    .download()
                    .peer_info(&peer_id)
                    .map(|peer| peer.socket_address);
                found
            };
            let address = address
                .ok_or_else(|| RpcError::InputError("p.call_target: could not find peer".into()))?;

            let command_arg = match list.len() {
                3 => Object::create_empty(),
                4 => list[3].clone(),
                _ => Object::create_list_with(list[3..].to_vec()),
            };

            let map = call_map_guard.read().unwrap();
            map.call_command(
                &command_key,
                &command_arg,
                &Target::Peer(hash, address),
                true,
            )
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "<hash>, <peer_id>, <command>, [args...]",
        "Call a command with a peer selected by peer id.",
    )?;

    Ok(())
}
