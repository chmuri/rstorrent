//! RPC command surface for the rules engine (plan.md §6.4): `seed.*` global
//! seed-policy settings, the `label.*` registry and `rules.*` automation-rule
//! management. All handlers are thin bridges onto [`RuleService`]; domain
//! validation lives in `rt-rules` (compile/resolve) and the store.
//!
//! Contracts:
//! - Complex types (Rule, SeedPolicy, LabelDef lists, history, export blob)
//!   travel as JSON strings; parse failures are [`RpcError::InputError`].
//! - `seed.ratio` is an f64. `Object` has no float variant, so the getter
//!   returns a string flagged [`FLAG_XMLRPC_DOUBLE`] (the XML-RPC layer
//!   renders it as `<double>`); the setter accepts values and strings alike.
//!   `0` means unlimited and is stored as `LimitValue::Unlimited` — a bare
//!   `Set(0)` would fire immediately in `policy_decision`.
//! - Trust flags (§6.4): getters/list are `PUBLIC_RPC | UNTRUSTED_SAFE`
//!   except `rules.get/list/export/test`, which are trusted-only (webhook
//!   URLs / torrent names). Every mutation is `PUBLIC_RPC` (trusted).
//! - `rules.check` forces an async worker pass from a sync command: it uses
//!   `block_in_place` + `Handle::block_on` on the multi-thread daemon runtime
//!   and returns an error (never panics) outside such a runtime.
//!
//! Registration happens in `Control::initialize` once the rules service is up
//! (after the builtins and the first TOML apply pass); the config layer then
//! re-applies `[seed]`/`[rules]` and calls [`apply_legacy_ratio_fallback`].

use std::sync::Arc;

use lt_common::object::flags::FLAG_XMLRPC_DOUBLE;
use lt_common::Object;
use rt_core::download::{format_seed_action, parse_seed_action};
use rt_rpc::command_flags::*;
use rt_rpc::{convert_to_string, convert_to_value, CommandMap, RpcError, Target};
use rt_rules::{LabelDef, LimitMode, LimitValue, Rule, SeedAction, SeedPolicy};
use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::rules::{RuleService, RulesSettings, SeedSettings};
use crate::rules_store::{ExportBlob, RuleStoreError};

/// Default number of history entries returned by `rules.history`.
const HISTORY_DEFAULT_LIMIT: usize = 100;
/// Hard cap for `rules.history` (plan §6.4; the store itself caps at 500).
const HISTORY_MAX_LIMIT: usize = 500;

// ---- Shared helpers ----

/// Store/validation failures surface as input errors; I/O and serialization
/// failures are internal (the caller's document was fine).
fn store_error(err: RuleStoreError) -> RpcError {
    match err {
        RuleStoreError::Validation(err) => RpcError::InputError(err.to_string()),
        other => RpcError::InternalError(other.to_string()),
    }
}

/// Parses a JSON string argument into `T`.
fn parse_json_arg<T: DeserializeOwned>(obj: &Object, what: &str) -> Result<T, RpcError> {
    let text = convert_to_string(obj)?;
    serde_json::from_str(&text)
        .map_err(|err| RpcError::InputError(format!("invalid {what} JSON: {err}")))
}

/// Serializes a value into a JSON string object.
fn json_object<T: Serialize>(value: &T) -> Result<Object, RpcError> {
    let json = serde_json::to_string(value)
        .map_err(|err| RpcError::InternalError(format!("JSON serialization failed: {err}")))?;
    Ok(Object::from(json))
}

/// Argument strings: a real argument list stays element-wise, a single
/// string argument stays whole (comma splitting is command-specific).
fn arg_strings(obj: &Object) -> Result<Vec<String>, RpcError> {
    if let Ok(list) = obj.as_list() {
        return list.iter().map(convert_to_string).collect();
    }
    Ok(vec![convert_to_string(obj)?])
}

/// Two-argument commands accept either a two-element argument list or one
/// `<first>,<second>` string; only the first comma separates (the second
/// argument may be JSON full of commas).
fn two_args(obj: &Object, usage: &str) -> Result<(String, String), RpcError> {
    let args = arg_strings(obj)?;
    match args.len() {
        2 => Ok((args[0].trim().to_string(), args[1].trim().to_string())),
        1 => {
            let (first, second) = args[0]
                .split_once(',')
                .ok_or_else(|| RpcError::InputError(usage.to_string()))?;
            Ok((first.trim().to_string(), second.trim().to_string()))
        }
        _ => Err(RpcError::InputError(usage.to_string())),
    }
}

