//! MSE/PE encrypted handshake (BitTorrent peer encryption).
//!
//! Implements both roles of the MSE handshake:
//! - [`initiate`] (Alice, outgoing),
//! - [`accept`] (Bob, incoming).
//!
//! The flow follows the legacy `HandshakeEncryption`/`Handshake` state machine:
//! DH exchange, `req1` synchronisation, obfuscated `SKEY`, RC4 key derivation
//! with the 1024-byte discard, VC verification and crypto-mode negotiation.
//!
//! Wire flow (matching `old/libtorrent/src/protocol/handshake.cc`):
//!
//! ```text
//! Alice -> Bob: Ya (96) | PadA (0..512)
//! Bob   -> Alice: Yb (96) | PadB (0..512)
//! Alice -> Bob: req1 (20) | SKEY^req3 (20) |
//!               ENC(VC | crypto_provide | len(PadC) | PadC | len(IA) | IA)
//! Bob   -> Alice: ENC(VC | crypto_select | len(PadD) | PadD) | [ENC](BT handshake)
//! ```
//!
//! The initial BitTorrent handshake (`IA`) is always RC4 encrypted, even when
//! the negotiated *stream* mode is plaintext. After the handshake, RC4 is only
//! used on the wire when `crypto_select == crypto_encrypted`.

use std::time::Duration;

use lt_common::hash_string::HashString;
use lt_crypto::dh::{mse_req1, DiffieHellman};
use lt_crypto::rc4::Rc4;
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::time::timeout;

use crate::encryption::{constant_time_eq, CryptoMode, EncryptionPolicy, HandshakeEncryption};
use crate::handshake::{HandshakeError, HandshakeMessage, BT_PROTOCOL_STR};

const MSE_TIMEOUT: Duration = Duration::from_secs(15);
/// Legacy `Handshake::enc_pad_size`: pad lengths are in `0..MAX_PAD`.
const MAX_PAD: usize = 512;
const DH_KEY_LENGTH: usize = 96;
const INFO_HASH_LENGTH: usize = 20;
const BT_HANDSHAKE_LENGTH: usize = 68;
const VC_LENGTH: usize = 8;
const CRYPTO_FIELDS_LENGTH: usize = VC_LENGTH + 4 + 2;

const CRYPTO_PLAINTEXT: u32 = 0x1;
const CRYPTO_ENCRYPTED: u32 = 0x2;

#[derive(Debug, Error)]
pub enum MseError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("MSE handshake timed out")]
    Timeout,
    #[error("MSE handshake is disabled by policy")]
    EncryptionDisabled,
    /// The peer sent a plaintext BitTorrent handshake where a DH key was
    /// expected. The caller may reconnect and retry in plaintext if policy
    /// allows it.
    #[error("Peer started a plaintext handshake")]
    PlaintextPeer,
    #[error("Invalid Diffie-Hellman public key")]
    InvalidDh,
    #[error("Could not derive the shared secret")]
    Secret,
    #[error("Unknown torrent for obfuscated info-hash {0}")]
    UnknownTorrent(String),
    #[error("Info-hash mismatch after MSE handshake")]
    HashMismatch,
    #[error("Invalid verification constant")]
    InvalidVc,
    #[error("Peer does not support the required encryption mode")]
    EncryptionRequired,
    #[error("Peer selected a stream mode that policy disallows")]
    EncryptionNotAllowed,
    #[error("Peer selected an invalid crypto mode")]
    InvalidCryptoMode,
    #[error("MSE protocol error: {0}")]
    Protocol(String),
    #[error(transparent)]
    Handshake(#[from] HandshakeError),
}

impl MseError {
    /// Whether retrying the same peer with a plaintext handshake is safe.
    ///
    /// Mirrors the legacy `retry_plaintext_handshake()` guard: the retry is
    /// only armed when the peer did not complete a verifiable MSE key/sync
    /// exchange. Verification failures deliberately do not fall back, so a
    /// MITM cannot force a downgrade by corrupting the VC. The caller also
    /// refuses every fallback when the policy requires encryption.
    pub fn allows_plaintext_fallback(&self) -> bool {
        matches!(
            self,
            MseError::Io(_) | MseError::Timeout | MseError::PlaintextPeer
        )
    }
}

