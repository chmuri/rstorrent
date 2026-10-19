use std::sync::{Arc, RwLock};

use lt_common::Object;
use lt_net::config::NetworkConfig;
use lt_storage::disk_thread::global_file_manager_stats;
use lt_storage::memory_manager;
use rt_core::commands::groups::{register_group_commands, ChokeGroupManager};
use rt_core::commands::network::register_network_commands;
use rt_core::commands::pieces::{parse_byte_size, register_pieces_commands};
use rt_core::commands::protocol::register_protocol_commands;
use rt_core::commands::system::register_system_commands;
use rt_rpc::{CommandMap, Target};

fn call(map: &CommandMap, name: &str, args: Vec<Object>) -> Result<Object, rt_rpc::RpcError> {
    map.call_command(name, &Object::create_list_with(args), &Target::None, true)
}

fn value(map: &CommandMap, name: &str, args: Vec<Object>) -> i64 {
    call(map, name, args).unwrap().as_value().unwrap()
}

fn text(map: &CommandMap, name: &str, args: Vec<Object>) -> String {
    call(map, name, args).unwrap().as_str().unwrap().to_string()
}

fn list_text(map: &CommandMap, name: &str, args: Vec<Object>) -> Vec<String> {
    call(map, name, args)
        .unwrap()
        .as_list()
        .unwrap()
        .iter()
        .map(|item| item.as_str().unwrap().to_string())
        .collect()
}

#[test]
fn network_addresses_use_best_match_and_aliases() {
    let config = NetworkConfig::new();
    let mut map = CommandMap::new();
    register_network_commands(&mut map, config.clone()).unwrap();

    assert!(call(
        &map,
        "network.bind_address.set",
        vec![Object::from("2001:db8::1")]
    )
    .is_ok());
    assert_eq!(text(&map, "network.bind_address", vec![]), "2001:db8::1");
    assert_eq!(
        text(&map, "network.bind_address.ipv6", vec![]),
        "2001:db8::1"
    );

    // `bind` is the deprecated redirect for the combined setter.
    assert!(call(&map, "bind", vec![Object::from("127.0.0.1")]).is_ok());
    assert_eq!(text(&map, "network.bind_address", vec![]), "127.0.0.1");

    assert!(call(
        &map,
        "network.local_address.set",
        vec![Object::from("2001:db8::2")]
    )
    .is_ok());
    assert_eq!(text(&map, "network.local_address", vec![]), "2001:db8::2");
    assert!(call(&map, "ip", vec![Object::from("127.0.0.2")]).is_ok());
    assert_eq!(text(&map, "network.local_address", vec![]), "127.0.0.2");

    // When both families are configured the best-match selection follows
    // `network.prefer.ipv6`.
    assert!(call(
        &map,
        "network.bind_address.ipv4.set",
        vec![Object::from("127.0.0.1")]
    )
    .is_ok());
    assert!(call(
        &map,
        "network.bind_address.ipv6.set",
        vec![Object::from("2001:db8::1")]
    )
    .is_ok());
    assert!(call(&map, "network.prefer.ipv6.set", vec![Object::from(0i64)]).is_ok());
    assert_eq!(text(&map, "network.bind_address", vec![]), "127.0.0.1");
    assert!(call(&map, "network.prefer.ipv6.set", vec![Object::from(1i64)]).is_ok());
    assert_eq!(text(&map, "network.bind_address", vec![]), "2001:db8::1");

    // The generic device setter updates both address-family views.
    assert!(call(&map, "network.bind_device.set", vec![Object::from("eth0")]).is_ok());
    assert_eq!(text(&map, "network.bind_device.ipv4", vec![]), "eth0");
    assert_eq!(text(&map, "network.bind_device.ipv6", vec![]), "eth0");

    // Invalid addresses are rejected without changing the shared config.
    assert!(call(
        &map,
        "network.bind_address.set",
        vec![Object::from("127.0.0.1:80")]
    )
    .is_err());
    assert_eq!(text(&map, "network.bind_address", vec![]), "2001:db8::1");
}

#[test]
fn network_tos_accepts_symbolic_and_hexadecimal_values() {
    let mut map = CommandMap::new();
    register_network_commands(&mut map, NetworkConfig::new()).unwrap();

    for (raw, expected) in [
        ("default", 0),
        ("lowdelay", 0x10),
        ("throughput", 0x08),
        ("reliability", 0x04),
        ("mincost", 0x02),
        ("10", 0x10),
        ("0x2e", 0x2e),
        ("ff", 255),
    ] {
        assert!(
            call(&map, "network.tos.set", vec![Object::from(raw)]).is_ok(),
            "TOS value {raw} should be accepted"
        );
        assert_eq!(value(&map, "network.tos", vec![]), expected);
    }

    assert!(call(&map, "network.tos.set", vec![Object::from("100")]).is_err());
    assert_eq!(value(&map, "network.tos", vec![]), 255);
    assert!(call(&map, "network.tos.set", vec![Object::from("bogus")]).is_err());
    assert_eq!(value(&map, "network.tos", vec![]), 255);
}