fn parse_rule_id(text: &str) -> Result<u64, RpcError> {
    text.trim()
        .parse::<u64>()
        .map_err(|_| RpcError::InputError(format!("invalid rule id '{}'", text.trim())))
}

/// 0/1 switch argument (any non-zero value enables, tiering.enabled pattern).
fn parse_switch(obj: &Object) -> Result<bool, RpcError> {
    Ok(convert_to_value(obj, 10, 0)? != 0)
}

/// Non-negative seconds argument.
fn parse_secs(obj: &Object) -> Result<u64, RpcError> {
    let value = convert_to_value(obj, 10, 0)?;
    if value < 0 {
        return Err(RpcError::InputError(
            "seconds must be >= 0 (0 = unlimited)".into(),
        ));
    }
    Ok(value as u64)
}

/// Ratio argument: integer values and (double-flagged) strings alike.
fn parse_ratio(obj: &Object) -> Result<f64, RpcError> {
    let ratio = if let Ok(value) = obj.as_value() {
        value as f64
    } else {
        let text = convert_to_string(obj)?;
        text.trim().parse::<f64>().map_err(|_| {
            RpcError::InputError(format!("invalid ratio '{text}': expected a number"))
        })?
    };
    if !ratio.is_finite() || ratio < 0.0 {
        return Err(RpcError::InputError(
            "ratio must be a finite number >= 0 (0 = unlimited)".into(),
        ));
    }
    Ok(ratio)
}

/// f64 wire form: a string flagged as an XML-RPC double.
fn ratio_object(ratio: f64) -> Object {
    let mut obj = Object::create_string(format!("{ratio}"));
    obj.set_flags(FLAG_XMLRPC_DOUBLE);
    obj
}

/// Global-level wire value of a ratio limit: `0` = unlimited.
fn wire_ratio(limit: &LimitValue<f64>) -> f64 {
    match limit {
        LimitValue::Set(ratio) => *ratio,
        _ => 0.0,
    }
}

/// Global-level wire value of a seconds limit: `0` = unlimited.
fn wire_secs(limit: &LimitValue<u64>) -> i64 {
    match limit {
        LimitValue::Set(secs) => *secs as i64,
        _ => 0,
    }
}

fn mode_str(mode: LimitMode) -> &'static str {
    match mode {
        LimitMode::Any => "any",
        LimitMode::All => "all",
    }
}

fn parse_mode(text: &str) -> Result<LimitMode, RpcError> {
    match text.trim() {
        "any" => Ok(LimitMode::Any),
        "all" => Ok(LimitMode::All),
        other => Err(RpcError::InputError(format!(
            "invalid seed mode '{other}': expected any|all"
        ))),
    }
}

/// Global action wire form; `None` resolves to `stop` (resolve.rs default).
fn action_string(action: &Option<SeedAction>) -> String {
    action
        .as_ref()
        .map(format_seed_action)
        .unwrap_or_else(|| "stop".to_string())
}

fn bool_object(enabled: bool) -> Object {
    Object::from(if enabled { 1i64 } else { 0i64 })
}

/// Validates a label/global seed policy beyond what serde enforces
/// (same spirit as rule compilation: reject before persisting).
fn validate_policy(policy: &SeedPolicy) -> Result<(), RpcError> {
    if let LimitValue::Set(ratio) = policy.ratio {
        if !ratio.is_finite() || ratio < 0.0 {
            return Err(RpcError::InputError(
                "policy ratio must be a finite number >= 0".into(),
            ));
        }
    }
    if let Some(SeedAction::Throttle(name)) = &policy.action {
        if name.trim().is_empty() {
            return Err(RpcError::InputError(
                "throttle action requires a non-empty throttle name".into(),
            ));
        }
    }
    Ok(())
}

fn find_label(service: &RuleService, name: &str) -> Result<LabelDef, RpcError> {
    service
        .list_labels()
        .into_iter()
        .find(|label| label.name == name)
        .ok_or_else(|| RpcError::InputError(format!("unknown label {name}")))
}

