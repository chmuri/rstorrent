use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use lt_common::{HashString, Object};
use lt_net::config::NetworkConfig;
use lt_storage::disk_thread::{global_disk_thread, global_file_manager_stats};
use lt_storage::writer::TargetFileInfo;
use rt_core::commands::groups::{register_group_commands, ChokeGroupManager};
use rt_core::commands::network::register_network_commands;
use rt_core::commands::pieces::register_pieces_commands;
use rt_core::commands::protocol::register_protocol_commands;
use rt_core::commands::system::{
    clear_shutdown_request, file_status_cache, register_system_commands, shutdown_request,
    ShutdownRequest,
};
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

#[test]
fn network_port_range_random_and_aliases() {
    let mut map = CommandMap::new();
    register_network_commands(&mut map, NetworkConfig::new()).unwrap();

    assert_eq!(text(&map, "network.listen.port.range", vec![]), "6881-6999");
    assert!(call(
        &map,
        "network.listen.port.range.set",
        vec![Object::from("7000-7100")]
    )
    .is_ok());
    assert_eq!(
        text(&map, "network.listen.port.range", vec![]),
        "7000-7100"
    );

    // Legacy aliases.
    assert_eq!(text(&map, "network.port_range", vec![]), "7000-7100");
    assert!(call(
        &map,
        "network.port_range.set",
        vec![Object::from("7200-7300")]
    )
    .is_ok());
    assert_eq!(
        text(&map, "network.listen.port.range", vec![]),
        "7200-7300"
    );
    assert!(call(&map, "port_range", vec![Object::from("7400-7500")]).is_ok());
    assert_eq!(
        text(&map, "network.listen.port.range", vec![]),
        "7400-7500"
    );

    assert_eq!(value(&map, "network.listen.port.random", vec![]), 1);
    assert!(call(
        &map,
        "network.port_random.set",
        vec![Object::from(0i64)]
    )
    .is_ok());
    assert_eq!(value(&map, "network.listen.port.random", vec![]), 0);
    assert!(call(&map, "port_random", vec![Object::from(1i64)]).is_ok());
    assert_eq!(value(&map, "network.port_random", vec![]), 1);

    // `0-0` is the documented "disabled" value used by `rtorrent -p 0-0`.
    assert!(call(
        &map,
        "network.listen.port.range.set",
        vec![Object::from("0-0")]
    )
    .is_ok());

    for invalid in ["7000-6999", "1-65536", "abc", "0-1", "1-2-3"] {
        assert!(
            call(
                &map,
                "network.listen.port.range.set",
                vec![Object::from(invalid)]
            )
            .is_err(),
            "range {invalid} should be rejected"
        );
    }

    assert!(call(
        &map,
        "network.local_port.set",
        vec![Object::from(65536i64)]
    )
    .is_err());
    assert!(call(
        &map,
        "network.local_port.set",
        vec![Object::from(-1i64)]
    )
    .is_err());
    assert!(call(
        &map,
        "network.listen.port.set",
        vec![Object::from(7000i64)]
    )
    .is_ok());
    assert_eq!(value(&map, "network.listen.port", vec![]), 7000);
    assert!(call(
        &map,
        "network.listen.port.set",
        vec![Object::from(65536i64)]
    )
    .is_err());

    // Byte-size suffix support for socket buffers.
    assert!(call(
        &map,
        "network.send_buffer.size.set",
        vec![Object::from("1mb")]
    )
    .is_ok());
    assert_eq!(
        value(&map, "network.send_buffer.size", vec![]),
        1024 * 1024
    );
    assert!(call(
        &map,
        "network.receive_buffer.size.set",
        vec![Object::from("64kb")]
    )
    .is_ok());
    assert_eq!(
        value(&map, "network.receive_buffer.size", vec![]),
        64 * 1024
    );
    assert!(call(
        &map,
        "network.send_buffer.size.set",
        vec![Object::from("1xb")]
    )
    .is_err());
}

