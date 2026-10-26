//! Tests for the watch-directory feature: `.torrent` files dropped into a
//! directory configured with `-w/--watch-dir` are loaded and started on
//! startup.
//!
//! The suite covers the CLI surface, the `Control::load_watch_directory`
//! behaviour in isolation, and the full `Control::initialize` integration path.

use clap::Parser;
use rstorrent::{Cli, Control};
use std::fs;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

/// Self-cleaning temporary directory, unique per test invocation.
struct TestDir {
    path: PathBuf,
}

impl TestDir {
    fn new(label: &str) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let id = COUNTER.fetch_add(1, Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!(
            "rstorrent_watch_{}_{}_{}",
            label,
            std::process::id(),
            id
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("create test dir");
        Self { path }
    }

    fn join(&self, name: &str) -> PathBuf {
        self.path.join(name)
    }

    fn as_str(&self) -> String {
        self.path.to_string_lossy().into_owned()
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn free_port() -> u16 {
    let probe = TcpListener::bind(("127.0.0.1", 0)).expect("bind probe");
    probe.local_addr().expect("probe addr").port()
}

/// Builds a minimal, valid single-file torrent for `payload`.
fn make_torrent(name: &str, payload: &[u8]) -> Vec<u8> {
    const PIECE_LENGTH: usize = 16384;

    let mut pieces = Vec::new();
    for chunk in payload.chunks(PIECE_LENGTH) {
        pieces.extend_from_slice(&lt_crypto::sha1::sha1_hash(chunk));
    }

    let mut out = Vec::new();
    out.push(b'd');
    out.extend_from_slice(b"4:infod");
    out.extend_from_slice(format!("6:lengthi{}e", payload.len()).as_bytes());
    out.extend_from_slice(format!("4:name{}:{}", name.len(), name).as_bytes());
    out.extend_from_slice(format!("12:piece lengthi{}e", PIECE_LENGTH).as_bytes());
    out.extend_from_slice(format!("6:pieces{}:", pieces.len()).as_bytes());
    out.extend_from_slice(&pieces);
    out.extend_from_slice(b"ee");
    out
}

fn write_torrent(dir: &Path, file_name: &str, torrent_name: &str, payload: &[u8]) -> PathBuf {
    let path = dir.join(file_name);
    fs::write(&path, make_torrent(torrent_name, payload)).expect("write torrent");
    path
}

fn loaded_names(control: &Control) -> Vec<String> {
    let dl_list = control.download_list.read().unwrap();
    dl_list
        .all()
        .iter()
        .map(|dl| dl.read().unwrap().name().to_string())
        .collect()
}

fn main_view_len(control: &Control) -> usize {
    let vm = control.view_manager.read().unwrap();
    vm.find("main").expect("main view").all_downloads().len()
}

fn initialize_cli(directory: Option<&str>, watch_dir: Option<&str>, session: &str) -> Cli {
    let port = free_port();
    Cli {
        directory: directory.map(str::to_string),
        watch_dir: watch_dir.map(str::to_string),
        session: Some(session.to_string()),
        no_rc: true,
        port_range: Some(format!("{}-{}", port, port)),
        daemon: true,
        ..Default::default()
    }
}

// ---------------------------------------------------------------------------
// CLI surface
// ---------------------------------------------------------------------------

#[test]
fn cli_accepts_short_flags() {
    let cli = Cli::try_parse_from(["rstorrent", "-d", "/downloads/path", "-w", "/watch/path"])
        .expect("parse short flags");

    assert_eq!(cli.directory.as_deref(), Some("/downloads/path"));
    assert_eq!(cli.watch_dir.as_deref(), Some("/watch/path"));
}

#[test]
fn cli_accepts_canonical_long_flags() {
    let cli = Cli::try_parse_from([
        "rstorrent",
        "--directory",
        "/downloads/canonical",
        "--watch-dir",
        "/watch/canonical",
    ])
    .expect("parse canonical long flags");

    assert_eq!(cli.directory.as_deref(), Some("/downloads/canonical"));
    assert_eq!(cli.watch_dir.as_deref(), Some("/watch/canonical"));
}

#[test]
fn cli_accepts_long_aliases() {
    let cli = Cli::try_parse_from([
        "rstorrent",
        "--download-dir",
        "/downloads/alias",
        "--watch",
        "/watch/alias",
    ])
    .expect("parse alias flags");

    assert_eq!(cli.directory.as_deref(), Some("/downloads/alias"));
    assert_eq!(cli.watch_dir.as_deref(), Some("/watch/alias"));
}

#[test]
fn cli_watch_dir_defaults_to_none() {
    let cli = Cli::try_parse_from(["rstorrent"]).expect("parse defaults");
    assert_eq!(cli.watch_dir, None);
}

#[test]
fn cli_watch_dir_requires_a_value() {
    assert!(Cli::try_parse_from(["rstorrent", "--watch-dir"]).is_err());
    assert!(Cli::try_parse_from(["rstorrent", "--watch"]).is_err());
    assert!(Cli::try_parse_from(["rstorrent", "-w"]).is_err());
}

#[test]
fn cli_rejects_unknown_watch_spelling() {
    assert!(Cli::try_parse_from(["rstorrent", "--watchdir", "/tmp"]).is_err());
}

// ---------------------------------------------------------------------------
// Control::load_watch_directory — argument validation
// ---------------------------------------------------------------------------

#[test]
fn missing_watch_directory_is_an_error() {
    let base = TestDir::new("missing");
    let control = Control::new();
    let missing = base.join("does-not-exist");

    let err = control
        .load_watch_directory(&missing.to_string_lossy())
        .expect_err("missing directory must fail");

    assert!(
        err.contains("does not exist"),
        "unexpected error message: {err}"
    );
}

#[test]
fn watch_path_that_is_a_file_is_an_error() {
    let base = TestDir::new("file-path");
    let file = base.join("not-a-dir.torrent");
    fs::write(&file, b"irrelevant").unwrap();

    let control = Control::new();
    let err = control
        .load_watch_directory(&file.to_string_lossy())
        .expect_err("file path must fail");

    assert!(
        err.contains("not a directory"),
        "unexpected error message: {err}"
    );
}

#[test]
fn empty_watch_directory_loads_nothing() {
    let base = TestDir::new("empty");
    let control = Control::new();

    let loaded = control
        .load_watch_directory(&base.as_str())
        .expect("empty dir is valid");

    assert_eq!(loaded, 0);
    assert_eq!(control.download_list.read().unwrap().len(), 0);
    assert_eq!(main_view_len(&control), 0);
}

// ---------------------------------------------------------------------------
// Control::load_watch_directory — file selection
// ---------------------------------------------------------------------------

#[test]
fn non_torrent_files_are_ignored() {
    let base = TestDir::new("non-torrent");
    fs::write(base.join("notes.txt"), b"hello").unwrap();
    fs::write(base.join("README"), b"hello").unwrap();
    fs::write(
        base.join("backup.torrent.bak"),
        make_torrent("bak", b"payload"),
    )
    .unwrap();

    let control = Control::new();
    let loaded = control.load_watch_directory(&base.as_str()).unwrap();

    assert_eq!(loaded, 0);
    assert_eq!(control.download_list.read().unwrap().len(), 0);
}

#[test]
fn nested_directories_are_not_scanned() {
    let base = TestDir::new("nested");
    let nested = base.join("nested");
    fs::create_dir_all(&nested).unwrap();
    write_torrent(&nested, "inside.torrent", "inside", b"nested payload");

    let control = Control::new();
    let loaded = control.load_watch_directory(&base.as_str()).unwrap();

    assert_eq!(loaded, 0);
    assert_eq!(control.download_list.read().unwrap().len(), 0);
}

#[test]
fn directory_named_with_torrent_extension_is_ignored() {
    let base = TestDir::new("dir-torrent");
    fs::create_dir_all(base.join("bundle.torrent")).unwrap();

    let control = Control::new();
    let loaded = control.load_watch_directory(&base.as_str()).unwrap();

    assert_eq!(loaded, 0);
    assert_eq!(control.download_list.read().unwrap().len(), 0);
}

#[test]
fn uppercase_extension_is_loaded() {
    let base = TestDir::new("uppercase");
    write_torrent(&base.path, "UPPER.TORRENT", "upper", b"uppercase payload");

    let control = Control::new();
    let loaded = control.load_watch_directory(&base.as_str()).unwrap();

    assert_eq!(loaded, 1);
    assert_eq!(loaded_names(&control), vec!["upper".to_string()]);
}

// ---------------------------------------------------------------------------
// Control::load_watch_directory — loading behaviour
// ---------------------------------------------------------------------------

#[test]
fn multiple_torrents_are_loaded_in_sorted_order() {
    let base = TestDir::new("multiple");
    write_torrent(&base.path, "03_c.torrent", "c", b"payload c");
    write_torrent(&base.path, "01_a.torrent", "a", b"payload a");
    write_torrent(&base.path, "02_b.torrent", "b", b"payload b");

    let control = Control::new();
    let loaded = control.load_watch_directory(&base.as_str()).unwrap();

    assert_eq!(loaded, 3);
    assert_eq!(
        loaded_names(&control),
        vec!["a".to_string(), "b".to_string(), "c".to_string()]
    );
    assert_eq!(main_view_len(&control), 3);
}

#[test]
fn corrupt_torrent_is_skipped_without_aborting_the_scan() {
    let base = TestDir::new("corrupt");
    fs::write(base.join("00_broken.torrent"), b"this is not bencode").unwrap();
    write_torrent(&base.path, "01_good.torrent", "good", b"good payload");

    let control = Control::new();
    let loaded = control.load_watch_directory(&base.as_str()).unwrap();

    assert_eq!(loaded, 1);
    assert_eq!(loaded_names(&control), vec!["good".to_string()]);
}

#[test]
fn torrent_with_invalid_metadata_is_skipped() {
    let base = TestDir::new("invalid-metadata");
    // Structurally valid bencode that is not a usable torrent:
    // an empty dictionary and one with an empty `info` dictionary.
    fs::write(base.join("00_empty.torrent"), b"de").unwrap();
    fs::write(base.join("01_empty_info.torrent"), b"d4:infodee").unwrap();
    write_torrent(&base.path, "02_good.torrent", "good", b"good payload");

    let control = Control::new();
    let loaded = control.load_watch_directory(&base.as_str()).unwrap();

    assert_eq!(loaded, 1);
    assert_eq!(loaded_names(&control), vec!["good".to_string()]);
}

#[test]
fn duplicate_info_hash_is_loaded_only_once() {
    let base = TestDir::new("duplicate");
    let payload = b"same payload";
    write_torrent(&base.path, "a.torrent", "dupe", payload);
    write_torrent(&base.path, "b.torrent", "dupe", payload);

    let control = Control::new();
    let loaded = control.load_watch_directory(&base.as_str()).unwrap();

    assert_eq!(loaded, 1, "only the first copy is a new load");
    assert_eq!(control.download_list.read().unwrap().len(), 1);
    assert_eq!(main_view_len(&control), 1);
}

#[test]
fn loading_the_same_directory_twice_is_idempotent() {
    let base = TestDir::new("idempotent");
    write_torrent(&base.path, "one.torrent", "one", b"one payload");

    let control = Control::new();
    assert_eq!(control.load_watch_directory(&base.as_str()).unwrap(), 1);
    assert_eq!(control.load_watch_directory(&base.as_str()).unwrap(), 0);
    assert_eq!(control.download_list.read().unwrap().len(), 1);
}

#[test]
fn loaded_torrents_are_started_and_inserted_into_every_view() {
    let base = TestDir::new("started");
    write_torrent(&base.path, "started.torrent", "started", b"payload");

    let control = Control::new();
    control.load_watch_directory(&base.as_str()).unwrap();

    let dl_list = control.download_list.read().unwrap();
    let download = dl_list.all()[0].clone();
    assert!(
        download.read().unwrap().is_active(),
        "watch-directory torrents must be started"
    );

    let vm = control.view_manager.read().unwrap();
    for view in vm.all() {
        assert_eq!(
            view.all_downloads().len(),
            1,
            "view '{}' should contain the watch torrent",
            view.name()
        );
    }
}

// ---------------------------------------------------------------------------
// Control::initialize integration
// ---------------------------------------------------------------------------

#[tokio::test]
async fn initialize_loads_watch_directory_into_session_and_views() {
    let base = TestDir::new("init");
    let watch = base.join("watch");
    let download = base.join("download");
    let session = base.join("session");
    fs::create_dir_all(&watch).unwrap();
    fs::create_dir_all(&download).unwrap();
    fs::create_dir_all(&session).unwrap();

    let payload = b"Hello rstorrent watch directory verification!";
    write_torrent(&watch, "watched.torrent", "watched.bin", payload);

    let cli = initialize_cli(
        Some(&download.to_string_lossy()),
        Some(&watch.to_string_lossy()),
        &session.to_string_lossy(),
    );

    let mut control = Control::new();
    control.initialize(&cli).expect("initialize");

    {
        let dl_list = control.download_list.read().unwrap();
        assert_eq!(dl_list.len(), 1);
        let dl_arc = dl_list.all()[0].clone();
        let dl = dl_arc.read().unwrap();
        assert_eq!(dl.name(), "watched.bin");
        assert_eq!(dl.directory(), download.to_string_lossy().as_ref());
    }
    assert_eq!(main_view_len(&control), 1);

    // Importing must not write the session before the TUI starts: at 10k
    // torrents that was 30k files and ~56 s of startup. The first checkpoint
    // (every 300 s) persists the anchor and resume data off the startup path.
    let entries_before = control
        .session_manager
        .lock()
        .unwrap()
        .get_valid_entries()
        .expect("read session entries");
    assert!(
        entries_before.is_empty(),
        "watch-directory load must not persist synchronously"
    );

    control
        .command_map
        .read()
        .unwrap()
        .call_command(
            "session.checkpoint",
            &lt_common::Object::create_empty(),
            &rt_rpc::Target::None,
            true,
        )
        .expect("checkpoint");

    let entries_after = control
        .session_manager
        .lock()
        .unwrap()
        .get_valid_entries()
        .expect("read session entries");
    assert_eq!(
        entries_after.len(),
        1,
        "the periodic checkpoint must persist imported torrents"
    );

    control.shutdown().await;
}

#[tokio::test]
async fn initialize_uses_directory_default_when_no_download_dir_is_given() {
    let base = TestDir::new("init-default-dir");
    let watch = base.join("watch");
    let session = base.join("session");
    fs::create_dir_all(&watch).unwrap();
    fs::create_dir_all(&session).unwrap();

    write_torrent(&watch, "default-dir.torrent", "default-dir", b"payload");

    let cli = initialize_cli(
        None,
        Some(&watch.to_string_lossy()),
        &session.to_string_lossy(),
    );

    let mut control = Control::new();
    control.initialize(&cli).expect("initialize");

    {
        let dl_list = control.download_list.read().unwrap();
        let dl_arc = dl_list.all()[0].clone();
        let dl = dl_arc.read().unwrap();
        assert_eq!(dl.directory(), "./");
    }

    control.shutdown().await;
}

#[tokio::test]
async fn initialize_is_not_aborted_by_a_missing_watch_directory() {
    let base = TestDir::new("init-missing");
    let download = base.join("download");
    let session = base.join("session");
    fs::create_dir_all(&download).unwrap();
    fs::create_dir_all(&session).unwrap();
    let missing = base.join("no-such-watch");

    let cli = initialize_cli(
        Some(&download.to_string_lossy()),
        Some(&missing.to_string_lossy()),
        &session.to_string_lossy(),
    );

    let mut control = Control::new();
    assert!(
        control.initialize(&cli).is_ok(),
        "a bad watch directory must not abort startup"
    );
    assert_eq!(control.download_list.read().unwrap().len(), 0);

    control.shutdown().await;
}

#[tokio::test]
async fn initialize_does_not_duplicate_a_torrent_also_passed_on_the_cli() {
    let base = TestDir::new("init-duplicate");
    let watch = base.join("watch");
    let download = base.join("download");
    let session = base.join("session");
    fs::create_dir_all(&watch).unwrap();
    fs::create_dir_all(&download).unwrap();
    fs::create_dir_all(&session).unwrap();

    let payload = b"duplicate across cli files and watch directory";
    let explicit = write_torrent(&download, "explicit.torrent", "same-item.bin", payload);
    // Same infohash, different filename in the watch directory.
    write_torrent(&watch, "watched-copy.torrent", "same-item.bin", payload);

    let mut cli = initialize_cli(
        Some(&download.to_string_lossy()),
        Some(&watch.to_string_lossy()),
        &session.to_string_lossy(),
    );
    cli.files = vec![explicit.to_string_lossy().into_owned()];

    let mut control = Control::new();
    control.initialize(&cli).expect("initialize");

    assert_eq!(
        control.download_list.read().unwrap().len(),
        1,
        "the same infohash must not be added twice"
    );
    assert_eq!(main_view_len(&control), 1);

    control.shutdown().await;
}

#[tokio::test]
async fn initialize_with_empty_watch_directory_is_a_no_op() {
    let base = TestDir::new("init-empty");
    let watch = base.join("watch");
    let download = base.join("download");
    let session = base.join("session");
    fs::create_dir_all(&watch).unwrap();
    fs::create_dir_all(&download).unwrap();
    fs::create_dir_all(&session).unwrap();

    let cli = initialize_cli(
        Some(&download.to_string_lossy()),
        Some(&watch.to_string_lossy()),
        &session.to_string_lossy(),
    );

    let mut control = Control::new();
    control.initialize(&cli).expect("initialize");
    assert_eq!(control.download_list.read().unwrap().len(), 0);
    assert_eq!(main_view_len(&control), 0);

    control.shutdown().await;
}
