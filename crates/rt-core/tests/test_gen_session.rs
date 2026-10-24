//! Phase 0 — canonical session file names derived from an info hash.
//!
//! The session directory is restored by scanning for `<HASH>.torrent`. A name
//! that does not match `is_correct_format` is silently invisible after a
//! restart, so the generator is the single place that decides the casing and
//! suffix of every session stream.

use rt_core::session::{gen_session, is_correct_format, SessionPaths};
use std::io::ErrorKind;

const LOWER: &str = "0123456789abcdef0123456789abcdef01234567";
const UPPER: &str = "0123456789ABCDEF0123456789ABCDEF01234567";

fn assert_canonical(paths: &SessionPaths, session_dir: &str) {
    let expected = format!("{}{}.torrent", session_dir, UPPER);

    assert_eq!(paths.torrent, expected);
    assert_eq!(paths.resume, format!("{}.libtorrent_resume", expected));
    assert_eq!(paths.sidecar, format!("{}.rtorrent", expected));

    let file_name = paths.torrent.rsplit('/').next().unwrap();
    assert_eq!(file_name.len(), 48, "a session entry is name + suffix");
    assert!(
        is_correct_format(file_name),
        "{file_name} must be scannable"
    );
}

#[test]
fn gen_session_builds_canonical_paths_for_an_uppercase_hash() {
    let paths = gen_session("/var/session/", UPPER).unwrap();
    assert_canonical(&paths, "/var/session/");
}

#[test]
fn gen_session_normalizes_lowercase_hashes_to_the_same_paths() {
    let lower = gen_session("/var/session/", LOWER).unwrap();
    let upper = gen_session("/var/session/", UPPER).unwrap();

    assert_eq!(lower, upper);
    assert_canonical(&lower, "/var/session/");
    assert!(
        !lower.torrent.contains("abcdef"),
        "file names are uppercase"
    );
}

#[test]
fn gen_session_normalizes_mixed_case_hashes() {
    let mixed = "0123456789AbCdEf0123456789aBcDeF01234567";
    let paths = gen_session("/s/", mixed).unwrap();
    assert_canonical(&paths, "/s/");
}

#[test]
fn gen_session_uses_the_session_directory_verbatim() {
    let paths = gen_session("/tmp/session dir/", LOWER).unwrap();
    assert_canonical(&paths, "/tmp/session dir/");
}

#[test]
fn gen_session_accepts_an_all_zero_and_an_all_f_hash() {
    for hash in ["0".repeat(40), "f".repeat(40)] {
        let paths = gen_session("/s/", &hash).unwrap();
        assert!(paths.torrent.ends_with(".torrent"));
        assert!(paths.torrent.starts_with("/s/"));
        assert_eq!(paths.torrent.len(), "/s/".len() + 48);
    }
}

#[test]
fn gen_session_rejects_malformed_hashes() {
    let cases = [
        String::new(),
        "0".repeat(39),
        "0".repeat(41),
        "0".repeat(40) + "0",
        "../escape".to_string(),
        "0123456789ABCDEF0123456789ABCDEF0123456G".to_string(),
        "0123456789ABCDEF0123456789ABCDEF0123456 ".to_string(),
        "0123456789ABCDEF0123456789ABCDEF01234567\n".to_string(),
        // 40 bytes, but the first character is a two-byte non-ASCII scalar.
        format!("é{}", "0".repeat(38)),
    ];

    for hash in cases {
        match gen_session("/s/", &hash) {
            Ok(paths) => panic!("hash {hash:?} must be rejected, got {paths:?}"),
            Err(err) => assert_eq!(err.kind(), ErrorKind::InvalidInput, "hash {hash:?}"),
        }
    }
}

#[test]
fn gen_session_rejects_an_empty_session_path() {
    let err = gen_session("", LOWER).unwrap_err();
    assert_eq!(err.kind(), ErrorKind::InvalidInput);
}

#[test]
fn gen_session_requires_a_trailing_slash() {
    let err = gen_session("/var/session", LOWER).unwrap_err();
    assert_eq!(err.kind(), ErrorKind::InvalidInput);
}

#[test]
fn gen_session_paths_are_distinct() {
    let paths = gen_session("/s/", LOWER).unwrap();
    assert_ne!(paths.torrent, paths.resume);
    assert_ne!(paths.torrent, paths.sidecar);
    assert_ne!(paths.resume, paths.sidecar);
}
