//! T6 remote CLI: parser matrix + network-free request-building smoke tests
//! (plan.md §6.7 / §6.4). The keyword router itself lives in `lib.rs`
//! (orchestrator-owned) and is intentionally not tested here.

use rstorrent::cli_remote::{
    build_calls, inject_allow_destructive, merge_policy_json, parse_duration, parse_ratio_limit,
    parse_remote_command, parse_secs_limit, policy_set_call, resolve_endpoint, save_path_call,
    truncate_test_matches, ActionFlag, CommandKind, JsonSource, LabelsCmd, LimitMode, LimitValue,
    PolicyFlags, RemoteCommand, RpcArg, RpcCall, RulesCmd, SeedAction, SeedSetArgs,
    TorrentLimitsArgs,
};

fn args(items: &[&str]) -> Vec<String> {
    items.iter().map(|item| (*item).to_string()).collect()
}

fn parse(keyword: &str, items: &[&str]) -> RemoteCommand {
    parse_remote_command(keyword, &args(items))
        .unwrap_or_else(|err| panic!("parse {keyword} {items:?} failed: {err}"))
        .unwrap_or_else(|| panic!("keyword {keyword} was not recognized"))
}

fn parse_err(keyword: &str, items: &[&str]) -> String {
    parse_remote_command(keyword, &args(items))
        .expect_err("expected a parse error")
        .to_string()
}

const HASH: &str = "1111222233334444555566667777888899990000";

// ---- parse_duration matrix ----

#[test]
fn duration_matrix() {
    let cases = [
        ("120", 120u64),
        ("0", 0),
        ("30s", 30),
        ("30m", 1_800),
        ("2h", 7_200),
        ("7d", 604_800),
        ("  2h  ", 7_200),
        ("1s", 1),
        ("365d", 31_536_000),
    ];
    for (text, expected) in cases {
        assert_eq!(
            parse_duration(text).unwrap_or_else(|err| panic!("{text}: {err}")),
            expected,
            "duration {text}"
        );
    }
}

#[test]
fn duration_rejects_bad_input() {
    for text in [
        "",
        "   ",
        "-5",
        "abc",
        "10x",
        "1.5h",
        "1h30m",
        "m",
        "h2",
        "99999999999999999999d",
    ] {
        assert!(parse_duration(text).is_err(), "duration {text} should fail");
    }
}

// ---- limit / action parsers ----

#[test]
fn ratio_limit_three_forms() {
    assert_eq!(parse_ratio_limit("inherit").unwrap(), LimitValue::Inherit);
    assert_eq!(
        parse_ratio_limit("unlimited").unwrap(),
        LimitValue::Unlimited
    );
    assert_eq!(parse_ratio_limit("3.0").unwrap(), LimitValue::Set(3.0));
    assert_eq!(parse_ratio_limit("0").unwrap(), LimitValue::Set(0.0));
    for bad in ["-1", "NaN", "inf", "abc", ""] {
        assert!(parse_ratio_limit(bad).is_err(), "ratio {bad} should fail");
    }
}

#[test]
fn secs_limit_accepts_durations() {
    assert_eq!(parse_secs_limit("inherit").unwrap(), LimitValue::Inherit);
    assert_eq!(
        parse_secs_limit("unlimited").unwrap(),
        LimitValue::Unlimited
    );
    assert_eq!(parse_secs_limit("7d").unwrap(), LimitValue::Set(604_800));
    assert_eq!(parse_secs_limit("120").unwrap(), LimitValue::Set(120));
    assert!(parse_secs_limit("-30m").is_err());
}

// ---- keyword routing (no regression for FILES) ----

#[test]
fn non_remote_keywords_fall_through() {
    for keyword in [
        "config",
        "file.torrent",
        "magnet:?xt=urn:btih:abc",
        "https://x/y",
        "",
    ] {
        let parsed = parse_remote_command(keyword, &args(&["list"])).unwrap();
        assert!(parsed.is_none(), "keyword {keyword} must fall through");
    }
}

// ---- rules parsing ----