#[test]
fn network_backlog_port_and_http_limits_are_validated() {
    let mut map = CommandMap::new();
    register_network_commands(&mut map, NetworkConfig::new()).unwrap();

    assert!(call(&map, "network.listen.backlog.set", vec![Object::from(1i64)]).is_ok());
    assert_eq!(value(&map, "network.listen.backlog", vec![]), 1);
    assert!(call(
        &map,
        "network.listen.backlog.set",
        vec![Object::from(65536i64)]
    )
    .is_ok());
    assert_eq!(value(&map, "network.listen.backlog", vec![]), 65536);
    for invalid in [0i64, -1, 65537] {
        assert!(call(
            &map,
            "network.listen.backlog.set",
            vec![Object::from(invalid)]
        )
        .is_err());
    }
    assert_eq!(value(&map, "network.listen.backlog", vec![]), 65536);

    assert!(call(
        &map,
        "network.local_port.ipv4.set",
        vec![Object::from(7001i64)]
    )
    .is_ok());
    assert!(call(
        &map,
        "network.local_port.ipv6.set",
        vec![Object::from(7002i64)]
    )
    .is_ok());
    assert_eq!(value(&map, "network.local_port.ipv4", vec![]), 7001);
    assert_eq!(value(&map, "network.local_port.ipv6", vec![]), 7002);
    // Integer setters use the C++ base-0 parser, so hexadecimal works.
    assert!(call(&map, "network.local_port.set", vec![Object::from("0x1b59")]).is_ok());
    assert_eq!(value(&map, "network.local_port", vec![]), 7001);
    assert!(call(
        &map,
        "network.local_port.ipv4.set",
        vec![Object::from(-1i64)]
    )
    .is_err());
    assert!(call(
        &map,
        "network.local_port.ipv6.set",
        vec![Object::from(70000i64)]
    )
    .is_err());

    // HTTP cache/host limits use the C++ 0..1024 bounds.
    for name in [
        "network.http.max_cache_connections",
        "network.http.max_host_connections",
    ] {
        for (raw, ok) in [("0", true), ("1024", true), ("1025", false), ("-1", false)] {
            let result = call(&map, &format!("{name}.set"), vec![Object::from(raw)]);
            assert_eq!(result.is_ok(), ok, "{name}.set({raw})");
        }
    }
}

#[test]
fn system_prioritize_toc_and_extended_file_counters() {
    let mut map = CommandMap::new();
    register_system_commands(&mut map).unwrap();

    // The C++ default is "./", not an empty string.
    assert_eq!(text(&map, "directory.default", vec![]), "./");

    assert_eq!(value(&map, "file.prioritize_toc", vec![]), 0);
    assert!(call(&map, "file.prioritize_toc.set", vec![Object::from(1i64)]).is_ok());
    assert_eq!(value(&map, "file.prioritize_toc", vec![]), 1);

    let patterns = Object::create_list_with(vec![
        Object::from("*.avi"),
        Object::from("*.mp4"),
        Object::from("*.mkv"),
    ]);
    assert!(call(&map, "file.prioritize_toc.first.set", vec![patterns]).is_ok());
    assert_eq!(
        list_text(&map, "file.prioritize_toc.first", vec![]),
        vec!["*.avi", "*.mp4", "*.mkv"]
    );

    // A single string is treated as a one-element list.
    assert!(call(
        &map,
        "file.prioritize_toc.last.set",
        vec![Object::from("*.zip")]
    )
    .is_ok());
    assert_eq!(
        list_text(&map, "file.prioritize_toc.last", vec![]),
        vec!["*.zip"]
    );

    // An empty argument clears the list.
    assert!(call(&map, "file.prioritize_toc.first.set", vec![]).is_ok());
    assert!(list_text(&map, "file.prioritize_toc.first", vec![]).is_empty());

    // The count comes from the live disk-worker FileManager, and it is read-only.
    let expected = global_file_manager_stats()
        .map(|stats| stats.fadvise)
        .unwrap_or(0) as i64;
    assert_eq!(
        value(&map, "system.files.advised_counter", vec![]),
        expected
    );
    assert!(call(
        &map,
        "system.files.advised_counter.set",
        vec![Object::from(1i64)]
    )
    .is_err());
}

