#![no_main]
//! Fuzz the uTP packet decoder: arbitrary bytes must never panic.
use libfuzzer_sys::fuzz_target;
use lt_net::utp::packet::UtpPacket;

fuzz_target!(|data: &[u8]| {
    let _ = UtpPacket::decode(data);
});
