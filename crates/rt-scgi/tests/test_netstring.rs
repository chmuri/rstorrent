//! Phase 0 — SCGI client netstring framing.
//!
//! These tests pin the wire format a client must produce and the decoder
//! behaviour it must tolerate. They are written before the `netstring` module
//! exists so every assertion starts red.

use rt_scgi::netstring::{
    decode, decode_request, encode, encode_request, NetstringError, RequestFrame,
};

#[test]
fn encode_decode_round_trip_for_text_and_binary_payloads() {
    let payloads: &[&[u8]] = &[
        b"",
        b"hello world",
        b"a\0b\0c",
        &[0xff, 0x00, 0x80, 0x7f],
        b"unterminated-looking,",
    ];

    for payload in payloads {
        let frame = encode(payload);
        let (decoded, consumed) = decode(&frame).unwrap_or_else(|err| {
            panic!("payload {payload:?} did not decode: {err}");
        });

        assert_eq!(decoded, *payload);
        assert_eq!(
            consumed,
            frame.len(),
            "decoder must report the whole frame as consumed"
        );
    }
}

#[test]
fn encode_uses_the_netstring_wire_format() {
    assert_eq!(encode(b""), b"0:,".to_vec());
    assert_eq!(encode(b"abc"), b"3:abc,".to_vec());
    assert_eq!(encode(b"a\0b"), b"3:a\0b,".to_vec());

    let long = encode(&[b'x'; 100]);
    assert!(long.starts_with(b"100:"));
    assert!(long.ends_with(b","));
}

#[test]
fn decode_reports_bytes_consumed_for_pipelined_frames() {
    let mut buffer = encode(b"one");
    buffer.extend_from_slice(&encode(b"two"));

    let (first, first_len) = decode(&buffer).unwrap();
    let (second, second_len) = decode(&buffer[first_len..]).unwrap();

    assert_eq!(first, b"one");
    assert_eq!(second, b"two");
    assert_eq!(first_len + second_len, buffer.len());
}

#[test]
fn decode_accepts_zero_padded_lengths() {
    // Not canonical, but some clients pad the length. Accepting it must not
    // change the payload or the consumed count.
    let frame = b"003:abc,";
    let (payload, consumed) = decode(frame).unwrap();
    assert_eq!(payload, b"abc");
    assert_eq!(consumed, frame.len());
}

#[test]
fn decode_rejects_malformed_length_prefixes() {
    assert_eq!(decode(b""), Err(NetstringError::MissingColon));
    assert_eq!(decode(b"12"), Err(NetstringError::MissingColon));
    assert_eq!(decode(b":abc,"), Err(NetstringError::EmptyLength));

    assert_eq!(decode(b"1x:abc,"), Err(NetstringError::InvalidLength));
    assert_eq!(decode(b"-1:a,"), Err(NetstringError::InvalidLength));
    assert_eq!(decode(b"+1:a,"), Err(NetstringError::InvalidLength));
    assert_eq!(decode(b" 1:a,"), Err(NetstringError::InvalidLength));
}

#[test]
fn decode_rejects_truncated_payload_and_missing_terminator() {
    assert_eq!(
        decode(b"5:abc"),
        Err(NetstringError::Truncated {
            declared: 5,
            available: 3
        })
    );
    assert_eq!(decode(b"3:abc"), Err(NetstringError::MissingTerminator));
    assert_eq!(decode(b"3:abc;"), Err(NetstringError::MissingTerminator));
}

#[test]
fn decode_rejects_oversized_length_without_allocating() {
    assert_eq!(
        decode(b"999999999999999999999999:"),
        Err(NetstringError::LengthOverflow)
    );
    // usize::MAX on 64-bit is 20 digits; one more digit cannot be a payload
    // length this process could ever receive.
    assert_eq!(
        decode(b"18446744073709551616:,"),
        Err(NetstringError::LengthOverflow)
    );
}

#[test]
fn request_round_trip_injects_content_length_and_scgi_marker() {
    let body = b"<?xml version=\"1.0\"?><methodCall/>";
    let frame = encode_request(&[("Content-Type", "text/xml")], body).unwrap();

    let request = decode_request(&frame).unwrap();

    assert_eq!(request.body, body);
    assert_eq!(request.consumed, frame.len());
    assert_eq!(
        request.header("CONTENT_LENGTH"),
        Some(body.len().to_string().as_str())
    );
    assert_eq!(request.header("SCGI"), Some("1"));
    assert_eq!(request.header("Content-Type"), Some("text/xml"));
}

