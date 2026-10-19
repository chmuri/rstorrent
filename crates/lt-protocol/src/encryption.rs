use lt_crypto::dh::{
    mse_deobfuscate_hash, mse_key_a, mse_key_b, mse_req1, mse_req2, mse_req3, DiffieHellman,
    VC_DATA, VC_LENGTH,
};
use lt_crypto::rc4::Rc4;

bitflags::bitflags! {
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
    pub struct CryptoMode: u32 {
        const PLAINTEXT = 0x1;
        const ENCRYPTED = 0x2;
    }
}

/// One half of the legacy `EncryptionPolicy` (handshake or stream mode).
///
/// The names and ordering mirror libtorrent's `encryption_mode`:
/// `DENY`, `ALLOW`, `PREFER`, `REQUIRE`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum EncryptionMode {
    /// Encryption is disabled for this half of the connection.
    Disabled,
    /// Accept encryption when the peer offers it, but do not require it.
    #[default]
    Allow,
    /// Prefer encryption, falling back to plaintext when policy allows it.
    Prefer,
    /// Encryption is mandatory; never downgrade.
    Require,
}

impl EncryptionMode {
    /// Legacy/libtorrent spelling for [`EncryptionMode::Disabled`].
    #[allow(non_upper_case_globals)]
    pub const Deny: Self = Self::Disabled;

    pub fn allows_plaintext(self) -> bool {
        self != Self::Require
    }

    pub fn allows_encrypted(self) -> bool {
        self != Self::Disabled
    }

    pub fn prefers_encrypted(self) -> bool {
        matches!(self, Self::Prefer | Self::Require)
    }

    pub fn requires_encrypted(self) -> bool {
        self == Self::Require
    }
}

impl From<lt_net::config::EncryptionMode> for EncryptionMode {
    fn from(value: lt_net::config::EncryptionMode) -> Self {
        use lt_net::config::EncryptionMode as NetMode;
        match value {
            NetMode::Deny => Self::Disabled,
            NetMode::Allow => Self::Allow,
            NetMode::Prefer => Self::Prefer,
            NetMode::Require => Self::Require,
        }
    }
}

/// Handshake and stream encryption policy.
///
/// This is the Rust equivalent of the legacy `EncryptionPolicy`
/// (`handshake_mode` + `stream_mode`). The same policy object is used for
/// inbound and outbound connections; direction only decides who initiates the
/// MSE handshake.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EncryptionPolicy {
    pub handshake_mode: EncryptionMode,
    pub stream_mode: EncryptionMode,
}

impl Default for EncryptionPolicy {
    fn default() -> Self {
        Self {
            handshake_mode: EncryptionMode::Allow,
            stream_mode: EncryptionMode::Allow,
        }
    }
}

impl EncryptionPolicy {
    pub const fn new(handshake_mode: EncryptionMode, stream_mode: EncryptionMode) -> Self {
        Self {
            handshake_mode,
            stream_mode,
        }
    }

    pub fn allow() -> Self {
        Self::default()
    }

    pub fn prefer() -> Self {
        Self::new(EncryptionMode::Prefer, EncryptionMode::Prefer)
    }

    pub fn require() -> Self {
        Self::new(EncryptionMode::Require, EncryptionMode::Require)
    }

    pub fn disabled() -> Self {
        Self::new(EncryptionMode::Disabled, EncryptionMode::Disabled)
    }

    /// Builds a policy from the legacy pair of modes.
    ///
    /// `handshake = DENY` combined with `stream = REQUIRE` is impossible: a
    /// plaintext handshake can never carry an encrypted stream. The legacy
    /// configuration rejects this pair; we defensively treat an invalid pair
    /// as "require an encrypted handshake".
    pub fn from_modes(handshake_mode: EncryptionMode, stream_mode: EncryptionMode) -> Self {
        if handshake_mode == EncryptionMode::Disabled && stream_mode == EncryptionMode::Require {
            return Self::new(EncryptionMode::Require, stream_mode);
        }

        Self::new(handshake_mode, stream_mode)
    }

