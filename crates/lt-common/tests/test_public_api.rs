//! Integration tests exercising the public `lt-common` API from an external
//! consumer's perspective: cross-module data structures, encoding round-trips
//! and bencode serialization. These complement the inline unit tests rather
//! than duplicating their internals.

use std::collections::BTreeMap;

use lt_common::encoding::{
    bytes_to_hex, decode_base64, encode_base64, gzip_compress, gzip_decompress, hex_to_bytes,
    is_valid_utf8, sanitize_string, trim_spaces,
};
use lt_common::object::{decode, encode, object_sha1, read_bencode};
use lt_common::{
    Bitfield, Extents, ExtentsError, HashString, Object, Path, Ranges, Rate, StringUtf8,
};

#[test]
fn bitfield_set_get_roundtrip_and_search() {
    let mut bf = Bitfield::new(200);
    assert!(bf.is_all_unset());
    assert_eq!(bf.size_bits(), 200);

    bf.set(0);
    bf.set(63);
    bf.set(64);
    bf.set(199);
    assert!(bf.get(0));
    assert!(bf.get(63));
    assert!(bf.get(64));
    assert!(bf.get(199));
    assert!(!bf.get(1));
    assert!(!bf.get(200));
    assert_eq!(bf.size_set(), 4);
    assert_eq!(bf.size_unset(), 196);

    let bytes = bf.as_bytes().to_vec();
    let restored = Bitfield::from_bytes(&bytes, 200);
    assert_eq!(restored.size_set(), 4);
    for i in 0..200 {
        assert_eq!(
            restored.get(i),
            bf.get(i),
            "bit {i} differs after round-trip"
        );
    }

    assert_eq!(bf.find_first_set(0, 200), 0);
    assert_eq!(bf.find_first_set(1, 200), 63);
    assert_eq!(bf.find_first_set(65, 200), 199);
    assert_eq!(bf.find_first_set(200, 200), 200);

    bf.set_range(10, 20);
    assert_eq!(bf.size_set(), 14);
    assert!(bf.get(10));
    assert!(bf.get(19));
    assert!(!bf.get(20));
    bf.unset_range(10, 20);
    assert_eq!(bf.size_set(), 4);

    bf.set_all();
    assert!(bf.is_all_set());
    assert_eq!(bf.size_set(), 200);
    bf.unset_all();
    assert!(bf.is_all_unset());
    assert_eq!(bf.size_set(), 0);
}

#[test]
fn bitfield_peer_interesting_difference() {
    let mut ours = Bitfield::new(128);
    let mut peer = Bitfield::new(128);
    ours.set(0);
    ours.set(1);
    peer.set(1);
    peer.set(2);
    peer.set(3);

    assert!(ours.has_interesting(&peer));
    assert_eq!(ours.count_interesting(&peer), 2);
    let interesting: Vec<usize> = ours.iter_interesting(&peer).collect();
    assert_eq!(interesting, vec![2, 3]);

    let mut subset = Bitfield::new(128);
    subset.set(0);
    assert!(!ours.has_interesting(&subset));
    assert_eq!(ours.count_interesting(&subset), 0);
}

#[test]
fn hash_string_hex_slice_roundtrip() {
    let hex = "0123456789abcdef0123456789abcdef01234567";
    let h = HashString::from_hex(hex).unwrap();
    assert_eq!(h.to_hex(), hex);
    assert_eq!(h.to_hex_upper(), hex.to_uppercase());
    assert_eq!(h.as_slice().len(), 20);

    let bytes = *h.as_bytes();
    let h2 = HashString::from_slice(&bytes).unwrap();
    assert_eq!(h2.to_hex(), hex);

    assert!(HashString::from_hex("short").is_none());
    assert!(HashString::from_hex(&"z".repeat(40)).is_none());
    assert!(HashString::from_slice(&[0u8; 19]).is_none());

    let mut z = HashString::from_slice(&[0u8; 20]).unwrap();
    assert!(z.is_zero());
    z.clear_byte(0xff);
    assert!(!z.is_zero());
    z.clear();
    assert!(z.is_zero());
}

#[test]
fn ranges_merge_query_and_union() {
    let mut r = Ranges::<u64>::new();
    assert!(r.is_empty());
    r.insert(0, 5);
    r.insert(5, 10);
    r.insert(20, 30);
    assert_eq!(r.len(), 2);
    assert_eq!(r.as_slice().to_vec(), vec![(0u64, 10u64), (20u64, 30u64)]);

    assert!(r.has(0));
    assert!(r.has(9));
    assert!(!r.has(10));
    assert!(r.has(20));
    assert!(!r.has(19));
    assert_eq!(r.find(9), Some(0));
    assert_eq!(r.find(10), Some(1));
    assert_eq!(r.intersect_distance(3, 25), 12);

    r.erase(2, 8);
    assert_eq!(
        r.as_slice().to_vec(),
        vec![(0u64, 2u64), (8u64, 10u64), (20u64, 30u64)]
    );

    let mut a = Ranges::<u64>::new();
    a.insert(0, 5);
    a.insert(10, 15);
    let mut b = Ranges::<u64>::new();
    b.insert(3, 8);
    b.insert(12, 20);
    let u = Ranges::create_union(&a, &b);
    assert_eq!(u.as_slice().to_vec(), vec![(0u64, 8u64), (10u64, 20u64)]);
}

