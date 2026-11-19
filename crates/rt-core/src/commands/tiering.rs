use crate::download_list::DownloadList;
use crate::tiering::{TierCondition, TierDefinition, TierTransitionRule, TieringManager};
use lt_common::Object;
use rt_rpc::command_flags::*;
use rt_rpc::{convert_to_string, convert_to_value, CommandMap, RpcError};
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

pub fn register_tiering_commands(
    map: &mut CommandMap,
    manager: Arc<TieringManager>,
    download_list: Arc<RwLock<DownloadList>>,
) -> Result<(), RpcError> {
    let mgr = Arc::clone(&manager);
    map.insert_func(
        "tiering.enabled",
        move |_target, _obj| {
            let config = mgr.config();
            let enabled = config.read().map(|c| c.enabled).unwrap_or(false);
            Ok(Object::from(if enabled { 1i64 } else { 0i64 }))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "Returns 1 if automated storage tiering is enabled, 0 otherwise.",
    )?;

    let mgr = Arc::clone(&manager);
    map.insert_func(
        "tiering.enabled.set",
        move |_target, obj| {
            let val = convert_to_value(obj, 10, 0)?;
            let enabled = val != 0;
            if let Ok(mut config) = mgr.config().write() {
                config.enabled = enabled;
            }
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "<0|1>",
        "Enable or disable automated storage tiering.",
    )?;

    let mgr = Arc::clone(&manager);
    let dl_list = Arc::clone(&download_list);
    map.insert_func(
        "tiering.check",
        move |_target, _obj| {
            let moved = {
                let list = dl_list.read().unwrap();
                mgr.check_tier_transitions(&list)
            };
            let mut res = Object::create_list();
            for hash in moved {
                let _ = res.insert_back(Object::from(hash.as_str()));
            }
            Ok(res)
        },
        FLAG_PUBLIC_RPC,
        "",
        "Evaluate tier transition rules for all torrents and move qualifying ones immediately.",
    )?;

    let mgr = Arc::clone(&manager);
    map.insert_func(
        "tiering.tier.add",
        move |_target, obj| {
            let s = convert_to_string(obj)?;
            let parts: Vec<&str> = s.split(',').map(|p| p.trim()).collect();
            if parts.len() < 2 {
                return Err(RpcError::InputError(
                    "tiering.tier.add requires <name>,<path>[,<capacity_bytes>]".into(),
                ));
            }
            let name = parts[0].to_string();
            let path = PathBuf::from(parts[1]);
            let capacity = if parts.len() >= 3 {
                parts[2].parse::<u64>().unwrap_or(0)
            } else {
                0
            };

            if let Ok(mut config) = mgr.config().write() {
                config.tiers.push(TierDefinition::new(name, path, capacity));
            }
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "<name>,<path>[,<capacity_bytes>]",
        "Add a storage tier definition.",
    )?;

    let mgr = Arc::clone(&manager);
    map.insert_func(
        "tiering.rule.add",
        move |_target, obj| {
            let s = convert_to_string(obj)?;
            let parts: Vec<&str> = s.split(',').map(|p| p.trim()).collect();
            if parts.len() < 3 {
                return Err(RpcError::InputError(
                    "tiering.rule.add requires <from_tier>,<to_tier>,<condition1>[,<condition2>...]".into(),
                ));
            }
            let from_tier: u8 = parts[0]
                .parse()
                .map_err(|_| RpcError::InputError("invalid from_tier index".into()))?;
            let to_tier: u8 = parts[1]
                .parse()
                .map_err(|_| RpcError::InputError("invalid to_tier index".into()))?;

            let mut conditions = Vec::new();
            for cond_str in &parts[2..] {
                if *cond_str == "complete" {
                    conditions.push(TierCondition::IsComplete);
                } else if let Some(stripped) = cond_str.strip_prefix("time=") {
                    let secs = stripped
                        .parse()
                        .map_err(|_| RpcError::InputError("invalid time= seconds".into()))?;
                    conditions.push(TierCondition::TimeOnTier(secs));
                } else if let Some(stripped) = cond_str.strip_prefix("ratio=") {
                    let ratio = stripped
                        .parse()
                        .map_err(|_| RpcError::InputError("invalid ratio= value".into()))?;
                    conditions.push(TierCondition::MinRatio(ratio));
                } else if let Some(stripped) = cond_str.strip_prefix("uploaded=") {
                    let bytes = stripped
                        .parse()
                        .map_err(|_| RpcError::InputError("invalid uploaded= bytes".into()))?;
                    conditions.push(TierCondition::MinUploaded(bytes));
                } else if let Some(stripped) = cond_str.strip_prefix("inactive=") {
                    let secs = stripped
                        .parse()
                        .map_err(|_| RpcError::InputError("invalid inactive= seconds".into()))?;
                    conditions.push(TierCondition::InactiveSince(secs));
                }
            }

            if let Ok(mut config) = mgr.config().write() {
                config
                    .rules
                    .push(TierTransitionRule::new(from_tier, to_tier, conditions));
            }
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "<from_tier>,<to_tier>,<condition1>...",
        "Add a tier transition rule.",
    )?;

    let mgr = Arc::clone(&manager);
    map.insert_func(
        "tiering.tiers",
        move |_target, _obj| {
            let config = mgr.config();
            let guard = match config.read() {
                Ok(g) => g,
                Err(_) => return Ok(Object::create_list()),
            };
            let mut list = Object::create_list();
            for tier in &guard.tiers {
                let mut map = Object::create_map();
                let _ = map.insert_key("name", Object::from(tier.name.as_str()));
                let _ = map.insert_key(
                    "path",
                    Object::from(tier.path.to_string_lossy().to_string().as_str()),
                );
                let _ = map.insert_key("capacity_bytes", Object::from(tier.capacity_bytes as i64));
                let _ = list.insert_back(map);
            }
            Ok(list)
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "List all configured storage tiers.",
    )?;

    Ok(())
}