/// Result of a successful MSE handshake.
pub struct MseSession {
    pub info_hash: HashString,
    pub peer_id: HashString,
    pub peer_handshake: HandshakeMessage,
    pub encryption: HandshakeEncryption,
    /// True when the negotiated *stream* is RC4 encrypted.
    pub encrypted_stream: bool,
}

impl MseSession {
    pub fn crypto_mode(&self) -> Option<CryptoMode> {
        self.encryption.crypto_selected
    }

    /// True when post-handshake traffic is RC4 encrypted.
    pub fn is_encrypted(&self) -> bool {
        self.encrypted_stream
    }

    /// True when the connection completed an MSE handshake. A plaintext MSE
    /// stream still counts as obfuscated (legacy `p.is_obfuscated`).
    pub fn is_obfuscated(&self) -> bool {
        self.encryption.is_obfuscated()
    }

    /// Returns the RC4 ciphers that apply to post-handshake traffic.
    ///
    /// Plaintext MSE streams intentionally return `None` even though the MSE
    /// handshake used RC4 internally: the negotiated stream is unencrypted.
    pub fn take_ciphers(&mut self) -> (Option<Rc4>, Option<Rc4>) {
        if self.encrypted_stream {
            (
                self.encryption.cipher_enc.take(),
                self.encryption.cipher_dec.take(),
            )
        } else {
            (None, None)
        }
    }

    pub fn encrypt(&mut self, data: &mut [u8]) {
        if self.encrypted_stream {
            self.encryption.encrypt(data);
        }
    }

    pub fn decrypt(&mut self, data: &mut [u8]) {
        if self.encrypted_stream {
            self.encryption.decrypt(data);
        }
    }
}

async fn write_all<S: AsyncWrite + Unpin>(stream: &mut S, data: &[u8]) -> Result<(), MseError> {
    match timeout(MSE_TIMEOUT, stream.write_all(data)).await {
        Ok(Ok(())) => Ok(()),
        Ok(Err(err)) => Err(MseError::Io(err)),
        Err(_) => Err(MseError::Timeout),
    }
}

async fn read_exact<S: AsyncRead + Unpin>(stream: &mut S, data: &mut [u8]) -> Result<(), MseError> {
    match timeout(MSE_TIMEOUT, stream.read_exact(data)).await {
        Ok(Ok(_)) => Ok(()),
        Ok(Err(err)) => Err(MseError::Io(err)),
        Err(_) => Err(MseError::Timeout),
    }
}

/// Scans for a fixed-size synchronisation value while preserving partial
/// reads. Each comparison is constant time; the number of bytes scanned is
/// bounded by the caller (pad size + sync length).
async fn read_until_pattern<S: AsyncRead + Unpin>(
    stream: &mut S,
    needle: &[u8],
    max_bytes: usize,
) -> Result<(), MseError> {
    if needle.is_empty() || needle.len() > 20 {
        return Err(MseError::Protocol("invalid sync pattern".into()));
    }

    let mut window = Vec::with_capacity(needle.len());
    let mut byte = [0u8; 1];
    let deadline = tokio::time::Instant::now() + MSE_TIMEOUT;

    for _ in 0..max_bytes {
        match tokio::time::timeout_at(deadline, stream.read_exact(&mut byte)).await {
            Ok(Ok(_)) => {}
            Ok(Err(err)) => return Err(MseError::Io(err)),
            Err(_) => return Err(MseError::Timeout),
        }

        if window.len() == needle.len() {
            window.remove(0);
        }
        window.push(byte[0]);

        if window.len() == needle.len() && constant_time_eq(&window, needle) {
            return Ok(());
        }
    }

    Err(MseError::Protocol(
        "MSE synchronisation sequence not found".into(),
    ))
}

fn random_padding() -> Result<Vec<u8>, MseError> {
    let mut length_bytes = [0u8; 2];
    getrandom::getrandom(&mut length_bytes)
        .map_err(|err| MseError::Protocol(format!("getrandom failed: {err}")))?;
    // Legacy: random() % enc_pad_size, i.e. strictly less than 512.
    let length = u16::from_be_bytes(length_bytes) as usize % MAX_PAD;

    let mut padding = vec![0u8; length];
    getrandom::getrandom(&mut padding)
        .map_err(|err| MseError::Protocol(format!("getrandom failed: {err}")))?;
    Ok(padding)
}

