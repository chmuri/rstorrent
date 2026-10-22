//! `ratio.*` and `group.seeding.ratio.*` settings.
//!
//! Ratio values use the legacy fixed-point convention: 200 means 2.00.

use std::sync::{Arc, RwLock};

use lt_common::Object;
use rt_rpc::command_flags::*;
use rt_rpc::{convert_to_string, convert_to_value, CommandMap, RpcError};

#[derive(Debug, Clone)]
pub struct RatioConfig {
    pub enabled: bool,
    pub min: i64,
    pub max: i64,
    pub upload: i64,
}

impl Default for RatioConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            min: 200,
            max: 300,
            upload: 20 << 20,
        }
    }
}

/// Returns whether a seeding download satisfies the ratio rule.
///
/// `downloaded == 0` only requires the minimum upload threshold, avoiding a
/// division by zero. `max <= 0` disables the maximum-ratio branch.
pub fn ratio_reached(uploaded: i64, downloaded: i64, config: &RatioConfig) -> bool {
    if !config.enabled {
        return false;
    }
    if downloaded <= 0 {
        return uploaded >= config.upload;
    }

    let min_reached = uploaded >= config.upload
        && uploaded.saturating_mul(100) >= downloaded.saturating_mul(config.min);
    let max_exceeded =
        config.max > 0 && uploaded.saturating_mul(100) > downloaded.saturating_mul(config.max);
    min_reached || max_exceeded
}

fn add_value(
    map: &mut CommandMap,
    config: &Arc<RwLock<RatioConfig>>,
    name: &str,
    get: fn(&RatioConfig) -> i64,
    set: fn(&mut RatioConfig, i64),
) -> Result<(), RpcError> {
    let get_config = Arc::clone(config);
    map.insert_func(
        name,
        move |_target, _obj| Ok(Object::from(get(&get_config.read().unwrap()))),
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        name,
    )?;

    let set_config = Arc::clone(config);
    map.insert_func(
        &format!("{name}.set"),
        move |_target, obj| {
            let value = convert_to_value(obj, 10, 1)
                .map_err(|_| RpcError::InputError("Not a value.".to_string()))?;
            set(&mut set_config.write().unwrap(), value);
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "<value>",
        name,
    )
}

pub fn register_ratio_commands(map: &mut CommandMap) -> Result<(), RpcError> {
    let config = Arc::new(RwLock::new(RatioConfig::default()));

    for (name, enabled) in [
        ("ratio.enable", true),
        ("ratio.disable", false),
        ("group.seeding.ratio.enable", true),
        ("group.seeding.ratio.disable", false),
    ] {
        let config = Arc::clone(&config);
        map.insert_func(
            name,
            move |_target, _obj| {
                config.write().unwrap().enabled = enabled;
                Ok(Object::create_empty())
            },
            FLAG_PUBLIC_RPC,
            "",
            name,
        )?;
    }

    for (name, get, set) in [
        (
            "ratio.min",
            (|config: &RatioConfig| config.min) as fn(&RatioConfig) -> i64,
            (|config: &mut RatioConfig, value| config.min = value) as fn(&mut RatioConfig, i64),
        ),
        (
            "ratio.max",
            |config| config.max,
            |config, value| config.max = value,
        ),
        (
            "ratio.upload",
            |config| config.upload,
            |config, value| config.upload = value,
        ),
    ] {
        add_value(map, &config, name, get, set)?;
        add_value(map, &config, &format!("group.seeding.{}", name), get, set)?;
    }

    let config_get = Arc::clone(&config);
    map.insert_func(
        "ratio.enabled",
        move |_target, _obj| {
            Ok(Object::from(if config_get.read().unwrap().enabled {
                1i64
            } else {
                0
            }))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "Whether ratio rules are enabled.",
    )?;

    // Common upstream default command: close/ignore a download that reached
    // its ratio. Kept configurable so rc files can override it.
    let command = Arc::new(RwLock::new(
        "d.try_close= ;d.ignore_commands.set=1".to_string(),
    ));
    let command_get = Arc::clone(&command);
    map.insert_func(
        "group.seeding.ratio.command",
        move |_target, _obj| Ok(Object::from(command_get.read().unwrap().clone())),
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "Ratio action command.",
    )?;
    let command_set = Arc::clone(&command);
    map.insert_func(
        "group.seeding.ratio.command.set",
        move |_target, obj| {
            *command_set.write().unwrap() = convert_to_string(obj)?;
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "<command>",
        "Set the ratio action command.",
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ratio_thresholds_avoid_division_by_zero() {
        let config = RatioConfig {
            enabled: true,
            min: 200,
            max: 300,
            upload: 100,
        };

        assert!(!ratio_reached(10, 0, &config));
        assert!(ratio_reached(100, 0, &config));
        assert!(!ratio_reached(199, 100, &config));
        assert!(ratio_reached(200, 100, &config));
        assert!(ratio_reached(301, 100, &config));
    }

    #[test]
    fn disabled_ratio_never_matches() {
        let config = RatioConfig::default();
        assert!(!ratio_reached(1_000_000, 1, &config));
    }
}