#[test]
fn network_proxy_http_and_rpc_toggles() {
    let mut map = CommandMap::new();
    register_network_commands(&mut map, NetworkConfig::new()).unwrap();

    assert_eq!(value(&map, "network.http.max_total_connections", vec![]), 32);
    assert_eq!(value(&map, "network.http.ssl_verify_host", vec![]), 1);
    assert_eq!(value(&map, "network.http.ssl_verify_peer", vec![]), 1);
    assert_eq!(value(&map, "network.http.dns_cache_timeout", vec![]), 60);
    assert_eq!(value(&map, "network.rpc.use_xmlrpc", vec![]), 1);
    assert_eq!(value(&map, "network.rpc.use_jsonrpc", vec![]), 1);
    assert_eq!(value(&map, "network.scgi.use_gzip", vec![]), 1);
    assert_eq!(value(&map, "network.scgi.gzip.min_size", vec![]), 1000);

    // HTTP cache counters and limits. `current_open` is a read-only socket
    // statistic; the limit setters reject negative values.
    assert!(value(&map, "network.http.current_open", vec![]) >= 0);
    assert!(call(
        &map,
        "network.http.max_cache_connections.set",
        vec![Object::from(64i64)]
    )
    .is_ok());
    assert_eq!(
        value(&map, "network.http.max_cache_connections", vec![]),
        64
    );
    assert!(call(
        &map,
        "network.http.max_host_connections.set",
        vec![Object::from(8i64)]
    )
    .is_ok());
    assert_eq!(
        value(&map, "network.http.max_host_connections", vec![]),
        8
    );
    assert!(call(
        &map,
        "network.http.max_cache_connections.set",
        vec![Object::from(-1i64)]
    )
    .is_err());
    assert!(call(
        &map,
        "network.http.max_cache_connections.set",
        vec![Object::from(1025i64)]
    )
    .is_err());
    assert!(call(
        &map,
        "network.http.max_host_connections.set",
        vec![Object::from(1025i64)]
    )
    .is_err());
    assert!(call(
        &map,
        "network.http.dns_cache_timeout.set",
        vec![Object::from(-1i64)]
    )
    .is_err());
    assert!(call(
        &map,
        "network.http.current_open.set",
        vec![Object::from(1i64)]
    )
    .is_err());

    assert!(call(
        &map,
        "network.proxy.http.set",
        vec![Object::from("http://127.0.0.1:8080")]
    )
    .is_ok());
    assert_eq!(
        text(&map, "network.http.proxy_address", vec![]),
        "http://127.0.0.1:8080"
    );
    assert!(call(
        &map,
        "proxy_address",
        vec![Object::from("socks5://127.0.0.1:1080")]
    )
    .is_ok());
    assert_eq!(
        text(&map, "network.proxy.http", vec![]),
        "socks5://127.0.0.1:1080"
    );
    assert!(call(
        &map,
        "network.http.proxy_address.set",
        vec![Object::from("not-a-url")]
    )
    .is_err());

    assert!(call(
        &map,
        "network.http.max_open.set",
        vec![Object::from(10i64)]
    )
    .is_ok());
    assert_eq!(value(&map, "network.http.max_total_connections", vec![]), 10);
    assert_eq!(value(&map, "network.http.max_open", vec![]), 10);
    assert!(call(
        &map,
        "network.http.max_open.set",
        vec![Object::from(-1i64)]
    )
    .is_err());
    assert!(call(
        &map,
        "network.http.max_open.set",
        vec![Object::from(u32::MAX as i64 + 1)]
    )
    .is_err());
    assert!(call(
        &map,
        "network.http.max_total_connections.set",
        vec![Object::from(4097i64)]
    )
    .is_err());
    assert!(call(
        &map,
        "network.http.max_open.set",
        vec![Object::from(4097i64)]
    )
    .is_err());

    assert!(call(
        &map,
        "network.http.ssl_verify_peer.set",
        vec![Object::from(0i64)]
    )
    .is_ok());
    assert_eq!(value(&map, "network.http.ssl_verify_peer", vec![]), 0);

    assert!(call(
        &map,
        "network.rpc.use_jsonrpc.set",
        vec![Object::from(0i64)]
    )
    .is_ok());
    assert_eq!(value(&map, "network.rpc.use_jsonrpc", vec![]), 0);
    assert!(call(
        &map,
        "network.scgi.gzip.min_size.set",
        vec![Object::from(-1i64)]
    )
    .is_err());

    // Deprecated no-op commands still exist for rc compatibility.
    assert_eq!(value(&map, "network.port_open", vec![]), 0);
    assert!(call(&map, "network.port_open.set", vec![Object::from(1i64)]).is_ok());
}

