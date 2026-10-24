//! Ordering and fault isolation of the parallel session restore.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use lt_common::object::bencode;
use lt_common::Object;
use lt_crypto::sha1::sha1_hash;
use rt_core::session::{get_restorable_entries, restore_entries};

static COUNTER: AtomicU64 = AtomicU64::new(1);

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let id = COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!(
            "rtorrent_restore_parallel_{}_{}_{}",
            tag,
            std::process::id(),
            id
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn path(&self) -> &str {
        self.0.to_str().unwrap()
    }

    fn join(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn write_torrent(dir: &Path, index: usize) -> String {
    let mut info = Object::create_map();
    info.insert_key("name", Object::from(format!("parallel-{index:06}")))
        .unwrap();
    info.insert_key("piece length", Object::from(16384i64))
        .unwrap();
    info.insert_key("pieces", Object::create_string(vec![0u8; 20]))
        .unwrap();
    info.insert_key("length", Object::from(16384i64)).unwrap();

    let mut root = Object::create_map();
    root.insert_key("info", info.clone()).unwrap();

    let info_bytes = bencode::encode(&info).unwrap();
    let hex = lt_common::hash_string::HashString::new(sha1_hash(&info_bytes)).to_hex();
    fs::write(
        dir.join(format!("{hex}.torrent")),
        bencode::encode(&root).unwrap(),
    )
    .unwrap();
    hex.to_uppercase()
}

#[test]
fn parallel_restore_preserves_entry_order_and_hashes() {
    let temp = TempDir::new("order");
    let expected: Vec<String> = (0..64).map(|index| write_torrent(&temp.0, index)).collect();

    let entries = get_restorable_entries(temp.path()).unwrap();
    assert_eq!(entries.len(), expected.len());
    // Enumeration is by file name, not creation order; compare against it.
    let expected: Vec<String> = entries
        .iter()
        .map(|entry| entry.base_name[..40].to_uppercase())
        .collect();

    let outcomes = restore_entries(temp.path(), &entries, 4);
    assert_eq!(outcomes.len(), expected.len());

    for (position, (index, result)) in outcomes.iter().enumerate() {
        assert_eq!(*index, position, "results must be indexed by input order");
        let restored = result
            .as_ref()
            .unwrap_or_else(|err| panic!("entry {position}: {err}"));
        assert_eq!(
            restored.download.info_hash_hex().to_uppercase(),
            expected[position]
        );
    }
}

#[test]
fn serial_and_parallel_restore_agree() {
    let temp = TempDir::new("parity");
    for index in 0..16 {
        write_torrent(&temp.0, index);
    }
    let entries = get_restorable_entries(temp.path()).unwrap();

    let serial = restore_entries(temp.path(), &entries, 1);
    let parallel = restore_entries(temp.path(), &entries, 4);

    let serial_hashes: Vec<String> = serial
        .into_iter()
        .map(|(_, result)| result.unwrap().download.info_hash_hex())
        .collect();
    let parallel_hashes: Vec<String> = parallel
        .into_iter()
        .map(|(_, result)| result.unwrap().download.info_hash_hex())
        .collect();
    assert_eq!(serial_hashes, parallel_hashes);
}

#[test]
fn a_corrupt_entry_is_skipped_without_reordering_the_rest() {
    let temp = TempDir::new("corrupt");
    for index in 0..8 {
        write_torrent(&temp.0, index);
    }
    let entries = get_restorable_entries(temp.path()).unwrap();
    let expected: Vec<String> = entries
        .iter()
        .map(|entry| entry.base_name[..40].to_uppercase())
        .collect();

    // Truncate the middle entry; its filename still parses, its contents do not.
    fs::write(temp.join(&entries[3].base_name), b"not a bencoded torrent").unwrap();

    let outcomes = restore_entries(temp.path(), &entries, 4);

    for (position, (index, result)) in outcomes.iter().enumerate() {
        assert_eq!(*index, position);
        if position == 3 {
            assert!(result.is_err(), "corrupt entry must fail on its own");
            continue;
        }
        let restored = result
            .as_ref()
            .unwrap_or_else(|err| panic!("entry {position}: {err}"));
        assert_eq!(
            restored.download.info_hash_hex().to_uppercase(),
            expected[position]
        );
    }
}

#[test]
fn zero_load_threads_is_clamped_to_serial_restore() {
    let temp = TempDir::new("clamp");
    write_torrent(&temp.0, 0);
    write_torrent(&temp.0, 1);
    let entries = get_restorable_entries(temp.path()).unwrap();
    let outcomes = restore_entries(temp.path(), &entries, 0);
    assert_eq!(outcomes.len(), 2);
    assert!(outcomes.iter().all(|(_, result)| result.is_ok()));
}

#[test]
#[ignore = "scale: 14k session entries; run with --ignored"]
fn restores_fourteen_thousand_entries() {
    let temp = TempDir::new("14k");
    for index in 0..14_000 {
        write_torrent(&temp.0, index);
    }
    let entries = get_restorable_entries(temp.path()).unwrap();
    assert_eq!(entries.len(), 14_000);

    let outcomes = restore_entries(temp.path(), &entries, 4);
    assert_eq!(outcomes.len(), 14_000);
    assert!(outcomes
        .iter()
        .enumerate()
        .all(|(position, (index, result))| *index == position && result.is_ok()));
}
