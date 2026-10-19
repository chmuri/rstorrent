//! `protocol.*` encryption, PEX and choke heuristic commands.

use std::sync::{Arc, RwLock};

use lt_common::option_strings::{option_find_string, OptionEnum};
use lt_common::Object;
use lt_net::config::{EncryptionMode, NetworkConfig};
use rt_rpc::command_flags::*;
use rt_rpc::{convert_to_string, convert_to_value, CommandMap, RpcError};

#[derive(Debug, Clone)]
pub struct ProtocolConfig {
    pub pex: bool,
    pub connection_leech: String,
    pub connection_seed: String,
    pub up_leech: String,
    pub up_seed: String,
    pub down_leech: String,
    pub down_seed: String,
}

impl Default for ProtocolConfig {
    fn default() -> Self {
        Self {
            pex: true,
            connection_leech: "leech".into(),
            connection_seed: "seed".into(),
            up_leech: "upload_leech".into(),
            up_seed: "upload_leech".into(),
            down_leech: "download_leech".into(),
            down_seed: "download_leech".into(),
        }
    }
}

fn mode_name(mode: EncryptionMode) -> &'static str {
    match mode {
        EncryptionMode::Deny => "deny",
        EncryptionMode::Allow => "allow",
        EncryptionMode::Prefer => "prefer",
        EncryptionMode::Require => "require",
    }
}

fn parse_mode(value: &str) -> Result<EncryptionMode, RpcError> {
    let value = value.trim().to_ascii_lowercase();
    let value = value
        .strip_prefix("handshake_")
        .or_else(|| value.strip_prefix("stream_"))
        .unwrap_or(&value);

    match value {
        "deny" | "disable" | "disabled" | "off" | "none" => Ok(EncryptionMode::Deny),
        "allow" | "enabled" | "allow_incoming" | "try_outgoing" | "enable_retry"
        | "prefer_plaintext" => Ok(EncryptionMode::Allow),
        "prefer" => Ok(EncryptionMode::Prefer),
        "require" | "required" | "require_rc4" => Ok(EncryptionMode::Require),
        _ => Err(RpcError::InputError(format!(
            "invalid encryption mode '{value}'"
        ))),
    }
}

fn set_modes(
    network: &NetworkConfig,
    handshake: EncryptionMode,
    stream: EncryptionMode,
) -> Result<(), RpcError> {
    network
        .set_encryption_modes(handshake, stream)
        .map_err(|err| RpcError::InputError(err.to_string()))
}

/// Validates the protocol-wide connection type. An empty value is retained for
/// compatibility with rc files that clear the preference and let per-download
/// settings win.
fn valid_connection_type(value: &str) -> bool {
    value.is_empty() || option_find_string(OptionEnum::ConnectionType, value).is_some()
}

fn valid_upload_heuristic(value: &str) -> bool {
    value.is_empty()
        || option_find_string(OptionEnum::ChokeHeuristicsUpload, value).is_some()
}

fn valid_download_heuristic(value: &str) -> bool {
    value.is_empty()
        || option_find_string(OptionEnum::ChokeHeuristicsDownload, value).is_some()
}

fn validate_protocol_value(name: &str, value: &str) -> Result<(), RpcError> {
    let valid = match name {
        "protocol.connection.leech" | "protocol.connection.seed" => {
            valid_connection_type(value)
        }
        "protocol.choke_heuristics.up.leech" | "protocol.choke_heuristics.up.seed" => {
            valid_upload_heuristic(value)
        }
        "protocol.choke_heuristics.down.leech" | "protocol.choke_heuristics.down.seed" => {
            valid_download_heuristic(value)
        }
        _ => true,
    };

    if valid {
        Ok(())
    } else {
        Err(RpcError::InputError(format!(
            "Invalid value '{}' for {}.",
            value, name
        )))
    }
}

