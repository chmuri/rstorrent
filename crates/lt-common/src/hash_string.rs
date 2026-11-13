use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;

/// A 20-byte hash representation (used for SHA-1 info_hash, peer_id, node_id, etc.).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct HashString(pub [u8; 20]);

impl HashString {
    pub const SIZE: usize = 20;

    pub const fn new(bytes: [u8; 20]) -> Self {
        Self(bytes)
    }

    pub const fn zero() -> Self {
        Self([0u8; 20])
    }

    pub fn from_slice(slice: &[u8]) -> Option<Self> {
        if slice.len() == 20 {
            let mut arr = [0u8; 20];
            arr.copy_from_slice(slice);
            Some(Self(arr))
        } else {
            None
        }
    }

    pub fn from_hex(hex: &str) -> Option<Self> {
        if hex.len() != 40 {
            return None;
        }
        let mut bytes = [0u8; 20];
        for i in 0..20 {
            bytes[i] = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok()?;
        }
        Some(Self(bytes))
    }

    pub fn to_hex(&self) -> String {
        let mut s = String::with_capacity(40);
        for &b in &self.0 {
            use std::fmt::Write;
            write!(&mut s, "{:02x}", b).unwrap();
        }
        s
    }

    pub fn to_hex_upper(&self) -> String {
        let mut s = String::with_capacity(40);
        for &b in &self.0 {
            use std::fmt::Write;
            write!(&mut s, "{:02X}", b).unwrap();
        }
        s
    }

    pub fn as_bytes(&self) -> &[u8; 20] {
        &self.0
    }

    pub fn as_mut_bytes(&mut self) -> &mut [u8; 20] {
        &mut self.0
    }

    pub const fn size_data() -> usize {
        Self::SIZE
    }

    pub fn as_slice(&self) -> &[u8] {
        &self.0
    }

    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        &mut self.0
    }

    pub fn clear(&mut self) {
        self.0 = [0u8; 20];
    }

    pub fn clear_byte(&mut self, b: u8) {
        self.0 = [b; 20];
    }

    pub fn is_zero(&self) -> bool {
        self.0.iter().all(|&b| b == 0)
    }
}

impl AsRef<[u8]> for HashString {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

impl AsMut<[u8]> for HashString {
    fn as_mut(&mut self) -> &mut [u8] {
        &mut self.0
    }
}

impl fmt::Debug for HashString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "HashString({})", self.to_hex())
    }
}

impl fmt::Display for HashString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_hex())
    }
}

impl Serialize for HashString {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        if serializer.is_human_readable() {
            serializer.serialize_str(&self.to_hex())
        } else {
            serializer.serialize_bytes(&self.0)
        }
    }
}

impl<'de> Deserialize<'de> for HashString {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        if deserializer.is_human_readable() {
            let s = String::deserialize(deserializer)?;
            Self::from_hex(&s)
                .ok_or_else(|| serde::de::Error::custom("invalid hex string for HashString"))
        } else {
            let bytes = <&[u8]>::deserialize(deserializer)?;
            Self::from_slice(bytes)
                .ok_or_else(|| serde::de::Error::custom("expected 20 bytes for HashString"))
        }
    }
}

/// A 32-byte SHA-256 hash, used for the full BitTorrent v2 info-hash (BEP52).
///
/// The wire/tracker/DHT identifier is always 20 bytes: v2 torrents use this
/// hash truncated to 20 bytes (see [`Hash256::truncate`]). `HashString` remains
/// the 20-byte wire identity; `Hash256` carries the full v2 hash for metadata
/// verification, magnet `btmh` and the v2 upgrade handshake.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Hash256(pub [u8; 32]);

impl Hash256 {
    pub const SIZE: usize = 32;