#[test]
fn system_file_advice_umask_and_shutdown() {
    let mut map = CommandMap::new();
    register_system_commands(&mut map).unwrap();

    assert_eq!(
        value(&map, "system.file.max_size", vec![]),
        512i64 << 30
    );
    assert_eq!(value(&map, "system.file.split_size", vec![]), -1);
    assert_eq!(text(&map, "system.file.split_suffix", vec![]), ".part");
    assert_eq!(text(&map, "system.file_name.replace_slash", vec![]), "_");
    assert_eq!(value(&map, "system.files.advise_random", vec![]), 1);
    assert_eq!(
        value(&map, "system.files.advise_random.hashing", vec![]),
        0
    );
    assert_eq!(
        value(&map, "system.files.session.fdatasync", vec![]),
        1
    );
    assert_eq!(value(&map, "system.daemon", vec![]), 0);

    assert!(call(
        &map,
        "system.file.max_size.set",
        vec![Object::from("1gb")]
    )
    .is_ok());
    assert_eq!(value(&map, "system.file.max_size", vec![]), 1i64 << 30);
    assert!(call(
        &map,
        "system.file.split_size.set",
        vec![Object::from("-1")]
    )
    .is_ok());
    assert_eq!(value(&map, "system.file.split_size", vec![]), -1);
    assert!(call(
        &map,
        "system.file.split_size.set",
        vec![Object::from("-2")]
    )
    .is_err());
    assert!(call(
        &map,
        "system.file.allocate.set",
        vec![Object::from(-1i64)]
    )
    .is_err());

    assert!(call(
        &map,
        "system.file_name.replace_slash.set",
        vec![Object::from("a/b")]
    )
    .is_err());
    assert!(call(
        &map,
        "system.file_name.replace_slash.set",
        vec![Object::from(".")]
    )
    .is_ok());
    assert_eq!(text(&map, "system.file_name.replace_slash", vec![]), ".");

    assert!(call(
        &map,
        "system.files.advise_random.set",
        vec![Object::from(0i64)]
    )
    .is_ok());
    assert_eq!(value(&map, "system.files.advise_random", vec![]), 0);
    assert!(call(
        &map,
        "system.files.session.fdatasync.set",
        vec![Object::from(0i64)]
    )
    .is_ok());
    assert_eq!(
        value(&map, "system.files.session.fdatasync", vec![]),
        0
    );
    assert!(call(
        &map,
        "system.daemon.set",
        vec![Object::from(1i64)]
    )
    .is_ok());
    assert_eq!(value(&map, "system.daemon", vec![]), 1);

    // Invalid umask is rejected before changing the process value.
    assert!(call(
        &map,
        "system.umask.set",
        vec![Object::from(0o1000i64)]
    )
    .is_err());

    clear_shutdown_request();
    assert!(call(&map, "system.shutdown.normal", vec![]).is_ok());
    assert_eq!(shutdown_request(), Some(ShutdownRequest::Normal));
    assert!(call(&map, "system.shutdown.quick", vec![]).is_ok());
    assert_eq!(shutdown_request(), Some(ShutdownRequest::Quick));
    clear_shutdown_request();
    assert_eq!(shutdown_request(), None);
}

