use std::fmt;

/// A bitfield representing piece completion or block states in BitTorrent.
/// Bits are indexed from MSB (bit 0 = 0x80) to LSB (bit 7 = 0x01).
#[derive(Clone, PartialEq, Eq, Default)]
pub struct Bitfield {
    size_bits: usize,
    size_set: usize,
    data: Vec<u8>,
}

impl Bitfield {
    pub fn new(size_bits: usize) -> Self {
        let size_bytes = size_bits.div_ceil(8);
        Self {
            size_bits,
            size_set: 0,
            data: vec![0u8; size_bytes],
        }
    }

    pub fn from_bytes(bytes: &[u8], size_bits: usize) -> Self {
        let size_bytes = size_bits.div_ceil(8);
        let mut data = vec![0u8; size_bytes];
        let copy_len = bytes.len().min(size_bytes);
        data[..copy_len].copy_from_slice(&bytes[..copy_len]);

        let mut bf = Self {
            size_bits,
            size_set: 0,
            data,
        };
        bf.update();
        bf
    }

    pub fn is_empty(&self) -> bool {
        self.size_bits == 0
    }

    pub fn is_all_set(&self) -> bool {
        self.size_set == self.size_bits
    }

    pub fn is_all_unset(&self) -> bool {
        self.size_set == 0
    }

    pub fn is_tail_cleared(&self) -> bool {
        let rem = self.size_bits % 8;
        if rem == 0 || self.data.is_empty() {
            true
        } else {
            let last_byte = *self.data.last().unwrap();
            let mask = 0xffu8 >> rem;
            (last_byte & mask) == 0
        }
    }

    pub fn size_bits(&self) -> usize {
        self.size_bits
    }

    pub fn size_bytes(&self) -> usize {
        self.size_bits.div_ceil(8)
    }

    pub fn size_set(&self) -> usize {
        self.size_set
    }

    pub fn size_unset(&self) -> usize {
        self.size_bits - self.size_set
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.data
    }

    pub fn as_mut_bytes(&mut self) -> &mut [u8] {
        &mut self.data
    }

    #[inline]
    fn mask_at(idx: usize) -> u8 {
        1 << (7 - (idx % 8))
    }

    pub fn clear_tail(&mut self) {
        let rem = self.size_bits % 8;
        if rem != 0 && !self.data.is_empty() {
            let mask = 0xffu8 << (8 - rem);
            let last = self.data.len() - 1;
            self.data[last] &= mask;
        }
    }

    /// Re-tallies the number of set bits and ensures tail bits are cleared.
    pub fn update(&mut self) {
        self.clear_tail();
        self.size_set = self.data.iter().map(|&b| b.count_ones() as usize).sum();
    }

    pub fn get(&self, idx: usize) -> bool {
        if idx >= self.size_bits {
            return false;
        }
        (self.data[idx / 8] & Self::mask_at(idx)) != 0
    }

    pub fn set(&mut self, idx: usize) {
        if idx >= self.size_bits {
            return;
        }
        let byte_idx = idx / 8;
        let mask = Self::mask_at(idx);
        if (self.data[byte_idx] & mask) == 0 {
            self.data[byte_idx] |= mask;
            self.size_set += 1;
        }
    }

    pub fn unset(&mut self, idx: usize) {
        if idx >= self.size_bits {
            return;
        }
        let byte_idx = idx / 8;
        let mask = Self::mask_at(idx);
        if (self.data[byte_idx] & mask) != 0 {
            self.data[byte_idx] &= !mask;
            self.size_set -= 1;
        }
    }

    pub fn set_all(&mut self) {
        self.data.fill(0xff);
        self.clear_tail();
        self.size_set = self.size_bits;
    }

    pub fn unset_all(&mut self) {
        self.data.fill(0);
        self.size_set = 0;
    }

    pub fn set_range(&mut self, first: usize, last: usize) {
        let last = last.min(self.size_bits);
        for i in first..last {
            self.set(i);
        }
    }

    pub fn unset_range(&mut self, first: usize, last: usize) {
        let last = last.min(self.size_bits);
        for i in first..last {
            self.unset(i);
        }
    }

    /// Finds the index of the first set bit in `[first, last)`.
    /// Returns `last` if no set bit is found.
    pub fn find_first_set(&self, first: usize, last: usize) -> usize {
        let last = last.min(self.size_bits);
        for i in first..last {
            if self.get(i) {
                return i;
            }
        }
        last
    }
}

impl fmt::Debug for Bitfield {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Bitfield({}/{} bits set)", self.size_set, self.size_bits)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bitfield_basic() {
        let mut bf = Bitfield::new(10);
        assert_eq!(bf.size_bits(), 10);
        assert_eq!(bf.size_bytes(), 2);
        assert_eq!(bf.size_set(), 0);
        assert!(bf.is_all_unset());
        assert!(!bf.is_all_set());
        assert!(bf.is_tail_cleared());

        bf.set(0);
        bf.set(9);
        assert_eq!(bf.size_set(), 2);
        assert!(bf.get(0));
        assert!(bf.get(9));
        assert!(!bf.get(1));
        assert!(!bf.get(8));

        bf.unset(0);
        assert!(!bf.get(0));
        assert_eq!(bf.size_set(), 1);

        bf.set_all();
        assert!(bf.is_all_set());
        assert_eq!(bf.size_set(), 10);
        assert!(bf.is_tail_cleared());
        // Tail bits 10..16 must be zero in the last byte
        assert_eq!(bf.as_bytes()[1], 0b11000000);

        bf.unset_all();
        assert!(bf.is_all_unset());
        assert_eq!(bf.size_set(), 0);
    }

    #[test]
    fn test_find_first_set() {
        let mut bf = Bitfield::new(20);
        assert_eq!(bf.find_first_set(0, 20), 20);

        bf.set(5);
        bf.set(12);
        assert_eq!(bf.find_first_set(0, 20), 5);
        assert_eq!(bf.find_first_set(6, 20), 12);
        assert_eq!(bf.find_first_set(13, 20), 20);
    }

    #[test]
    fn test_from_bytes() {
        let raw = [0b10100000, 0b11000000];
        let bf = Bitfield::from_bytes(&raw, 10);
        assert_eq!(bf.size_set(), 4);
        assert!(bf.get(0));
        assert!(!bf.get(1));
        assert!(bf.get(2));
        assert!(bf.get(8));
        assert!(bf.get(9));
    }
}
