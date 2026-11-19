use lt_common::hash_string::HashString;
use lt_engine::constructor::{TorrentFileEntry, TorrentMetadata};
use lt_engine::download::Download as EngineDownload;
use rt_core::download::Download;
use rt_core::download_list::DownloadList;
use rt_core::tiering::{
    FileMover, MoveStatus, TierCondition, TierDefinition, TierEvaluator, TierTransitionRule,
    TieringConfig, TieringManager, TorrentEvaluationMetrics, TorrentTierState,
};
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

fn make_lt_path(p: &str) -> lt_common::path::Path {
    let mut path = lt_common::path::Path::new();
    for part in p.trim_start_matches('/').split('/') {
        if !part.is_empty() {
            path.push(part);
        }
    }
    path
}

fn create_test_download(
    name: &str,
    is_multi: bool,
    dir: &str,
    downloaded: u64,
    uploaded: u64,
) -> Download {
    let hash = HashString::from_hex("0123456789abcdef0123456789abcdef01234567").unwrap();

    let files = if is_multi {
        vec![
            TorrentFileEntry {
                path: make_lt_path(&format!("{}/file1.bin", name)),
                length: 500,
                offset: 0,
            },
            TorrentFileEntry {
                path: make_lt_path(&format!("{}/file2.bin", name)),
                length: 500,
                offset: 500,
            },
        ]
    } else {
        vec![TorrentFileEntry {
            path: make_lt_path(name),
            length: 1000,
            offset: 0,
        }]
    };

    let metadata = TorrentMetadata {
        info_hash: hash,
        name: name.to_string(),
        piece_length: 262144,
        pieces: vec![0u8; 20],
        num_pieces: 1,
        total_size: 1000,
        is_multi_file: is_multi,
        files,
        announce: None,
        announce_list: Vec::new(),
        is_private: false,
        is_meta_download: false,
        v2: None,
    };

    let mut dl = Download::new(EngineDownload::new(metadata));
    dl.set_directory(dir);
    dl.set_downloaded_bytes(downloaded);
    dl.set_uploaded_bytes(uploaded);
    dl
}

#[test]
fn test_file_mover_single_file() {
    let temp_root = std::env::temp_dir().join(format!("test_mover_single_{}", std::process::id()));
    let src_dir = temp_root.join("src");
    let dst_dir = temp_root.join("dst");

    fs::create_dir_all(&src_dir).unwrap();
    let src_file = src_dir.join("sample.iso");
    fs::write(&src_file, b"test content inside iso").unwrap();

    let relative_files = vec![PathBuf::from("sample.iso")];
    let moved =
        FileMover::move_torrent_data(&src_dir, &dst_dir, "sample.iso", false, &relative_files)
            .unwrap();

    assert_eq!(moved, 23);
    assert!(!src_file.exists());
    assert!(dst_dir.join("sample.iso").exists());
    assert_eq!(
        fs::read(dst_dir.join("sample.iso")).unwrap(),
        b"test content inside iso"
    );

    let _ = fs::remove_dir_all(temp_root);
}

#[test]
fn test_file_mover_multi_file_directory() {
    let temp_root = std::env::temp_dir().join(format!("test_mover_multi_{}", std::process::id()));
    let src_dir = temp_root.join("tier_nvme");
    let dst_dir = temp_root.join("tier_hdd");

    let torrent_folder = src_dir.join("MyMedia");
    fs::create_dir_all(&torrent_folder).unwrap();
    fs::write(torrent_folder.join("track1.flac"), vec![1u8; 100]).unwrap();
    fs::write(torrent_folder.join("track2.flac"), vec![2u8; 200]).unwrap();

    let relative_files = vec![
        PathBuf::from("MyMedia/track1.flac"),
        PathBuf::from("MyMedia/track2.flac"),
    ];

    let moved =
        FileMover::move_torrent_data(&src_dir, &dst_dir, "MyMedia", true, &relative_files).unwrap();

    assert_eq!(moved, 300);
    assert!(!src_dir.join("MyMedia").exists());
    assert!(dst_dir.join("MyMedia/track1.flac").exists());
    assert!(dst_dir.join("MyMedia/track2.flac").exists());

    let _ = fs::remove_dir_all(temp_root);
}