#[test]
fn pieces_safe_diskspace_aliases_and_byte_sizes() {
    let mut map = CommandMap::new();
    register_pieces_commands(&mut map).unwrap();

    // The value is sourced from the live process-wide memory manager.
    assert_eq!(
        value(&map, "pieces.sync.safe_free_diskspace", vec![]),
        memory_manager().sync_safe_free_diskspace() as i64
    );

    assert!(call(&map, "pieces.memory.max.set", vec![Object::from("2MiB")]).is_ok());
    assert_eq!(value(&map, "pieces.memory.max", vec![]), 2 << 20);

    assert!(call(
        &map,
        "pieces.hash.on_completion.set",
        vec![Object::from(0i64)]
    )
    .is_ok());
    assert_eq!(value(&map, "pieces.hash.on_completion", vec![]), 0);
    assert!(call(&map, "check_hash", vec![Object::from("yes")]).is_ok());
    assert_eq!(value(&map, "pieces.hash.on_completion", vec![]), 1);

    // Byte suffixes are case-insensitive, and malformed/overflowing values
    // are rejected before reaching the memory manager.
    for (raw, expected) in [
        ("1b", 1u64),
        ("1K", 1 << 10),
        ("2kb", 2 << 10),
        ("3MB", 3 << 20),
        ("4GiB", 4u64 << 30),
        ("1t", 1u64 << 40),
    ] {
        assert_eq!(
            parse_byte_size(&Object::from(raw)).unwrap(),
            expected,
            "{raw}"
        );
    }
    for invalid in ["", "-1", "1.5mb", "1xb", "1mib extra"] {
        assert!(
            parse_byte_size(&Object::from(invalid)).is_err(),
            "{invalid} should be rejected"
        );
    }

    for name in [
        "pieces.memory.current",
        "pieces.memory.sync_queue",
        "pieces.memory.block_count",
        "pieces.sync.queue_size",
        "pieces.sync.safe_free_diskspace",
        "pieces.stats_preloaded",
        "pieces.stats_not_preloaded",
        "pieces.stats.total_size",
        "pieces.hash.queue_size",
    ] {
        assert!(
            value(&map, name, vec![]) >= 0,
            "{name} must be non-negative"
        );
        assert!(
            call(&map, &format!("{name}.set"), vec![Object::from(1i64)]).is_err(),
            "{name} must be read-only"
        );
    }
}

#[test]
fn protocol_encryption_and_heuristic_validation_matrix() {
    let mut map = CommandMap::new();
    register_protocol_commands(&mut map, NetworkConfig::new()).unwrap();

    // A single option applies to both handshake and stream modes.
    for (raw, expected) in [
        ("deny", "deny,deny"),
        ("allow", "allow,allow"),
        ("prefer", "prefer,prefer"),
        ("require", "require,require"),
        ("handshake_prefer", "prefer,prefer"),
        ("stream_require", "require,require"),
    ] {
        assert!(
            call(&map, "protocol.encryption.set", vec![Object::from(raw)]).is_ok(),
            "{raw} should be accepted"
        );
        assert_eq!(text(&map, "protocol.encryption", vec![]), expected);
    }

    // Mixed handshake/stream options.
    assert!(call(
        &map,
        "protocol.encryption.set",
        vec![
            Object::from("handshake_require"),
            Object::from("stream_prefer")
        ]
    )
    .is_ok());
    assert_eq!(text(&map, "protocol.encryption", vec![]), "require,prefer");

    // Obsolete options use the C++ fallback semantics.
    assert!(call(
        &map,
        "encryption",
        vec![
            Object::from("allow_incoming"),
            Object::from("enable_retry"),
            Object::from("prefer_plaintext")
        ]
    )
    .is_ok());
    assert_eq!(text(&map, "protocol.encryption", vec![]), "allow,allow");
    assert!(call(&map, "encryption", vec![Object::from("none")]).is_ok());
    assert_eq!(text(&map, "protocol.encryption", vec![]), "deny,deny");
    assert!(call(&map, "encryption", vec![Object::from("require_RC4")]).is_ok());
    assert_eq!(text(&map, "protocol.encryption", vec![]), "require,require");
    assert!(call(
        &map,
        "protocol.encryption.set",
        vec![Object::from("handshake_bogus")]
    )
    .is_err());

    // All connection types are accepted; unknown values are not.
    for connection in ["leech", "seed", "initial_seed", "metadata", ""] {
        assert!(
            call(
                &map,
                "protocol.connection.leech.set",
                vec![Object::from(connection)]
            )
            .is_ok(),
            "connection type {connection:?}"
        );
    }
    assert!(call(
        &map,
        "protocol.connection.seed.set",
        vec![Object::from("bogus")]
    )
    .is_err());

    // Upload/down heuristics are direction specific.
    for heuristic in [
        "upload_leech",
        "upload_leech_experimental",
        "upload_seed",
        "",
    ] {
        assert!(call(
            &map,
            "protocol.choke_heuristics.up.leech.set",
            vec![Object::from(heuristic)]
        )
        .is_ok());
        assert!(call(
            &map,
            "protocol.choke_heuristics.up.seed.set",
            vec![Object::from(heuristic)]
        )
        .is_ok());
    }
    assert!(call(
        &map,
        "protocol.choke_heuristics.up.seed.set",
        vec![Object::from("download_leech")]
    )
    .is_err());
    for heuristic in ["download_leech", ""] {
        assert!(call(
            &map,
            "protocol.choke_heuristics.down.leech.set",
            vec![Object::from(heuristic)]
        )
        .is_ok());
        assert!(call(
            &map,
            "protocol.choke_heuristics.down.seed.set",
            vec![Object::from(heuristic)]
        )
        .is_ok());
    }
    assert!(call(
        &map,
        "protocol.choke_heuristics.down.seed.set",
        vec![Object::from("upload_seed")]
    )
    .is_err());

    assert_eq!(value(&map, "protocol.pex", vec![]), 1);
    assert!(call(&map, "protocol.pex.set", vec![Object::from("no")]).is_ok());
    assert_eq!(value(&map, "protocol.pex", vec![]), 0);
    assert!(call(&map, "protocol.pex.set", vec![Object::from("bogus")]).is_err());
}