async fn read_bounded_padding<S: AsyncRead + Unpin>(
    stream: &mut S,
    length: usize,
    encryption: Option<&mut HandshakeEncryption>,
) -> Result<(), MseError> {
    if length > MAX_PAD {
        return Err(MseError::Protocol(format!(
            "padding length {length} exceeds {MAX_PAD}"
        )));
    }
    if length == 0 {
        return Ok(());
    }

    // The bound is checked before allocating.
    let mut padding = vec![0u8; length];
    read_exact(stream, &mut padding).await?;
    if let Some(encryption) = encryption {
        encryption.decrypt(&mut padding);
    }
    Ok(())
}

fn crypto_provide(policy: &EncryptionPolicy) -> Result<u32, MseError> {
    let mut provide = 0;
    if policy.allow_plaintext_stream() {
        provide |= CRYPTO_PLAINTEXT;
    }
    if policy.allow_encrypted_stream() {
        provide |= CRYPTO_ENCRYPTED;
    }

    if provide == 0 {
        return Err(MseError::EncryptionDisabled);
    }
    Ok(provide)
}

/// Receiver-side selection, matching `Handshake::read_encryption_negotiation`.
fn select_mode(provide: u32, policy: &EncryptionPolicy) -> Result<u32, MseError> {
    let mut can_plaintext = provide & CRYPTO_PLAINTEXT != 0;
    let mut can_encrypted = provide & CRYPTO_ENCRYPTED != 0;

    if !can_plaintext && !can_encrypted {
        return Err(MseError::InvalidCryptoMode);
    }

    // Ignore modes the local policy forbids.
    if !policy.allow_plaintext_stream() {
        can_plaintext = false;
    }
    if !policy.allow_encrypted_stream() {
        can_encrypted = false;
    }

    if !can_plaintext && !can_encrypted {
        return if policy.require_encrypted_stream() {
            Err(MseError::EncryptionRequired)
        } else {
            Err(MseError::EncryptionNotAllowed)
        };
    }

    if can_encrypted && (policy.prefer_encrypted_stream() || !can_plaintext) {
        Ok(CRYPTO_ENCRYPTED)
    } else {
        Ok(CRYPTO_PLAINTEXT)
    }
}

/// Initiator-side validation of Bob's `crypto_select`.
fn validate_selection(
    selected: u32,
    provide: u32,
    policy: &EncryptionPolicy,
) -> Result<(), MseError> {
    match selected {
        CRYPTO_PLAINTEXT if policy.allow_plaintext_stream() && provide & CRYPTO_PLAINTEXT != 0 => {
            Ok(())
        }
        CRYPTO_ENCRYPTED if policy.allow_encrypted_stream() && provide & CRYPTO_ENCRYPTED != 0 => {
            Ok(())
        }
        CRYPTO_PLAINTEXT => Err(MseError::EncryptionRequired),
        CRYPTO_ENCRYPTED => Err(MseError::EncryptionNotAllowed),
        _ => Err(MseError::InvalidCryptoMode),
    }
}

fn shared_secret(encryption: &HandshakeEncryption) -> Result<[u8; 96], MseError> {
    encryption.shared_secret.ok_or(MseError::Secret)
}

fn looks_like_plaintext_handshake(bytes: &[u8]) -> bool {
    bytes.len() >= INFO_HASH_LENGTH
        && bytes[0] == 19
        && constant_time_eq(&bytes[1..20], BT_PROTOCOL_STR)
}

