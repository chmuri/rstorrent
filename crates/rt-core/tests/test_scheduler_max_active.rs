//! `scheduler.max_active` caps the number of concurrently active torrents.
//! The cap is enforced by `DownloadList::start`/`try_start`, so every start
//! path (RPC, auto-resume, resume after recheck) respects it. A negative value
//! means unlimited; a capped start leaves the torrent open but stopped.

use std::sync::{Arc, RwLock};

use lt_common::hash_string::HashString;
use lt_crypto::sha1::sha1_hash;
use lt_engine::constructor::TorrentMetadata;
use rt_core::download::Download;
use rt_core::download_list::DownloadList;

type SharedDownload = Arc<RwLock<Download>>;

fn download(name: &str) -> SharedDownload {
    let metadata = TorrentMetadata {
        info_hash: HashString::new(sha1_hash(name.as_bytes())),
        name: name.to_string(),
        piece_length: 16384,
        pieces: vec![0u8; 20],
        num_pieces: 1,
        total_size: 16384,
        is_multi_file: false,
        files: Vec::new(),
        announce: None,
        announce_list: Vec::new(),
        is_private: false,
        is_meta_download: false,
        v2: None,
    };
    Arc::new(RwLock::new(Download::from_metadata(metadata)))
}

fn list_with_three() -> (DownloadList, SharedDownload, SharedDownload, SharedDownload) {
    let mut list = DownloadList::new();
    let a = download("alpha");
    let b = download("beta");
    let c = download("gamma");
    list.insert(Arc::clone(&a));
    list.insert(Arc::clone(&b));
    list.insert(Arc::clone(&c));
    (list, a, b, c)
}

#[test]
fn max_active_caps_concurrently_started_torrents() {
    let (list, a, b, c) = list_with_three();
    list.set_max_active(2);

    list.start(&a);
    list.start(&b);
    assert!(a.read().unwrap().is_active());
    assert!(b.read().unwrap().is_active());

    // The third torrent is queued: opened but not started.
    list.start(&c);
    assert!(!c.read().unwrap().is_active());
    assert!(c.read().unwrap().is_open());

    // Freeing a slot lets the queued torrent start.
    list.stop(&a);
    assert!(!a.read().unwrap().is_active());
    list.start(&c);
    assert!(c.read().unwrap().is_active());
}

#[test]
fn negative_max_active_is_unlimited() {
    let (list, a, b, c) = list_with_three();
    list.set_max_active(-1);

    list.start(&a);
    list.start(&b);
    list.start(&c);
    assert!(a.read().unwrap().is_active());
    assert!(b.read().unwrap().is_active());
    assert!(c.read().unwrap().is_active());
}

#[test]
fn zero_max_active_blocks_every_start() {
    let (list, a, _b, _c) = list_with_three();
    list.set_max_active(0);

    list.start(&a);
    assert!(!a.read().unwrap().is_active());
}

#[test]
fn try_start_reports_the_cap_as_an_error() {
    let (list, a, b, _c) = list_with_three();
    list.set_max_active(1);

    list.start(&a);
    assert!(a.read().unwrap().is_active());

    list.open(&b);
    assert!(list.try_start(&b).is_err());
    assert!(!b.read().unwrap().is_active());
}

#[test]
fn restarting_an_active_torrent_is_not_blocked_by_the_cap() {
    let (list, a, _b, _c) = list_with_three();
    list.set_max_active(1);

    list.start(&a);
    assert!(a.read().unwrap().is_active());
    // Re-starting the already-active torrent must stay a no-op, not a reject.
    list.start(&a);
    assert!(a.read().unwrap().is_active());
    assert!(list.try_start(&a).is_ok());
}
