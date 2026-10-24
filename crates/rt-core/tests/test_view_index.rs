//! Phase 2 — `View` hash index, partition stability and focus clamping.
//!
//! A view keeps a visible prefix (`0..size_visible`) followed by filtered
//! downloads. Both partitions preserve relative order. The index must stay
//! correct across insert, erase, `set_visible`/`set_not_visible`, `filter`
//! and `sort`, and focus must always point at a real visible slot.

use lt_common::hash_string::HashString;
use lt_common::Object;
use lt_engine::constructor::TorrentMetadata;
use rt_core::download::Download;
use rt_core::view::View;
use rt_rpc::command_flags::FLAG_PUBLIC_RPC;
use rt_rpc::{CommandMap, Target};
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
        name: format!("download-{id:06}"),
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

fn ids(downloads: &[Arc<RwLock<Download>>]) -> Vec<u64> {
    downloads
        .iter()
        .map(|download| id_of(download.read().unwrap().info_hash()))
        .collect()
}

fn inserted_view(name: &str, count: u64) -> View {
    let mut view = View::with_name(name);
    for id in 0..count {
        view.insert(make_download(id));
    }
    view
}

fn focus_id(view: &View) -> Option<u64> {
    view.focus()
        .map(|download| id_of(download.read().unwrap().info_hash()))
}

fn install_id_filter(map: &mut CommandMap, matching: &Arc<Mutex<Vec<HashString>>>) {
    let matching = Arc::clone(matching);
    map.insert_func(
        "test.view.index_match",
        move |target, _obj| {
            let matched = match target {
                Target::Download(hash) => matching.lock().unwrap().contains(hash),
                _ => false,
            };
            Ok(Object::from(if matched { 1i64 } else { 0i64 }))
        },
        FLAG_PUBLIC_RPC,
        "",
        "",
    )
    .unwrap();
}

#[test]
fn insert_preserves_order_and_rejects_duplicates() {
    let mut view = View::with_name("index_order");

    view.insert(make_download(2));
    view.insert(make_download(0));
    view.insert(make_download(1));
    view.insert(make_download(0)); // duplicate

    assert_eq!(view.total_size(), 3);
    assert_eq!(ids(view.all_downloads()), vec![2, 0, 1]);
    assert_eq!(view.size_visible(), 0);
    assert_eq!(ids(view.filtered_downloads()), vec![2, 0, 1]);

    for id in [2, 0, 1] {
        view.set_visible(&hash_for(id), None);
    }

    assert_eq!(view.size_visible(), 3);
    assert_eq!(ids(view.visible_downloads()), vec![2, 0, 1]);
    assert!(view.filtered_downloads().is_empty());
}

#[test]
fn erase_preserves_order_in_both_partitions() {
    let mut view = inserted_view("index_erase", 4);
    for id in [0, 1, 2] {
        view.set_visible(&hash_for(id), None);
    }
    // id 3 stays filtered.

    view.erase(&hash_for(1), None);
    assert_eq!(view.total_size(), 3);
    assert_eq!(ids(view.visible_downloads()), vec![0, 2]);
    assert_eq!(ids(view.filtered_downloads()), vec![3]);

    view.erase(&hash_for(3), None);
    assert_eq!(view.total_size(), 2);
    assert_eq!(ids(view.visible_downloads()), vec![0, 2]);
    assert!(view.filtered_downloads().is_empty());

    view.erase(&hash_for(999), None);
    assert_eq!(view.total_size(), 2);
}

#[test]
fn erase_clamps_focus_to_a_live_visible_slot() {
    let mut view = inserted_view("index_focus_erase", 3);
    for id in [0, 1, 2] {
        view.set_visible(&hash_for(id), None);
    }

    view.set_focus_index(2);
    assert_eq!(focus_id(&view), Some(2));

    view.erase(&hash_for(2), None);
    assert_eq!(view.focus_index(), 1);
    assert_eq!(focus_id(&view), Some(1));

    view.erase(&hash_for(1), None);
    assert_eq!(view.focus_index(), 0);
    assert_eq!(focus_id(&view), Some(0));

    view.erase(&hash_for(0), None);
    assert_eq!(view.focus_index(), 0);
    assert!(view.focus().is_none());
}

#[test]
fn focus_never_points_past_the_last_visible_slot() {
    let mut view = View::with_name("index_focus_clamp");

    // Empty view.
    view.set_focus_index(7);
    view.next_focus(1);
    view.prev_focus(1);
    assert_eq!(view.focus_index(), 0);
    assert!(view.focus().is_none());

    for id in [0, 1, 2] {
        view.insert(make_download(id));
        view.set_visible(&hash_for(id), None);
    }

    view.set_focus_index(usize::MAX);
    assert_eq!(view.focus_index(), 2);
    assert_eq!(focus_id(&view), Some(2));

    view.next_focus(usize::MAX);
    assert_eq!(view.focus_index(), 2, "next must stop at the last slot");

    view.prev_focus(1);
    assert_eq!(view.focus_index(), 1);

    view.next_focus(0);
    assert_eq!(view.focus_index(), 1);

    view.set_focus_index(0);
    view.prev_focus(1);
    assert_eq!(view.focus_index(), 2, "prev at zero wraps to the last slot");
}

