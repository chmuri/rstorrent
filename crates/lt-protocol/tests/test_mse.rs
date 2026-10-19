use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};

use lt_common::hash_string::HashString;
use lt_crypto::dh::mse_req2;
use lt_protocol::encryption::{
    constant_time_eq, EncryptionMode, EncryptionPolicy, HandshakeEncryption,
};
use lt_protocol::handshake::HandshakeMessage;
use lt_protocol::mse::{accept, initiate, MseError};
use tokio::io::{duplex, AsyncRead, AsyncWrite, AsyncWriteExt, ReadBuf};

fn id(byte: u8) -> HashString {
    HashString::new([byte; 20])
}

async fn expect_handshake(
    info_hash: HashString,
    initiator_peer: HashString,
    receiver_peer: HashString,
    initiator_policy: EncryptionPolicy,
    receiver_policy: EncryptionPolicy,
) -> (
    Result<lt_protocol::mse::MseSession, MseError>,
    Result<lt_protocol::mse::MseSession, MseError>,
) {
    let (mut a, mut b) = duplex(64 * 1024);
    let obfuscated = HashString::new(mse_req2(info_hash.as_bytes()));

    let receiver = tokio::spawn(async move {
        accept(&mut b, receiver_peer, &receiver_policy, move |hash| {
            if *hash == obfuscated {
                Some(info_hash)
            } else {
                None
            }
        })
        .await
    });

    let initiator = initiate(&mut a, info_hash, initiator_peer, &initiator_policy).await;
    let receiver = receiver.await.unwrap();
    (initiator, receiver)
}

#[tokio::test]
async fn encrypted_handshake_and_cipher_interop() {
    let info_hash = id(0x11);
    let initiator_peer = id(0xaa);
    let receiver_peer = id(0xbb);
    let policy = EncryptionPolicy::require();

    let (initiator, receiver) =
        expect_handshake(info_hash, initiator_peer, receiver_peer, policy, policy).await;

    let mut initiator = initiator.expect("initiator handshake");
    let mut receiver = receiver.expect("receiver handshake");

    assert!(initiator.encrypted_stream);
    assert!(receiver.encrypted_stream);
    assert!(initiator.is_encrypted());
    assert!(receiver.is_encrypted());
    assert!(initiator.is_obfuscated());
    assert!(receiver.is_obfuscated());
    assert_eq!(initiator.info_hash, info_hash);
    assert_eq!(receiver.info_hash, info_hash);
    assert_eq!(initiator.peer_id, receiver_peer);
    assert_eq!(receiver.peer_id, initiator_peer);

    let original = b"post-handshake payload".to_vec();

    let mut from_initiator = original.clone();
    initiator.encrypt(&mut from_initiator);
    assert_ne!(from_initiator, original);
    let mut at_receiver = from_initiator.clone();
    receiver.decrypt(&mut at_receiver);
    assert_eq!(at_receiver, original);

    let mut from_receiver = original.clone();
    receiver.encrypt(&mut from_receiver);
    assert_ne!(from_receiver, original);
    let mut at_initiator = from_receiver.clone();
    initiator.decrypt(&mut at_initiator);
    assert_eq!(at_initiator, original);
}

#[tokio::test]
async fn plaintext_stream_is_selected_when_allowed() {
    let info_hash = id(0x22);
    let policy = EncryptionPolicy::default();

    let (initiator, receiver) =
        expect_handshake(info_hash, id(0xdd), id(0xcc), policy, policy).await;

    let initiator = initiator.expect("initiator handshake");
    let receiver = receiver.expect("receiver handshake");

    assert!(!initiator.encrypted_stream);
    assert!(!receiver.encrypted_stream);
    assert!(!initiator.is_encrypted());
    assert!(!receiver.is_encrypted());
    // Both used MSE even though the resulting stream is plaintext.
    assert!(initiator.is_obfuscated());
    assert!(receiver.is_obfuscated());
    assert_eq!(initiator.peer_id, id(0xcc));
    assert_eq!(receiver.peer_id, id(0xdd));
}

#[tokio::test]
async fn encrypted_handshake_with_plaintext_stream() {
    // `handshake_prefer` + `stream_deny`: MSE is used for the handshake, but
    // only the plaintext stream mode is offered.
    let info_hash = id(0x23);
    let policy = EncryptionPolicy::new(EncryptionMode::Prefer, EncryptionMode::Disabled);

    let (initiator, receiver) =
        expect_handshake(info_hash, id(0x01), id(0x02), policy, policy).await;

    let mut initiator = initiator.expect("initiator handshake");
    let receiver = receiver.expect("receiver handshake");

    assert!(!initiator.encrypted_stream);
    assert!(!receiver.encrypted_stream);
    assert!(initiator.is_obfuscated());
    assert!(receiver.is_obfuscated());

    // Ciphers are intentionally dropped for a plaintext MSE stream.
    assert!(initiator.take_ciphers().0.is_none());
    assert!(initiator.take_ciphers().1.is_none());
}