#[test]
fn system_socket_accounting_and_aliases() {
    let mut map = CommandMap::new();
    register_system_commands(&mut map).unwrap();
    register_network_commands(&mut map, NetworkConfig::new()).unwrap();

    assert!(value(&map, "system.sockets.max_size", vec![]) >= 512);
    assert!(call(
        &map,
        "system.sockets.max_size.set",
        vec![Object::from(100i64)]
    )
    .is_err());
    assert!(call(
        &map,
        "system.sockets.max_size.set",
        vec![Object::from(2048i64)]
    )
    .is_ok());
    assert_eq!(value(&map, "system.sockets.max_size", vec![]), 2048);
    assert_eq!(value(&map, "network.max_open_sockets", vec![]), 2048);

    assert!(call(
        &map,
        "system.sockets.files.min_alloc.set",
        vec![Object::from(16i64)]
    )
    .is_ok());
    assert_eq!(
        value(&map, "system.sockets.files.min_alloc", vec![]),
        16
    );
    assert!(call(
        &map,
        "system.sockets.files.max_alloc.set",
        vec![Object::from(1024i64)]
    )
    .is_ok());
    assert_eq!(
        value(&map, "system.sockets.files.max_alloc", vec![]),
        1024
    );
    assert!(call(
        &map,
        "system.sockets.files.max_alloc.set",
        vec![Object::from(70000i64)]
    )
    .is_err());

    assert!(call(&map, "system.sockets.adjust_alloc", vec![]).is_ok());
    assert!(value(&map, "system.sockets.available_alloc", vec![]) > 0);
    assert_eq!(
        value(&map, "system.sockets.generic.size", vec![]),
        0
    );

    assert_eq!(value(&map, "network.max_open_files", vec![]), 500);
    assert!(call(
        &map,
        "network.max_open_files.set",
        vec![Object::from(1024i64)]
    )
    .is_ok());
    assert_eq!(value(&map, "network.max_open_files", vec![]), 1024);
    assert!(call(
        &map,
        "network.max_open_files.set",
        vec![Object::from(2i64)]
    )
    .is_err());

    assert!(value(&map, "network.total_handshakes", vec![]) >= 0);
    assert!(call(
        &map,
        "network.total_handshakes.set",
        vec![Object::from(1i64)]
    )
    .is_err());
}