/// Runs the initiating (outgoing) side of the MSE handshake.
pub async fn initiate<S>(
    stream: &mut S,
    info_hash: HashString,
    peer_id: HashString,
    policy: &EncryptionPolicy,
) -> Result<MseSession, MseError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    if !policy.allow_encrypted_handshake() {
        return Err(MseError::EncryptionDisabled);
    }
    if !policy.is_valid() {
        return Err(MseError::EncryptionDisabled);
    }

    let mut encryption = HandshakeEncryption::with_policy(*policy);

    // 1. Ya + PadA.
    let ya = encryption.pub_key();
    write_all(stream, &ya).await?;
    write_all(stream, &random_padding()?).await?;

    // 2. Yb (PadB remains in the stream and is skipped by the VC sync scan).
    // Read the first 20 bytes separately so a peer that answers with a
    // plaintext handshake is detected immediately instead of waiting for 96
    // bytes that will never arrive.
    let mut yb = [0u8; DH_KEY_LENGTH];
    read_exact(stream, &mut yb[..INFO_HASH_LENGTH]).await?;

    // Some clients send a plaintext handshake immediately instead of
    // speaking MSE. Report it so the caller can reconnect in plaintext.
    if looks_like_plaintext_handshake(&yb[..INFO_HASH_LENGTH]) {
        return Err(MseError::PlaintextPeer);
    }

    read_exact(stream, &mut yb[INFO_HASH_LENGTH..]).await?;

    if !encryption.compute_secret(&yb) {
        return Err(MseError::InvalidDh);
    }

    let secret = shared_secret(&encryption)?;
    let info_bytes = *info_hash.as_bytes();

    if !encryption.setup_ciphers(false, &info_bytes) {
        return Err(MseError::Secret);
    }

    // 3. req1 + obfuscated SKEY + encrypted negotiation + initial payload.
    let req1 = mse_req1(&secret);
    let obfuscated = HandshakeEncryption::obfuscate_skey(&info_bytes, &secret);
    let bt_handshake = HandshakeMessage::new(info_hash, peer_id).encode();

    let provide = crypto_provide(policy)?;

    let mut negotiation = Vec::with_capacity(CRYPTO_FIELDS_LENGTH + 2 + BT_HANDSHAKE_LENGTH);
    negotiation.extend_from_slice(&HandshakeEncryption::vc());
    negotiation.extend_from_slice(&provide.to_be_bytes());
    negotiation.extend_from_slice(&0u16.to_be_bytes()); // len(PadC)
    negotiation.extend_from_slice(&(BT_HANDSHAKE_LENGTH as u16).to_be_bytes());
    negotiation.extend_from_slice(&bt_handshake);
    encryption.encrypt(&mut negotiation);

    let mut block = Vec::with_capacity(40 + negotiation.len());
    block.extend_from_slice(&req1);
    block.extend_from_slice(&obfuscated);
    block.extend_from_slice(&negotiation);
    write_all(stream, &block).await?;

    // 4. B replies with PadB, ENC(VC), ENC(crypto_select), ENC(len(PadD)).
    //    Compute the expected VC ciphertext without advancing the real
    //    decrypt cipher; the sync scan consumes the preceding pad.
    let vc_ciphertext = {
        let mut probe = encryption
            .cipher_dec
            .as_ref()
            .ok_or(MseError::Secret)?
            .clone();
        let mut vc = HandshakeEncryption::vc();
        probe.crypt(&mut vc);
        vc
    };

    read_until_pattern(stream, &vc_ciphertext, MAX_PAD + VC_LENGTH).await?;

    let mut vc = vc_ciphertext;
    encryption.decrypt(&mut vc);
    if !HandshakeEncryption::verify_vc(&vc) {
        return Err(MseError::InvalidVc);
    }

    let mut selection = [0u8; 6];
    read_exact(stream, &mut selection).await?;
    encryption.decrypt(&mut selection);

    let selected = u32::from_be_bytes(
        selection[0..4]
            .try_into()
            .map_err(|_| MseError::Protocol("bad crypto_select".into()))?,
    );
    let pad_length = u16::from_be_bytes(
        selection[4..6]
            .try_into()
            .map_err(|_| MseError::Protocol("bad pad length".into()))?,
    ) as usize;

    if pad_length > MAX_PAD {
        return Err(MseError::Protocol("PadD too large".into()));
    }

    validate_selection(selected, provide, policy)?;

    // Bob encrypts PadD even when the stream is plaintext.
    read_bounded_padding(stream, pad_length, Some(&mut encryption)).await?;

    // 5. B's BT handshake: encrypted only when RC4 was selected.
    let mut peer_handshake_bytes = [0u8; BT_HANDSHAKE_LENGTH];
    read_exact(stream, &mut peer_handshake_bytes).await?;
    if selected == CRYPTO_ENCRYPTED {
        encryption.decrypt(&mut peer_handshake_bytes);
    }

    let peer_handshake = HandshakeMessage::decode(&peer_handshake_bytes)?;
    if peer_handshake.info_hash != info_hash {
        return Err(MseError::HashMismatch);
    }

    let encrypted_stream = selected == CRYPTO_ENCRYPTED;
    encryption.crypto_selected = Some(if encrypted_stream {
        CryptoMode::ENCRYPTED
    } else {
        CryptoMode::PLAINTEXT
    });
    encryption.is_obfuscated = true;
    if !encrypted_stream {
        // Legacy `EncryptionInfo::set_obfuscated`: the stream is plaintext,
        // so drop the RC4 state. The MSE handshake itself was still
        // obfuscated.
        encryption.cipher_enc = None;
        encryption.cipher_dec = None;
    }

    Ok(MseSession {
        info_hash,
        peer_id: peer_handshake.peer_id,
        peer_handshake,
        encryption,
        encrypted_stream,
    })
}

