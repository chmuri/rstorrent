//! Property tests for magnet URI parsing and chunk selection (plan.md :261).

use lt_common::HashString;
use lt_engine::{ChunkSelector, DownloadConstructor};
use proptest::prelude::*;

/// RFC 4648 base32 (no padding) of a 20-byte info-hash: 160 bits -> 32 chars.
fn base32_encode(bytes: &[u8; 20]) -> String {
    const ALPHABET: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
    let mut out = String::new();
    let mut buffer: u32 = 0;
    let mut bits = 0u32;
    for &byte in bytes.iter() {
        buffer = (buffer << 8) | u32::from(byte);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(ALPHABET[((buffer >> bits) & 0x1f) as usize] as char);
        }
    }
    if bits > 0 {
        out.push(ALPHABET[((buffer << (5 - bits)) & 0x1f) as usize] as char);
    }
    out
}

proptest! {
    /// A 40-char hex `btih` roundtrips to the exact 20-byte info-hash.
    #[test]
    fn magnet_hex_roundtrip(bytes in proptest::array::uniform20(any::<u8>())) {
        let hex = HashString::new(bytes).to_hex();
        let uri = format!("magnet:?xt=urn:btih:{hex}");
        let meta = DownloadConstructor::from_magnet_uri(&uri).unwrap();
        prop_assert_eq!(meta.info_hash.as_bytes(), &bytes);
        prop_assert!(meta.is_meta_download);
    }

    /// A 32-char base32 `btih` roundtrips to the exact 20-byte info-hash.
    #[test]
    fn magnet_base32_roundtrip(bytes in proptest::array::uniform20(any::<u8>())) {
        let b32 = base32_encode(&bytes);
        let uri = format!("magnet:?xt=urn:btih:{b32}");
        let meta = DownloadConstructor::from_magnet_uri(&uri).unwrap();
        prop_assert_eq!(meta.info_hash.as_bytes(), &bytes);
    }

    /// `dn` and `tr` are extracted verbatim when they need no percent-decoding.
    #[test]
    fn magnet_name_and_tracker(
        bytes in proptest::array::uniform20(any::<u8>()),
        name in "[a-z0-9]{1,24}",
        host in "[a-z0-9]{1,24}",
    ) {
        let hex = HashString::new(bytes).to_hex();
        let tracker = format!("http://{host}/announce");
        let uri = format!("magnet:?xt=urn:btih:{hex}&dn={name}&tr={tracker}");
        let meta = DownloadConstructor::from_magnet_uri(&uri).unwrap();
        prop_assert_eq!(meta.name, name);
        prop_assert_eq!(meta.announce.as_deref(), Some(tracker.as_str()));
        prop_assert_eq!(meta.announce_list, vec![vec![tracker]]);
    }

    /// No magnet helper panics on an arbitrary string.
    #[test]
    fn magnet_arbitrary_never_panics(uri in any::<String>()) {
        let _ = DownloadConstructor::from_magnet_uri(&uri);
        let _ = DownloadConstructor::magnet_metadata_hash(&uri);
        let _ = DownloadConstructor::magnet_v2_hash(&uri);
        let _ = DownloadConstructor::magnet_web_seeds(&uri);
    }

    /// Marking pieces completed partitions `completed`/`is_wanted` exactly,
    /// and out-of-range marks are ignored.
    #[test]
    fn selector_completed_partition(
        size in 0usize..=256,
        marks in prop::collection::vec(0u32..=300, 0..=64),
    ) {
        let mut selector = ChunkSelector::new(size);
        let mut expected = vec![false; size];
        for &mark in &marks {
            selector.mark_completed(mark);
            if (mark as usize) < size {
                expected[mark as usize] = true;
            }
        }
        for (index, &done) in expected.iter().enumerate() {
            prop_assert_eq!(selector.completed().get(index), done);
            prop_assert_eq!(selector.is_wanted(index as u32), !done);
        }
        prop_assert_eq!(selector.is_wanted(size as u32), false);
    }

    /// `collect_missing(limit)` is the ascending uncompleted prefix of length
    /// `min(limit, missing)`, with no duplicates and only wanted indices.
    #[test]
    fn selector_collect_missing(
        size in 0usize..=256,
        marks in prop::collection::vec(0u32..=300, 0..=64),
        limit in 0usize..=300,
    ) {
        let mut selector = ChunkSelector::new(size);
        let mut completed = vec![false; size];
        for &mark in &marks {
            selector.mark_completed(mark);
            if (mark as usize) < size {
                completed[mark as usize] = true;
            }
        }
        let missing: Vec<u32> = (0..size)
            .filter(|&index| !completed[index])
            .map(|index| index as u32)
            .collect();
        let expected: Vec<u32> = missing.iter().copied().take(limit).collect();
        prop_assert_eq!(selector.collect_missing(limit), expected);
        prop_assert_eq!(selector.collect_missing(size), missing);
    }

    /// `using_index` hides a piece and `not_using_index` restores it, unless it
    /// is out of range (then it stays unwanted throughout).
    #[test]
    fn selector_using_roundtrip(size in 1usize..=256, index in 0u32..=300) {
        let mut selector = ChunkSelector::new(size);
        let in_range = (index as usize) < size;
        prop_assert_eq!(selector.is_wanted(index), in_range);
        selector.using_index(index);
        prop_assert_eq!(selector.is_wanted(index), false);
        selector.not_using_index(index);
        prop_assert_eq!(selector.is_wanted(index), in_range);
    }
}
