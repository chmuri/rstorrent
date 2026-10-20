use lt_common::Object;
use rt_core::session::DownloadStorer;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(1);

const ENTRY_NAME: &str = "0123456789ABCDEF0123456789ABCDEF01234567.torrent";
const LINK_NAME: &str = "FEDCBA9876543210FEDCBA9876543210FEDCBA98.torrent";

struct TempSessionFixture {
    temp_dir: std::path::PathBuf,
    session_dir: std::path::PathBuf,
}

impl TempSessionFixture {
    fn new() -> Self {
        let id = COUNTER.fetch_add(1, Ordering::SeqCst);
        let temp_dir = std::env::temp_dir().join(format!(
            "rtorrent_test_session_{}_{}",
            std::process::id(),
            id
        ));
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).unwrap();

        let session_dir = temp_dir.join("session");
        fs::create_dir_all(&session_dir).unwrap();

        Self {
            temp_dir,
            session_dir,
        }
    }
}

impl Drop for TempSessionFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.temp_dir);
    }
}

fn permissions_of(path: &std::path::Path) -> u32 {
    let meta = fs::metadata(path).unwrap();
    meta.permissions().mode() & 0o7777
}

fn save_session_files(path: &str) {
    DownloadStorer::save_and_move_streams(
        path,
        false,
        Some(b"torrent-data"),
        b"rtorrent-data",
        b"libtorrent-data",
    )
    .unwrap();
}

#[test]
fn test_temp_file_symlink_is_not_followed() {
    let fixture = TempSessionFixture::new();
    let outside = fixture.temp_dir.join("outside.txt");
    let path = fixture.session_dir.join(ENTRY_NAME);

    fs::write(&outside, "original").unwrap();
    let new_path = fixture.session_dir.join(format!("{}.new", ENTRY_NAME));
    std::os::unix::fs::symlink(&outside, &new_path).unwrap();

    save_session_files(path.to_str().unwrap());

    assert_eq!("original", fs::read_to_string(&outside).unwrap());
    assert_eq!("torrent-data", fs::read_to_string(&path).unwrap());
}

#[test]
fn test_saved_files_are_owner_only() {
    let fixture = TempSessionFixture::new();
    let path = fixture.session_dir.join(ENTRY_NAME);
    let path_str = path.to_str().unwrap();

    // Set umask to 0 to make sure permissions are strictly 0600
    let prev_umask = nix::sys::stat::umask(nix::sys::stat::Mode::empty());
    save_session_files(path_str);
    nix::sys::stat::umask(prev_umask);

    assert_eq!(0o600, permissions_of(&path));
    assert_eq!(
        0o600,
        permissions_of(&fixture.session_dir.join(format!("{}.rtorrent", ENTRY_NAME)))
    );
    assert_eq!(
        0o600,
        permissions_of(
            &fixture
                .session_dir
                .join(format!("{}.libtorrent_resume", ENTRY_NAME))
        )
    );
}

#[test]
fn test_symlinked_entry_is_not_a_file() {
    let fixture = TempSessionFixture::new();
    let outside = fixture.temp_dir.join("outside.txt");
    let regular = fixture.session_dir.join(ENTRY_NAME);
    let symlink = fixture.session_dir.join(LINK_NAME);

    fs::write(&outside, "d0:e").unwrap();
    fs::write(&regular, "d0:e").unwrap();
    std::os::unix::fs::symlink(&outside, &symlink).unwrap();

    let entries =
        DownloadStorer::get_formatted_entries(fixture.session_dir.to_str().unwrap()).unwrap();
    assert_eq!(2, entries.len());

    for entry in &entries {
        if entry.s_name == ENTRY_NAME {
            assert!(entry.is_file());
        } else {
            assert!(!entry.is_file());
        }
    }
}

#[test]
fn test_interrupted_write_is_recovered_on_next_startup() {
    let fixture = TempSessionFixture::new();
    let path = fixture.session_dir.join(ENTRY_NAME);
    let new_path = fixture.session_dir.join(format!("{}.new", ENTRY_NAME));

    // A crash between fsync and rename leaves a complete staged file but no
    // destination. Enumeration must publish it so the entry is not lost.
    let encoded = lt_common::object::bencode::encode(&Object::create_map()).unwrap();
    fs::write(&new_path, &encoded).unwrap();

    let entries =
        DownloadStorer::get_formatted_entries(fixture.session_dir.to_str().unwrap()).unwrap();
    assert!(path.is_file());
    assert!(!new_path.exists());
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].s_name, ENTRY_NAME);
}

#[test]
fn test_interrupted_write_is_recovered_with_trailing_slash() {
    let fixture = TempSessionFixture::new();
    let path = fixture.session_dir.join(ENTRY_NAME);
    let new_path = fixture.session_dir.join(format!("{}.new", ENTRY_NAME));

    let encoded = lt_common::object::bencode::encode(&Object::create_map()).unwrap();
    fs::write(&new_path, &encoded).unwrap();

    let session_path = format!("{}/", fixture.session_dir.display());
    let entries = DownloadStorer::get_formatted_entries(&session_path).unwrap();

    assert!(path.is_file());
    assert!(!new_path.exists());
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].s_name, ENTRY_NAME);
}

#[test]
fn test_stale_staged_file_with_trailing_slash_never_replaces_committed_entry() {
    let fixture = TempSessionFixture::new();
    let path = fixture.session_dir.join(ENTRY_NAME);
    let new_path = fixture.session_dir.join(format!("{}.new", ENTRY_NAME));
    let resume_path = fixture
        .session_dir
        .join(format!("{}.libtorrent_resume", ENTRY_NAME));
    let resume_new = fixture
        .session_dir
        .join(format!("{}.libtorrent_resume.new", ENTRY_NAME));

    fs::write(&path, b"committed-torrent").unwrap();
    fs::write(&new_path, b"stale-torrent").unwrap();

    let encoded = lt_common::object::bencode::encode(&Object::create_map()).unwrap();
    fs::write(&resume_new, encoded).unwrap();

    let session_path = format!("{}/", fixture.session_dir.display());
    let entries = DownloadStorer::get_formatted_entries(&session_path).unwrap();
    assert_eq!(entries.len(), 1);

    assert_eq!(fs::read(&path).unwrap(), b"committed-torrent");
    assert!(!new_path.exists());
    assert!(!resume_new.exists());
    assert!(resume_path.is_file());
}

#[test]
fn test_stale_staged_file_never_replaces_committed_entry() {
    let fixture = TempSessionFixture::new();
    let path = fixture.session_dir.join(ENTRY_NAME);
    let new_path = fixture.session_dir.join(format!("{}.new", ENTRY_NAME));
    let resume_path = fixture
        .session_dir
        .join(format!("{}.libtorrent_resume", ENTRY_NAME));
    let resume_new = fixture
        .session_dir
        .join(format!("{}.libtorrent_resume.new", ENTRY_NAME));

    fs::write(&path, b"committed-torrent").unwrap();
    fs::write(&new_path, b"stale-torrent").unwrap();

    let encoded = lt_common::object::bencode::encode(&Object::create_map()).unwrap();
    fs::write(&resume_new, encoded).unwrap();

    let entries =
        DownloadStorer::get_formatted_entries(fixture.session_dir.to_str().unwrap()).unwrap();
    assert_eq!(entries.len(), 1);

    // The committed generation wins and stale temporaries are removed.
    assert_eq!(fs::read(&path).unwrap(), b"committed-torrent");
    assert!(!new_path.exists());
    assert!(!resume_new.exists());

    // A complete staged resume sibling is promoted when the destination is
    // missing.
    assert!(resume_path.is_file());
}
