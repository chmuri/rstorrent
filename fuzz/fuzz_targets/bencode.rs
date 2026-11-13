#![no_main]
//! Fuzz the bencode decoder: arbitrary bytes must never panic, and a decoded
//! object must re-encode without panicking.
use libfuzzer_sys::fuzz_target;
use lt_common::object::{decode, encode};

fuzz_target!(|data: &[u8]| {
    if let Ok(obj) = decode(data) {
        let _ = encode(&obj);
    }
});