#[test]
fn rules_list_and_json_switch() {
    let command = parse("rules", &["list"]);
    assert_eq!(command.kind, CommandKind::Rules(RulesCmd::List));
    assert!(!command.json);

    let command = parse("rules", &["list", "--json"]);
    assert!(command.json);
    assert!(parse_err("rules", &["list", "--json=yes"]).contains("--json"));
    assert!(parse_err("rules", &["list", "extra"]).contains("unexpected"));
}

#[test]
fn rules_add_inline_json_and_destructive_flag() {
    let json = r#"{"name":"cleanup","conditions":[],"actions":[]}"#;
    let command = parse("rules", &["add", "--json", json, "--allow-destructive"]);
    match command.kind {
        CommandKind::Rules(RulesCmd::Add {
            source,
            allow_destructive,
        }) => {
            assert_eq!(source, JsonSource::Inline(json.to_string()));
            assert!(allow_destructive);
        }
        other => panic!("unexpected kind: {other:?}"),
    }

    // --flag=value form works too.
    let command = parse("rules", &["add", &format!("--json={json}")]);
    match command.kind {
        CommandKind::Rules(RulesCmd::Add {
            source,
            allow_destructive,
        }) => {
            assert_eq!(source, JsonSource::Inline(json.to_string()));
            assert!(!allow_destructive);
        }
        other => panic!("unexpected kind: {other:?}"),
    }
}

#[test]
fn rules_add_file_source() {
    let command = parse("rules", &["add", "--file", "/tmp/rule.json"]);
    match command.kind {
        CommandKind::Rules(RulesCmd::Add { source, .. }) => {
            assert_eq!(source, JsonSource::File("/tmp/rule.json".into()));
        }
        other => panic!("unexpected kind: {other:?}"),
    }
}

#[test]
fn rules_add_requires_exactly_one_source() {
    assert!(parse_err("rules", &["add"]).contains("--file"));
    assert!(
        parse_err("rules", &["add", "--file", "a.json", "--json", "{}"]).contains("exactly one")
    );
    assert!(parse_err("rules", &["add", "--json"]).contains("requires a value"));
}

#[test]
fn rules_id_subcommands() {
    for (sub, expected) in [
        ("remove", RulesCmd::Remove { id: 7 }),
        ("enable", RulesCmd::Enable { id: 7 }),
        ("disable", RulesCmd::Disable { id: 7 }),
        ("get", RulesCmd::Get { id: 7 }),
    ] {
        let command = parse("rules", &[sub, "7"]);
        assert_eq!(command.kind, CommandKind::Rules(expected));
    }
    assert!(parse_err("rules", &["remove", "abc"]).contains("invalid rule id"));
    assert!(parse_err("rules", &["remove"]).contains("rule id"));
    assert!(parse_err("rules", &["get", "7", "8"]).contains("unexpected"));
    // get supports the output switch.
    assert!(parse("rules", &["get", "7", "--json"]).json);
}

#[test]
fn rules_test_with_limit() {
    let command = parse(
        "rules",
        &["test", "--file", "rule.json", "--limit", "50", "--json"],
    );
    assert!(command.json);
    match command.kind {
        CommandKind::Rules(RulesCmd::Test { source, limit }) => {
            assert_eq!(source, JsonSource::File("rule.json".into()));
            assert_eq!(limit, Some(50));
        }
        other => panic!("unexpected kind: {other:?}"),
    }
    assert!(parse_err("rules", &["test"]).contains("--file"));
    assert!(
        parse_err("rules", &["test", "--file", "r.json", "--limit", "x"])
            .contains("invalid --limit")
    );
}

#[test]
fn rules_check_history_export_import() {
    assert_eq!(
        parse("rules", &["check"]).kind,
        CommandKind::Rules(RulesCmd::Check)
    );

    let command = parse("rules", &["history"]);
    assert_eq!(
        command.kind,
        CommandKind::Rules(RulesCmd::History { limit: None })
    );
    let command = parse("rules", &["history", "--limit", "20"]);
    assert_eq!(
        command.kind,
        CommandKind::Rules(RulesCmd::History { limit: Some(20) })
    );

    assert_eq!(
        parse("rules", &["export"]).kind,
        CommandKind::Rules(RulesCmd::Export)
    );
    let command = parse("rules", &["import", "backup.json"]);
    assert_eq!(
        command.kind,
        CommandKind::Rules(RulesCmd::Import {
            file: "backup.json".into()
        })
    );
    assert!(parse_err("rules", &["bogus"]).contains("unknown rules subcommand"));
    assert!(parse_err("rules", &[]).contains("missing rules subcommand"));
}