/// Reads a legacy i64 setting; `None` when the command is absent or fails.
fn query_value(map: &CommandMap, name: &str) -> Option<i64> {
    map.call_command(name, &Object::create_empty(), &Target::None, true)
        .ok()?
        .as_value()
        .ok()
}

/// Reads a legacy string setting; `None` when the command is absent or fails.
fn query_string(map: &CommandMap, name: &str) -> Option<String> {
    map.call_command(name, &Object::create_empty(), &Target::None, true)
        .ok()?
        .as_str()
        .ok()
        .map(str::to_string)
}

// ---- Registration scaffolding ----

/// Registers a `seed.*` getter over the global settings snapshot.
fn seed_getter<F>(
    map: &mut CommandMap,
    service: &Arc<RuleService>,
    name: &str,
    doc: &str,
    get: F,
) -> Result<(), RpcError>
where
    F: Fn(&SeedSettings) -> Object + Send + Sync + 'static,
{
    let svc = Arc::clone(service);
    map.insert_func(
        name,
        move |_target, _obj| Ok(get(&svc.seed_settings())),
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        doc,
    )
}

/// Registers a `seed.*.set` mutation: snapshot → change one field → store.
/// The service is only updated when the argument parsed cleanly.
fn seed_setter<F>(
    map: &mut CommandMap,
    service: &Arc<RuleService>,
    name: &str,
    parm: &str,
    doc: &str,
    mutate: F,
) -> Result<(), RpcError>
where
    F: Fn(&mut SeedSettings, &Object) -> Result<(), RpcError> + Send + Sync + 'static,
{
    let svc = Arc::clone(service);
    map.insert_func(
        &format!("{name}.set"),
        move |_target, obj| {
            let mut settings = svc.seed_settings();
            mutate(&mut settings, obj)?;
            svc.set_seed_settings(settings);
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        parm,
        doc,
    )
}

/// Registers a `rules.*` settings getter (kill switch / destructive gate).
fn rules_getter<F>(
    map: &mut CommandMap,
    service: &Arc<RuleService>,
    name: &str,
    doc: &str,
    get: F,
) -> Result<(), RpcError>
where
    F: Fn(&RulesSettings) -> Object + Send + Sync + 'static,
{
    let svc = Arc::clone(service);
    map.insert_func(
        name,
        move |_target, _obj| Ok(get(&svc.rules_settings())),
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        doc,
    )
}

/// Registers a `rules.*` settings mutation.
fn rules_setter<F>(
    map: &mut CommandMap,
    service: &Arc<RuleService>,
    name: &str,
    doc: &str,
    mutate: F,
) -> Result<(), RpcError>
where
    F: Fn(&mut RulesSettings, &Object) -> Result<(), RpcError> + Send + Sync + 'static,
{
    let svc = Arc::clone(service);
    map.insert_func(
        &format!("{name}.set"),
        move |_target, obj| {
            let mut settings = svc.rules_settings();
            mutate(&mut settings, obj)?;
            svc.set_rules_settings(settings);
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "<0|1>",
        doc,
    )
}

/// Registers every `seed.*`, `label.*` and `rules.*` command from plan §6.4.
pub fn register_rules_commands(
    map: &mut CommandMap,
    service: Arc<RuleService>,
) -> Result<(), RpcError> {
    register_seed_commands(map, &service)?;
    register_label_commands(map, &service)?;
    register_rule_commands(map, &service)?;
    Ok(())
}

// ---- seed.* ----

fn register_seed_commands(
    map: &mut CommandMap,
    service: &Arc<RuleService>,
) -> Result<(), RpcError> {
    seed_getter(
        map,
        service,
        "seed.enabled",
        "Whether global seed policies are active (0/1).",
        |s| bool_object(s.enabled),
    )?;
    seed_setter(
        map,
        service,
        "seed.enabled",
        "<0|1>",
        "Enable or disable global seed policies (master switch).",
        |s, obj| {
            s.enabled = parse_switch(obj)?;
            Ok(())
        },
    )?;

    seed_getter(
        map,
        service,
        "seed.ratio",
        "Global default share ratio (0 = unlimited).",
        |s| ratio_object(wire_ratio(&s.policy.ratio)),
    )?;
    seed_setter(
        map,
        service,
        "seed.ratio",
        "<ratio>",
        "Set the global default share ratio (1.0 = 100%, 0 = unlimited).",
        |s, obj| {
            let ratio = parse_ratio(obj)?;
            s.policy.ratio = if ratio > 0.0 {
                LimitValue::Set(ratio)
            } else {
                LimitValue::Unlimited
            };
            Ok(())
        },
    )?;

    seed_getter(
        map,
        service,
        "seed.time",
        "Global seeding time limit in seconds (0 = unlimited).",
        |s| Object::from(wire_secs(&s.policy.seeding_time_secs)),
    )?;
    seed_setter(
        map,
        service,
        "seed.time",
        "<secs>",
        "Set the global seeding time limit in seconds (0 = unlimited).",
        |s, obj| {
            let secs = parse_secs(obj)?;
            s.policy.seeding_time_secs = if secs > 0 {
                LimitValue::Set(secs)
            } else {
                LimitValue::Unlimited
            };
            Ok(())
        },
    )?;

    seed_getter(
        map,
        service,
        "seed.idle",
        "Global inactive limit in seconds (0 = unlimited).",
        |s| Object::from(wire_secs(&s.policy.inactive_secs)),
    )?;
    seed_setter(
        map,
        service,
        "seed.idle",
        "<secs>",
        "Set the global inactive (no-upload) limit in seconds (0 = unlimited).",
        |s, obj| {
            let secs = parse_secs(obj)?;
            s.policy.inactive_secs = if secs > 0 {
                LimitValue::Set(secs)
            } else {
                LimitValue::Unlimited
            };
            Ok(())
        },
    )?;

    seed_getter(
        map,
        service,
        "seed.mode",
        "How seed limits combine: any | all.",
        |s| Object::from(mode_str(s.policy.mode)),
    )?;
    seed_setter(
        map,
        service,
        "seed.mode",
        "<any|all>",
        "Set how seed limits combine: any (first reached) or all.",
        |s, obj| {
            s.policy.mode = parse_mode(&convert_to_string(obj)?)?;
            Ok(())
        },
    )?;

    seed_getter(
        map,
        service,
        "seed.action",
        "Global action when a seed limit is reached.",
        |s| Object::from(action_string(&s.policy.action)),
    )?;
    seed_setter(
        map,
        service,
        "seed.action",
        "<action>",
        "Set the global seed action: stop|close|remove|remove_with_data|throttle:<name>|super_seed|notify|command:<str>.",
        |s, obj| {
            let text = convert_to_string(obj)?;
            let action = parse_seed_action(&text).ok_or_else(|| {
                RpcError::InputError(format!("invalid seed action '{text}'"))
            })?;
            s.policy.action = Some(action);
            Ok(())
        },
    )?;

    seed_getter(
        map,
        service,
        "seed.check_interval",
        "Rules worker tick cadence in seconds.",
        |s| Object::from(s.check_interval_secs as i64),
    )?;
    seed_setter(
        map,
        service,
        "seed.check_interval",
        "<secs>",
        "Set the rules worker tick cadence in seconds (>= 1).",
        |s, obj| {
            let secs = parse_secs(obj)?;
            if secs < 1 {
                return Err(RpcError::InputError(
                    "seed.check_interval must be >= 1 second".into(),
                ));
            }
            s.check_interval_secs = secs;
            Ok(())
        },
    )?;

    let svc = Arc::clone(service);
    map.insert_func(
        "seed.show",
        move |_target, _obj| {
            let settings = svc.seed_settings();
            let mut out = Object::create_map();
            let _ = out.insert_key("enabled", bool_object(settings.enabled));
            let _ = out.insert_key("ratio", ratio_object(wire_ratio(&settings.policy.ratio)));
            let _ = out.insert_key(
                "seeding_time_secs",
                Object::from(wire_secs(&settings.policy.seeding_time_secs)),
            );
            let _ = out.insert_key(
                "inactive_secs",
                Object::from(wire_secs(&settings.policy.inactive_secs)),
            );
            let _ = out.insert_key("mode", Object::from(mode_str(settings.policy.mode)));
            let _ = out.insert_key(
                "action",
                Object::from(action_string(&settings.policy.action)),
            );
            let _ = out.insert_key(
                "check_interval_secs",
                Object::from(settings.check_interval_secs as i64),
            );
            // These are the global settings themselves: every value originates
            // at the global level (per-torrent sources come from d.seed_limits).
            let mut sources = Object::create_map();
            for key in [
                "ratio",
                "seeding_time_secs",
                "inactive_secs",
                "mode",
                "action",
            ] {
                let _ = sources.insert_key(key, Object::from("global"));
            }
            let _ = out.insert_key("sources", sources);
            Ok(out)
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "Show the full global seed settings as a dict (values + sources).",
    )?;

    Ok(())
}

// ---- label.* ----

fn register_label_commands(
    map: &mut CommandMap,
    service: &Arc<RuleService>,
) -> Result<(), RpcError> {
    let svc = Arc::clone(service);
    map.insert_func(
        "label.add",
        move |_target, obj| {
            let args = arg_strings(obj)?;
            let (name, color) = match args.len() {
                1 => match args[0].split_once(',') {
                    Some((name, color)) => (name.to_string(), Some(color.to_string())),
                    None => (args[0].clone(), None),
                },
                2 => (args[0].clone(), Some(args[1].clone())),
                _ => {
                    return Err(RpcError::InputError(
                        "label.add requires <name>[,<color>]".into(),
                    ))
                }
            };
            let label = LabelDef {
                name: name.trim().to_string(),
                color: color
                    .map(|c| c.trim().to_string())
                    .filter(|c| !c.is_empty()),
                policy: SeedPolicy::default(),
                save_path: None,
            };
            svc.add_label(label).map_err(store_error)?;
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "<name>[,<color>]",
        "Register a label (color is #rrggbb for the UI).",
    )?;

    let svc = Arc::clone(service);
    map.insert_func(
        "label.remove",
        move |_target, obj| {
            let name = convert_to_string(obj)?;
            svc.remove_label(name.trim()).map_err(store_error)?;
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "<name>",
        "Remove a label from the registry (torrent d.custom1 values are untouched).",
    )?;

    let svc = Arc::clone(service);
    map.insert_func(
        "label.list",
        move |_target, _obj| json_object(&svc.list_labels()),
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "List registered labels as a JSON array of LabelDef.",
    )?;

    let svc = Arc::clone(service);
    map.insert_func(
        "label.policy.get",
        move |_target, obj| {
            let name = convert_to_string(obj)?;
            let label = find_label(&svc, name.trim())?;
            json_object(&label.policy)
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "<name>",
        "Get a label's seed policy as a JSON SeedPolicy.",
    )?;

    let svc = Arc::clone(service);
    map.insert_func(
        "label.policy.set",
        move |_target, obj| {
            let (name, json) = two_args(obj, "label.policy.set requires <name>,<json SeedPolicy>")?;
            let policy: SeedPolicy = serde_json::from_str(&json)
                .map_err(|err| RpcError::InputError(format!("invalid SeedPolicy JSON: {err}")))?;
            validate_policy(&policy)?;
            let mut label = find_label(&svc, &name)?;
            label.policy = policy;
            svc.update_label(label).map_err(store_error)?;
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "<name>,<json SeedPolicy>",
        "Replace a label's seed policy (validated like rules).",
    )?;

    let svc = Arc::clone(service);
    map.insert_func(
        "label.save_path.set",
        move |_target, obj| {
            let (name, path) = two_args(obj, "label.save_path.set requires <name>,<path>")?;
            let mut label = find_label(&svc, &name)?;
            let path = path.trim();
            label.save_path = if path.is_empty() {
                None
            } else {
                Some(path.to_string())
            };
            svc.update_label(label).map_err(store_error)?;
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "<name>,<path>",
        "Set a label's default save path for new torrents (empty clears it).",
    )?;

    Ok(())
}

// ---- rules.* ----

fn register_rule_commands(
    map: &mut CommandMap,
    service: &Arc<RuleService>,
) -> Result<(), RpcError> {
    let svc = Arc::clone(service);
    map.insert_func(
        "rules.add",
        move |_target, obj| {
            let rule: Rule = parse_json_arg(obj, "rule")?;
            let inserted = svc.add_rule(rule).map_err(store_error)?;
            Ok(Object::from(inserted.id as i64))
        },
        FLAG_PUBLIC_RPC,
        "<json Rule>",
        "Validate, compile and store a new rule; returns its id.",
    )?;

    let svc = Arc::clone(service);
    map.insert_func(
        "rules.update",
        move |_target, obj| {
            let (id_text, json) = two_args(obj, "rules.update requires <id>,<json Rule>")?;
            let id = parse_rule_id(&id_text)?;
            let mut rule: Rule = serde_json::from_str(&json)
                .map_err(|err| RpcError::InputError(format!("invalid rule JSON: {err}")))?;
            if svc.get_rule(id).is_none() {
                return Err(RpcError::InputError(format!("unknown rule {id}")));
            }
            rule.id = id;
            svc.update_rule(rule).map_err(store_error)?;
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "<id>,<json Rule>",
        "Validate, compile and replace the rule with the given id.",
    )?;

    let svc = Arc::clone(service);
    map.insert_func(
        "rules.remove",
        move |_target, obj| {
            let id = parse_rule_id(&convert_to_string(obj)?)?;
            svc.remove_rule(id).map_err(store_error)?;
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "<id>",
        "Remove a rule.",
    )?;

    let svc = Arc::clone(service);
    map.insert_func(
        "rules.get",
        move |_target, obj| {
            let id = parse_rule_id(&convert_to_string(obj)?)?;
            let rule = svc
                .get_rule(id)
                .ok_or_else(|| RpcError::InputError(format!("unknown rule {id}")))?;
            json_object(&rule)
        },
        FLAG_PUBLIC_RPC,
        "<id>",
        "Get one rule as JSON (trusted: may contain webhook URLs).",
    )?;

    let svc = Arc::clone(service);
    map.insert_func(
        "rules.list",
        move |_target, _obj| json_object(&svc.list_rules()),
        FLAG_PUBLIC_RPC,
        "",
        "List all rules as JSON, sorted by (priority, id) (trusted: webhook URLs).",
    )?;

    for (name, enabled, doc) in [
        ("rules.enable", true, "Enable a rule."),
        ("rules.disable", false, "Disable a rule."),
    ] {
        let svc = Arc::clone(service);
        map.insert_func(
            name,
            move |_target, obj| {
                let id = parse_rule_id(&convert_to_string(obj)?)?;
                svc.set_rule_enabled(id, enabled).map_err(store_error)?;
                Ok(Object::create_empty())
            },
            FLAG_PUBLIC_RPC,
            "<id>",
            doc,
        )?;
    }

    let svc = Arc::clone(service);
    map.insert_func(
        "rules.test",
        move |_target, obj| {
            let rule: Rule = parse_json_arg(obj, "rule")?;
            let result = svc.test_rule(&rule).map_err(store_error)?;
            json_object(&result)
        },
        FLAG_PUBLIC_RPC,
        "<json Rule>",
        "Dry-run a rule against the current torrents; zero mutations (trusted: torrent names).",
    )?;

    let svc = Arc::clone(service);
    map.insert_func(
        "rules.check",
        move |_target, _obj| {
            // The command surface is sync but the pass is async. Bridge via
            // block_in_place on the daemon's multi-thread runtime; refuse
            // (instead of panicking) on a current-thread runtime or no runtime.
            let handle = tokio::runtime::Handle::try_current().map_err(|_| {
                RpcError::InternalError("rules.check requires a running Tokio runtime".into())
            })?;
            if handle.runtime_flavor() != tokio::runtime::RuntimeFlavor::MultiThread {
                return Err(RpcError::InternalError(
                    "rules.check requires a multi-thread Tokio runtime".into(),
                ));
            }
            let entries = tokio::task::block_in_place(|| handle.block_on(svc.check_now()));
            json_object(&entries)
        },
        FLAG_PUBLIC_RPC,
        "",
        "Force one rules/policy pass now; returns the fired decisions as JSON history entries.",
    )?;

    let svc = Arc::clone(service);
    map.insert_func(
        "rules.history",
        move |_target, obj| {
            let text = convert_to_string(obj)?;
            let limit = if text.trim().is_empty() {
                HISTORY_DEFAULT_LIMIT
            } else {
                let value = convert_to_value(obj, 10, 0)?;
                if value < 0 {
                    return Err(RpcError::InputError(
                        "rules.history limit must be >= 0".into(),
                    ));
                }
                (value as usize).min(HISTORY_MAX_LIMIT)
            };
            json_object(&svc.history(limit))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "[limit]",
        "Audit history as JSON, newest first (default 100, capped at 500).",
    )?;

    let svc = Arc::clone(service);
    map.insert_func(
        "rules.clear_history",
        move |_target, _obj| {
            svc.clear_history().map_err(store_error)?;
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "",
        "Clear the rule audit history.",
    )?;

    let svc = Arc::clone(service);
    map.insert_func(
        "rules.export",
        move |_target, _obj| json_object(&svc.export()),
        FLAG_PUBLIC_RPC,
        "",
        "Export labels and rules as a JSON backup blob (trusted: webhook URLs).",
    )?;

    let svc = Arc::clone(service);
    map.insert_func(
        "rules.import",
        move |_target, obj| {
            let blob: ExportBlob = parse_json_arg(obj, "export blob")?;
            svc.import(blob).map_err(store_error)?;
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "<json>",
        "Merge a backup blob (labels by name, rules get fresh ids); conflicts are errors.",
    )?;

    rules_getter(
        map,
        service,
        "rules.enabled",
        "Whether automation rules are active (0/1).",
        |s| bool_object(s.enabled),
    )?;
    rules_setter(
        map,
        service,
        "rules.enabled",
        "Enable or disable automation rules (kill switch).",
        |s, obj| {
            s.enabled = parse_switch(obj)?;
            Ok(())
        },
    )?;

    rules_getter(
        map,
        service,
        "rules.allow_destructive",
        "Global destructive-action gate (0/1).",
        |s| bool_object(s.allow_destructive),
    )?;
    rules_setter(
        map,
        service,
        "rules.allow_destructive",
        "Open or close the global gate for remove/remove_with_data actions.",
        |s, obj| {
            s.allow_destructive = parse_switch(obj)?;
            Ok(())
        },
    )?;

    Ok(())
}

// ---- Legacy migration (plan §6.5) ----

/// Migrates legacy `ratio.*` settings into the global seed policy when the
/// user enabled `[seed]` but left the corresponding fields unset.
///
/// - `policy.ratio == Inherit` → legacy `ratio.min` (fixed-point /100) as
///   `Set(f64)` (values <= 0 become `Unlimited`).
/// - `policy.action == None` → legacy `group.seeding.ratio.command` as
///   `SeedAction::Command`.
/// - `ratio.upload` / `ratio.max` are **not** migrated: the legacy OR
///   semantics (upload threshold alone can fire) have no 1:1 equivalent in
///   the limit model; a warning is logged instead.
///
/// Explicit values always win: only `Inherit`/`None` fields are touched.
/// Call after the TOML `[seed]` section has been applied and the legacy
/// ratio commands are registered.
pub fn apply_legacy_ratio_fallback(service: &RuleService, map: &CommandMap) {
    let mut settings = service.seed_settings();
    if !settings.enabled {
        return;
    }
    let mut changed = false;

    if settings.policy.ratio == LimitValue::Inherit {
        if let Some(min) = query_value(map, "ratio.min") {
            let ratio = min as f64 / 100.0;
            settings.policy.ratio = if ratio > 0.0 {
                LimitValue::Set(ratio)
            } else {
                LimitValue::Unlimited
            };
            tracing::info!(
                legacy_ratio_min = min,
                ratio,
                "seed policies: migrated legacy ratio.min to the global seed ratio"
            );
            changed = true;
        }
    }

    if settings.policy.action.is_none() {
        if let Some(command) =
            query_string(map, "group.seeding.ratio.command").filter(|c| !c.trim().is_empty())
        {
            tracing::info!(
                command = %command,
                "seed policies: migrated legacy group.seeding.ratio.command to the global seed action"
            );
            settings.policy.action = Some(SeedAction::Command(command));
            changed = true;
        }
    }

    if changed {
        if let Some(upload) = query_value(map, "ratio.upload") {
            tracing::warn!(
                legacy_ratio_upload = upload,
                "seed policies: legacy ratio.upload threshold not migrated \
                 (OR semantics have no 1:1 equivalent, plan §6.5)"
            );
        }
        if let Some(max) = query_value(map, "ratio.max") {
            tracing::warn!(
                legacy_ratio_max = max,
                "seed policies: legacy ratio.max threshold not migrated (plan §6.5)"
            );
        }
        service.set_seed_settings(settings);
    }
}
