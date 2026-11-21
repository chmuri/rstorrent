//! Seed state per torrent (plan.md §6.2, task T3a): the new `Download`
//! fields (`seeding_time`, `last_transfer_at`, tags, seed overrides, rule
//! ledger), the tick accumulators, `idle_secs`, and their additive
//! `.rtorrent` sidecar persistence — including the timestamp/group/throttle
//! gap fix and a tolerant reader for old files without the new keys.

use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use lt_common::bitfield::Bitfield;
use lt_common::hash_string::HashString;
use lt_common::object::bencode;
use lt_common::path::Path as TorrentPath;
use lt_common::Object;
use lt_crypto::sha1::sha1_hash;
use lt_engine::constructor::{TorrentFileEntry, TorrentMetadata};
use lt_storage::resume::ResumeData;
use rt_core::download::{
    format_limit_value, format_seed_action, parse_limit_value, parse_seed_action, Download,
};
use rt_core::manager::Manager;
use rt_core::session::restore_rtorrent_state;
use rt_rules::{LimitValue, SeedAction, TorrentOverrides};

const PAYLOAD: &[u8] = b"seed-state-payload";

static COUNTER: AtomicU64 = AtomicU64::new(1);

fn metadata() -> TorrentMetadata {
    let mut file_path = TorrentPath::new();
    file_path.push("payload.bin");

    TorrentMetadata {
        info_hash: HashString::new(sha1_hash(b"seed-state-test")),
        name: "seed-state-test".to_string(),
        piece_length: PAYLOAD.len() as u32,
        pieces: sha1_hash(PAYLOAD).to_vec(),
        num_pieces: 1,
        total_size: PAYLOAD.len() as u64,
        is_multi_file: false,
        files: vec![TorrentFileEntry {
            path: file_path,
            length: PAYLOAD.len() as u64,
            offset: 0,
        }],
        announce: None,
        announce_list: Vec::new(),
        is_private: false,
        is_meta_download: false,
        v2: None,
    }
}

fn temp_root() -> std::path::PathBuf {
    let id = COUNTER.fetch_add(1, Ordering::SeqCst);
    let root = std::env::temp_dir().join(format!(
        "rstorrent_seed_state_{}_{}",
        std::process::id(),
        id
    ));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    root
}

fn write_sidecar(base: &Path, download: &Download) {
    let entry = Manager::build_commit_entry(download).expect("entry should serialize");
    fs::write(
        format!("{}.rtorrent", base.display()),
        entry.rtorrent_stream,
    )
    .unwrap();
}

fn write_raw_sidecar(base: &Path, object: &Object) {
    let bytes = bencode::encode(object).unwrap();
    fs::write(format!("{}.rtorrent", base.display()), bytes).unwrap();
}

fn decode_sidecar(base: &Path) -> Object {
    let bytes = fs::read(format!("{}.rtorrent", base.display())).unwrap();
    bencode::decode(&bytes).unwrap()
}

fn tags_of(download: &Download) -> Vec<&str> {
    download.tags().iter().map(String::as_str).collect()
}

/// A seeding download that never transferred anything: a trusted full
/// bitfield promotes the engine straight to `Seeding` with zero rates.
fn seeding_download_without_transfer() -> Download {
    let mut download = Download::from_metadata(metadata());
    let mut bitfield = Bitfield::new(1);
    bitfield.set(0);
    let resume = ResumeData {
        bitfield: Some(bitfield),
        ..ResumeData::new()
    };
    download.apply_resume(&resume).expect("resume accepted");
    assert!(download.is_seeding());
    assert_eq!(download.rate_up(), 0);
    assert_eq!(download.rate_down(), 0);
    download
}

// ---------------------------------------------------------------------------
// Sidecar roundtrip
// ---------------------------------------------------------------------------