#[test]
fn request_encoder_overrides_a_caller_supplied_content_length() {
    let body = b"1234567890";
    let frame = encode_request(&[("CONTENT_LENGTH", "1"), ("SCGI", "1")], body).unwrap();

    let request = decode_request(&frame).unwrap();
    assert_eq!(request.header("CONTENT_LENGTH"), Some("10"));
    assert_eq!(
        request
            .headers
            .iter()
            .filter(|(name, _)| name == "CONTENT_LENGTH")
            .count(),
        1,
        "the caller value must be replaced, not duplicated"
    );
    assert_eq!(request.body, body);
}

#[test]
fn request_round_trip_handles_an_empty_body_and_no_headers() {
    let frame = encode_request(&[], b"").unwrap();
    let request = decode_request(&frame).unwrap();

    assert_eq!(request.header("CONTENT_LENGTH"), Some("0"));
    assert_eq!(request.header("SCGI"), Some("1"));
    assert!(request.body.is_empty());
    assert_eq!(request.consumed, frame.len());
}

#[test]
fn request_encoder_rejects_nul_bytes_in_headers() {
    assert!(matches!(
        encode_request(&[("X\0Bad", "value")], b""),
        Err(NetstringError::HeaderContainsNul(_))
    ));
    assert!(matches!(
        encode_request(&[("X-Good", "val\0ue")], b""),
        Err(NetstringError::HeaderContainsNul(_))
    ));
    assert!(matches!(
        encode_request(&[("", "value")], b""),
        Err(NetstringError::EmptyHeaderName)
    ));
}

#[test]
fn decode_request_rejects_frames_without_content_length() {
    let mut header_block = b"SCGI\0".to_vec();
    header_block.extend_from_slice(b"1\0");
    let mut frame = Vec::new();
    frame.extend_from_slice(format!("{}:", header_block.len()).as_bytes());
    frame.extend_from_slice(&header_block);
    frame.push(b',');
    frame.extend_from_slice(b"body");

    assert_eq!(
        decode_request(&frame),
        Err(NetstringError::MissingContentLength)
    );
}

#[test]
fn decode_request_rejects_invalid_or_short_content_length() {
    let build = |content_length: &str, body: &[u8]| {
        let mut header_block = Vec::new();
        header_block.extend_from_slice(b"CONTENT_LENGTH\0");
        header_block.extend_from_slice(content_length.as_bytes());
        header_block.push(0);

        let mut frame = Vec::new();
        frame.extend_from_slice(format!("{}:", header_block.len()).as_bytes());
        frame.extend_from_slice(&header_block);
        frame.push(b',');
        frame.extend_from_slice(body);
        frame
    };

    assert_eq!(
        decode_request(&build("not-a-number", b"")),
        Err(NetstringError::InvalidContentLength)
    );
    assert_eq!(
        decode_request(&build("4", b"ab")),
        Err(NetstringError::Truncated {
            declared: 4,
            available: 2
        })
    );
}

#[test]
fn decode_request_rejects_an_odd_header_count() {
    // `orphan` has no value field.
    let header_block = b"CONTENT_LENGTH\0" as &[u8];
    let mut header_block = header_block.to_vec();
    header_block.extend_from_slice(b"0\0orphan\0");

    let mut frame = Vec::new();
    frame.extend_from_slice(format!("{}:", header_block.len()).as_bytes());
    frame.extend_from_slice(&header_block);
    frame.push(b',');

    assert_eq!(
        decode_request(&frame),
        Err(NetstringError::MalformedHeaders)
    );
}

#[test]
fn request_frame_header_lookup_is_case_sensitive() {
    let frame = encode_request(&[("Content-Type", "text/xml")], b"x").unwrap();
    let request: RequestFrame = decode_request(&frame).unwrap();

    // SCGI header names are CGI variables and therefore case-sensitive.
    assert_eq!(request.header("content-type"), None);
    assert_eq!(request.header("Content-Type"), Some("text/xml"));
}