// ---- labels parsing ----

#[test]
fn labels_add_remove_list() {
    let command = parse("labels", &["add", "movies", "--color", "#7c3aed"]);
    assert_eq!(
        command.kind,
        CommandKind::Labels(LabelsCmd::Add {
            name: "movies".to_string(),
            color: Some("#7c3aed".to_string()),
        })
    );
    let command = parse("labels", &["add", "plain"]);
    assert_eq!(
        command.kind,
        CommandKind::Labels(LabelsCmd::Add {
            name: "plain".to_string(),
            color: None
        })
    );
    assert_eq!(
        parse("labels", &["remove", "movies"]).kind,
        CommandKind::Labels(LabelsCmd::Remove {
            name: "movies".to_string()
        })
    );
    assert_eq!(
        parse("labels", &["list"]).kind,
        CommandKind::Labels(LabelsCmd::List)
    );
    assert!(parse_err("labels", &["add"]).contains("label name"));
}

#[test]
fn labels_policy_flags() {
    let command = parse(
        "labels",
        &[
            "policy",
            "movies",
            "--ratio",
            "3.0",
            "--time",
            "7d",
            "--idle",
            "inherit",
            "--action",
            "remove",
            "--save-path",
            "/disk/movies",
            "--json",
        ],
    );
    assert!(command.json);
    match command.kind {
        CommandKind::Labels(LabelsCmd::Policy { name, flags }) => {
            assert_eq!(name, "movies");
            assert_eq!(flags.ratio, Some(LimitValue::Set(3.0)));
            assert_eq!(flags.time, Some(LimitValue::Set(604_800)));
            assert_eq!(flags.idle, Some(LimitValue::Inherit));
            assert_eq!(flags.action, Some(ActionFlag::Set(SeedAction::Remove)));
            assert_eq!(flags.save_path.as_deref(), Some("/disk/movies"));
        }
        other => panic!("unexpected kind: {other:?}"),
    }

    // unlimited + inherit action forms.
    let command = parse(
        "labels",
        &["policy", "x", "--ratio", "unlimited", "--action", "inherit"],
    );
    match command.kind {
        CommandKind::Labels(LabelsCmd::Policy { flags, .. }) => {
            assert_eq!(flags.ratio, Some(LimitValue::Unlimited));
            assert_eq!(flags.action, Some(ActionFlag::Inherit));
        }
        other => panic!("unexpected kind: {other:?}"),
    }

    assert!(parse_err("labels", &["policy", "x", "--ratio", "nope"]).contains("invalid ratio"));
    assert!(parse_err("labels", &["policy", "x", "--action", "explode"]).contains("invalid action"));
    assert!(parse_err("labels", &["policy", "x", "--time", "5w"]).contains("invalid duration"));
    assert!(parse_err("labels", &["policy"]).contains("label name"));
}

// ---- seed parsing ----

#[test]
fn seed_show_and_set_full_matrix() {
    assert_eq!(parse("seed", &["show"]).kind, CommandKind::SeedShow);

    let command = parse(
        "seed",
        &[
            "set",
            "--ratio",
            "2.0",
            "--time",
            "7d",
            "--idle",
            "30m",
            "--mode",
            "all",
            "--action",
            "throttle:seed",
            "--enabled",
            "true",
            "--check-interval",
            "10",
        ],
    );
    match command.kind {
        CommandKind::SeedSet(set) => {
            assert_eq!(
                set,
                SeedSetArgs {
                    ratio: Some(2.0),
                    time: Some(604_800),
                    idle: Some(1_800),
                    mode: Some(LimitMode::All),
                    action: Some(SeedAction::Throttle("seed".to_string())),
                    enabled: Some(true),
                    check_interval: Some(10),
                }
            );
        }
        other => panic!("unexpected kind: {other:?}"),
    }
}

