//! Safety tests for `d.delete_data`, source-path tracking on `load.*`, and
//! `d.delete_tied`. Data deletion must never touch the shared download root or
//! sibling files, and must reject unsafe names.

use std::sync::{Arc, RwLock};

use lt_common::hash_string::HashString;
use lt_common::object::bencode;
use lt_common::Object;
use lt_engine::constructor::{TorrentFileEntry, TorrentMetadata};
use rt_core::commands::download::register_download_commands;
use rt_core::download::Download;
use rt_core::download_list::DownloadList;
use rt_core::view_manager::ViewManager;
use rt_rpc::{CommandMap, Target};

static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

struct TempDir(std::path::PathBuf);
impl TempDir {
    fn new() -> Self {
        let id = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let p = std::env::temp_dir().join(format!("rt_delete_data_{}_{}", std::process::id(), id));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        Self(p)
    }
}
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn meta(name: &str, multi: bool, hash_byte: u8) -> TorrentMetadata {
    let info_hash = HashString::new([hash_byte; 20]);
    let mut path = lt_common::path::Path::new();
    path.push("payload.bin");
    TorrentMetadata {
        info_hash,
        name: name.to_string(),
        piece_length: 16384,
        pieces: vec![0u8; 20],
        num_pieces: 1,
        total_size: 16384,
        is_multi_file: multi,
        files: vec![TorrentFileEntry {
            path,
            length: 16384,
            offset: 0,
        }],
        announce: Some("http://tracker/announce".into()),
        announce_list: vec![vec!["http://tracker/announce".into()]],
        is_private: false,
        is_meta_download: false,
        v2: None,
    }
}

fn setup(
    dir: &std::path::Path,
    name: &str,
    multi: bool,
    hash_byte: u8,
) -> (CommandMap, Target, Arc<RwLock<DownloadList>>) {
    let download = Arc::new(RwLock::new(Download::from_metadata(meta(
        name, multi, hash_byte,
    ))));
    download
        .write()
        .unwrap()
        .set_directory(dir.to_str().unwrap().to_string());
    let hash = *download.read().unwrap().info_hash();
    let mut list = DownloadList::new();
    list.insert(Arc::clone(&download));
    let list = Arc::new(RwLock::new(list));
    let mut vm = ViewManager::new();
    let main = vm.insert("main");
    main.insert(Arc::clone(&download));
    let vm = Arc::new(RwLock::new(vm));
    let mut map = CommandMap::new();
    register_download_commands(&mut map, Arc::clone(&list), vm).unwrap();
    (map, Target::Download(hash), list)
}

fn call(
    map: &CommandMap,
    name: &str,
    target: &Target,
    args: Vec<Object>,
) -> Result<Object, rt_rpc::RpcError> {
    let args = Object::create_list_with(args);
    map.call_command(name, &args, target, true)
}

#[test]
fn delete_data_single_file_removes_only_own_file() {
    let tmp = TempDir::new();
    let dir = tmp.0.join("dl");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("movie.iso"), b"data").unwrap();
    std::fs::write(dir.join("sibling.txt"), b"keep").unwrap();

    let (map, target, _list) = setup(&dir, "movie.iso", false, 0x41);
    call(&map, "d.delete_data", &target, vec![]).unwrap();

    assert!(!dir.join("movie.iso").exists());
    assert!(dir.join("sibling.txt").exists(), "sibling must survive");
    assert!(dir.exists(), "download root must survive");
}

#[test]
fn delete_data_multi_file_removes_only_subtree() {
    let tmp = TempDir::new();
    let dir = tmp.0.join("dl");
    std::fs::create_dir_all(dir.join("album").join("disc1")).unwrap();
    std::fs::write(dir.join("album").join("disc1").join("a.bin"), b"a").unwrap();
    std::fs::write(dir.join("other.txt"), b"keep").unwrap();

    let (map, target, _list) = setup(&dir, "album", true, 0x42);
    call(&map, "d.delete_data", &target, vec![]).unwrap();

    assert!(!dir.join("album").exists(), "torrent subtree removed");
    assert!(dir.join("other.txt").exists(), "sibling must survive");
    assert!(dir.exists(), "download root must survive");
}

#[test]
fn delete_data_rejects_unsafe_names() {
    for (name, byte) in [
        ("../evil", 0x43u8),
        ("a/b", 0x44),
        ("..", 0x45),
        (".", 0x46),
    ] {
        let tmp = TempDir::new();
        let dir = tmp.0.join("dl");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("keep.txt"), b"keep").unwrap();
        // A file that a traversal might target outside the root.
        std::fs::write(tmp.0.join("evil"), b"outside").unwrap();

        let (map, target, _list) = setup(&dir, name, false, byte);
        let res = call(&map, "d.delete_data", &target, vec![]);
        assert!(res.is_err(), "name {name:?} must be rejected");
        assert!(dir.join("keep.txt").exists());
        assert!(tmp.0.join("evil").exists(), "outside file must survive");
        assert!(dir.exists());
    }
}

#[test]
fn delete_data_idempotent_when_missing() {
    let tmp = TempDir::new();
    let dir = tmp.0.join("dl");
    std::fs::create_dir_all(&dir).unwrap();
    let (map, target, _list) = setup(&dir, "gone.iso", false, 0x47);
    // No file on disk: still success.
    call(&map, "d.delete_data", &target, vec![]).unwrap();
    call(&map, "d.delete_data", &target, vec![]).unwrap();
}

fn write_torrent_file(path: &std::path::Path, name: &str) {
    let mut info = Object::create_map();
    info.insert_key("length", Object::create_value(100))
        .unwrap();
    info.insert_key("name", Object::create_string(name.to_string()))
        .unwrap();
    info.insert_key("piece length", Object::create_value(16384))
        .unwrap();
    info.insert_key("pieces", Object::create_string(vec![0u8; 20]))
        .unwrap();
    let mut top = Object::create_map();
    top.insert_key("announce", Object::create_string("http://tracker/announce"))
        .unwrap();
    top.insert_key("info", info).unwrap();
    let bytes = bencode::encode(&top).unwrap();
    std::fs::write(path, bytes).unwrap();
}

#[test]
fn load_normal_records_source_and_delete_tied_removes_it() {
    let tmp = TempDir::new();
    let dir = tmp.0.join("dl");
    std::fs::create_dir_all(&dir).unwrap();
    let src = tmp.0.join("source.torrent");
    write_torrent_file(&src, "loaded.iso");

    let list = Arc::new(RwLock::new(DownloadList::new()));
    let vm = Arc::new(RwLock::new(ViewManager::new()));
    let mut map = CommandMap::new();
    register_download_commands(&mut map, Arc::clone(&list), vm).unwrap();

    // load.normal <path>
    call(
        &map,
        "load.normal",
        &Target::None,
        vec![Object::create_string(src.to_str().unwrap().to_string())],
    )
    .unwrap();

    let hash = list.read().unwrap().all()[0]
        .read()
        .unwrap()
        .info_hash_hex();
    let target = Target::Download(HashString::from_hex(&hash).unwrap());
    // Source path recorded.
    let tied = call(&map, "d.tied_to_file", &target, vec![]).unwrap();
    assert_eq!(tied.as_str().unwrap(), src.to_str().unwrap());
    let loaded = call(&map, "d.loaded_file", &target, vec![]).unwrap();
    assert_eq!(loaded.as_str().unwrap(), src.to_str().unwrap());

    // delete_tied removes the source file.
    assert!(src.exists());
    call(&map, "d.delete_tied", &target, vec![]).unwrap();
    assert!(!src.exists(), "tied source file must be deleted");
}