/// Implements the C++ `apply_encryption` semantics, including the obsolete
/// option names (`none`, `allow_incoming`, `require_RC4`, ...).
fn apply_encryption(network: &NetworkConfig, obj: &Object) -> Result<(), RpcError> {
    let values = if let Ok(list) = obj.as_list() {
        list.iter()
            .map(convert_to_string)
            .collect::<Result<Vec<_>, _>>()?
    } else {
        vec![convert_to_string(obj)?]
    };

    if values.is_empty() {
        return Err(RpcError::InputError(
            "No encryption options specified.".into(),
        ));
    }

    let has_obsolete_only = values.iter().any(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "none" | "allow_incoming" | "try_outgoing" | "enable_retry"
                | "prefer_plaintext" | "require_rc4"
        )
    });
    if has_obsolete_only {
        return apply_obsolete_encryption(network, &values);
    }

    if values.len() == 1 {
        if let Ok(mode) = parse_mode(&values[0]) {
            return set_modes(network, mode, mode);
        }
    }

    if values.len() == 2 {
        if let (Ok(handshake), Ok(stream)) = (parse_mode(&values[0]), parse_mode(&values[1])) {
            return set_modes(network, handshake, stream);
        }
    }

    apply_obsolete_encryption(network, &values)
}

fn apply_obsolete_encryption(
    network: &NetworkConfig,
    values: &[String],
) -> Result<(), RpcError> {
    let mut handshake = EncryptionMode::Allow;
    let mut stream = EncryptionMode::Allow;

    for value in values {
        match value.trim().to_ascii_lowercase().as_str() {
            "none" => {
                handshake = EncryptionMode::Deny;
                stream = EncryptionMode::Deny;
                break;
            }
            "allow_incoming" | "try_outgoing" | "enable_retry" | "prefer_plaintext" => {}
            "require" => handshake = EncryptionMode::Require,
            "require_rc4" => {
                handshake = EncryptionMode::Require;
                stream = EncryptionMode::Require;
                break;
            }
            other => {
                return Err(RpcError::InputError(format!(
                    "Invalid encryption option: '{}'",
                    other
                )))
            }
        }
    }

    set_modes(network, handshake, stream)
}