/// Runs the accepting (incoming) side of the MSE handshake.
///
/// `resolve` is called with the deobfuscated value of `SKEY`, which is
/// `SHA1("req2" + info_hash)`. It must return the real info-hash when the
/// session manages that torrent.
pub async fn accept<S, F>(
    stream: &mut S,
    our_peer_id: HashString,
    policy: &EncryptionPolicy,
    resolve: F,
) -> Result<MseSession, MseError>
where
    S: AsyncRead + AsyncWrite + Unpin,
    F: Fn(&HashString) -> Option<HashString>,
{
    if !policy.allow_encrypted_handshake() {
        return Err(MseError::EncryptionDisabled);
    }
    if !policy.is_valid() {
        return Err(MseError::EncryptionDisabled);
    }

    let mut encryption = HandshakeEncryption::with_policy(*policy);

    // 1. Ya + PadA (we only need Ya; PadA stays in the stream). Read the
    // first 20 bytes separately to detect a plaintext handshake early.
    let mut ya = [0u8; DH_KEY_LENGTH];
    read_exact(stream, &mut ya[..INFO_HASH_LENGTH]).await?;

    // Defensive: if a peer sent a plaintext handshake despite the caller
    // choosing the MSE path, tell the caller instead of treating it as a key.
    if looks_like_plaintext_handshake(&ya[..INFO_HASH_LENGTH]) {
        return Err(MseError::PlaintextPeer);
    }

    read_exact(stream, &mut ya[INFO_HASH_LENGTH..]).await?;

    if !encryption.compute_secret(&ya) {
        return Err(MseError::InvalidDh);
    }

    let secret = shared_secret(&encryption)?;

    // 2. Yb + PadB.
    let yb = encryption.pub_key();
    write_all(stream, &yb).await?;
    write_all(stream, &random_padding()?).await?;

    // 3. Wait for req1 (skipping PadA), then read the obfuscated SKEY.
    read_until_pattern(stream, &mse_req1(&secret), MAX_PAD + INFO_HASH_LENGTH).await?;

    let mut obfuscated = [0u8; INFO_HASH_LENGTH];
    read_exact(stream, &mut obfuscated).await?;

    if !encryption.deobfuscate_skey(&mut obfuscated) {
        return Err(MseError::Secret);
    }

    let obfuscated_info_hash = HashString::new(obfuscated);
    let info_hash = resolve(&obfuscated_info_hash)
        .ok_or_else(|| MseError::UnknownTorrent(obfuscated_info_hash.to_hex()))?;

    if !encryption.setup_ciphers(true, info_hash.as_bytes()) {
        return Err(MseError::Secret);
    }

    // 4. ENC(VC + crypto_provide + len(PadC)).
    let mut negotiation = [0u8; CRYPTO_FIELDS_LENGTH];
    read_exact(stream, &mut negotiation).await?;
    encryption.decrypt(&mut negotiation);

    if !HandshakeEncryption::verify_vc(&negotiation[0..VC_LENGTH]) {
        return Err(MseError::InvalidVc);
    }

    let provide = u32::from_be_bytes(
        negotiation[VC_LENGTH..VC_LENGTH + 4]
            .try_into()
            .map_err(|_| MseError::Protocol("bad crypto_provide".into()))?,
    );
    let pad_length = u16::from_be_bytes(
        negotiation[VC_LENGTH + 4..CRYPTO_FIELDS_LENGTH]
            .try_into()
            .map_err(|_| MseError::Protocol("bad PadC length".into()))?,
    ) as usize;

    if pad_length > MAX_PAD {
        return Err(MseError::Protocol("PadC too large".into()));
    }

    let selected = select_mode(provide, policy)?;

    // Send ENC(VC + crypto_select + len(PadD)) immediately after selecting,
    // like the legacy state machine. This lets peers that wait for the
    // negotiation reply before finishing their initial payload proceed.
    let mut reply = [0u8; CRYPTO_FIELDS_LENGTH];
    reply[0..VC_LENGTH].copy_from_slice(&HandshakeEncryption::vc());
    reply[VC_LENGTH..VC_LENGTH + 4].copy_from_slice(&selected.to_be_bytes());
    reply[VC_LENGTH + 4..CRYPTO_FIELDS_LENGTH].copy_from_slice(&0u16.to_be_bytes());
    encryption.encrypt(&mut reply);
    write_all(stream, &reply).await?;

    // 5. PadC, len(IA) and IA (Alice's BT handshake). All of it is encrypted
    //    for the duration of the MSE handshake, regardless of stream mode.
    read_bounded_padding(stream, pad_length, Some(&mut encryption)).await?;

    let mut ia_length_bytes = [0u8; 2];
    read_exact(stream, &mut ia_length_bytes).await?;
    encryption.decrypt(&mut ia_length_bytes);
    let ia_length = u16::from_be_bytes(ia_length_bytes) as usize;

    if ia_length > BT_HANDSHAKE_LENGTH {
        return Err(MseError::Protocol(format!(
            "initial payload length {ia_length} exceeds {BT_HANDSHAKE_LENGTH}"
        )));
    }

    let mut peer_handshake_bytes = [0u8; BT_HANDSHAKE_LENGTH];
    if ia_length > 0 {
        read_exact(stream, &mut peer_handshake_bytes[..ia_length]).await?;
        encryption.decrypt(&mut peer_handshake_bytes[..ia_length]);
    }

    // The legacy state machine tolerates a short initial payload and resumes
    // the BT handshake in READ_INFO. After IA the encryption mode applies:
    // encrypted streams continue to be decrypted, plaintext streams are not.
    if ia_length < BT_HANDSHAKE_LENGTH {
        read_exact(stream, &mut peer_handshake_bytes[ia_length..]).await?;
        if selected == CRYPTO_ENCRYPTED {
            encryption.decrypt(&mut peer_handshake_bytes[ia_length..]);
        }
    }

    let peer_handshake = HandshakeMessage::decode(&peer_handshake_bytes)?;
    if peer_handshake.info_hash != info_hash {
        return Err(MseError::HashMismatch);
    }

    // 6. Our BT handshake. It is encrypted only when RC4 was selected.
    let encrypted_stream = selected == CRYPTO_ENCRYPTED;
    let mut handshake = HandshakeMessage::new(info_hash, our_peer_id).encode();
    if encrypted_stream {
        encryption.encrypt(&mut handshake);
    }
    write_all(stream, &handshake).await?;

    encryption.crypto_selected = Some(if encrypted_stream {
        CryptoMode::ENCRYPTED
    } else {
        CryptoMode::PLAINTEXT
    });
    encryption.is_obfuscated = true;
    if !encrypted_stream {
        encryption.cipher_enc = None;
        encryption.cipher_dec = None;
    }

    Ok(MseSession {
        info_hash,
        peer_id: peer_handshake.peer_id,
        peer_handshake,
        encryption,
        encrypted_stream,
    })
}

/// Computes `SHA1("req2" + info_hash)`, the value carried (obfuscated) in
/// `SKEY` and used to identify a torrent before the real info-hash is known.
pub fn obfuscated_info_hash(info_hash: &HashString) -> HashString {
    HashString::new(lt_crypto::dh::mse_req2(info_hash.as_bytes()))
}

/// Constant-time comparison of an advertised obfuscated info-hash with the
/// local `SHA1("req2" + info_hash)` value. Used by inbound resolvers so the
/// torrent lookup does not leak a matching prefix.
pub fn matches_obfuscated_info_hash(advertised: &HashString, info_hash: &HashString) -> bool {
    let expected = obfuscated_info_hash(info_hash);
    constant_time_eq(advertised.as_bytes(), expected.as_bytes())
}

/// Number of DH public-key bytes exchanged by MSE.
pub const MSE_KEY_LENGTH: usize = DH_KEY_LENGTH;

/// Reference to the MSE prime size (also exposed by `lt-crypto`).
pub fn dh_key_length() -> usize {
    DiffieHellman::new_mse().size()
}

/// Exposes the RC4 type used by callers that want to pre-warm ciphers.
pub type MseRc4 = Rc4;