#[tokio::test]
async fn required_stream_rejects_plaintext_only_offer() {
    let info_hash = id(0x24);
    let initiator_policy = EncryptionPolicy::new(EncryptionMode::Prefer, EncryptionMode::Disabled);
    let receiver_policy = EncryptionPolicy::require();

    let (initiator, receiver) = expect_handshake(
        info_hash,
        id(0x03),
        id(0x04),
        initiator_policy,
        receiver_policy,
    )
    .await;

    // The initiator observes EOF while waiting for the reply.
    assert!(initiator.is_err());
    match receiver {
        Err(MseError::EncryptionRequired) | Err(MseError::EncryptionNotAllowed) => {}
        Err(err) => panic!("expected stream policy rejection, got {err:?}"),
        Ok(_) => panic!("receiver accepted a plaintext-only offer"),
    }
}

#[tokio::test]
async fn mse_is_disabled_by_policy() {
    let (mut a, _b) = duplex(1024);
    let err = match initiate(&mut a, id(0x25), id(0x05), &EncryptionPolicy::disabled()).await {
        Err(err) => err,
        Ok(_) => panic!("MSE handshake unexpectedly succeeded"),
    };

    match err {
        MseError::EncryptionDisabled => {}
        other => panic!("expected EncryptionDisabled, got {other:?}"),
    }
}

#[tokio::test]
async fn initiate_detects_plaintext_peer_for_fallback() {
    let (mut a, mut b) = duplex(8 * 1024);

    let peer = tokio::spawn(async move {
        b.write_all(&HandshakeMessage::new(id(0x31), id(0x32)).encode())
            .await
            .unwrap();
        // Keep the stream alive while the initiator reads.
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    });

    let result = initiate(&mut a, id(0x31), id(0x30), &EncryptionPolicy::require()).await;
    match result {
        Err(MseError::PlaintextPeer) => {}
        Err(err) => panic!("expected PlaintextPeer, got {err:?}"),
        Ok(_) => panic!("plaintext peer was treated as MSE"),
    }
    peer.abort();
}

#[tokio::test]
async fn unknown_torrent_is_rejected_by_receiver() {
    let info_hash = id(0x33);
    let policy = EncryptionPolicy::require();

    let (mut a, mut b) = duplex(64 * 1024);
    let receiver =
        tokio::spawn(async move { accept(&mut b, id(0xee), &policy, |_hash| None).await });

    // The initiator will observe EOF once the receiver drops the stream.
    let _ = initiate(&mut a, info_hash, id(0xff), &policy).await;

    match receiver.await.unwrap() {
        Err(MseError::UnknownTorrent(_)) => {}
        Err(err) => panic!("expected UnknownTorrent, got {err:?}"),
        Ok(_) => panic!("expected UnknownTorrent, handshake succeeded"),
    }
}

#[tokio::test]
async fn verification_failure_does_not_downgrade() {
    // The resolver returns the wrong torrent for the obfuscated hash. The RC4
    // keys then differ from Alice's, so VC verification fails instead of the
    // handshake silently succeeding.
    let info_hash = id(0x44);
    let (mut a, mut b) = duplex(64 * 1024);
    let receiver = tokio::spawn(async move {
        accept(&mut b, id(0x06), &EncryptionPolicy::require(), |_hash| {
            Some(id(0x45))
        })
        .await
    });

    let _ = initiate(&mut a, info_hash, id(0x07), &EncryptionPolicy::require()).await;

    match receiver.await.unwrap() {
        Err(MseError::InvalidVc) | Err(MseError::HashMismatch) => {}
        Err(err) => panic!("expected a verification failure, got {err:?}"),
        Ok(_) => panic!("verification failure was not detected"),
    }
}

