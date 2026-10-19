use lt_common::hash_string::HashString;
use lt_net::addr::SocketAddress;
use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::encryption::{CryptoMode, EncryptionPolicy, HandshakeEncryption};
use crate::handshake::HandshakeMessage;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HandshakePhase {
    Connecting,
    NegotiatingEncryption,
    ExchangingHandshake,
    Established,
    Failed,
}

pub struct HandshakeSession {
    pub sa: SocketAddress,
    pub is_incoming: bool,
    pub phase: HandshakePhase,
    pub encryption: HandshakeEncryption,
    pub started: Instant,
    pub info_hash: Option<HashString>,
    pub peer_id: Option<HashString>,
    pub peer_handshake: Option<HandshakeMessage>,
    pub crypto_mode: CryptoMode,
}

impl HandshakeSession {
    pub fn new_outgoing(
        sa: SocketAddress,
        policy: EncryptionPolicy,
        info_hash: HashString,
    ) -> Self {
        let encryption = HandshakeEncryption::with_policy(policy);
        // `prefer`/`require` start with MSE; `allow`/`disabled` start with the
        // classic plaintext handshake (with a possible MSE retry on failure).
        let phase = if policy.prefer_encrypted_handshake() {
            HandshakePhase::NegotiatingEncryption
        } else {
            HandshakePhase::ExchangingHandshake
        };

        Self {
            sa,
            is_incoming: false,
            phase,
            encryption,
            started: Instant::now(),
            info_hash: Some(info_hash),
            peer_id: None,
            peer_handshake: None,
            crypto_mode: CryptoMode::PLAINTEXT,
        }
    }

    pub fn new_incoming(sa: SocketAddress, policy: EncryptionPolicy) -> Self {
        let encryption = HandshakeEncryption::with_policy(policy);
        let phase = if policy.allow_encrypted_handshake() {
            HandshakePhase::NegotiatingEncryption
        } else {
            HandshakePhase::ExchangingHandshake
        };
        Self {
            sa,
            is_incoming: true,
            phase,
            encryption,
            started: Instant::now(),
            info_hash: None,
            peer_id: None,
            peer_handshake: None,
            crypto_mode: CryptoMode::PLAINTEXT,
        }
    }

    pub fn is_expired(&self, timeout: Duration) -> bool {
        self.started.elapsed() > timeout
    }

    /// True when post-handshake traffic is RC4 encrypted.
    pub fn is_encrypted(&self) -> bool {
        self.crypto_mode == CryptoMode::ENCRYPTED
    }

    /// True when the connection performed an MSE handshake (legacy
    /// `p.is_obfuscated`). A plaintext MSE stream still counts.
    pub fn is_obfuscated(&self) -> bool {
        self.encryption.is_obfuscated()
    }
}

pub struct HandshakeManager {
    pub policy: EncryptionPolicy,
    pub timeout: Duration,
    pub sessions: HashMap<SocketAddress, HandshakeSession>,
    pub own_peer_id: HashString,
}

impl HandshakeManager {
    pub const DEFAULT_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(30);

    pub fn new(own_peer_id: HashString, policy: EncryptionPolicy) -> Self {
        Self {
            policy,
            timeout: Self::DEFAULT_HANDSHAKE_TIMEOUT,
            sessions: HashMap::new(),
            own_peer_id,
        }
    }

    pub fn start_outgoing(&mut self, sa: SocketAddress, info_hash: HashString) {
        let session = HandshakeSession::new_outgoing(sa.clone(), self.policy, info_hash);
        self.sessions.insert(sa, session);
    }

    pub fn start_incoming(&mut self, sa: SocketAddress) {
        let session = HandshakeSession::new_incoming(sa.clone(), self.policy);
        self.sessions.insert(sa, session);
    }

    pub fn get_session_mut(&mut self, sa: &SocketAddress) -> Option<&mut HandshakeSession> {
        self.sessions.get_mut(sa)
    }

    pub fn remove_session(&mut self, sa: &SocketAddress) {
        self.sessions.remove(sa);
    }

    pub fn cleanup_expired(&mut self) {
        let timeout = self.timeout;
        self.sessions.retain(|_, s| !s.is_expired(timeout));
    }
}