#[test]
fn seed_state_roundtrips_through_the_sidecar() {
    let root = temp_root();
    let base = root.join("entry");

    let mut download = Download::from_metadata(metadata());
    download.set_seeding_time(3600);
    download.set_last_transfer_at(1_700_000_000);
    download.set_tags(vec![" keep ", "keep", "movies"]);
    {
        let overrides = download.seed_overrides_mut();
        overrides.ratio = LimitValue::Set(2.5);
        overrides.seeding_time_secs = LimitValue::Unlimited;
        overrides.inactive_secs = LimitValue::Set(1800);
        overrides.action = Some(SeedAction::Throttle("slow".into()));
    }
    download.rule_ledger_mut().0.insert(1, 1_700_000_100);
    download.rule_ledger_mut().0.insert(7, 1_700_000_200);
    // Persistence gap fix keys.
    download.set_timestamp_started(1_699_999_000);
    download.set_timestamp_finished(1_699_999_500);
    download.set_load_date(1_699_998_000);
    download.set_group(4);
    download.set_throttle_name("seed");

    write_sidecar(&base, &download);

    // Wire format spot-checks (plan §6.2 string forms).
    let object = decode_sidecar(&base);
    assert_eq!(object.get_key_value("seeding_time").unwrap(), 3600);
    assert_eq!(
        object.get_key_value("last_transfer_at").unwrap(),
        1_700_000_000
    );
    assert_eq!(
        object.get_key_string("seed_ratio").unwrap(),
        "2.5".as_bytes()
    );
    assert_eq!(
        object.get_key_string("seed_time").unwrap(),
        "unlimited".as_bytes()
    );
    assert_eq!(
        object.get_key_string("seed_idle").unwrap(),
        "1800".as_bytes()
    );
    assert_eq!(
        object.get_key_string("seed_action").unwrap(),
        "throttle:slow".as_bytes()
    );
    assert_eq!(
        object.get_key_value("timestamp_started").unwrap(),
        1_699_999_000
    );
    assert_eq!(
        object.get_key_value("timestamp_finished").unwrap(),
        1_699_999_500
    );
    assert_eq!(object.get_key_value("load_date").unwrap(), 1_699_998_000);
    assert_eq!(object.get_key_value("group").unwrap(), 4);
    assert_eq!(
        object.get_key_string("throttle_name").unwrap(),
        "seed".as_bytes()
    );
    let tags = object.get_key_list("tags").unwrap();
    assert_eq!(tags.len(), 2);
    assert_eq!(tags[0].as_str().unwrap(), "keep");
    assert_eq!(tags[1].as_str().unwrap(), "movies");
    let ledger = object.get_key_map("rule_ledger").unwrap();
    assert_eq!(ledger.len(), 2);
    assert_eq!(
        ledger.get(b"1".as_slice()).unwrap().as_value().unwrap(),
        1_700_000_100
    );
    assert_eq!(
        ledger.get(b"7".as_slice()).unwrap().as_value().unwrap(),
        1_700_000_200
    );

    let mut restored = Download::from_metadata(metadata());
    let state = restore_rtorrent_state(base.to_str().unwrap(), &mut restored);
    assert!(!state.was_active);

    assert_eq!(restored.seeding_time(), 3600);
    assert_eq!(restored.last_transfer_at(), 1_700_000_000);
    assert_eq!(tags_of(&restored), vec!["keep", "movies"]);
    assert_eq!(restored.seed_overrides().ratio, LimitValue::Set(2.5));
    assert_eq!(
        restored.seed_overrides().seeding_time_secs,
        LimitValue::Unlimited
    );
    assert_eq!(
        restored.seed_overrides().inactive_secs,
        LimitValue::Set(1800)
    );
    assert_eq!(
        restored.seed_overrides().action,
        Some(SeedAction::Throttle("slow".into()))
    );
    assert_eq!(restored.rule_ledger().0.get(&1), Some(&1_700_000_100));
    assert_eq!(restored.rule_ledger().0.get(&7), Some(&1_700_000_200));
    assert_eq!(restored.timestamp_started(), 1_699_999_000);
    assert_eq!(restored.timestamp_finished(), 1_699_999_500);
    assert_eq!(restored.load_date(), 1_699_998_000);
    assert_eq!(restored.group(), 4);
    assert_eq!(restored.throttle_name(), "seed");

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn default_sidecar_omits_new_keys() {
    let root = temp_root();
    let base = root.join("entry");

    let download = Download::from_metadata(metadata());
    write_sidecar(&base, &download);

    let object = decode_sidecar(&base);
    let map = object.as_map().unwrap();
    for key in [
        "timestamp_started",
        "timestamp_finished",
        "group",
        "throttle_name",
        "seeding_time",
        "last_transfer_at",
        "seed_ratio",
        "seed_time",
        "seed_idle",
        "seed_action",
        "tags",
        "rule_ledger",
    ] {
        assert!(
            map.get(key.as_bytes()).is_none(),
            "default key '{key}' should not be persisted"
        );
    }
    // The engine stamps load_date at construction, so it is always present.
    assert!(object.get_key_value("load_date").is_ok());

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn old_sidecar_without_new_keys_restores_defaults() {
    let root = temp_root();
    let base = root.join("entry");

    // Hand-built legacy fixture: only keys written before T3.
    let mut object = Object::create_map();
    object.insert_key("priority", Object::from(3i64)).unwrap();
    object
        .insert_key("directory", Object::from("/tmp/legacy-seed"))
        .unwrap();
    object.insert_key("state", Object::from(1i64)).unwrap();
    object
        .insert_key("state_changed", Object::from(111i64))
        .unwrap();
    object
        .insert_key("state_counter", Object::from(2i64))
        .unwrap();
    object
        .insert_key("total_uploaded", Object::from(500i64))
        .unwrap();
    object
        .insert_key("total_downloaded", Object::from(1000i64))
        .unwrap();
    object
        .insert_key("custom1", Object::from("movies"))
        .unwrap();
    write_raw_sidecar(&base, &object);

    let mut restored = Download::from_metadata(metadata());
    let load_date_before = restored.load_date();
    let state = restore_rtorrent_state(base.to_str().unwrap(), &mut restored);

    // Legacy keys still restore exactly as before.
    assert!(state.was_active);
    assert_eq!(restored.priority(), 3);
    assert_eq!(restored.custom_field(1), "movies");
    assert_eq!(restored.uploaded_bytes(), 500);
    assert_eq!(restored.downloaded_bytes(), 1000);
    assert_eq!(restored.directory(), "/tmp/legacy-seed");
    assert_eq!(restored.state_changed(), 111);
    assert_eq!(restored.state_counter(), 2);

    // New keys fall back to defaults without touching unrelated state.
    assert_eq!(restored.seeding_time(), 0);
    assert_eq!(restored.last_transfer_at(), 0);
    assert!(restored.tags().is_empty());
    assert_eq!(restored.seed_overrides(), &TorrentOverrides::default());
    assert!(restored.rule_ledger().0.is_empty());
    assert_eq!(restored.timestamp_started(), 0);
    assert_eq!(restored.timestamp_finished(), 0);
    assert_eq!(restored.load_date(), load_date_before);
    assert_eq!(restored.group(), 0);
    assert_eq!(restored.throttle_name(), "");

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn malformed_seed_values_restore_defaults() {
    let root = temp_root();
    let base = root.join("entry");

    let mut object = Object::create_map();
    object.insert_key("priority", Object::from(3i64)).unwrap();
    object
        .insert_key("seeding_time", Object::from("abc"))
        .unwrap();
    object
        .insert_key("last_transfer_at", Object::from(-5i64))
        .unwrap();
    object
        .insert_key("seed_ratio", Object::from("bogus"))
        .unwrap();
    object.insert_key("seed_time", Object::from("1.5")).unwrap();
    object.insert_key("seed_idle", Object::from("-1")).unwrap();
    object
        .insert_key("seed_action", Object::from("explode"))
        .unwrap();
    object
        .insert_key("tags", Object::from("not-a-list"))
        .unwrap();
    object
        .insert_key("rule_ledger", Object::from("nope"))
        .unwrap();
    object.insert_key("group", Object::from(-1i64)).unwrap();
    object
        .insert_key("throttle_name", Object::from(42i64))
        .unwrap();
    object
        .insert_key("timestamp_started", Object::from("x"))
        .unwrap();
    object
        .insert_key("timestamp_finished", Object::from(-1i64))
        .unwrap();
    object.insert_key("load_date", Object::from("y")).unwrap();
    write_raw_sidecar(&base, &object);

    let mut restored = Download::from_metadata(metadata());
    let load_date_before = restored.load_date();
    restore_rtorrent_state(base.to_str().unwrap(), &mut restored);

    // A malformed value never aborts the restore: legacy keys still apply.
    assert_eq!(restored.priority(), 3);
    assert_eq!(restored.seeding_time(), 0);
    assert_eq!(restored.last_transfer_at(), 0);
    assert!(restored.tags().is_empty());
    assert_eq!(restored.seed_overrides(), &TorrentOverrides::default());
    assert!(restored.rule_ledger().0.is_empty());
    assert_eq!(restored.timestamp_started(), 0);
    assert_eq!(restored.timestamp_finished(), 0);
    assert_eq!(restored.load_date(), load_date_before);
    assert_eq!(restored.group(), 0);
    assert_eq!(restored.throttle_name(), "");

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn tags_and_rule_ledger_skip_invalid_entries() {
    let root = temp_root();
    let base = root.join("entry");

    let mut object = Object::create_map();
    object
        .insert_key(
            "tags",
            Object::create_list_with(vec![
                Object::from("ok"),
                Object::from(42i64),
                Object::from(" ok "),
                Object::from("fine"),
            ]),
        )
        .unwrap();
    let mut ledger = Object::create_map();
    ledger
        .insert_key("3", Object::from(1_700_000_000i64))
        .unwrap();
    ledger.insert_key("bad", Object::from(5i64)).unwrap();
    ledger.insert_key("4", Object::from(-1i64)).unwrap();
    ledger.insert_key("5", Object::from("x")).unwrap();
    ledger.insert_key("6", Object::from(123i64)).unwrap();
    object.insert_key("rule_ledger", ledger).unwrap();
    write_raw_sidecar(&base, &object);

    let mut restored = Download::from_metadata(metadata());
    restore_rtorrent_state(base.to_str().unwrap(), &mut restored);

    // Non-string tags are dropped; the rest is normalised (trim + dedup).
    assert_eq!(tags_of(&restored), vec!["ok", "fine"]);
    // Only entries with a numeric rule id and a non-negative timestamp survive.
    assert_eq!(restored.rule_ledger().0.len(), 2);
    assert_eq!(restored.rule_ledger().0.get(&3), Some(&1_700_000_000));
    assert_eq!(restored.rule_ledger().0.get(&6), Some(&123));

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn seed_action_strings_roundtrip_all_variants() {
    let actions = [
        SeedAction::Stop,
        SeedAction::Close,
        SeedAction::Remove,
        SeedAction::RemoveWithData,
        SeedAction::Throttle("slow".into()),
        SeedAction::SuperSeed,
        SeedAction::Notify,
        SeedAction::Command("d.stop= ;d.ignore_commands.set=1".into()),
    ];

    for action in actions {
        let root = temp_root();
        let base = root.join("entry");

        let mut download = Download::from_metadata(metadata());
        download.seed_overrides_mut().action = Some(action.clone());
        write_sidecar(&base, &download);

        let mut restored = Download::from_metadata(metadata());
        restore_rtorrent_state(base.to_str().unwrap(), &mut restored);
        assert_eq!(restored.seed_overrides().action, Some(action));

        let _ = fs::remove_dir_all(&root);
    }
}

#[test]
fn limit_and_action_string_helpers_are_symmetric() {
    assert_eq!(format_limit_value(&LimitValue::<f64>::Inherit), None);
    assert_eq!(
        format_limit_value(&LimitValue::<f64>::Unlimited),
        Some("unlimited".to_string())
    );
    assert_eq!(
        format_limit_value(&LimitValue::Set(2.5f64)),
        Some("2.5".to_string())
    );
    assert_eq!(
        format_limit_value(&LimitValue::Set(3600u64)),
        Some("3600".to_string())
    );

    assert_eq!(
        parse_limit_value::<f64>("inherit"),
        Some(LimitValue::Inherit)
    );
    assert_eq!(
        parse_limit_value::<f64>(" unlimited "),
        Some(LimitValue::Unlimited)
    );
    assert_eq!(parse_limit_value::<f64>("2.5"), Some(LimitValue::Set(2.5)));
    assert_eq!(parse_limit_value::<f64>("abc"), None);
    assert_eq!(parse_limit_value::<u64>("1.5"), None);
    assert_eq!(parse_limit_value::<u64>("-1"), None);
    assert_eq!(
        parse_limit_value::<u64>("3600"),
        Some(LimitValue::Set(3600))
    );

    for action in [
        SeedAction::Stop,
        SeedAction::Close,
        SeedAction::Remove,
        SeedAction::RemoveWithData,
        SeedAction::Throttle("slow".into()),
        SeedAction::SuperSeed,
        SeedAction::Notify,
        SeedAction::Command("d.stop=".into()),
    ] {
        assert_eq!(
            parse_seed_action(&format_seed_action(&action)),
            Some(action)
        );
    }
    assert_eq!(parse_seed_action("bogus"), None);
    assert_eq!(parse_seed_action("throttle:"), None);
    assert_eq!(
        parse_seed_action("command:"),
        Some(SeedAction::Command(String::new()))
    );
}

// ---------------------------------------------------------------------------
// Tags
// ---------------------------------------------------------------------------

#[test]
fn tags_are_trimmed_deduplicated_and_case_sensitive() {
    let mut download = Download::from_metadata(metadata());
    assert!(download.tags().is_empty());

    download.set_tags(vec![" foo ", "foo", "Bar", "", "   ", "bar", "baz"]);
    assert_eq!(tags_of(&download), vec!["foo", "Bar", "bar", "baz"]);

    assert!(download.add_tag(" qux "));
    assert!(!download.add_tag("foo"));
    assert!(!download.add_tag("   "));
    assert_eq!(tags_of(&download), vec!["foo", "Bar", "bar", "baz", "qux"]);

    assert!(download.has_tag("foo"));
    assert!(download.has_tag(" foo "));
    assert!(!download.has_tag("FOO"));
    assert!(!download.has_tag("missing"));

    assert!(download.remove_tag("Bar"));
    assert!(!download.remove_tag("Bar"));
    assert!(!download.remove_tag("missing"));
    assert_eq!(tags_of(&download), vec!["foo", "bar", "baz", "qux"]);

    download.set_tags(Vec::<String>::new());
    assert!(download.tags().is_empty());
}

// ---------------------------------------------------------------------------
// Accumulators and idle time
// ---------------------------------------------------------------------------

#[test]
fn accumulators_advance_only_while_seeding_and_transferring() {
    let mut download = Download::from_metadata(metadata());

    // Stopped: nothing accumulates.
    download.tick_seed_accumulators(1000, 10);
    assert_eq!(download.seeding_time(), 0);
    assert_eq!(download.last_transfer_at(), 0);

    // Started but incomplete and idle: still nothing.
    download.open().unwrap();
    download.try_start().unwrap();
    assert!(download.is_active());
    assert!(!download.is_seeding());
    download.tick_seed_accumulators(1010, 10);
    assert_eq!(download.seeding_time(), 0);
    assert_eq!(download.last_transfer_at(), 0);

    // Completing the piece makes it seeding with a live download rate.
    assert!(download
        .download_mut()
        .receive_block(0, 0, PAYLOAD)
        .unwrap());
    assert!(download.is_seeding());
    assert!(download.rate_down() > 0);
    download.tick_seed_accumulators(1020, 10);
    assert_eq!(download.seeding_time(), 10);
    assert_eq!(download.last_transfer_at(), 1020);
    download.tick_seed_accumulators(1030, 5);
    assert_eq!(download.seeding_time(), 15);
    assert_eq!(download.last_transfer_at(), 1030);

    // Stopped again: seeding time freezes.
    download.stop();
    assert!(download.is_stopped());
    download.tick_seed_accumulators(1040, 10);
    assert_eq!(download.seeding_time(), 15);

    // Restarted seeding continues accumulating.
    download.start();
    assert!(download.is_seeding());
    download.tick_seed_accumulators(1050, 3);
    assert_eq!(download.seeding_time(), 18);
}

#[test]
fn idle_seeding_accumulates_time_but_not_transfer_timestamp() {
    let mut download = seeding_download_without_transfer();

    download.tick_seed_accumulators(2000, 7);
    download.tick_seed_accumulators(2007, 7);
    assert_eq!(download.seeding_time(), 14);
    // No rate was ever observed, so the transfer timestamp stays unset.
    assert_eq!(download.last_transfer_at(), 0);
}

#[test]
fn idle_secs_uses_transfer_finish_and_last_active_bases() {
    let mut download = seeding_download_without_transfer();

    // Seeding with a recorded transfer: idle counts from the transfer.
    download.set_last_transfer_at(900);
    assert_eq!(download.idle_secs(1000), 100);
    assert_eq!(download.idle_secs(500), 0);

    // Seeding without any transfer: fall back to the finish timestamp.
    download.set_last_transfer_at(0);
    download.set_timestamp_finished(800);
    assert_eq!(download.idle_secs(1000), 200);
    assert_eq!(download.idle_secs(700), 0);

    // Stopped: idle counts from last_active (tiering InactiveSince basis).
    download.stop();
    assert!(download.is_stopped());
    download.set_last_active(700);
    assert_eq!(download.idle_secs(1000), 300);
    assert_eq!(download.idle_secs(600), 0);
    download.set_last_active(0);
    assert_eq!(download.idle_secs(1000), 1000);
}