#[test]
fn seed_set_unlimited_and_defaults() {
    let command = parse(
        "seed",
        &[
            "set",
            "--ratio",
            "unlimited",
            "--time",
            "unlimited",
            "--enabled",
            "false",
        ],
    );
    match command.kind {
        CommandKind::SeedSet(set) => {
            assert_eq!(set.ratio, Some(0.0));
            assert_eq!(set.time, Some(0));
            assert_eq!(set.enabled, Some(false));
            assert_eq!(set.mode, None);
        }
        other => panic!("unexpected kind: {other:?}"),
    }
}

#[test]
fn seed_set_rejects_invalid_input() {
    assert!(parse_err("seed", &["set"]).contains("at least one flag"));
    assert!(parse_err("seed", &["set", "--ratio", "inherit"]).contains("inherit"));
    assert!(parse_err("seed", &["set", "--time", "inherit"]).contains("inherit"));
    assert!(parse_err("seed", &["set", "--action", "inherit"]).contains("inherit"));
    assert!(parse_err("seed", &["set", "--enabled", "maybe"]).contains("true|false"));
    assert!(parse_err("seed", &["set", "--mode", "both"]).contains("any|all"));
    assert!(parse_err("seed", &["set", "--check-interval", "0"]).contains(">= 1"));
    assert!(parse_err("seed", &["bogus"]).contains("unknown seed subcommand"));
}

// ---- torrent parsing ----

#[test]
fn torrent_limits_read_and_overrides() {
    let command = parse("torrent", &["limits", HASH]);
    match command.kind {
        CommandKind::TorrentLimits(limits) => {
            assert_eq!(limits.hash, HASH);
            assert_eq!(limits.ratio, None);
            assert_eq!(limits.action, None);
        }
        other => panic!("unexpected kind: {other:?}"),
    }

    let command = parse(
        "torrent",
        &[
            "limits",
            HASH,
            "--ratio",
            "3.0",
            "--time",
            "unlimited",
            "--idle",
            "inherit",
            "--action",
            "stop",
        ],
    );
    match command.kind {
        CommandKind::TorrentLimits(limits) => {
            assert_eq!(
                limits,
                TorrentLimitsArgs {
                    hash: HASH.to_string(),
                    ratio: Some(LimitValue::Set(3.0)),
                    time: Some(LimitValue::Unlimited),
                    idle: Some(LimitValue::Inherit),
                    action: Some(ActionFlag::Set(SeedAction::Stop)),
                }
            );
        }
        other => panic!("unexpected kind: {other:?}"),
    }

    // Uppercase hashes normalise to lowercase.
    let command = parse("torrent", &["limits", &HASH.to_uppercase()]);
    match command.kind {
        CommandKind::TorrentLimits(limits) => assert_eq!(limits.hash, HASH),
        other => panic!("unexpected kind: {other:?}"),
    }

    assert!(parse_err("torrent", &["limits", "short"]).contains("invalid torrent hash"));
    assert!(parse_err("torrent", &["limits"]).contains("missing torrent hash"));
    assert!(parse_err("torrent", &["bogus"]).contains("unknown torrent subcommand"));
}

// ---- cmd parsing (raw passthrough) ----

#[test]
fn cmd_passes_args_verbatim() {
    let command = parse("cmd", &["d.custom.set", "key", "--json"]);
    match command.kind {
        CommandKind::Raw {
            command: rpc_command,
            args: raw_args,
        } => {
            assert_eq!(rpc_command, "d.custom.set");
            // --json is an RPC argument here, not an output switch.
            assert_eq!(raw_args, args(&["key", "--json"]));
        }
        other => panic!("unexpected kind: {other:?}"),
    }
    assert!(!command.json);
    assert!(parse_err("cmd", &[]).contains("missing command"));
}

// ---- request building (network-free smoke) ----