#[test]
fn choke_group_alias_counter_and_duplicate_semantics() {
    let manager = Arc::new(RwLock::new(ChokeGroupManager::new()));
    let mut map = CommandMap::new();
    register_group_commands(&mut map, Arc::clone(&manager)).unwrap();

    assert_eq!(list_text(&map, "choke_group.list", vec![]), vec!["global"]);
    assert_eq!(value(&map, "choke_group.size", vec![]), 1);

    // Legacy `group.insert` creates the same resource-manager groups used by
    // the choke-group commands.
    assert_eq!(
        text(
            &map,
            "group.insert",
            vec![Object::from("seeding"), Object::from("seeding")]
        ),
        "seeding"
    );
    assert!(call(
        &map,
        "group.insert",
        vec![Object::from("bad name"), Object::from("view")]
    )
    .is_err());
    assert!(call(&map, "group.insert", vec![Object::from("only-name")]).is_err());
    assert!(map.has("group.seeding.ratio.min"));
    assert!(call(
        &map,
        "group.seeding.ratio.min.set",
        vec![Object::from("seeding"), Object::from(150i64)]
    )
    .is_ok());
    assert_eq!(
        value(
            &map,
            "group.seeding.ratio.min",
            vec![Object::from("seeding")]
        ),
        150
    );

    let index = value(&map, "choke_group.insert", vec![Object::from("fast")]);
    assert_eq!(index, 2);
    assert_eq!(
        list_text(&map, "choke_group.list", vec![]),
        vec!["global", "seeding", "fast"]
    );
    // Names and negative indexes resolve to the same group.
    assert_eq!(
        value(&map, "choke_group.index_of", vec![Object::from("fast")]),
        index
    );

    assert!(call(
        &map,
        "choke_group.up.max.set",
        vec![Object::from("fast"), Object::from(5i64)]
    )
    .is_ok());
    assert_eq!(
        value(&map, "choke_group.up.max", vec![Object::from(-1i64)]),
        5
    );
    assert!(call(
        &map,
        "choke_group.up.min.set",
        vec![Object::from("fast"), Object::from(20i64)]
    )
    .is_ok());
    assert_eq!(
        value(&map, "choke_group.up.min", vec![Object::from("fast")]),
        5
    );
    assert!(call(
        &map,
        "choke_group.down.max.set",
        vec![Object::from("fast"), Object::from(-1i64)]
    )
    .is_ok());
    assert_eq!(
        value(
            &map,
            "choke_group.down.max.unlimited",
            vec![Object::from("fast")]
        ),
        1
    );

    // Invalid ranges and names leave the previous configuration intact.
    for bad in ["-2", "4294967296", "not-a-number"] {
        assert!(call(
            &map,
            "choke_group.up.max.set",
            vec![Object::from("fast"), Object::from(bad)]
        )
        .is_err());
    }
    assert_eq!(
        value(&map, "choke_group.up.max", vec![Object::from("fast")]),
        5
    );
    assert!(call(&map, "choke_group.up.max", vec![Object::from("missing")]).is_err());
    assert!(call(&map, "choke_group.up.max", vec![Object::from(99i64)]).is_err());

    // `u32::MAX` is the engine sentinel for unlimited, so it must read back
    // exactly like "-1" instead of as a 4294967295-slot cap.
    assert!(call(
        &map,
        "choke_group.up.max.set",
        vec![Object::from("fast"), Object::from(u32::MAX as i64)]
    )
    .is_ok());
    assert_eq!(
        value(&map, "choke_group.up.max", vec![Object::from("fast")]),
        -1
    );
    assert_eq!(
        value(
            &map,
            "choke_group.up.max.unlimited",
            vec![Object::from("fast")]
        ),
        1
    );
    assert!(call(
        &map,
        "choke_group.up.max.set",
        vec![Object::from("fast"), Object::from(5i64)]
    )
    .is_ok());
    assert_eq!(
        value(&map, "choke_group.up.max", vec![Object::from("fast")]),
        5
    );

    // Group integer arguments use the legacy base-0 parser as well.
    assert!(call(
        &map,
        "choke_group.up.max.set",
        vec![Object::from("fast"), Object::from("0x10")]
    )
    .is_ok());
    assert_eq!(
        value(&map, "choke_group.up.max", vec![Object::from("fast")]),
        16
    );
    assert!(call(
        &map,
        "choke_group.up.max.set",
        vec![Object::from("fast"), Object::from(5i64)]
    )
    .is_ok());

    for counter in [
        "choke_group.up.rate",
        "choke_group.down.rate",
        "choke_group.up.total",
        "choke_group.up.queued",
        "choke_group.up.unchoked",
        "choke_group.down.total",
        "choke_group.down.queued",
        "choke_group.down.unchoked",
    ] {
        assert_eq!(
            value(&map, counter, vec![Object::from("fast")]),
            0,
            "{counter}"
        );
        assert!(
            call(
                &map,
                &format!("{counter}.set"),
                vec![Object::from("fast"), Object::from(1i64)]
            )
            .is_err(),
            "{counter} must be read-only"
        );
    }

    // `erase` is an alias for `remove`; the global group cannot be removed.
    assert!(call(&map, "choke_group.erase", vec![Object::from("fast")]).is_ok());
    assert_eq!(value(&map, "choke_group.size", vec![]), 2);
    assert!(call(&map, "choke_group.remove", vec![Object::from(0i64)]).is_err());

    // Duplicate and numeric names are rejected.
    assert!(call(&map, "choke_group.insert", vec![Object::from("global")]).is_err());
    assert!(call(&map, "choke_group.insert", vec![Object::from("123")]).is_err());
    assert!(call(&map, "choke_group.insert", vec![Object::from("")]).is_err());
}

