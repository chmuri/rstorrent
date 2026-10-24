//! Phase 1 — `DownloadList` hash index.
//!
//! `DownloadList` is ordered by insertion. The index must accelerate duplicate
//! rejection and lookup without changing that order, including after erasing
//! from the middle of the list.

use lt_common::hash_string::HashString;
use lt_engine::constructor::TorrentMetadata;
use rt_core::download::Download;
use rt_core::download_list::DownloadList;
use std::sync::{Arc, Mutex, RwLock};

fn hash_for(id: u64) -> HashString {
    let mut bytes = [0u8; 20];
    bytes[..8].copy_from_slice(&id.to_be_bytes());
    HashString::new(bytes)
}

fn id_of(hash: &HashString) -> u64 {
    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(&hash.as_bytes()[..8]);
    u64::from_be_bytes(bytes)
}

fn make_download(id: u64) -> Arc<RwLock<Download>> {
    let metadata = TorrentMetadata {
        info_hash: hash_for(id),
        name: format!("indexed-download-{id}"),
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
    };

    Arc::new(RwLock::new(Download::from_metadata(metadata)))
}

fn insert(list: &mut DownloadList, id: u64) -> bool {
    list.insert(make_download(id))
}

fn order(list: &DownloadList) -> Vec<u64> {
    list.all()
        .iter()
        .map(|download| id_of(download.read().unwrap().info_hash()))
        .collect()
}

fn assert_findable(list: &DownloadList, id: u64, expected: bool) {
    let hash = hash_for(id);
    assert_eq!(
        list.find(&hash).is_some(),
        expected,
        "find(hash {id}) returned the wrong presence"
    );
    assert_eq!(
        list.find_hex(&hash.to_hex()).is_some(),
        expected,
        "find_hex(lower {id}) returned the wrong presence"
    );
    assert_eq!(
        list.find_hex(&hash.to_hex_upper()).is_some(),
        expected,
        "find_hex(upper {id}) returned the wrong presence"
    );
}

#[test]
fn insert_rejects_duplicates_without_changing_order() {
    let mut list = DownloadList::new();
    let events = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&events);
    list.add_event_listener(Arc::new(move |event, download| {
        sink.lock().unwrap().push((
            event.to_string(),
            id_of(download.read().unwrap().info_hash()),
        ));
    }));

    assert!(insert(&mut list, 1));
    assert!(insert(&mut list, 2));
    assert!(insert(&mut list, 3));

    assert!(!insert(&mut list, 2), "duplicate insert must be rejected");
    assert_eq!(list.len(), 3);
    assert_eq!(order(&list), vec![1, 2, 3]);

    let inserted: Vec<u64> = events
        .lock()
        .unwrap()
        .iter()
        .filter(|(event, _)| event == "event.download.inserted")
        .map(|(_, id)| *id)
        .collect();
    assert_eq!(inserted, vec![1, 2, 3], "a rejected insert must not emit");
}

#[test]
fn find_hex_validates_shape_before_lookup() {
    let mut list = DownloadList::new();
    assert!(insert(&mut list, 42));
    assert_findable(&list, 42, true);

    let hex = hash_for(42).to_hex();
    assert!(list.find_hex(&hex[..39]).is_none());
    assert!(list.find_hex(&format!("{hex}0")).is_none());
    assert!(list
        .find_hex("zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz")
        .is_none());
    assert!(list.find_hex("").is_none());
}

#[test]
fn erase_from_the_middle_keeps_remaining_order_and_heals_the_index() {
    let mut list = DownloadList::new();
    for id in [10, 20, 30, 40] {
        assert!(insert(&mut list, id));
    }

    assert!(list.erase(&hash_for(20)).is_some());

    assert_eq!(order(&list), vec![10, 30, 40]);
    assert_findable(&list, 10, true);
    assert_findable(&list, 20, false);
    assert_findable(&list, 30, true);
    assert_findable(&list, 40, true);

    // Every surviving position shifted by one; lookups must still resolve.
    assert!(list.erase(&hash_for(10)).is_some());
    assert_eq!(order(&list), vec![30, 40]);
    assert_findable(&list, 30, true);
    assert_findable(&list, 40, true);
}

