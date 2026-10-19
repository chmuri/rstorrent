use std::fmt;
use std::hash::{Hash, Hasher};

#[derive(Clone, Copy)]
pub struct FixedKey<const N: usize> {
    data: [u8; N],
    len: u8,
}

impl<const N: usize> Default for FixedKey<N> {
    fn default() -> Self {
        Self {
            data: [0; N],
            len: 0,
        }
    }
}

impl<const N: usize> FixedKey<N> {
    pub const MAX_SIZE: usize = N;

    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_bytes(bytes: &[u8]) -> Self {
        let mut key = Self::default();
        key.set_bytes(bytes);
        key
    }

    pub fn from_str(s: &str) -> Self {
        Self::from_bytes(s.as_bytes())
    }

    pub fn set_bytes(&mut self, bytes: &[u8]) {
        let copy_len = bytes.len().min(N);
        self.data[..copy_len].copy_from_slice(&bytes[..copy_len]);
        if copy_len < N {
            self.data[copy_len..].fill(0);
        }
        self.len = copy_len as u8;
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn len(&self) -> usize {
        self.len as usize
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.data[..self.len as usize]
    }

    pub fn as_str(&self) -> Result<&str, std::str::Utf8Error> {
        std::str::from_utf8(self.as_bytes())
    }
}

impl<const N: usize> PartialEq for FixedKey<N> {
    fn eq(&self, other: &Self) -> bool {
        self.as_bytes() == other.as_bytes()
    }
}

impl<const N: usize> Eq for FixedKey<N> {}

impl<const N: usize> PartialOrd for FixedKey<N> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl<const N: usize> Ord for FixedKey<N> {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.as_bytes().cmp(other.as_bytes())
    }
}

impl<const N: usize> Hash for FixedKey<N> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.as_bytes().hash(state);
    }
}

impl<const N: usize> PartialEq<&[u8]> for FixedKey<N> {
    fn eq(&self, other: &&[u8]) -> bool {
        self.as_bytes() == *other
    }
}

impl<const N: usize> PartialEq<&str> for FixedKey<N> {
    fn eq(&self, other: &&str) -> bool {
        self.as_bytes() == other.as_bytes()
    }
}

impl<const N: usize> fmt::Debug for FixedKey<N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Ok(s) = self.as_str() {
            write!(f, "FixedKey({:?})", s)
        } else {
            write!(f, "FixedKey({:?})", self.as_bytes())
        }
    }
}

impl<const N: usize> fmt::Display for FixedKey<N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", String::from_utf8_lossy(self.as_bytes()))
    }
}
