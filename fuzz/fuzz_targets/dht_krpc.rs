#![no_main]
//! Fuzz the DHT KRPC decoder: arbitrary bencode bytes must never panic.
use libfuzzer_sys::fuzz_target;
use lt_dht::transaction::KrpcMessage;

fuzz_target!(|data: &[u8]| {
    let _ = KrpcMessage::decode(data);
});