#[test]
fn erase_of_the_tail_does_not_clobber_earlier_entries() {
    let mut list = DownloadList::new();
    for id in [1, 2, 3] {
        assert!(insert(&mut list, id));
    }

    assert!(list.erase(&hash_for(3)).is_some());
    assert_findable(&list, 1, true);
    assert_findable(&list, 2, true);

    assert!(insert(&mut list, 4));
    assert_eq!(order(&list), vec![1, 2, 4]);
}

#[test]
fn erase_of_an_unknown_or_already_erased_hash_is_a_no_op() {
    let mut list = DownloadList::new();
    assert!(insert(&mut list, 7));
    assert!(insert(&mut list, 8));

    assert!(list.erase(&hash_for(999)).is_none());
    assert_eq!(order(&list), vec![7, 8]);
    assert_findable(&list, 7, true);
    assert_findable(&list, 8, true);

    assert!(list.erase(&hash_for(7)).is_some());
    assert!(list.erase(&hash_for(7)).is_none());
    assert_eq!(order(&list), vec![8]);
    assert_findable(&list, 8, true);
}

#[test]
fn clear_resets_the_index_so_the_same_download_can_be_added_again() {
    let mut list = DownloadList::new();
    assert!(insert(&mut list, 5));
    assert!(insert(&mut list, 6));

    list.clear();
    assert_eq!(list.len(), 0);
    assert_findable(&list, 5, false);
    assert_findable(&list, 6, false);

    assert!(insert(&mut list, 5));
    assert_eq!(order(&list), vec![5]);
    assert_findable(&list, 5, true);
}

#[test]
fn interleaved_insert_and_erase_matches_a_reference_model() {
    let mut list = DownloadList::new();
    let mut model: Vec<u64> = Vec::new();

    for id in 0..600u64 {
        if id % 5 == 4 && !model.is_empty() {
            let victim = model.remove(model.len() / 2);
            assert!(
                list.erase(&hash_for(victim)).is_some(),
                "erase({victim}) must find a live entry"
            );
            assert!(
                list.erase(&hash_for(victim)).is_none(),
                "erase({victim}) twice must be a no-op"
            );
        } else {
            assert!(list.insert(make_download(id)), "insert({id}) must succeed");
            model.push(id);
        }
    }

    assert_eq!(list.len(), model.len());
    assert_eq!(order(&list), model);

    for &id in &model {
        assert_findable(&list, id, true);
    }
}

#[test]
fn fourteen_thousand_inserts_keep_insertion_order_and_lookup() {
    const COUNT: u64 = 14_000;

    let mut list = DownloadList::new();
    for id in 0..COUNT {
        assert!(list.insert(make_download(id)), "insert({id}) must succeed");
    }

    assert_eq!(list.len(), COUNT as usize);

    let all = order(&list);
    assert_eq!(all.len(), COUNT as usize);
    assert_eq!(all[0], 0);
    assert_eq!(all[COUNT as usize / 2], COUNT / 2);
    assert_eq!(all[COUNT as usize - 1], COUNT - 1);
    assert!(all.windows(2).all(|pair| pair[0] < pair[1]));

    for id in (0..COUNT).step_by(137) {
        assert_findable(&list, id, true);
    }

    assert!(!list.insert(make_download(COUNT / 2)));
    assert_eq!(list.len(), COUNT as usize);

    // Erasing a spread of entries must not corrupt the index for the rest.
    for id in (0..COUNT).step_by(500) {
        assert!(list.erase(&hash_for(id)).is_some());
    }
    for id in (0..COUNT).step_by(500) {
        assert_findable(&list, id, false);
    }
    assert_eq!(list.len(), COUNT as usize - (COUNT as usize).div_ceil(500));

    let survivors = order(&list);
    assert!(survivors.windows(2).all(|pair| pair[0] < pair[1]));
}