    pub fn is_valid(&self) -> bool {
        !(self.handshake_mode == EncryptionMode::Disabled
            && self.stream_mode == EncryptionMode::Require)
    }

    pub fn handshake_mode(&self) -> EncryptionMode {
        self.handshake_mode
    }

    pub fn stream_mode(&self) -> EncryptionMode {
        self.stream_mode
    }

    /// Whether an outgoing plaintext (classic BitTorrent) handshake is
    /// permitted. A plaintext handshake implies a plaintext stream, so a
    /// `stream = REQUIRE` policy also forbids it.
    pub fn allow_plaintext_handshake(&self) -> bool {
        self.is_valid()
            && self.handshake_mode.allows_plaintext()
            && self.stream_mode.allows_plaintext()
    }

    pub fn allow_encrypted_handshake(&self) -> bool {
        self.handshake_mode.allows_encrypted()
    }

    /// Whether the MSE handshake should be attempted before the plaintext
    /// handshake on outgoing connections.
    pub fn prefer_encrypted_handshake(&self) -> bool {
        if !self.handshake_mode.allows_encrypted() {
            return false;
        }
        self.handshake_mode.prefers_encrypted() || self.stream_mode.prefers_encrypted()
    }

    /// Whether encryption is mandatory for the handshake *or* the stream.
    /// Downgrading to plaintext is forbidden when this returns `true`.
    pub fn require_encrypted_handshake(&self) -> bool {
        self.handshake_mode.requires_encrypted() || self.stream_mode.requires_encrypted()
    }

    pub fn allow_plaintext_stream(&self) -> bool {
        self.stream_mode.allows_plaintext()
    }

    pub fn allow_encrypted_stream(&self) -> bool {
        self.stream_mode.allows_encrypted()
    }

    /// Whether the receiver should select `crypto_encrypted` when the peer
    /// offers both stream modes.
    pub fn prefer_encrypted_stream(&self) -> bool {
        self.stream_mode.prefers_encrypted()
    }

    /// Legacy name: `stream = DENY` requires a plaintext stream.
    pub fn require_plaintext_stream(&self) -> bool {
        self.stream_mode == EncryptionMode::Disabled
    }

    pub fn require_encrypted_stream(&self) -> bool {
        self.stream_mode.requires_encrypted()
    }
}

impl
    From<(
        lt_net::config::EncryptionMode,
        lt_net::config::EncryptionMode,
    )> for EncryptionPolicy
{
    fn from(
        (handshake, stream): (
            lt_net::config::EncryptionMode,
            lt_net::config::EncryptionMode,
        ),
    ) -> Self {
        Self::from_modes(handshake.into(), stream.into())
    }
}

/// Constant-time byte-slice comparison.
///
/// Used for VC/`req1`/`req2`/`req3` checks so verification failures do not
/// leak how many prefix bytes matched. The length check is not secret: all
/// MSE verification values have a public, fixed length.
#[inline]
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }

    let mut diff = 0u8;
    for (lhs, rhs) in a.iter().zip(b.iter()) {
        diff |= lhs ^ rhs;
    }
    diff == 0
}

pub struct HandshakeEncryption {
    pub dh: DiffieHellman,
    pub policy: EncryptionPolicy,
    pub crypto_selected: Option<CryptoMode>,
    pub cipher_enc: Option<Rc4>,
    pub cipher_dec: Option<Rc4>,
    pub shared_secret: Option<[u8; 96]>,
    /// True once an MSE handshake completed, even if the negotiated stream is
    /// plaintext (the legacy `EncryptionInfo::m_obfuscated` flag).
    pub is_obfuscated: bool,
}

impl Default for HandshakeEncryption {
    fn default() -> Self {
        Self::new()
    }
}

impl HandshakeEncryption {
    pub fn new() -> Self {
        Self {
            dh: DiffieHellman::new_mse(),
            policy: EncryptionPolicy::default(),
            crypto_selected: None,
            cipher_enc: None,
            cipher_dec: None,
            shared_secret: None,
            is_obfuscated: false,
        }
    }

