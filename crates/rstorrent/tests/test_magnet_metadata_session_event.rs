use std::fs;
use std::path::PathBuf;

use lt_common::hash_string::HashString;
use lt_common::Object;
use lt_crypto::sha1::sha1_hash;
use rstorrent::cli::Cli;
use rstorrent::control::Control;

const TRACKER: &str = "http://tracker.example.com/announce";

fn info_bytes(name: &str) -> (Vec<u8>, HashString) {
    let mut info = Object::create_map();
    info.insert_key("name", Object::from(name)).unwrap();
    info.insert_key("piece length", Object::from(16384i64))
        .unwrap();
    info.insert_key("pieces", Object::create_string(vec![0u8; 20]))
        .unwrap();
    info.insert_key("length", Object::from(16384i64)).unwrap();

    let bytes = lt_common::object::bencode::encode(&info).unwrap();
    let hash = HashString::new(sha1_hash(&bytes));
    (bytes, hash)
}

#[tokio::test]
async fn metadata_event_persists_torrent_immediately() {
    let root =
        std::env::temp_dir().join(format!("rstorrent_metadata_event_{}", std::process::id()));
    let session_dir = root.join("session");
    let data_dir = root.join("data");
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&session_dir).unwrap();
    fs::create_dir_all(&data_dir).unwrap();

    let port = 45000 + (std::process::id() % 1000) as u16;
    let cli = Cli {
        bind_address: None,
        directory: Some(data_dir.to_string_lossy().to_string()),
        local_address: None,
        port_range: Some(port.to_string()),
        session: Some(session_dir.to_string_lossy().to_string()),
        no_rc: true,
        config_file: None,
        daemon: true,
        grpc_port: None,
        grpc_socket: None,
        options: vec!["dht.mode.set=off".to_string()],
        command_options: Vec::new(),
        deprecated: false,
        disable_intermediate: false,
        intermediate_no_xmlrpc: false,
        files: Vec::new(),
    };

    let mut control = Control::new();
    control.initialize(&cli).expect("control initialization");

    let (bytes, hash) = info_bytes("event-session");
    let uri = format!(
        "magnet:?xt=urn:btih:{}&dn=event-session&tr={}",
        hash.to_hex(),
        TRACKER.replace(':', "%3A").replace('/', "%2F")
    );

    control
        .try_load_torrent(&uri, true)
        .expect("magnet should load");
    let download = control.download_list.read().unwrap().all()[0].clone();

    {
        let mut guard = download.write().unwrap();
        guard
            .download_mut()
            .apply_metadata_bytes(
                &bytes,
                hash,
                Some(TRACKER.to_string()),
                vec![vec![TRACKER.to_string()]],
            )
            .expect("metadata should apply");
        guard.set_directory(data_dir.to_string_lossy().to_string());
        guard.sync_metadata_from_engine();
        assert!(
            guard.bencode().is_some(),
            "wrapper must rebuild the static torrent"
        );
    }

    control
        .download_list
        .read()
        .unwrap()
        .notify_metadata_updated(&download);

    let torrent_path: PathBuf =
        session_dir.join(format!("{}.torrent", hash.to_hex().to_uppercase()));
    let saved = fs::read(&torrent_path)
        .expect("metadata event must write <info-hash>.torrent synchronously");
    let object = lt_common::object::bencode::decode(&saved).unwrap();
    let info = object.get_key("info").unwrap();
    let raw_info = lt_common::object::bencode::encode(info).unwrap();
    assert_eq!(HashString::new(sha1_hash(&raw_info)), hash);

    control.shutdown().await;
    let _ = fs::remove_dir_all(&root);
}