#[test]
fn build_rules_list_and_id_commands() {
    let calls = build_calls(&parse("rules", &["list"])).unwrap();
    assert_eq!(
        calls,
        vec![RpcCall {
            command: "rules.list".into(),
            args: vec![],
            target: None
        }]
    );

    let calls = build_calls(&parse("rules", &["remove", "7"])).unwrap();
    assert_eq!(calls[0].command, "rules.remove");
    assert_eq!(calls[0].args, vec![RpcArg::Int(7)]);
    assert_eq!(calls[0].target, None);

    let calls = build_calls(&parse("rules", &["enable", "3"])).unwrap();
    assert_eq!(calls[0].command, "rules.enable");
    let calls = build_calls(&parse("rules", &["disable", "3"])).unwrap();
    assert_eq!(calls[0].command, "rules.disable");
    let calls = build_calls(&parse("rules", &["get", "3"])).unwrap();
    assert_eq!(calls[0].command, "rules.get");
}

#[test]
fn build_rules_add_injects_destructive_flag() {
    let json = r#"{"name":"r","allow_destructive":false}"#;
    let calls = build_calls(&parse(
        "rules",
        &["add", "--json", json, "--allow-destructive"],
    ))
    .unwrap();
    assert_eq!(calls[0].command, "rules.add");
    match &calls[0].args[0] {
        RpcArg::Text(text) => {
            let value: serde_json::Value = serde_json::from_str(text).unwrap();
            assert_eq!(value["allow_destructive"], serde_json::Value::Bool(true));
            assert_eq!(value["name"], "r");
        }
        other => panic!("expected JSON string arg, got {other:?}"),
    }

    // Without the flag the document passes through untouched.
    let calls = build_calls(&parse("rules", &["add", "--json", json])).unwrap();
    assert_eq!(calls[0].args, vec![RpcArg::Text(json.to_string())]);
}

#[test]
fn build_rules_history_default_and_limit() {
    let calls = build_calls(&parse("rules", &["history"])).unwrap();
    assert_eq!(calls[0].command, "rules.history");
    assert_eq!(calls[0].args, vec![RpcArg::Int(100)]);

    let calls = build_calls(&parse("rules", &["history", "--limit", "20"])).unwrap();
    assert_eq!(calls[0].args, vec![RpcArg::Int(20)]);

    let calls = build_calls(&parse("rules", &["check"])).unwrap();
    assert_eq!(calls[0].command, "rules.check");
    let calls = build_calls(&parse("rules", &["export"])).unwrap();
    assert_eq!(calls[0].command, "rules.export");
}

#[test]
fn build_rules_test_sends_rule_json() {
    let json = r#"{"name":"r"}"#;
    let calls = build_calls(&parse(
        "rules",
        &["test", "--file", "/nonexistent/rule.json"],
    ));
    // Missing file surfaces as a readable I/O error, not a panic.
    assert!(calls.is_err());

    let command = RemoteCommand {
        kind: CommandKind::Rules(RulesCmd::Test {
            source: JsonSource::Inline(json.to_string()),
            limit: Some(5),
        }),
        json: false,
    };
    let calls = build_calls(&command).unwrap();
    assert_eq!(calls[0].command, "rules.test");
    assert_eq!(calls[0].args, vec![RpcArg::Text(json.to_string())]);
}

#[test]
fn build_labels_calls() {
    let calls = build_calls(&parse("labels", &["list"])).unwrap();
    assert_eq!(calls[0].command, "label.list");

    let calls = build_calls(&parse("labels", &["add", "movies", "--color", "#7c3aed"])).unwrap();
    assert_eq!(calls[0].command, "label.add");
    assert_eq!(
        calls[0].args,
        vec![
            RpcArg::Text("movies".into()),
            RpcArg::Text("#7c3aed".into())
        ]
    );

    let calls = build_calls(&parse("labels", &["add", "plain"])).unwrap();
    assert_eq!(calls[0].args, vec![RpcArg::Text("plain".into())]);

    let calls = build_calls(&parse("labels", &["remove", "movies"])).unwrap();
    assert_eq!(calls[0].command, "label.remove");

    // Policy starts with the read half of read-modify-write.
    let calls = build_calls(&parse("labels", &["policy", "movies", "--ratio", "2.0"])).unwrap();
    assert_eq!(calls[0].command, "label.policy.get");
    assert_eq!(calls[0].args, vec![RpcArg::Text("movies".into())]);
}

