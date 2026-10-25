//! End-to-end check that the network supervisor verifies a payload that was
//! already on disk when a magnet download receives its metadata.

use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

use lt_common::hash_string::HashString;
use lt_common::object::Object;
use lt_crypto::sha1::sha1_hash;
use rstorrent::cli::Cli;
use rstorrent::control::Control;

const PIECE_LENGTH: usize = 1024;
const NAME: &str = "payload.bin";

fn payload() -> Vec<u8> {
    (0..(3 * PIECE_LENGTH)).map(|i| (i % 251) as u8).collect()
}

fn info_bytes() -> (Vec<u8>, HashString) {
    let payload = payload();
    let mut pieces = Vec::new();
    for chunk in payload.chunks(PIECE_LENGTH) {
        pieces.extend_from_slice(&sha1_hash(chunk));
    }

    let mut info = Object::create_map();
    info.insert_key("name", Object::from(NAME)).unwrap();
    info.insert_key("piece length", Object::from(PIECE_LENGTH as i64))
        .unwrap();
    info.insert_key("pieces", Object::create_string(pieces))
        .unwrap();
    info.insert_key("length", Object::from(payload.len() as i64))
        .unwrap();

    let bytes = lt_common::object::bencode::encode(&info).unwrap();
    let hash = HashString::new(sha1_hash(&bytes));
    (bytes, hash)
}

fn cli(data_dir: &Path, session_dir: &Path, port: u16) -> Cli {
    Cli {
        bind_address: None,
        directory: Some(data_dir.to_string_lossy().to_string()),
        watch_dir: None,
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
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn supervisor_verifies_existing_payload_after_metadata() {
    let root =
        std::env::temp_dir().join(format!("rstorrent_magnet_payload_{}", std::process::id()));
    let session_dir = root.join("session");
    let data_dir = root.join("data");
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&session_dir).unwrap();
    fs::create_dir_all(&data_dir).unwrap();
    fs::write(data_dir.join(NAME), payload()).unwrap();

    let port = 46000 + (std::process::id() % 1000) as u16;
    let mut control = Control::new();
    control
        .initialize(&cli(&data_dir, &session_dir, port))
        .expect("control initialization");

    let (bytes, hash) = info_bytes();
    let uri = format!("magnet:?xt=urn:btih:{}&dn={}", hash.to_hex(), NAME);
    control
        .try_load_torrent(&uri, true)
        .expect("magnet should load");
    let download = control.download_list.read().unwrap().all()[0].clone();

    // Simulate `ut_metadata` applying the info dictionary on the engine. The
    // supervisor must notice this on its next tick and start the hash check
    // without any test-side trigger.
    download
        .write()
        .unwrap()
        .download_mut()
        .apply_metadata_bytes(&bytes, hash, None, Vec::new())
        .expect("metadata should apply");

    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if download.read().unwrap().is_done() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "supervisor did not verify the existing payload in time"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    {
        let guard = download.read().unwrap();
        assert_eq!(guard.state_name(), "Seeding");
        assert_eq!(guard.completed_bytes(), payload().len() as u64);
        assert!(!guard.is_metadata_download());
    }
    assert_eq!(fs::read(data_dir.join(NAME)).unwrap(), payload());

    control.shutdown().await;
    let _ = fs::remove_dir_all(&root);
}