    pub const fn new(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub const fn zero() -> Self {
        Self([0u8; 32])
    }

    pub fn from_slice(slice: &[u8]) -> Option<Self> {
        if slice.len() == 32 {
            let mut arr = [0u8; 32];
            arr.copy_from_slice(slice);
            Some(Self(arr))
        } else {
            None
        }
    }

    pub fn from_hex(hex: &str) -> Option<Self> {
        if hex.len() != 64 {
            return None;
        }
        let mut bytes = [0u8; 32];
        for i in 0..32 {
            bytes[i] = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok()?;
        }
        Some(Self(bytes))
    }

    pub fn to_hex(&self) -> String {
        let mut s = String::with_capacity(64);
        for &b in &self.0 {
            use std::fmt::Write;
            write!(&mut s, "{:02x}", b).unwrap();
        }
        s
    }

    pub fn to_hex_upper(&self) -> String {
        let mut s = String::with_capacity(64);
        for &b in &self.0 {
            use std::fmt::Write;
            write!(&mut s, "{:02X}", b).unwrap();
        }
        s
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    pub fn as_mut_bytes(&mut self) -> &mut [u8; 32] {
        &mut self.0
    }

    pub fn as_slice(&self) -> &[u8] {
        &self.0
    }

    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        &mut self.0
    }

    pub fn is_zero(&self) -> bool {
        self.0.iter().all(|&b| b == 0)
    }

    pub fn clear(&mut self) {
        self.0 = [0u8; 32];
    }

    /// BEP52: the 20-byte wire/tracker/DHT identifier for a v2 torrent is the
    /// SHA-256 info-hash truncated to its first 20 bytes.
    pub fn truncate(&self) -> HashString {
        let mut bytes = [0u8; 20];
        bytes.copy_from_slice(&self.0[..20]);
        HashString::new(bytes)
    }
}

impl AsRef<[u8]> for Hash256 {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

impl AsMut<[u8]> for Hash256 {
    fn as_mut(&mut self) -> &mut [u8] {
        &mut self.0
    }
}

impl fmt::Debug for Hash256 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Hash256({})", self.to_hex())
    }
}

impl fmt::Display for Hash256 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_hex())
    }
}

impl Serialize for Hash256 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        if serializer.is_human_readable() {
            serializer.serialize_str(&self.to_hex())
        } else {
            serializer.serialize_bytes(&self.0)
        }
    }
}

impl<'de> Deserialize<'de> for Hash256 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        if deserializer.is_human_readable() {
            let s = String::deserialize(deserializer)?;
            Self::from_hex(&s)
                .ok_or_else(|| serde::de::Error::custom("invalid hex string for Hash256"))
        } else {
            let bytes = <&[u8]>::deserialize(deserializer)?;
            Self::from_slice(bytes)
                .ok_or_else(|| serde::de::Error::custom("expected 32 bytes for Hash256"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hex_conversion() {
        let hex = "0123456789abcdef0123456789abcdef01234567";
        let hash = HashString::from_hex(hex).unwrap();
        assert_eq!(hash.to_hex(), hex);
        assert!(!hash.is_zero());

        let zero = HashString::zero();
        assert!(zero.is_zero());
        assert_eq!(zero.to_hex(), "0000000000000000000000000000000000000000");
    }

    #[test]
    fn test_ord_and_hash() {
        let h1 = HashString::from_hex("0000000000000000000000000000000000000001").unwrap();
        let h2 = HashString::from_hex("0000000000000000000000000000000000000002").unwrap();
        assert!(h1 < h2);

        use std::collections::HashSet;
        let mut set = HashSet::new();
        set.insert(h1);
        assert!(set.contains(&h1));
        assert!(!set.contains(&h2));
    }

    #[test]
    fn test_hash256_hex_conversion() {
        let hex = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let hash = Hash256::from_hex(hex).unwrap();
        assert_eq!(hash.to_hex(), hex);
        assert_eq!(hash.to_hex_upper(), hex.to_uppercase());
        assert!(!hash.is_zero());
        assert!(Hash256::from_hex("00").is_none());
        assert!(Hash256::from_slice(&[0u8; 31]).is_none());
        assert_eq!(Hash256::from_slice(&[0u8; 32]), Some(Hash256::zero()));
        assert!(Hash256::zero().is_zero());
    }

    #[test]
    fn test_hash256_truncate_to_wire_hash() {
        let mut bytes = [0u8; 32];
        for (i, b) in bytes.iter_mut().enumerate() {
            *b = i as u8;
        }
        let hash = Hash256::new(bytes);
        let wire = hash.truncate();
        assert_eq!(wire.as_slice(), &bytes[..20]);
        assert_eq!(wire, HashString::from_slice(&bytes[..20]).unwrap());
    }

    #[test]
    fn test_hash256_ord_and_hash() {
        let h1 = Hash256::from_hex(&format!("{:0>64}", "1")).unwrap();
        let h2 = Hash256::from_hex(&format!("{:0>64}", "2")).unwrap();
        assert!(h1 < h2);

        use std::collections::HashSet;
        let mut set = HashSet::new();
        set.insert(h1);
        assert!(set.contains(&h1));
        assert!(!set.contains(&h2));
    }
}