#[test]
fn build_seed_set_one_call_per_flag() {
    let command = parse(
        "seed",
        &[
            "set",
            "--ratio",
            "2.5",
            "--time",
            "2h",
            "--enabled",
            "true",
            "--mode",
            "any",
            "--action",
            "stop",
        ],
    );
    let calls = build_calls(&command).unwrap();
    let summary: Vec<(&str, &RpcArg)> = calls
        .iter()
        .map(|call| (call.command.as_str(), &call.args[0]))
        .collect();
    assert_eq!(
        summary,
        vec![
            ("seed.enabled.set", &RpcArg::Int(1)),
            ("seed.ratio.set", &RpcArg::Text("2.5".into())),
            ("seed.time.set", &RpcArg::Int(7_200)),
            ("seed.mode.set", &RpcArg::Text("any".into())),
            ("seed.action.set", &RpcArg::Text("stop".into())),
        ]
    );
    assert!(calls.iter().all(|call| call.target.is_none()));

    let calls = build_calls(&parse("seed", &["show"])).unwrap();
    assert_eq!(calls[0].command, "seed.show");
}

#[test]
fn build_torrent_limits_targets_every_call() {
    let command = parse(
        "torrent",
        &[
            "limits",
            HASH,
            "--ratio",
            "3.0",
            "--time",
            "unlimited",
            "--idle",
            "inherit",
            "--action",
            "stop",
        ],
    );
    let calls = build_calls(&command).unwrap();
    let summary: Vec<(&str, &str)> = calls
        .iter()
        .map(|call| {
            let arg = match &call.args[..] {
                [RpcArg::Text(text)] => text.as_str(),
                [] => "",
                other => panic!("unexpected args: {other:?}"),
            };
            (call.command.as_str(), arg)
        })
        .collect();
    assert_eq!(
        summary,
        vec![
            ("d.seed_ratio.set", "3"),
            ("d.seed_time.set", "unlimited"),
            ("d.seed_idle.set", "inherit"),
            ("d.seed_action.set", "stop"),
            ("d.seed_limits", ""),
        ]
    );
    assert!(calls
        .iter()
        .all(|call| call.target.as_deref() == Some(HASH)));

    // Read-only: just the effective-limits getter.
    let calls = build_calls(&parse("torrent", &["limits", HASH])).unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].command, "d.seed_limits");
    assert_eq!(calls[0].target.as_deref(), Some(HASH));
}

#[test]
fn build_cmd_target_and_arg_typing() {
    // XML-RPC convention: leading hash becomes the download target.
    let calls = build_calls(&parse("cmd", &["d.stop", HASH])).unwrap();
    assert_eq!(calls[0].command, "d.stop");
    assert_eq!(calls[0].target.as_deref(), Some(HASH));
    assert!(calls[0].args.is_empty());

    let calls = build_calls(&parse(
        "cmd",
        &["d.seed_ratio.set", &format!("{HASH}:d"), "2.5"],
    ))
    .unwrap();
    assert_eq!(calls[0].target.as_deref(), Some(HASH));
    assert_eq!(calls[0].args, vec![RpcArg::Text("2.5".into())]);

    // Non-target args: integers typed as ints, the rest strings.
    let calls = build_calls(&parse("cmd", &["rules.history", "20"])).unwrap();
    assert_eq!(calls[0].target, None);
    assert_eq!(calls[0].args, vec![RpcArg::Int(20)]);

    let calls = build_calls(&parse("cmd", &["session.path"])).unwrap();
    assert_eq!(calls[0].target, None);
    assert!(calls[0].args.is_empty());

    // File/tracker targets are not representable in gRPC target_hash.
    assert!(build_calls(&parse("cmd", &["f.path", &format!("{HASH}:f0")])).is_err());
}

// ---- policy merge / destructive injection / dry-run limit ----

#[test]
fn merge_policy_partial_update_keeps_other_fields() {
    let existing = r#"{"ratio":{"set":1.5},"seeding_time_secs":"unlimited","inactive_secs":"inherit","mode":"all","action":"stop"}"#;
    let flags = PolicyFlags {
        ratio: Some(LimitValue::Set(3.0)),
        action: Some(ActionFlag::Inherit),
        ..PolicyFlags::default()
    };
    let merged = merge_policy_json(existing, &flags).unwrap();
    let value: serde_json::Value = serde_json::from_str(&merged).unwrap();
    assert_eq!(value["ratio"], serde_json::json!({"set": 3.0}));
    assert_eq!(value["seeding_time_secs"], "unlimited"); // untouched
    assert_eq!(value["inactive_secs"], "inherit"); // untouched
    assert_eq!(value["mode"], "all"); // untouched
    assert!(value["action"].is_null()); // inherit → None
}

