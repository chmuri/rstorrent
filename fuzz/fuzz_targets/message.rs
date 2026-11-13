#![no_main]
//! Fuzz the peer-wire message codec and the handshake decoder. The first byte
//! is treated as the message id and the remainder as the payload.
use libfuzzer_sys::fuzz_target;
use lt_protocol::handshake::HandshakeMessage;
use lt_protocol::message::PeerMessage;

fuzz_target!(|data: &[u8]| {
    let _ = HandshakeMessage::decode(data);
    if let Some((&id, payload)) = data.split_first() {
        let _ = PeerMessage::decode(id, payload);
    }
});
