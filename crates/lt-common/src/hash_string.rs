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
            Self::from_hex(&s).ok_or_else(|| serde::de::Error::custom("invalid hex string for HashString"))
        } else {
            let bytes = <&[u8]>::deserialize(deserializer)?;
            Self::from_slice(bytes).ok_or_else(|| serde::de::Error::custom("expected 20 bytes for HashString"))
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
}