pub fn register_protocol_commands(
    map: &mut CommandMap,
    network: NetworkConfig,
) -> Result<(), RpcError> {
    let protocol = Arc::new(RwLock::new(ProtocolConfig::default()));

    let net_get = network.clone();
    map.insert_func(
        "protocol.encryption",
        move |_target, _obj| {
            let (handshake, stream) = net_get.encryption_modes();
            Ok(Object::from(format!(
                "{},{}",
                mode_name(handshake),
                mode_name(stream)
            )))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "Get handshake/stream encryption modes.",
    )?;

    let net_set = network.clone();
    map.insert_func(
        "protocol.encryption.set",
        move |_target, obj| {
            apply_encryption(&net_set, obj)?;
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "<handshake>, [stream]",
        "Set encryption modes.",
    )?;

    // Legacy redirect: CMD_REDIRECT("encryption", "protocol.encryption.set")
    let net_alias = network.clone();
    map.insert_func(
        "encryption",
        move |_target, obj| {
            apply_encryption(&net_alias, obj)?;
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "<handshake>, [stream]",
        "Legacy alias for protocol.encryption.set.",
    )?;

    for (name, handshake) in [("protocol.encryption.handshake", true), ("protocol.encryption.stream", false)] {
        let net_get = network.clone();
        map.insert_func(
            name,
            move |_target, _obj| {
                let (handshake_mode, stream_mode) = net_get.encryption_modes();
                Ok(Object::from(if handshake {
                    mode_name(handshake_mode)
                } else {
                    mode_name(stream_mode)
                }))
            },
            FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
            "",
            name,
        )?;
    }

    let net_hs = network.clone();
    map.insert_func(
        "protocol.encryption.handshake.set",
        move |_target, obj| {
            let mode = parse_mode(&convert_to_string(obj)?)?;
            let (_, stream) = net_hs.encryption_modes();
            net_hs
                .set_encryption_modes(mode, stream)
                .map_err(|err| RpcError::InputError(err.to_string()))?;
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "<mode>",
        "Set handshake encryption mode.",
    )?;

    let net_stream = network.clone();
    map.insert_func(
        "protocol.encryption.stream.set",
        move |_target, obj| {
            let mode = parse_mode(&convert_to_string(obj)?)?;
            let (handshake, _) = net_stream.encryption_modes();
            net_stream
                .set_encryption_modes(handshake, mode)
                .map_err(|err| RpcError::InputError(err.to_string()))?;
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "<mode>",
        "Set stream encryption mode.",
    )?;

    let p_get = Arc::clone(&protocol);
    map.insert_func(
        "protocol.pex",
        move |_target, _obj| {
            Ok(Object::from(if p_get.read().unwrap().pex { 1i64 } else { 0 }))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "Check whether PEX is enabled.",
    )?;

    let p_set = Arc::clone(&protocol);
    map.insert_func(
        "protocol.pex.set",
        move |_target, obj| {
            let value = convert_to_value(obj, 0, 1)
                .map_err(|_| RpcError::InputError("Not a value.".into()))?;
            p_set.write().unwrap().pex = value != 0;
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "<0|1>",
        "Enable or disable PEX.",
    )?;

    for name in [
        "protocol.connection.leech",
        "protocol.connection.seed",
        "protocol.choke_heuristics.up.leech",
        "protocol.choke_heuristics.up.seed",
        "protocol.choke_heuristics.down.leech",
        "protocol.choke_heuristics.down.seed",
    ] {
        let p_get = Arc::clone(&protocol);
        map.insert_func(
            name,
            move |_target, _obj| {
                let config = p_get.read().unwrap();
                let value = match name {
                    "protocol.connection.leech" => config.connection_leech.clone(),
                    "protocol.connection.seed" => config.connection_seed.clone(),
                    "protocol.choke_heuristics.up.leech" => config.up_leech.clone(),
                    "protocol.choke_heuristics.up.seed" => config.up_seed.clone(),
                    "protocol.choke_heuristics.down.leech" => config.down_leech.clone(),
                    _ => config.down_seed.clone(),
                };
                Ok(Object::from(value))
            },
            FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
            "",
            name,
        )?;

        let p_set = Arc::clone(&protocol);
        let setter = format!("{name}.set");
        map.insert_func(
            &setter,
            move |_target, obj| {
                let value = convert_to_string(obj)?.trim().to_ascii_lowercase();
                validate_protocol_value(name, &value)?;
                let mut config = p_set.write().unwrap();
                match name {
                    "protocol.connection.leech" => config.connection_leech = value,
                    "protocol.connection.seed" => config.connection_seed = value,
                    "protocol.choke_heuristics.up.leech" => config.up_leech = value,
                    "protocol.choke_heuristics.up.seed" => config.up_seed = value,
                    "protocol.choke_heuristics.down.leech" => config.down_leech = value,
                    _ => config.down_seed = value,
                }
                Ok(Object::create_empty())
            },
            FLAG_PUBLIC_RPC,
            "<value>",
            &setter,
        )?;
    }

    for (name, field) in [
        ("connection_leech", "protocol.connection.leech.set"),
        ("connection_seed", "protocol.connection.seed.set"),
    ] {
        let p_set = Arc::clone(&protocol);
        map.insert_func(
            name,
            move |_target, obj| {
                let value = convert_to_string(obj)?.trim().to_ascii_lowercase();
                if !valid_connection_type(&value) {
                    return Err(RpcError::InputError(format!(
                        "Invalid connection type '{}'.",
                        value
                    )));
                }
                let mut config = p_set.write().unwrap();
                match field {
                    "protocol.connection.leech.set" => config.connection_leech = value,
                    _ => config.connection_seed = value,
                }
                Ok(Object::create_empty())
            },
            FLAG_PUBLIC_RPC,
            "<value>",
            "Legacy connection-type alias.",
        )?;
    }

    Ok(())
}