#[test]
fn extents_inclusive_lookup_and_errors() {
    let mut e = Extents::<u64, u32>::new();
    assert!(e.is_empty());
    e.insert(0, 10, 100);
    e.insert(20, 30, 200);
    assert_eq!(e.len(), 2);

    assert_eq!(e.at(0).ok(), Some(100));
    assert_eq!(e.at(10).ok(), Some(100));
    assert_eq!(e.at(11).ok(), None);
    assert_eq!(e.at(20).ok(), Some(200));
    assert_eq!(e.at(30).ok(), Some(200));
    assert!(e.defined(5));
    assert!(!e.defined(15));
    assert!(e.defined_range(5, 25));
    assert!(!e.defined_range(11, 19));
    assert_eq!(e.get_matching_key(5, 5).ok(), Some(0));
    assert_eq!(e.at_range(20, 30).ok(), Some(200));
    assert!(matches!(e.at(100), Err(ExtentsError::NotDefined)));
}

#[test]
fn encoding_base64_hex_gzip_roundtrips() {
    let data = b"hello world, rstorrent!";
    let b64 = encode_base64(data);
    assert_eq!(decode_base64(&b64).unwrap(), data.to_vec());

    let hex = bytes_to_hex(data);
    assert_eq!(hex_to_bytes(&hex).unwrap(), data.to_vec());
    assert!(hex_to_bytes("abc").is_err());
    assert!(hex_to_bytes("zz").is_err());

    let gz = gzip_compress(data).unwrap();
    assert_eq!(gzip_decompress(&gz).unwrap(), data.to_vec());

    assert!(is_valid_utf8(b"valid"));
    assert!(!is_valid_utf8(&[0xff, 0xfe]));
    assert_eq!(trim_spaces("  pad  "), "pad");
    assert_eq!(sanitize_string("a\u{0}b\tc"), "a b\tc");
}

#[test]
fn string_utf8_conversions() {
    let s = StringUtf8::from_string("hello");
    assert!(s.is_utf8());
    assert_eq!(s.as_str().unwrap(), "hello");
    assert_eq!(s.as_bytes(), b"hello".as_slice());
    assert_eq!(s.hex(), "68656c6c6f");
    assert_eq!(s.base64(), encode_base64(b"hello"));

    let bin = StringUtf8::from_string(vec![0xffu8, 0xfe]);
    assert!(!bin.is_utf8());
    assert!(bin.as_str().is_err());
}

#[test]
fn bencode_map_list_roundtrip_and_sha1() {
    let mut info = BTreeMap::new();
    info.insert(b"pieces".to_vec(), Object::create_string("abc"));
    info.insert(b"piece length".to_vec(), Object::create_value(16384));
    let mut root = BTreeMap::new();
    root.insert(b"name".to_vec(), Object::create_string("ubuntu"));
    root.insert(b"info".to_vec(), Object::create_map_with(info));
    let obj = Object::create_map_with(root);

    let enc = encode(&obj).unwrap();
    let dec = decode(&enc).unwrap();
    assert_eq!(dec.get_key_string(b"name").unwrap(), b"ubuntu".as_slice());
    let di = dec.get_key_map(b"info").unwrap();
    assert_eq!(
        di.get(b"pieces".as_slice()).unwrap().as_str().unwrap(),
        "abc"
    );
    assert_eq!(
        di.get(b"piece length".as_slice())
            .unwrap()
            .as_value()
            .unwrap(),
        16384
    );

    let h1 = object_sha1(&obj).unwrap();
    let h2 = object_sha1(&decode(&enc).unwrap()).unwrap();
    assert_eq!(h1, h2);

    let mut framed = enc.clone();
    framed.extend_from_slice(b"TRAILING");
    let (parsed, consumed) = read_bencode(&framed).unwrap();
    assert_eq!(consumed, enc.len());
    assert_eq!(encode(&parsed).unwrap(), enc);

    let list =
        Object::create_list_with(vec![Object::create_value(1), Object::create_string("two")]);
    let ld = decode(&encode(&list).unwrap()).unwrap();
    let items = ld.as_list().unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].as_value().unwrap(), 1);
    assert_eq!(items[1].as_str().unwrap(), "two");
}

#[test]
fn rate_sliding_window_deterministic() {
    let mut r = Rate::new(10);
    assert_eq!(r.span(), 10);
    assert_eq!(r.total(), 0);

    r.insert_at(1000, 100);
    assert_eq!(r.total(), 1000);
    assert_eq!(r.rate_at(100), 333);

    r.insert_at(2000, 101);
    assert_eq!(r.total(), 3000);
    assert_eq!(r.rate_at(101), 1000);

    assert_eq!(r.rate_at(200), 0);
    assert_eq!(r.total(), 3000);
}

#[test]
fn path_components_prefix_validation() {
    let mut p = Path::new();
    assert!(p.is_empty());
    p.push_back("a/b/c").unwrap();
    assert_eq!(p.len(), 3);
    assert_eq!(p.as_string(), "/a/b/c");
    let comps: Vec<&str> = p.components().iter().map(|s| s.as_str()).collect();
    assert_eq!(comps, vec!["a", "b", "c"]);

    let mut sub = Path::new();
    sub.push_back("a/b").unwrap();
    assert!(Path::is_prefix(&sub, &p));
    assert!(!Path::is_prefix(&p, &sub));
    assert!(sub < p);

    assert!(Path::is_valid_component("file.txt"));
    assert!(!Path::is_valid_component(".."));
    assert!(!Path::is_valid_component("a/b"));

    let mut bad = Path::new();
    assert!(bad.push_back_component("..").is_err());
    assert!(bad.push_back("x\0y").is_err());
}