#[test]
fn combined_registration_has_no_duplicate_command_names() {
    let manager = Arc::new(RwLock::new(ChokeGroupManager::new()));
    let mut map = CommandMap::new();
    // The generic ratio module registers `group.seeding.ratio.*` first; the
    // group-aware implementation must replace those entries instead of
    // failing with a duplicate-key error.
    rt_core::commands::ratio::register_ratio_commands(&mut map).unwrap();
    register_system_commands(&mut map).unwrap();
    register_network_commands(&mut map, NetworkConfig::new()).unwrap();
    register_pieces_commands(&mut map).unwrap();
    register_protocol_commands(&mut map, NetworkConfig::new()).unwrap();
    register_group_commands(&mut map, manager).unwrap();

    for name in [
        "file.prioritize_toc",
        "file.prioritize_toc.first",
        "file.prioritize_toc.first.set",
        "system.files.advised_counter",
        "network.bind_address",
        "network.local_address",
        "network.tos.set",
        "pieces.hash.on_completion",
        "protocol.encryption",
        "choke_group.list",
        "group.insert",
    ] {
        assert!(map.has(name), "{name} missing from the combined map");
    }

    // Read-only counters never gain a setter through another module.
    for name in [
        "system.files.opened_counter",
        "system.files.closed_counter",
        "system.files.failed_counter",
        "system.files.advised_counter",
        "network.open_files",
        "network.total_handshakes",
        "pieces.hash.queue_size",
        "choke_group.up.total",
    ] {
        assert!(
            !map.has(&format!("{name}.set")),
            "{name} unexpectedly has a setter"
        );
    }
}
