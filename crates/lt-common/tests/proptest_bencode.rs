//! Property tests for the bencode codec (plan.md :261).
//!
//! `Object::is_empty` is true only for `ObjectVal::None`, so the encoder never
//! drops the `Value`/`String`/`List`/`Map` nodes generated here. That makes a
//! full value roundtrip (`decode(encode(obj)) == obj`) a valid invariant, on top
//! of exact framing and canonical-encoding idempotence.

use std::collections::BTreeMap;

use lt_common::object::bencode::{decode, encode, read_bencode};
use lt_common::Object;
use proptest::prelude::*;

/// Bounded recursive strategy for normal bencode trees (no `None`/raw nodes).
fn any_object() -> impl Strategy<Value = Object> {
    let leaf = prop_oneof![
        any::<i64>().prop_map(Object::create_value),
        prop::collection::vec(any::<u8>(), 0..32).prop_map(Object::create_string),
    ];
    leaf.prop_recursive(4, 64, 4, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..8).prop_map(Object::create_list_with),
            prop::collection::hash_map(prop::collection::vec(any::<u8>(), 0..8), inner, 0..8)
                .prop_map(|map| {
                    Object::create_map_with(map.into_iter().collect::<BTreeMap<_, _>>())
                }),
        ]
    })
}

proptest! {
    /// A normal object survives encode/decode unchanged, the parser consumes
    /// exactly the encoded length, and re-encoding is byte-identical.
    #[test]
    fn roundtrip_normal(obj in any_object()) {
        let bytes = encode(&obj).unwrap();
        let (parsed, consumed) = read_bencode(&bytes).unwrap();
        prop_assert_eq!(consumed, bytes.len());
        prop_assert_eq!(&parsed, &obj);
        prop_assert_eq!(encode(&parsed).unwrap(), bytes);
    }

    /// The parser never panics on arbitrary bytes, well-formed or not.
    #[test]
    fn arbitrary_bytes_never_panic(bytes in prop::collection::vec(any::<u8>(), 0..256)) {
        let _ = read_bencode(&bytes);
        let _ = decode(&bytes);
    }
}