    pub fn with_policy(policy: EncryptionPolicy) -> Self {
        let mut s = Self::new();
        s.policy = policy;
        s
    }

    pub fn is_valid(&self) -> bool {
        self.dh.is_valid()
    }

    pub fn pub_key(&self) -> Vec<u8> {
        self.dh.pub_key_bytes()
    }

    pub fn compute_secret(&mut self, remote_pub: &[u8]) -> bool {
        if remote_pub.len() != 96 {
            return false;
        }
        if self.dh.compute_secret(remote_pub) {
            if let Some(sec) = self.dh.secret() {
                if sec.len() == 96 {
                    let mut s = [0u8; 96];
                    s.copy_from_slice(sec);
                    self.shared_secret = Some(s);
                    return true;
                }
            }
        }
        false
    }

    pub fn sync_hash(&self) -> Option<[u8; 20]> {
        self.shared_secret.as_ref().map(|s| mse_req1(s))
    }

    pub fn obfuscate_skey(info_hash: &[u8; 20], secret: &[u8; 96]) -> [u8; 20] {
        let req2 = mse_req2(info_hash);
        let req3 = mse_req3(secret);
        let mut obfuscated = [0u8; 20];
        for i in 0..20 {
            obfuscated[i] = req2[i] ^ req3[i];
        }
        obfuscated
    }

    pub fn deobfuscate_skey(&self, obfuscated: &mut [u8; 20]) -> bool {
        if let Some(ref sec) = self.shared_secret {
            mse_deobfuscate_hash(obfuscated, sec);
            true
        } else {
            false
        }
    }

    /// Constant-time verification of the MSE verification constant.
    pub fn verify_vc(vc: &[u8]) -> bool {
        constant_time_eq(vc, &VC_DATA)
    }

    /// Constant-time verification of an obfuscated SKEY (`req2 ^ req3`).
    pub fn verify_skey(obfuscated: &[u8; 20], info_hash: &[u8; 20], secret: &[u8; 96]) -> bool {
        constant_time_eq(obfuscated, &Self::obfuscate_skey(info_hash, secret))
    }

    pub fn setup_ciphers(&mut self, is_incoming: bool, info_hash: &[u8; 20]) -> bool {
        let secret = match self.shared_secret {
            Some(ref s) => *s,
            None => return false,
        };

        let key_a = mse_key_a(&secret, info_hash);
        let key_b = mse_key_b(&secret, info_hash);

        let (enc_key, dec_key) = if is_incoming {
            // Incoming (Bob): encrypt with keyB, decrypt with keyA
            (key_b, key_a)
        } else {
            // Outgoing (Alice): encrypt with keyA, decrypt with keyB
            (key_a, key_b)
        };

        let mut enc = Rc4::new(&enc_key);
        let mut dec = Rc4::new(&dec_key);

        // Crucial security requirement: discard first 1024 bytes of stream
        enc.discard(1024);
        dec.discard(1024);

        self.cipher_enc = Some(enc);
        self.cipher_dec = Some(dec);
        true
    }

    pub fn encrypt(&mut self, data: &mut [u8]) {
        if let Some(ref mut c) = self.cipher_enc {
            c.crypt(data);
        }
    }

    pub fn decrypt(&mut self, data: &mut [u8]) {
        if let Some(ref mut c) = self.cipher_dec {
            c.crypt(data);
        }
    }

    pub fn is_stream_encrypted(&self) -> bool {
        self.crypto_selected == Some(CryptoMode::ENCRYPTED)
    }

    pub fn is_stream_plaintext(&self) -> bool {
        self.crypto_selected == Some(CryptoMode::PLAINTEXT)
    }

    /// True when the connection completed an MSE handshake (with either
    /// stream mode). Matches the legacy `m_obfuscated` flag.
    pub fn is_obfuscated(&self) -> bool {
        self.is_obfuscated
    }

    pub fn vc() -> [u8; VC_LENGTH] {
        VC_DATA
    }
}
