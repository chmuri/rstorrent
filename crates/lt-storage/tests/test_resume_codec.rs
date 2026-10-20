use lt_common::Object;
use lt_storage::file::Priority;
use lt_storage::resume::{
    ResumeData, MTIME_DOWNLOADING, MTIME_RECHECK, MTIME_SKIPPED, MTIME_UNCHECKED,
};

fn string_key(value: &str) -> Object {
    Object::create_string(value.as_bytes())
}

#[test]
fn legacy_full_schema_is_decoded() {
    let mut object = Object::create_map();
    object
        .insert_key("bitfield", Object::create_string(vec![0b1010_0000]))
        .unwrap();

    let mut files = Object::create_list();
    let mut first = Object::create_map();
    first
        .insert_key("mtime", Object::create_value(1_690_000_000))
        .unwrap();
    first
        .insert_key("priority", Object::create_value(2))
        .unwrap();
    first
        .insert_key("completed", Object::create_value(3))
        .unwrap();
    files.insert_back(first).unwrap();

    let mut second = Object::create_map();
    second
        .insert_key("mtime", Object::create_value(MTIME_DOWNLOADING))
        .unwrap();
    second
        .insert_key("priority", Object::create_value(0))
        .unwrap();
    second
        .insert_key("completed", Object::create_value(1))
        .unwrap();
    files.insert_back(second).unwrap();

    let mut third = Object::create_map();
    // Missing optional fields must fall back to safe defaults.
    third
        .insert_key("completed", Object::create_value(0))
        .unwrap();
    files.insert_back(third).unwrap();

    object.insert_key("files", files).unwrap();
    object
        .insert_key("completed", Object::create_value(4096))
        .unwrap();

    // Fields written by rtorrent/libtorrent that ResumeData does not model
    // must be ignored rather than rejected.
    let mut trackers = Object::create_map();
    let mut tracker = Object::create_map();
    tracker
        .insert_key("enabled", Object::create_value(0))
        .unwrap();
    tracker
        .insert_key("extra_tracker", Object::create_value(1))
        .unwrap();
    trackers
        .insert_key("http://tracker/announce", tracker)
        .unwrap();
    object.insert_key("trackers", trackers).unwrap();
    object
        .insert_key("uncertain_pieces", Object::create_string(vec![0u8; 4]))
        .unwrap();
    object.insert_key("peers", Object::create_list()).unwrap();
    object
        .insert_key("future_field", Object::create_value(42))
        .unwrap();

    let resume = ResumeData::from_object_with_piece_count(&object, 8).unwrap();
    assert_eq!(resume.bitfield.unwrap().size_set(), 2);
    assert_eq!(
        resume.file_priorities,
        vec![Priority::High, Priority::Off, Priority::Normal]
    );
    assert_eq!(
        resume.file_mtimes,
        vec![1_690_000_000, MTIME_DOWNLOADING, MTIME_UNCHECKED]
    );
    assert_eq!(resume.completed_bytes, 4096);
}

#[test]
fn legacy_uniform_integer_bitfields_are_decoded() {
    let mut object = Object::create_map();
    object
        .insert_key("bitfield", Object::create_value(10))
        .unwrap();

    let all_set = ResumeData::from_object_with_piece_count(&object, 10).unwrap();
    assert!(all_set.bitfield.unwrap().is_all_set());

    object
        .insert_key("bitfield", Object::create_value(0))
        .unwrap();
    let all_unset = ResumeData::from_object_with_piece_count(&object, 10).unwrap();
    let all_unset = all_unset.bitfield.unwrap();
    assert_eq!(all_unset.size_bits(), 10);
    assert!(all_unset.is_all_unset());

    // A value that is neither zero nor the piece count is invalid progress.
    object
        .insert_key("bitfield", Object::create_value(5))
        .unwrap();
    let invalid = ResumeData::from_object_with_piece_count(&object, 10).unwrap();
    assert!(invalid.bitfield.is_none());
}

