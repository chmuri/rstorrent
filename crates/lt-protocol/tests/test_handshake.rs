use lt_common::hash_string::HashString;
use lt_net::addr::SocketAddress;
use lt_protocol::encryption::{EncryptionPolicy, HandshakeEncryption};
use lt_protocol::handshake::{HandshakeError, HandshakeMessage, BT_HANDSHAKE_SIZE};
use lt_protocol::handshake_manager::{HandshakeManager, HandshakePhase};
use std::str::FromStr;

#[test]
fn test_plaintext_handshake_roundtrip() {
    let mut info_bytes = [0u8; 20];
    info_bytes.fill(0xaa);
    let info_hash = HashString::new(info_bytes);

    let mut peer_bytes = [0u8; 20];
    peer_bytes.fill(0xbb);
    let peer_id = HashString::new(peer_bytes);

    let msg = HandshakeMessage::new(info_hash, peer_id);
    assert!(msg.has_ltep());
    assert!(msg.has_dht());
    assert!(msg.has_fast());

    let encoded = msg.encode();
    assert_eq!(encoded.len(), BT_HANDSHAKE_SIZE);
    assert_eq!(encoded[0], 19);
    assert_eq!(&encoded[1..20], b"BitTorrent protocol");

    let decoded = HandshakeMessage::decode(&encoded).unwrap();
    assert_eq!(msg, decoded);
    assert_eq!(decoded.info_hash, info_hash);
    assert_eq!(decoded.peer_id, peer_id);
    assert!(decoded.has_ltep());
    assert!(decoded.has_dht());
    assert!(decoded.has_fast());
}

#[test]
fn test_invalid_handshake_rejected() {
    // Too short
    let short_data = [0u8; 50];
    assert_eq!(
        HandshakeMessage::decode(&short_data),
        Err(HandshakeError::TooShort(50))
    );

    // Invalid protocol length
    let mut bad_len = [0u8; 68];
    bad_len[0] = 18;
    assert_eq!(
        HandshakeMessage::decode(&bad_len),
        Err(HandshakeError::InvalidProtocolLen(18))
    );

    // Invalid protocol string
    let mut bad_str = [0u8; 68];
    bad_str[0] = 19;
    bad_str[1..20].copy_from_slice(b"WrongProtocolString");
    assert!(matches!(
        HandshakeMessage::decode(&bad_str),
        Err(HandshakeError::InvalidProtocolStr(_))
    ));
}

#[test]
fn test_mse_pe_exchange_and_encryption() {
    let mut alice = HandshakeEncryption::new();
    let mut bob = HandshakeEncryption::new();

    assert!(alice.is_valid());
    assert!(bob.is_valid());

    let alice_pub = alice.pub_key();
    let bob_pub = bob.pub_key();
    assert_eq!(alice_pub.len(), 96);
    assert_eq!(bob_pub.len(), 96);

    // Both compute shared secret
    assert!(alice.compute_secret(&bob_pub));
    assert!(bob.compute_secret(&alice_pub));

    assert_eq!(alice.shared_secret, bob.shared_secret);
    let secret = alice.shared_secret.unwrap();

    // Verify sync hash matches
    let alice_sync = alice.sync_hash().unwrap();
    let bob_sync = bob.sync_hash().unwrap();
    assert_eq!(alice_sync, bob_sync);

    // SKEY (info_hash) obfuscation and deobfuscation
    let info_bytes = [0x55u8; 20];
    let mut obfuscated = HandshakeEncryption::obfuscate_skey(&info_bytes, &secret);
    assert_ne!(obfuscated, info_bytes);

    assert!(bob.deobfuscate_skey(&mut obfuscated));
    // After deobfuscation, it matches original SHA1(req2 || SKEY)
    let req2 = lt_crypto::dh::mse_req2(&info_bytes);
    assert_eq!(obfuscated, req2);

    // Setup ciphers with discard 1024
    assert!(alice.setup_ciphers(false, &info_bytes)); // Alice is outgoing
    assert!(bob.setup_ciphers(true, &info_bytes)); // Bob is incoming

    // Message transmission Alice -> Bob
    let mut msg_alice_to_bob = b"BitTorrent encrypted stream Alice -> Bob".to_vec();
    let original1 = msg_alice_to_bob.clone();
    alice.encrypt(&mut msg_alice_to_bob);
    assert_ne!(msg_alice_to_bob, original1);

    bob.decrypt(&mut msg_alice_to_bob);
    assert_eq!(msg_alice_to_bob, original1);

    // Message transmission Bob -> Alice
    let mut msg_bob_to_alice = b"BitTorrent encrypted reply Bob -> Alice".to_vec();
    let original2 = msg_bob_to_alice.clone();
    bob.encrypt(&mut msg_bob_to_alice);
    assert_ne!(msg_bob_to_alice, original2);

    alice.decrypt(&mut msg_bob_to_alice);
    assert_eq!(msg_bob_to_alice, original2);
}

#[test]
fn test_handshake_manager_lifecycle() {
    let mut own_id = HashString::default();
    own_id.clear_byte(0x77);

    let policy = EncryptionPolicy::require();

    let mut mgr = HandshakeManager::new(own_id, policy);
    let sa1 = SocketAddress::from_str("127.0.0.1:6881").unwrap();
    let sa2 = SocketAddress::from_str("127.0.0.1:6882").unwrap();

    let mut target_hash = HashString::default();
    target_hash.clear_byte(0x88);

    mgr.start_outgoing(sa1.clone(), target_hash);
    mgr.start_incoming(sa2.clone());

    assert_eq!(mgr.sessions.len(), 2);
    let s1 = mgr.get_session_mut(&sa1).unwrap();
    assert!(!s1.is_incoming);
    assert_eq!(s1.phase, HandshakePhase::NegotiatingEncryption);

    let s2 = mgr.get_session_mut(&sa2).unwrap();
    assert!(s2.is_incoming);
    assert_eq!(s2.phase, HandshakePhase::NegotiatingEncryption);

    mgr.remove_session(&sa1);
    assert_eq!(mgr.sessions.len(), 1);
    assert!(mgr.get_session_mut(&sa1).is_none());
}