#[test]
fn system_file_status_cache_and_file_counters() {
    let mut map = CommandMap::new();
    register_system_commands(&mut map).unwrap();

    let cache = file_status_cache();
    let before = cache.lock().unwrap().size();

    let dir = std::env::temp_dir().join(format!(
        "rtorrent-file-status-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("cached.bin");
    std::fs::write(&path, b"payload").unwrap();

    assert!(cache.lock().unwrap().insert(&path).unwrap());
    assert_eq!(cache.lock().unwrap().size(), before + 1);
    assert_eq!(
        value(&map, "system.file_status_cache.size", vec![]),
        (before + 1) as i64
    );

    // Removing the file lets `prune` drop the now-stale entry.
    std::fs::remove_file(&path).unwrap();
    assert!(call(&map, "system.file_status_cache.prune", vec![]).is_ok());
    assert_eq!(
        value(&map, "system.file_status_cache.size", vec![]),
        before as i64
    );
    let _ = std::fs::remove_dir_all(dir);

    // Cache and file counters are read-only.
    assert!(call(
        &map,
        "system.file_status_cache.size.set",
        vec![Object::from(1i64)]
    )
    .is_err());
    for name in [
        "system.files.opened_counter",
        "system.files.closed_counter",
        "system.files.failed_counter",
    ] {
        assert!(value(&map, name, vec![]) >= 0);
        assert!(call(&map, &format!("{name}.set"), vec![Object::from(1i64)]).is_err());
    }
}

#[test]
fn system_file_counters_and_open_files_use_disk_worker_stats() {
    let mut map = CommandMap::new();
    register_system_commands(&mut map).unwrap();
    let mut network_map = CommandMap::new();
    register_network_commands(&mut network_map, NetworkConfig::new()).unwrap();

    let root = std::env::temp_dir().join(format!(
        "rtorrent-command-file-counters-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();

    let before = global_file_manager_stats().unwrap_or_default();
    let files = Arc::new(vec![TargetFileInfo {
        path: PathBuf::from("counted.bin"),
        length: 8,
        offset: 0,
    }]);

    let thread = global_disk_thread().expect("global disk worker should spawn");
    thread
        .submit_write(&root, Arc::clone(&files), 8, 0, b"abcdefgh".to_vec())
        .unwrap()
        .wait()
        .unwrap();

    let opened = global_file_manager_stats().unwrap();
    assert!(opened.files_opened > before.files_opened);
    assert_eq!(
        value(&map, "system.files.opened_counter", vec![]),
        opened.files_opened as i64
    );
    assert_eq!(
        value(&network_map, "network.open_files", vec![]),
        opened.open_files as i64
    );

    // A missing path is accounted as a failed open by the FileManager.
    let missing = Arc::new(vec![TargetFileInfo {
        path: PathBuf::from("missing-dir/missing.bin"),
        length: 4,
        offset: 0,
    }]);
    assert!(thread
        .read_block(&root, missing, 4, 0, 0, 4)
        .is_err());
    let failed = global_file_manager_stats().unwrap();
    assert!(failed.files_failed > before.files_failed);
    assert_eq!(
        value(&map, "system.files.failed_counter", vec![]),
        failed.files_failed as i64
    );

    // Closing the worker's cached handles updates the closed counter.
    thread.close().unwrap();
    let closed = global_file_manager_stats().unwrap();
    assert!(closed.files_closed > before.files_closed);
    assert_eq!(
        value(&map, "system.files.closed_counter", vec![]),
        closed.files_closed as i64
    );

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn pieces_real_memory_stats_and_validation() {
    let mut map = CommandMap::new();
    register_pieces_commands(&mut map).unwrap();

    assert!(call(
        &map,
        "pieces.memory.max.set",
        vec![Object::from("8mb")]
    )
    .is_ok());
    assert_eq!(value(&map, "pieces.memory.max", vec![]), 8 * 1024 * 1024);
    assert!(value(&map, "pieces.memory.current", vec![]) >= 0);
    assert!(value(&map, "pieces.memory.block_count", vec![]) >= 0);
    assert!(value(&map, "pieces.sync.queue_size", vec![]) >= 0);
    assert!(value(&map, "pieces.sync.safe_free_diskspace", vec![]) > 0);
    assert!(value(&map, "pieces.hash.queue_size", vec![]) >= 0);

    // The hash queue length is the best available approximation: accepted hash
    // jobs minus completed jobs on the global disk worker.
    let stats = global_disk_thread()
        .expect("global disk worker should spawn")
        .stats();
    let outstanding = stats
        .hashes_submitted
        .saturating_sub(stats.hashes_completed)
        .min(i64::MAX as u64) as i64;
    assert_eq!(
        value(&map, "pieces.hash.queue_size", vec![]),
        outstanding
    );

    // Read-only counters must not expose setters.
    assert!(call(
        &map,
        "pieces.memory.block_count.set",
        vec![Object::from(1i64)]
    )
    .is_err());
    assert!(call(
        &map,
        "pieces.sync.queue_size.set",
        vec![Object::from(1i64)]
    )
    .is_err());
    assert!(call(
        &map,
        "pieces.hash.queue_size.set",
        vec![Object::from(1i64)]
    )
    .is_err());

    assert!(call(
        &map,
        "pieces.sync.timeout.set",
        vec![Object::from(3600i64)]
    )
    .is_ok());
    assert!(call(
        &map,
        "pieces.sync.timeout.set",
        vec![Object::from(3601i64)]
    )
    .is_err());
    assert!(call(
        &map,
        "pieces.preload.type.set",
        vec![Object::from(3i64)]
    )
    .is_err());
    assert!(call(
        &map,
        "pieces.preload.min_size.set",
        vec![Object::from(512i64)]
    )
    .is_err());
    assert!(call(
        &map,
        "pieces.preload.min_size.set",
        vec![Object::from("1kb")]
    )
    .is_ok());
    assert_eq!(
        value(&map, "pieces.preload.min_size", vec![]),
        1024
    );

    assert!(call(
        &map,
        "pieces.sync.always_safe.set",
        vec![Object::from(1i64)]
    )
    .is_ok());
    assert_eq!(value(&map, "pieces.sync.always_safe", vec![]), 1);

    assert!(call(&map, "pieces.hash.on_completion.set", vec![Object::from(0i64)]).is_ok());
    assert!(call(&map, "check_hash", vec![Object::from(1i64)]).is_ok());
    assert_eq!(
        value(&map, "pieces.hash.on_completion", vec![]),
        1
    );

    assert!(call(
        &map,
        "max_memory_usage",
        vec![Object::from(4 * 1024 * 1024i64)]
    )
    .is_ok());
    assert_eq!(
        value(&map, "pieces.memory.max", vec![]),
        4 * 1024 * 1024
    );
}

#[test]
fn protocol_encryption_aliases_and_defaults() {
    let mut map = CommandMap::new();
    register_protocol_commands(&mut map, NetworkConfig::new()).unwrap();

    assert_eq!(
        text(&map, "protocol.encryption", vec![]),
        "allow,allow"
    );
    assert!(call(
        &map,
        "protocol.encryption.set",
        vec![
            Object::from("handshake_require"),
            Object::from("stream_prefer")
        ]
    )
    .is_ok());
    assert_eq!(
        text(&map, "protocol.encryption", vec![]),
        "require,prefer"
    );
    assert_eq!(
        text(&map, "protocol.encryption.handshake", vec![]),
        "require"
    );
    assert_eq!(
        text(&map, "protocol.encryption.stream", vec![]),
        "prefer"
    );

    // Legacy single-value alias.
    assert!(call(&map, "encryption", vec![Object::from("none")]).is_ok());
    assert_eq!(
        text(&map, "protocol.encryption", vec![]),
        "deny,deny"
    );
    assert!(call(&map, "encryption", vec![Object::from("require_RC4")]).is_ok());
    assert_eq!(
        text(&map, "protocol.encryption", vec![]),
        "require,require"
    );
    assert!(call(&map, "protocol.encryption.set", vec![Object::from("bogus")]).is_err());

    assert_eq!(
        text(&map, "protocol.choke_heuristics.up.seed", vec![]),
        "upload_leech"
    );
    assert!(call(&map, "connection_leech", vec![Object::from("seed")]).is_ok());
    assert_eq!(
        text(&map, "protocol.connection.leech", vec![]),
        "seed"
    );
    assert!(call(&map, "connection_seed", vec![Object::from("leech")]).is_ok());
    assert_eq!(
        text(&map, "protocol.connection.seed", vec![]),
        "leech"
    );
    assert!(call(&map, "connection_seed", vec![Object::from("initial_seed")]).is_ok());
    assert!(call(&map, "connection_seed", vec![Object::from("bogus")]).is_err());
    assert_eq!(
        text(&map, "protocol.connection.seed", vec![]),
        "initial_seed"
    );

    // Choke heuristic setters validate the direction-specific option names.
    assert!(call(
        &map,
        "protocol.choke_heuristics.up.leech.set",
        vec![Object::from("upload_seed")]
    )
    .is_ok());
    assert_eq!(
        text(&map, "protocol.choke_heuristics.up.leech", vec![]),
        "upload_seed"
    );
    assert!(call(
        &map,
        "protocol.choke_heuristics.up.leech.set",
        vec![Object::from("download_leech")]
    )
    .is_err());
    assert!(call(
        &map,
        "protocol.choke_heuristics.down.seed.set",
        vec![Object::from("upload_leech")]
    )
    .is_err());
    assert!(call(
        &map,
        "protocol.choke_heuristics.down.seed.set",
        vec![Object::from("download_leech")]
    )
    .is_ok());
    // The documented experimental upload heuristic is a valid alias value.
    assert!(call(
        &map,
        "protocol.choke_heuristics.up.seed.set",
        vec![Object::from("upload_leech_experimental")]
    )
    .is_ok());
    assert_eq!(
        text(&map, "protocol.choke_heuristics.up.seed", vec![]),
        "upload_leech_experimental"
    );

    assert_eq!(value(&map, "protocol.pex", vec![]), 1);
    assert!(call(&map, "protocol.pex.set", vec![Object::from(0i64)]).is_ok());
    assert_eq!(value(&map, "protocol.pex", vec![]), 0);
}

#[test]
fn all_domains_register_without_duplicate_commands() {
    let manager = Arc::new(RwLock::new(ChokeGroupManager::new()));
    let mut map = CommandMap::new();
    register_system_commands(&mut map).unwrap();
    register_network_commands(&mut map, NetworkConfig::new()).unwrap();
    register_pieces_commands(&mut map).unwrap();
    register_protocol_commands(&mut map, NetworkConfig::new()).unwrap();
    register_group_commands(&mut map, manager).unwrap();

    for name in [
        "system.file_status_cache.size",
        "system.files.opened_counter",
        "system.files.closed_counter",
        "system.files.failed_counter",
        "network.http.current_open",
        "network.open_files",
        "network.total_handshakes",
        "pieces.hash.queue_size",
        "protocol.encryption",
        "choke_group.up.max",
        "choke_group.down.max",
    ] {
        assert!(call(&map, name, vec![]).is_ok(), "{name} missing");
    }
}

#[test]
fn choke_group_names_negative_indexes_and_validation() {
    let manager = Arc::new(RwLock::new(ChokeGroupManager::new()));
    let mut map = CommandMap::new();
    register_group_commands(&mut map, Arc::clone(&manager)).unwrap();

    assert!(call(
        &map,
        "choke_group.insert",
        vec![Object::from("named")]
    )
    .is_ok());
    let index = value(
        &map,
        "choke_group.index_of",
        vec![Object::from("named")],
    );
    assert!(index >= 1);

    // Group names are accepted anywhere an index is accepted.
    assert!(call(
        &map,
        "choke_group.up.max.set",
        vec![Object::from("named"), Object::from("7")]
    )
    .is_ok());
    assert_eq!(
        value(&map, "choke_group.up.max", vec![Object::from("named")]),
        7
    );
    assert_eq!(
        value(&map, "choke_group.up.max", vec![Object::from(index)]),
        7
    );

    // Negative indexes resolve from the end.
    assert_eq!(
        value(&map, "choke_group.up.max", vec![Object::from(-1i64)]),
        7
    );

    assert!(call(
        &map,
        "choke_group.down.max.set",
        vec![Object::from("named"), Object::from("-1")]
    )
    .is_ok());
    assert_eq!(
        value(&map, "choke_group.down.max", vec![Object::from("named")]),
        -1
    );
    assert_eq!(
        value(
            &map,
            "choke_group.down.max.unlimited",
            vec![Object::from("named")]
        ),
        1
    );
    assert!(call(
        &map,
        "choke_group.down.max.set",
        vec![Object::from("named"), Object::from("-2")]
    )
    .is_err());

    assert!(call(
        &map,
        "choke_group.insert",
        vec![Object::from("named")]
    )
    .is_err());
    assert!(call(
        &map,
        "choke_group.insert",
        vec![Object::from("123")]
    )
    .is_err());
    assert!(call(
        &map,
        "choke_group.index_of",
        vec![Object::from("missing")]
    )
    .is_err());
    assert!(call(
        &map,
        "choke_group.up.max",
        vec![Object::from("missing")]
    )
    .is_err());
    assert!(call(
        &map,
        "choke_group.up.max",
        vec![Object::from(99i64)]
    )
    .is_err());

    assert!(call(
        &map,
        "choke_group.tracker.mode.set",
        vec![Object::from("named"), Object::from("aggressive")]
    )
    .is_ok());
    assert_eq!(
        text(
            &map,
            "choke_group.tracker.mode",
            vec![Object::from("named")]
        ),
        "aggressive"
    );
    assert!(call(
        &map,
        "choke_group.tracker.mode.set",
        vec![Object::from("named"), Object::from("bogus")]
    )
    .is_err());

    assert!(call(
        &map,
        "choke_group.up.heuristics.set",
        vec![Object::from("named"), Object::from("upload_seed")]
    )
    .is_ok());
    assert!(call(
        &map,
        "choke_group.up.heuristics.set",
        vec![Object::from("named"), Object::from("download_leech")]
    )
    .is_err());
    assert!(call(
        &map,
        "choke_group.down.heuristics.set",
        vec![Object::from("named"), Object::from("download_leech")]
    )
    .is_ok());
    assert!(call(
        &map,
        "choke_group.down.heuristics.set",
        vec![Object::from("named"), Object::from("upload_seed")]
    )
    .is_err());

    assert_eq!(
        value(
            &map,
            "choke_group.general.size",
            vec![Object::from("named")]
        ),
        0
    );

    // The command reads engine membership, so entries inserted directly into
    // the resource manager are visible without a duplicate command-side list.
    manager
        .write()
        .unwrap()
        .resource_manager_mut()
        .insert_entry(HashString::new([0x5a; 20]), 1, index as u16);
    assert_eq!(
        value(
            &map,
            "choke_group.general.size",
            vec![Object::from("named")]
        ),
        1
    );

    // Rates and queue counters are read-only.
    for name in [
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
            value(&map, name, vec![Object::from("named")]),
            0,
            "{name} should start at zero"
        );
        assert!(
            call(&map, &format!("{name}.set"), vec![Object::from(1i64)]).is_err(),
            "{name} must remain read-only"
        );
    }
}
