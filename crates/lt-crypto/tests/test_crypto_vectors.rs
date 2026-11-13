//! Known-answer vectors for the MSE/PE crypto primitives. These pin the wire
//! format (RC4 key schedule, MSE salt order, MODP peer-key validation) so a
//! regression breaks the build instead of silently breaking interoperability
//! with other BitTorrent clients.

use lt_crypto::{
    mse_key_a, mse_key_b, mse_req1, mse_req2, mse_req3, sha1_hash, DiffieHellman, Rc4, DH_PRIME,
};
use num_bigint::BigUint;

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

#[test]
fn rc4_rfc6229_keystream_vectors() {
    // RFC 6229: key = 0102030405, first 16 keystream bytes.
    let mut cipher = Rc4::new(&[0x01, 0x02, 0x03, 0x04, 0x05]);
    let mut buf = [0u8; 16];
    cipher.crypt(&mut buf);
    assert_eq!(hex(&buf), "b2396305f03dc027ccc3524a0a1118a8");

    // RFC 6229: key = 0102030405060708, first 16 keystream bytes.
    let mut cipher = Rc4::new(&[0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08]);
    let mut buf = [0u8; 16];
    cipher.crypt(&mut buf);
    assert_eq!(hex(&buf), "97ab8a1bf0afb96132f2f67258da15a8");
}

#[test]
fn rc4_published_encryption_vectors() {
    let cases: &[(&[u8], &[u8], &str)] = &[
        (b"Key", b"Plaintext", "bbf316e8d940af0ad3"),
        (b"Wiki", b"pedia", "1021bf0420"),
        (b"Secret", b"Attack at dawn", "45a01f645fc35b383552544b9bf5"),
    ];
    for &(key, plaintext, expected) in cases {
        let mut buf = plaintext.to_vec();
        Rc4::new(key).crypt(&mut buf);
        assert_eq!(hex(&buf), expected);
        // RC4 is symmetric: a second pass restores the plaintext.
        Rc4::new(key).crypt(&mut buf);
        assert_eq!(&buf[..], plaintext);
    }
}

#[test]
fn rc4_mse_discard_1024_matches_reference() {
    // MSE/PE discards the first 1024 keystream bytes; reference produced by an
    // independent RC4 implementation.
    let key: Vec<u8> = (0u8..20).collect();
    let mut cipher = Rc4::new(&key);
    cipher.discard(1024);
    let mut buf = b"MSE handshake payload".to_vec();
    cipher.crypt(&mut buf);
    assert_eq!(hex(&buf), "86d7a6e936bf15d3167bf16cefc90716c4bf81e8be");
}

#[test]
fn sha1_fips_abc_vector() {
    assert_eq!(
        hex(&sha1_hash(b"abc")),
        "a9993e364706816aba3e25717850c26c9cd0d89d"
    );
}

#[test]
fn mse_key_derivation_known_answers() {
    let s = [42u8; 96];
    let skey = [7u8; 20];
    assert_eq!(
        hex(&mse_req1(&s)),
        "bb8d32207da4efac324c13fe07faa9f252558243"
    );
    assert_eq!(
        hex(&mse_req2(&skey)),
        "4a90947f810ab816d0d9b5370b2b1dddde511d77"
    );
    assert_eq!(
        hex(&mse_req3(&s)),
        "28944ea30bfdbf26603841e56556209e0f2b8c1a"
    );
    assert_eq!(
        hex(&mse_key_a(&s, &skey)),
        "42cfe0b5bca3acd97d6b385b9b0f4ffd8b8bb719"
    );
    assert_eq!(
        hex(&mse_key_b(&s, &skey)),
        "80c635382d4e07ca3db130b9143ee051ceadb81f"
    );
}

#[test]
fn dh_rejects_invalid_peer_public_keys() {
    let mut dh = DiffieHellman::new_mse();
    let prime = BigUint::from_bytes_be(&DH_PRIME);

    let zero = [0u8; 96];
    assert!(!dh.compute_secret(&zero), "peer key 0 must be rejected");

    let mut one = [0u8; 96];
    one[95] = 1;
    assert!(!dh.compute_secret(&one), "peer key 1 must be rejected");

    let equal_to_prime = prime.to_bytes_be();
    assert!(
        !dh.compute_secret(&equal_to_prime),
        "peer key == prime must be rejected"
    );

    assert!(dh.secret().is_none(), "no secret stored on rejection");
}

#[test]
fn dh_compute_secret_matches_modpow_identity() {
    // g = 2, so a peer public key of 4 = g^2 yields shared = (g^x)^2 mod p,
    // computable from our own public key without knowing the private key.
    let mut dh = DiffieHellman::new_mse();
    let prime = BigUint::from_bytes_be(&DH_PRIME);
    let pub_key = BigUint::from_bytes_be(&dh.pub_key_bytes());

    let mut peer = [0u8; 96];
    peer[95] = 4;
    assert!(dh.compute_secret(&peer));

    let expected = pub_key.modpow(&BigUint::from(2u32), &prime);
    let mut expected_bytes = [0u8; 96];
    let be = expected.to_bytes_be();
    expected_bytes[96 - be.len()..].copy_from_slice(&be);
    assert_eq!(dh.secret().unwrap(), &expected_bytes[..]);
}

#[test]
fn dh_public_key_is_padded_and_in_range() {
    let dh = DiffieHellman::new_mse();
    let bytes = dh.pub_key_bytes();
    assert_eq!(bytes.len(), 96);
    let value = BigUint::from_bytes_be(&bytes);
    let prime = BigUint::from_bytes_be(&DH_PRIME);
    assert!(value > BigUint::from(1u32) && value < prime);
    assert!(dh.is_valid());
}