#[test]
fn merge_policy_writes_externally_tagged_limits() {
    let existing = "{}"; // SeedPolicy defaults: everything inherit
    let flags = PolicyFlags {
        ratio: Some(LimitValue::Unlimited),
        time: Some(LimitValue::Set(604_800)),
        idle: Some(LimitValue::Inherit),
        action: Some(ActionFlag::Set(SeedAction::Throttle("seed".into()))),
        save_path: None,
    };
    let merged = merge_policy_json(existing, &flags).unwrap();
    let value: serde_json::Value = serde_json::from_str(&merged).unwrap();
    assert_eq!(value["ratio"], "unlimited");
    assert_eq!(
        value["seeding_time_secs"],
        serde_json::json!({"set": 604_800})
    );
    assert_eq!(value["inactive_secs"], "inherit");
    assert_eq!(value["action"], serde_json::json!({"throttle": "seed"}));
    assert!(merge_policy_json("not json", &flags).is_err());
}

#[test]
fn policy_and_save_path_set_call_shapes() {
    let call = policy_set_call("movies", "{\"ratio\":\"unlimited\"}");
    assert_eq!(call.command, "label.policy.set");
    assert_eq!(
        call.args,
        vec![
            RpcArg::Text("movies".into()),
            RpcArg::Text("{\"ratio\":\"unlimited\"}".into()),
        ]
    );
    let call = save_path_call("movies", "/disk/movies");
    assert_eq!(call.command, "label.save_path.set");
    assert_eq!(
        call.args,
        vec![
            RpcArg::Text("movies".into()),
            RpcArg::Text("/disk/movies".into())
        ]
    );
}

#[test]
fn inject_allow_destructive_only_when_requested() {
    let json = r#"{"name":"r"}"#;
    assert_eq!(inject_allow_destructive(json, false).unwrap(), json);
    let injected = inject_allow_destructive(json, true).unwrap();
    let value: serde_json::Value = serde_json::from_str(&injected).unwrap();
    assert_eq!(value["allow_destructive"], serde_json::Value::Bool(true));
    assert!(inject_allow_destructive("[1,2]", true).is_err());
    assert!(inject_allow_destructive("{bad", true).is_err());
}

#[test]
fn truncate_test_matches_caps_matches_only() {
    let json = r#"{"matches":[{"torrent_hash":"a"},{"torrent_hash":"b"},{"torrent_hash":"c"}],"no_data":["d"]}"#;
    let limited = truncate_test_matches(json, Some(2)).unwrap();
    let value: serde_json::Value = serde_json::from_str(&limited).unwrap();
    assert_eq!(value["matches"].as_array().unwrap().len(), 2);
    assert_eq!(value["no_data"], serde_json::json!(["d"]));
    assert_eq!(truncate_test_matches(json, None).unwrap(), json);
}

// ---- endpoint resolution (explicit values only; discovery touches disk) ----

#[test]
fn endpoint_normalizes_schemes() {
    assert_eq!(
        resolve_endpoint(Some("localhost:50051")).unwrap(),
        "localhost:50051"
    );
    assert_eq!(
        resolve_endpoint(Some("tcp://localhost:50051")).unwrap(),
        "localhost:50051"
    );
    assert_eq!(
        resolve_endpoint(Some("grpc://localhost:50051")).unwrap(),
        "localhost:50051"
    );
    assert_eq!(
        resolve_endpoint(Some("unix:///tmp/rstorrent.sock")).unwrap(),
        "unix:///tmp/rstorrent.sock"
    );
    assert!(resolve_endpoint(Some("localhost")).is_err()); // missing port
    assert!(resolve_endpoint(Some("")).is_err());
    assert!(resolve_endpoint(Some("ftp://localhost:21")).is_err());
}