#[test]
fn policy_modes_enforce_never_downgrade() {
    let allow = EncryptionPolicy::allow();
    assert!(allow.allow_plaintext_handshake());
    assert!(allow.allow_encrypted_handshake());
    assert!(!allow.require_encrypted_handshake());

    let require = EncryptionPolicy::require();
    assert!(!require.allow_plaintext_handshake());
    assert!(require.prefer_encrypted_handshake());
    assert!(require.require_encrypted_handshake());

    let stream_require = EncryptionPolicy::new(EncryptionMode::Allow, EncryptionMode::Require);
    assert!(!stream_require.allow_plaintext_handshake());
    assert!(stream_require.prefer_encrypted_handshake());
    assert!(stream_require.require_encrypted_handshake());

    let stream_deny = EncryptionPolicy::new(EncryptionMode::Allow, EncryptionMode::Disabled);
    assert!(stream_deny.allow_plaintext_handshake());
    assert!(stream_deny.allow_encrypted_handshake());
    assert!(!stream_deny.allow_encrypted_stream());

    assert!(!EncryptionPolicy::new(EncryptionMode::Disabled, EncryptionMode::Require).is_valid());

    // Legacy network-config modes map onto the policy, including the invalid
    // deny/require pair which is upgraded to require/require instead of
    // silently downgrading.
    let mapped = EncryptionPolicy::from((
        lt_net::config::EncryptionMode::Deny,
        lt_net::config::EncryptionMode::Require,
    ));
    assert!(mapped.require_encrypted_handshake());
    assert!(!mapped.allow_plaintext_handshake());
}

#[test]
fn verification_values_are_compared_constant_time() {
    assert!(HandshakeEncryption::verify_vc(&[0u8; 8]));
    let mut bad = [0u8; 8];
    bad[7] = 1;
    assert!(!HandshakeEncryption::verify_vc(&bad));

    assert!(constant_time_eq(b"abcdefgh", b"abcdefgh"));
    assert!(!constant_time_eq(b"abcdefgh", b"abcdefgi"));
    assert!(!constant_time_eq(b"short", b"longer"));
}

#[test]
fn skey_verification_is_constant_time() {
    let secret = [7u8; 96];
    let info_hash = [9u8; 20];
    let obfuscated = HandshakeEncryption::obfuscate_skey(&info_hash, &secret);

    assert!(HandshakeEncryption::verify_skey(
        &obfuscated,
        &info_hash,
        &secret
    ));

    let mut tampered = obfuscated;
    tampered[19] ^= 1;
    assert!(!HandshakeEncryption::verify_skey(
        &tampered, &info_hash, &secret
    ));
}

#[test]
fn verification_failure_predicate_disables_fallback() {
    assert!(!MseError::InvalidVc.allows_plaintext_fallback());
    assert!(!MseError::HashMismatch.allows_plaintext_fallback());
    assert!(!MseError::InvalidDh.allows_plaintext_fallback());
    assert!(MseError::PlaintextPeer.allows_plaintext_fallback());
    assert!(MseError::Timeout.allows_plaintext_fallback());
}

/// `AsyncRead`/`AsyncWrite` wrapper that limits every poll to a few bytes.
/// It exercises partial-read handling in the MSE state machine.
struct Chunked<T> {
    inner: T,
    read_max: usize,
    write_max: usize,
}

impl<T> Chunked<T> {
    fn new(inner: T, read_max: usize, write_max: usize) -> Self {
        Self {
            inner,
            read_max,
            write_max,
        }
    }
}

impl<T: AsyncRead + Unpin> AsyncRead for Chunked<T> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let max = self.read_max.min(buf.remaining());
        let filled = {
            let mut limited = buf.take(max);
            match Pin::new(&mut self.inner).poll_read(cx, &mut limited) {
                Poll::Ready(Ok(())) => limited.filled().len(),
                Poll::Ready(Err(err)) => return Poll::Ready(Err(err)),
                Poll::Pending => return Poll::Pending,
            }
        };
        buf.advance(filled);
        Poll::Ready(Ok(()))
    }
}

impl<T: AsyncWrite + Unpin> AsyncWrite for Chunked<T> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let max = self.write_max.min(buf.len());
        Pin::new(&mut self.inner).poll_write(cx, &buf[..max])
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

#[tokio::test]
async fn handshake_survives_partial_reads_and_writes() {
    let info_hash = id(0x55);
    let policy = EncryptionPolicy::require();
    let (a, b) = duplex(64 * 1024);
    // Both directions see at most 3-byte reads/writes.
    let mut a = Chunked::new(a, 3, 3);

    let obfuscated = HashString::new(mse_req2(info_hash.as_bytes()));
    let receiver = tokio::spawn(async move {
        let mut b = Chunked::new(b, 5, 2);
        accept(&mut b, id(0x08), &policy, move |hash| {
            if *hash == obfuscated {
                Some(info_hash)
            } else {
                None
            }
        })
        .await
    });

    let mut initiator = initiate(&mut a, info_hash, id(0x09), &policy)
        .await
        .expect("chunked initiator handshake");
    let mut receiver = receiver.await.unwrap().expect("chunked receiver handshake");

    assert!(initiator.encrypted_stream);
    assert!(receiver.encrypted_stream);

    let mut message = b"partial-read continuity".to_vec();
    let expected = message.clone();
    initiator.encrypt(&mut message);
    receiver.decrypt(&mut message);
    assert_eq!(message, expected);
}