#[test]
fn test_tier_evaluator_conditions() {
    let config = TieringConfig {
        enabled: true,
        check_interval_secs: 60,
        tiers: vec![
            TierDefinition::new("hot", "/mnt/nvme", 0),
            TierDefinition::new("warm", "/mnt/ssd", 0),
            TierDefinition::new("cold", "/mnt/hdd", 0),
        ],
        rules: vec![
            TierTransitionRule::new(
                0,
                1,
                vec![TierCondition::IsComplete, TierCondition::TimeOnTier(100)],
            ),
            TierTransitionRule::new(1, 2, vec![TierCondition::MinRatio(2.0)]),
        ],
    };

    let mut state = TorrentTierState::new_on_tier(0, 1000);

    // Rule 1: Not complete -> No transition
    let metrics1 = TorrentEvaluationMetrics {
        ratio: 1.5,
        is_done: false,
        uploaded_bytes: 1500,
        is_active: true,
        last_active: 1100,
    };
    let dec = TierEvaluator::evaluate_torrent(&state, &config, 1150, &metrics1);
    assert!(dec.is_none());

    // Rule 1: Complete and time elapsed (1150 - 1000 = 150 >= 100) -> Transition to tier 1
    let metrics2 = TorrentEvaluationMetrics {
        ratio: 1.5,
        is_done: true,
        uploaded_bytes: 1500,
        is_active: true,
        last_active: 1100,
    };
    let dec = TierEvaluator::evaluate_torrent(&state, &config, 1150, &metrics2);
    assert!(dec.is_some());
    let dec = dec.unwrap();
    assert_eq!(dec.from_tier, 0);
    assert_eq!(dec.to_tier, 1);
    assert_eq!(dec.target_path, PathBuf::from("/mnt/ssd"));

    // Advance state to tier 1
    state.record_transition(1, 1150, dec.reason);

    // Rule 2: Ratio 1.8 < 2.0 -> No transition
    let metrics3 = TorrentEvaluationMetrics {
        ratio: 1.8,
        is_done: true,
        uploaded_bytes: 1800,
        is_active: true,
        last_active: 1200,
    };
    let dec = TierEvaluator::evaluate_torrent(&state, &config, 1200, &metrics3);
    assert!(dec.is_none());

    // Rule 2: Ratio 2.2 >= 2.0 -> Transition to tier 2 (cold)
    let metrics4 = TorrentEvaluationMetrics {
        ratio: 2.2,
        is_done: true,
        uploaded_bytes: 2200,
        is_active: true,
        last_active: 1200,
    };
    let dec = TierEvaluator::evaluate_torrent(&state, &config, 1200, &metrics4);
    assert!(dec.is_some());
    let dec = dec.unwrap();
    assert_eq!(dec.from_tier, 1);
    assert_eq!(dec.to_tier, 2);
    assert_eq!(dec.target_path, PathBuf::from("/mnt/hdd"));
}

#[test]
fn test_live_move_preserves_statistics() {
    let temp_root =
        std::env::temp_dir().join(format!("test_live_move_stats_{}", std::process::id()));
    let tier0_dir = temp_root.join("tier0_nvme");
    let tier1_dir = temp_root.join("tier1_ssd");

    fs::create_dir_all(&tier0_dir).unwrap();
    let torrent_file = tier0_dir.join("ubuntu-24.04.iso");
    fs::write(&torrent_file, vec![42u8; 1000]).unwrap();

    let mut dl = create_test_download(
        "ubuntu-24.04.iso",
        false,
        tier0_dir.to_str().unwrap(),
        1000,
        2500, // 2.5 ratio
    );

    assert_eq!(dl.downloaded_bytes(), 1000);
    assert_eq!(dl.uploaded_bytes(), 2500);
    assert_eq!(dl.directory(), tier0_dir.to_str().unwrap());

    // Perform live move
    let moved_bytes = TieringManager::live_move(&mut dl, &tier1_dir).unwrap();
    assert_eq!(moved_bytes, 1000);

    // Verify disk files
    assert!(!torrent_file.exists());
    let target_file = tier1_dir.join("ubuntu-24.04.iso");
    assert!(target_file.exists());
    assert_eq!(fs::read(&target_file).unwrap(), vec![42u8; 1000]);

    // Critical requirement: statistics must remain unchanged!
    assert_eq!(dl.directory(), tier1_dir.to_str().unwrap());
    assert_eq!(dl.downloaded_bytes(), 1000);
    assert_eq!(dl.uploaded_bytes(), 2500);

    match dl.move_status() {
        MoveStatus::Completed {
            from_path, to_path, ..
        } => {
            assert_eq!(from_path, &tier0_dir);
            assert_eq!(to_path, &tier1_dir);
        }
        other => panic!("Expected Completed status, got {:?}", other),
    }

    let _ = fs::remove_dir_all(temp_root);
}

#[test]
fn test_automated_tier_transition_integration() {
    let temp_root = std::env::temp_dir().join(format!("test_auto_tier_{}", std::process::id()));
    let hot_dir = temp_root.join("hot");
    let cold_dir = temp_root.join("cold");

    fs::create_dir_all(&hot_dir).unwrap();
    let sample_file = hot_dir.join("data.bin");
    fs::write(&sample_file, vec![7u8; 500]).unwrap();

    let mut dl_list = DownloadList::new();
    let dl = create_test_download("data.bin", false, hot_dir.to_str().unwrap(), 500, 1500);
    let hash = *dl.info_hash();

    // Assign to tier 0 initially
    {
        let dl_arc = Arc::new(RwLock::new(dl));
        dl_arc.write().unwrap().tier_state_mut().current_tier = Some(0);
        dl_arc.write().unwrap().tier_state_mut().tier_entered_at = 100;
        dl_list.insert(dl_arc);
    }

    let config = TieringConfig {
        enabled: true,
        check_interval_secs: 10,
        tiers: vec![
            TierDefinition::new("hot", &hot_dir, 0),
            TierDefinition::new("cold", &cold_dir, 0),
        ],
        rules: vec![TierTransitionRule::new(
            0,
            1,
            vec![TierCondition::MinRatio(2.0)],
        )],
    };

    let manager = TieringManager::new(config);
    let moved = manager.check_tier_transitions(&dl_list);

    assert_eq!(moved.len(), 1);
    assert_eq!(moved[0], hash.to_hex());

    // Verify the download in the list
    let dl_found = dl_list.find(&hash).unwrap();
    let dl_guard = dl_found.read().unwrap();

    assert_eq!(dl_guard.directory(), cold_dir.to_str().unwrap());
    assert_eq!(dl_guard.tier_state().current_tier, Some(1));
    assert_eq!(dl_guard.tier_state().transition_history.len(), 1);
    assert_eq!(dl_guard.downloaded_bytes(), 500);
    assert_eq!(dl_guard.uploaded_bytes(), 1500);

    // Verify file on filesystem
    assert!(!sample_file.exists());
    assert!(cold_dir.join("data.bin").exists());

    let _ = fs::remove_dir_all(temp_root);
}
