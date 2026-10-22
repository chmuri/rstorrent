use lt_common::hash_string::HashString;
use lt_crypto::sha1::sha1_hash;
use lt_engine::constructor::{TorrentFileEntry, TorrentMetadata};
use lt_engine::download::Download;
use lt_storage::file::Priority;
use lt_storage::resume::{ResumeData, MTIME_RECHECK};

fn metadata() -> TorrentMetadata {
    let piece_hash = sha1_hash(&[0x33u8; 8]);
    let mut path = lt_common::path::Path::new();
    path.push("payload.bin");

    TorrentMetadata {
        info_hash: HashString::new(sha1_hash(&piece_hash)),
        name: "resume".to_string(),
        piece_length: 8,
        pieces: piece_hash.to_vec(),
        num_pieces: 1,
        total_size: 8,
        is_multi_file: false,
        files: vec![TorrentFileEntry {
            path,
            length: 8,
            offset: 0,
        }],
        announce: None,
        announce_list: Vec::new(),
        is_private: false,
        is_meta_download: false,
    }
}

#[test]
fn apply_resume_restores_bitfield_priorities_and_metadata() {
    let mut download = Download::new(metadata());

    let mut bitfield = lt_common::bitfield::Bitfield::new(1);
    bitfield.set(0);

    let resume = ResumeData {
        bitfield: Some(bitfield),
        file_priorities: vec![Priority::High],
        file_mtimes: vec![MTIME_RECHECK],
        completed_bytes: 8,
    };

    let completed = download.apply_resume(&resume).unwrap();
    assert_eq!(completed, 1);
    assert!(download.is_seeder());
    assert_eq!(download.resume_mtimes(), &[MTIME_RECHECK]);
    assert_eq!(download.resume_completed_bytes(), 8);
    assert_eq!(download.file_list().files()[0].priority(), Priority::High);
}

#[test]
fn apply_resume_rejects_wrong_bitfield_size() {
    let mut download = Download::new(metadata());
    let resume = ResumeData {
        bitfield: Some(lt_common::bitfield::Bitfield::new(99)),
        ..ResumeData::new()
    };

    assert!(download.apply_resume(&resume).is_err());
}

#[test]
fn uncertain_pieces_are_rechecked_instead_of_trusted() {
    let root =
        std::env::temp_dir().join(format!("rtorrent_uncertain_apply_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("payload.bin"), vec![0u8; 16]).unwrap();

    let mut metadata = metadata();
    metadata.piece_length = 8;
    metadata.num_pieces = 2;
    metadata.total_size = 16;
    metadata.pieces = vec![0u8; 40];

    let mut download = Download::new(metadata);
    download.set_save_directory(&root);

    let mut bitfield = lt_common::bitfield::Bitfield::new(2);
    bitfield.set(0);
    bitfield.set(1);

    let resume = ResumeData {
        bitfield: Some(bitfield),
        file_priorities: vec![Priority::Normal],
        // `~3`: the file was being downloaded when the snapshot was written.
        file_mtimes: vec![lt_storage::resume::MTIME_DOWNLOADING],
        completed_bytes: 16,
    };

    let completed = download
        .apply_resume_with_uncertain(&resume, &[1])
        .expect("resume should apply");

    // Only the fsynced piece survives; the uncertain one is queued for a hash.
    assert_eq!(completed, 1);
    assert!(download.chunk_selector().completed().get(0));
    assert!(!download.chunk_selector().completed().get(1));
    assert_eq!(download.pending_hash_pieces(), &[1]);
    assert!(!download.is_hash_checked());

    let _ = std::fs::remove_dir_all(&root);
}
