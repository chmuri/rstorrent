use lt_common::hash_string::HashString;
use thiserror::Error;

pub const BT_PROTOCOL_STR: &[u8] = b"BitTorrent protocol";
pub const BT_PROTOCOL_LEN: u8 = 19;
pub const BT_HANDSHAKE_SIZE: usize = 68;

/// Returns true when `bytes` begins with a complete plaintext BitTorrent
/// handshake preamble (`19` + `"BitTorrent protocol"`).
///
/// Used by listeners to distinguish a classic handshake from an MSE Diffie-
/// Hellman key, whose first byte is uniformly random.
pub fn is_plaintext_handshake_prefix(bytes: &[u8]) -> bool {
    bytes.len() >= 20 && bytes[0] == BT_PROTOCOL_LEN && &bytes[1..20] == BT_PROTOCOL_STR
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum HandshakeError {
    #[error("Handshake too short: required 68 bytes, got {0}")]
    TooShort(usize),
    #[error("Invalid protocol length: expected 19, got {0}")]
    InvalidProtocolLen(u8),
    #[error("Invalid protocol string: {0:?}")]
    InvalidProtocolStr(Vec<u8>),
    #[error("Info hash mismatch")]
    InfoHashMismatch,
    #[error("Connected to self")]
    SelfConnection,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HandshakeMessage {
    pub reserved: [u8; 8],
    pub info_hash: HashString,
    pub peer_id: HashString,
}

impl HandshakeMessage {
    pub fn new(info_hash: HashString, peer_id: HashString) -> Self {
        let mut reserved = [0u8; 8];
        // Enable BEP 10 LTEP by default (byte 5, bit 0x10)
        reserved[5] |= 0x10;
        // Enable BEP 5 DHT by default (byte 7, bit 0x01)
        reserved[7] |= 0x01;
        // Enable BEP 6 Fast extension (byte 7, bit 0x04)
        reserved[7] |= 0x04;

        Self {
            reserved,
            info_hash,
            peer_id,
        }
    }

    pub fn with_reserved(reserved: [u8; 8], info_hash: HashString, peer_id: HashString) -> Self {
        Self {
            reserved,
            info_hash,
            peer_id,
        }
    }

    pub fn has_ltep(&self) -> bool {
        (self.reserved[5] & 0x10) != 0
    }

    pub fn has_dht(&self) -> bool {
        (self.reserved[7] & 0x01) != 0
    }

    pub fn has_fast(&self) -> bool {
        (self.reserved[7] & 0x04) != 0
    }

    pub fn encode(&self) -> [u8; BT_HANDSHAKE_SIZE] {
        let mut buf = [0u8; BT_HANDSHAKE_SIZE];
        buf[0] = BT_PROTOCOL_LEN;
        buf[1..20].copy_from_slice(BT_PROTOCOL_STR);
        buf[20..28].copy_from_slice(&self.reserved);
        buf[28..48].copy_from_slice(self.info_hash.as_bytes());
        buf[48..68].copy_from_slice(self.peer_id.as_bytes());
        buf
    }

    pub fn decode(data: &[u8]) -> Result<Self, HandshakeError> {
        if data.len() < BT_HANDSHAKE_SIZE {
            return Err(HandshakeError::TooShort(data.len()));
        }

        if data[0] != BT_PROTOCOL_LEN {
            return Err(HandshakeError::InvalidProtocolLen(data[0]));
        }

        if &data[1..20] != BT_PROTOCOL_STR {
            return Err(HandshakeError::InvalidProtocolStr(data[1..20].to_vec()));
        }

        let mut reserved = [0u8; 8];
        reserved.copy_from_slice(&data[20..28]);

        let mut info_bytes = [0u8; 20];
        info_bytes.copy_from_slice(&data[28..48]);
        let info_hash = HashString::new(info_bytes);

        let mut peer_bytes = [0u8; 20];
        peer_bytes.copy_from_slice(&data[48..68]);
        let peer_id = HashString::new(peer_bytes);

        Ok(Self {
            reserved,
            info_hash,
            peer_id,
        })
    }
}