#[test]
fn legacy_bitfield_sentinels_are_preserved() {
    let resume = ResumeData {
        bitfield: None,
        file_priorities: vec![Priority::Normal; 4],
        file_mtimes: vec![MTIME_UNCHECKED, MTIME_SKIPPED, MTIME_RECHECK, 1234],
        completed_bytes: 0,
    };

    let restored = ResumeData::from_object(&resume.to_object()).unwrap();
    assert_eq!(
        restored.file_mtimes,
        vec![MTIME_UNCHECKED, MTIME_SKIPPED, MTIME_RECHECK, 1234]
    );
}

#[test]
fn to_object_writes_files_completed_and_global_completed() {
    let resume = ResumeData {
        bitfield: Some(lt_common::bitfield::Bitfield::new(1)),
        file_priorities: vec![Priority::High],
        file_mtimes: vec![MTIME_SKIPPED],
        completed_bytes: 1024,
    };

    let object = resume.to_object();
    assert!(object.has_key("bitfield"));
    assert!(object.has_key("completed"));
    assert_eq!(object.get_key_value("completed").unwrap(), 1024);

    let files = object.get_key_list("files").unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].get_key_value("mtime").unwrap(), MTIME_SKIPPED);
    assert_eq!(files[0].get_key_value("priority").unwrap(), 2);
}

#[test]
fn completed_bytes_are_recomputed_when_inconsistent() {
    let mut bitfield = lt_common::bitfield::Bitfield::new(3);
    bitfield.set(0);
    bitfield.set(1);
    bitfield.set(2);

    let mut resume = ResumeData {
        bitfield: Some(bitfield),
        completed_bytes: 1,
        ..ResumeData::new()
    };

    assert!(resume.normalize_completed(20, 8));
    assert_eq!(resume.completed_bytes, 20);

    // A stale `completed` with no bitfield means no saved progress.
    resume.bitfield = None;
    assert!(resume.normalize_completed(20, 8));
    assert_eq!(resume.completed_bytes, 0);
}

#[test]
fn string_keys_do_not_panic_on_non_utf8_payload() {
    // Unknown non-UTF-8 keys/values are common in tracker metadata; the codec
    // must ignore them.
    let mut object = Object::create_map();
    object.insert_key("bitfield", string_key("")).unwrap();
    object
        .insert_key("unknown", Object::create_string(vec![0xff, 0xfe]))
        .unwrap();

    let resume = ResumeData::from_object_with_piece_count(&object, 0).unwrap();
    assert!(resume.bitfield.is_some());
}

#[test]
fn legacy_extras_round_trip_per_file_completed_and_trackers() {
    use lt_storage::resume::{ResumeExtras, TrackerResumeSettings};

    let mut object = ResumeData {
        bitfield: Some(lt_common::bitfield::Bitfield::new(2)),
        file_priorities: vec![Priority::Normal, Priority::High],
        file_mtimes: vec![MTIME_RECHECK, 1234],
        completed_bytes: 0,
    }
    .to_object();

    let extras = ResumeExtras {
        file_completed_chunks: vec![Some(7), None],
        trackers: vec![
            TrackerResumeSettings {
                url: "http://a/announce".into(),
                enabled: false,
                extra_tracker: false,
                group: 0,
            },
            TrackerResumeSettings {
                url: "http://b/announce".into(),
                enabled: true,
                extra_tracker: true,
                group: 3,
            },
        ],
    };
    extras.write_into(&mut object);

    let restored = ResumeExtras::from_object(&object);
    assert_eq!(restored.file_completed_chunks, vec![Some(7), None]);
    assert_eq!(restored.trackers.len(), 2);
    assert!(restored
        .trackers
        .iter()
        .any(|tracker| tracker.url == "http://a/announce" && !tracker.enabled));
    assert!(restored.trackers.iter().any(|tracker| {
        tracker.url == "http://b/announce"
            && tracker.enabled
            && tracker.extra_tracker
            && tracker.group == 3
    }));

    // The regular ResumeData fields survived the extra merge.
    let resume = ResumeData::from_object_with_piece_count(&object, 2).unwrap();
    assert_eq!(
        resume.file_priorities,
        vec![Priority::Normal, Priority::High]
    );
    assert_eq!(resume.file_mtimes, vec![MTIME_RECHECK, 1234]);
}