#[test]
fn set_visible_appends_and_set_not_visible_moves_to_the_end() {
    let mut view = inserted_view("index_partitions", 4);

    view.set_visible(&hash_for(0), None);
    view.set_visible(&hash_for(2), None);
    assert_eq!(ids(view.visible_downloads()), vec![0, 2]);
    assert_eq!(ids(view.filtered_downloads()), vec![1, 3]);

    // Already visible: a repeated call must not duplicate or reorder.
    view.set_visible(&hash_for(0), None);
    assert_eq!(view.total_size(), 4);
    assert_eq!(ids(view.visible_downloads()), vec![0, 2]);

    // Promoting a filtered download appends it to the visible partition.
    view.set_visible(&hash_for(1), None);
    assert_eq!(ids(view.visible_downloads()), vec![0, 2, 1]);
    assert_eq!(ids(view.filtered_downloads()), vec![3]);

    // Demoting appends to the filtered partition.
    view.set_not_visible(&hash_for(2), None);
    assert_eq!(ids(view.visible_downloads()), vec![0, 1]);
    assert_eq!(ids(view.filtered_downloads()), vec![3, 2]);

    // Already filtered: no-op.
    view.set_not_visible(&hash_for(2), None);
    assert_eq!(view.total_size(), 4);
    assert_eq!(ids(view.filtered_downloads()), vec![3, 2]);

    // Unknown hash: no-op.
    view.set_visible(&hash_for(999), None);
    view.set_not_visible(&hash_for(999), None);
    assert_eq!(view.total_size(), 4);
}

#[test]
fn filter_is_a_stable_partition_and_clamps_focus() {
    let mut view = inserted_view("index_filter", 6);
    for id in [0, 1, 2, 3] {
        view.set_visible(&hash_for(id), None);
    }

    view.set_focus_index(3);
    assert_eq!(focus_id(&view), Some(3));

    let matching = Arc::new(Mutex::new(vec![hash_for(1), hash_for(4)]));
    let mut map = CommandMap::new();
    install_id_filter(&mut map, &matching);
    view.set_filter(Object::from("test.view.index_match"));

    view.filter(Some(&map));

    // matching visible (1), matching filtered (4), then non-matching in their
    // original order.
    assert_eq!(view.size_visible(), 2);
    assert_eq!(ids(view.visible_downloads()), vec![1, 4]);
    assert_eq!(ids(view.filtered_downloads()), vec![0, 2, 3, 5]);

    assert_eq!(
        view.focus_index(),
        1,
        "focus must clamp to the new last slot"
    );
    assert_eq!(focus_id(&view), Some(4));

    // Re-applying the same filter is idempotent.
    view.filter(Some(&map));
    assert_eq!(ids(view.visible_downloads()), vec![1, 4]);
    assert_eq!(ids(view.filtered_downloads()), vec![0, 2, 3, 5]);
}

#[test]
fn sort_keeps_the_index_consistent_for_duplicate_and_erase() {
    let mut view = View::with_name("index_sort");
    for id in [2, 0, 1] {
        view.insert(make_download(id));
        view.set_visible(&hash_for(id), None);
    }

    view.sort(None);
    assert_eq!(ids(view.visible_downloads()), vec![0, 1, 2]);

    // Duplicate rejection after sorting must still use the repaired index.
    view.insert(make_download(1));
    assert_eq!(view.total_size(), 3);

    // Erasing a middle entry after sorting must resolve the shifted position.
    view.erase(&hash_for(1), None);
    assert_eq!(view.total_size(), 2);
    assert_eq!(ids(view.visible_downloads()), vec![0, 2]);
}

#[test]
fn fourteen_thousand_inserts_keep_order_and_reject_duplicates() {
    const COUNT: u64 = 14_000;

    let view = inserted_view("index_scale", COUNT);

    assert_eq!(view.total_size(), COUNT as usize);
    let all = ids(view.all_downloads());
    assert_eq!(all.len(), COUNT as usize);
    assert_eq!(all[0], 0);
    assert_eq!(all[COUNT as usize / 2], COUNT / 2);
    assert_eq!(all[COUNT as usize - 1], COUNT - 1);
    assert!(all.windows(2).all(|pair| pair[0] < pair[1]));

    let mut view = view;
    view.insert(make_download(COUNT / 2));
    assert_eq!(
        view.total_size(),
        COUNT as usize,
        "duplicate rejection must not depend on insertion order"
    );
}

#[test]
fn filtered_to_visible_round_trip_keeps_every_entry_exactly_once() {
    let mut view = inserted_view("index_round_trip", 200);

    for id in (0..200).step_by(2) {
        view.set_visible(&hash_for(id), None);
    }
    for id in (0..200).step_by(2) {
        view.set_not_visible(&hash_for(id), None);
    }
    for id in (0..200).step_by(2) {
        view.set_visible(&hash_for(id), None);
    }

    assert_eq!(view.total_size(), 200);
    assert_eq!(view.size_visible(), 100);

    let mut seen: Vec<u64> = ids(view.all_downloads());
    seen.sort_unstable();
    assert_eq!(seen, (0..200).collect::<Vec<u64>>());
}
