//! Typed-error boundary of the session persistence layer (`SessionError`).
//!
//! The storer returns domain errors; the orchestration layer keeps `io::Result`.
//! These tests pin the `ErrorKind` parity that existing callers rely on and the
//! pass-through semantics of the `Io` variant.

use std::fs;
use std::io::{self, ErrorKind};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use lt_common::object::BencodeError;
use lt_storage::StorageError;
use rt_core::session::{load_resume_file, DownloadStorer, SessionError};

static COUNTER: AtomicU64 = AtomicU64::new(1);

struct TempDirFixture {
    dir: PathBuf,
}

impl TempDirFixture {
    fn new() -> Self {
        let id = COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!(
            "rtorrent_test_session_error_{}_{}",
            std::process::id(),
            id
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Self { dir }
    }

    fn path(&self) -> &str {
        self.dir.to_str().unwrap()
    }
}

impl Drop for TempDirFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

#[test]
fn kind_maps_every_variant_to_the_legacy_error_kind() {
    let cases: Vec<(SessionError, ErrorKind)> = vec![
        (
            SessionError::Io(io::Error::new(ErrorKind::PermissionDenied, "denied")),
            ErrorKind::PermissionDenied,
        ),
        (
            SessionError::Bencode(BencodeError::InvalidData("garbage".into())),
            ErrorKind::InvalidData,
        ),
        (
            SessionError::Resume(StorageError::FileNotOpen),
            ErrorKind::InvalidData,
        ),
        (
            SessionError::InvalidPath("empty".into()),
            ErrorKind::InvalidInput,
        ),
        (
            SessionError::InvalidInfoHash("short".into()),
            ErrorKind::InvalidInput,
        ),
        (
            SessionError::DestinationIsDirectory("/tmp/x".into()),
            ErrorKind::Other,
        ),
    ];
    for (err, expected) in cases {
        assert_eq!(err.kind(), expected, "variant: {err:?}");
    }
}

#[test]
fn conversion_into_io_error_preserves_kind_and_message() {
    let err = SessionError::DestinationIsDirectory("/tmp/dest".into());
    let display = err.to_string();
    let converted: io::Error = err.into();
    assert_eq!(converted.kind(), ErrorKind::Other);
    assert_eq!(converted.to_string(), display);
    assert!(converted.to_string().contains("/tmp/dest"));

    let err = SessionError::InvalidInfoHash("bad hash".into());
    let display = err.to_string();
    let converted: io::Error = err.into();
    assert_eq!(converted.kind(), ErrorKind::InvalidInput);
    assert_eq!(converted.to_string(), display);
}

#[test]
fn io_variant_passes_through_unchanged() {
    let original = io::Error::from_raw_os_error(13);
    let converted: io::Error = SessionError::Io(original).into();
    assert_eq!(converted.kind(), ErrorKind::PermissionDenied);
    assert_eq!(converted.raw_os_error(), Some(13));

    let original = io::Error::new(ErrorKind::NotFound, "missing");
    let converted: io::Error = SessionError::Io(original).into();
    assert_eq!(converted.kind(), ErrorKind::NotFound);
    assert_eq!(converted.to_string(), "missing");
}

#[test]
fn load_resume_file_missing_file_reports_not_found() {
    let fixture = TempDirFixture::new();
    let missing = format!("{}/absent.libtorrent_resume", fixture.path());
    let err = load_resume_file(&missing).unwrap_err();
    assert!(matches!(&err, SessionError::Io(e) if e.kind() == ErrorKind::NotFound));
    assert_eq!(err.kind(), ErrorKind::NotFound);
}

#[test]
fn load_resume_file_corrupt_bencode_reports_invalid_data() {
    let fixture = TempDirFixture::new();
    let path = format!("{}/corrupt.libtorrent_resume", fixture.path());
    fs::write(&path, b"!!!not-bencode!!!").unwrap();
    let err = load_resume_file(&path).unwrap_err();
    assert!(matches!(err, SessionError::Bencode(_)));
    assert_eq!(err.kind(), ErrorKind::InvalidData);
}

#[test]
fn build_path_validation_reports_invalid_input() {
    let hash = "0123456789abcdef0123456789abcdef01234567";

    let err = DownloadStorer::build_path("", hash).unwrap_err();
    assert!(matches!(err, SessionError::InvalidPath(_)));
    assert_eq!(err.kind(), ErrorKind::InvalidInput);

    let err = DownloadStorer::build_path("/tmp/no-trailing-slash", hash).unwrap_err();
    assert!(matches!(err, SessionError::InvalidPath(_)));
    assert_eq!(err.kind(), ErrorKind::InvalidInput);

    let err = DownloadStorer::build_path("/tmp/", "../escape").unwrap_err();
    assert!(matches!(err, SessionError::InvalidInfoHash(_)));
    assert_eq!(err.kind(), ErrorKind::InvalidInput);
}

#[test]
fn save_and_move_streams_rejects_directory_destination() {
    let fixture = TempDirFixture::new();
    let base = format!(
        "{}/0123456789abcdef0123456789abcdef01234567.torrent",
        fixture.path()
    );
    let blocked = format!("{base}.rtorrent");
    fs::create_dir(&blocked).unwrap();

    let err = DownloadStorer::save_and_move_streams(
        &base,
        false,
        Some(b"torrent-data"),
        b"rtorrent-data",
        b"libtorrent-data",
    )
    .unwrap_err();

    assert!(matches!(&err, SessionError::DestinationIsDirectory(p) if p == &blocked));
    assert_eq!(err.kind(), ErrorKind::Other);
    assert!(err.to_string().contains(&blocked));

    // The rejection happens before publishing: no partial files, and every
    // staged `.new` file is cleaned up.
    assert!(!std::path::Path::new(&base).exists());
    assert!(!std::path::Path::new(&format!("{base}.libtorrent_resume")).exists());
    for suffix in ["", ".libtorrent_resume", ".rtorrent"] {
        assert!(!std::path::Path::new(&format!("{base}{suffix}.new")).exists());
    }
}
