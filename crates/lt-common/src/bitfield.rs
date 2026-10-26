use std::fmt;

/// A high-performance bitfield representing piece completion or block states in BitTorrent.
/// Bits are indexed from MSB (bit 0 = 0x80) to LSB (bit 7 = 0x01).
/// Internally backed by both `u64` words (for SIMD / POPCNT / bitwise acceleration)
/// and byte representations for zero-overhead wire compatibility.
#[derive(Clone, PartialEq, Eq, Default)]
pub struct Bitfield {
    size_bits: usize,
    size_set: usize,
    words: Vec<u64>,
    bytes: Vec<u8>,
}

impl Bitfield {
    pub fn new(size_bits: usize) -> Self {
        let size_bytes = size_bits.div_ceil(8);
        let size_words = size_bits.div_ceil(64);
        Self {
            size_bits,
            size_set: 0,
            words: vec![0u64; size_words],
            bytes: vec![0u8; size_bytes],
        }
    }

    pub fn from_bytes(raw_bytes: &[u8], size_bits: usize) -> Self {
        let size_bytes = size_bits.div_ceil(8);
        let size_words = size_bits.div_ceil(64);

        let mut bytes = vec![0u8; size_bytes];
        let copy_len = raw_bytes.len().min(size_bytes);
        bytes[..copy_len].copy_from_slice(&raw_bytes[..copy_len]);

        let mut words = vec![0u64; size_words];
        for (w_idx, word) in words.iter_mut().enumerate() {
            let start = w_idx * 8;
            if start < bytes.len() {
                let mut chunk = [0u8; 8];
                let take = (bytes.len() - start).min(8);
                chunk[..take].copy_from_slice(&bytes[start..start + take]);
                *word = u64::from_be_bytes(chunk);
            }
        }

        let mut bf = Self {
            size_bits,
            size_set: 0,
            words,
            bytes,
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
        if rem == 0 || self.bytes.is_empty() {
            true
        } else {
            let last_byte = *self.bytes.last().unwrap();
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

    pub fn size_words(&self) -> usize {
        self.words.len()
    }

    pub fn size_set(&self) -> usize {
        self.size_set
    }

    pub fn size_unset(&self) -> usize {
        self.size_bits - self.size_set
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn as_mut_bytes(&mut self) -> &mut [u8] {
        &mut self.bytes
    }

    pub fn words(&self) -> &[u64] {
        &self.words
    }

    #[inline]
    fn mask_at(idx: usize) -> u8 {
        1 << (7 - (idx % 8))
    }

    #[inline]
    fn word_mask_at(idx: usize) -> u64 {
        1u64 << (63 - (idx % 64))
    }

    pub fn clear_tail(&mut self) {
        let rem_bits = self.size_bits % 64;
        if rem_bits != 0 && !self.words.is_empty() {
            let mask = !0u64 << (64 - rem_bits);
            let last = self.words.len() - 1;
            self.words[last] &= mask;
        }

        let rem_bytes = self.size_bits % 8;
        if rem_bytes != 0 && !self.bytes.is_empty() {
            let mask = 0xffu8 << (8 - rem_bytes);
            let last = self.bytes.len() - 1;
            self.bytes[last] &= mask;
        }
    }

    /// Re-tallies the number of set bits and ensures tail bits are cleared.
    pub fn update(&mut self) {
        self.clear_tail();
        // Hardware POPCNT on each 64-bit word
        self.size_set = self.words.iter().map(|&w| w.count_ones() as usize).sum();
    }

    #[inline]
    pub fn get(&self, idx: usize) -> bool {
        if idx >= self.size_bits {
            return false;
        }
        let word_idx = idx / 64;
        (self.words[word_idx] & Self::word_mask_at(idx)) != 0
    }

    #[inline]
    pub fn set(&mut self, idx: usize) {
        if idx >= self.size_bits {
            return;
        }
        let word_idx = idx / 64;
        let w_mask = Self::word_mask_at(idx);
        if (self.words[word_idx] & w_mask) == 0 {
            self.words[word_idx] |= w_mask;
            let byte_idx = idx / 8;
            self.bytes[byte_idx] |= Self::mask_at(idx);
            self.size_set += 1;
        }
    }

    #[inline]
    pub fn unset(&mut self, idx: usize) {
        if idx >= self.size_bits {
            return;
        }
        let word_idx = idx / 64;
        let w_mask = Self::word_mask_at(idx);
        if (self.words[word_idx] & w_mask) != 0 {
            self.words[word_idx] &= !w_mask;
            let byte_idx = idx / 8;
            self.bytes[byte_idx] &= !Self::mask_at(idx);
            self.size_set -= 1;
        }
    }

    pub fn set_all(&mut self) {
        self.words.fill(!0u64);
        self.bytes.fill(0xff);
        self.clear_tail();
        self.size_set = self.size_bits;
    }

    pub fn unset_all(&mut self) {
        self.words.fill(0);
        self.bytes.fill(0);
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

    /// Fast O(N/64) check whether `peer` has any piece that `self` does NOT have.
    /// Uses SIMD-friendly bitwise operations: `interesting = peer & !our`.
    pub fn has_interesting(&self, peer: &Bitfield) -> bool {
        let n = self.words.len().min(peer.words.len());
        for i in 0..n {
            let interesting = peer.words[i] & !self.words[i];
            if interesting != 0 {
                return true;
            }
        }
        false
    }

    /// Fast O(N/64) count of how many pieces `peer` has that `self` does not have.
    pub fn count_interesting(&self, peer: &Bitfield) -> usize {
        let n = self.words.len().min(peer.words.len());
        let mut count = 0;
        for i in 0..n {
            let interesting = peer.words[i] & !self.words[i];
            count += interesting.count_ones() as usize;
        }
        count
    }

    /// Fast iterator over piece indices that `peer` has and `self` does not.
    pub fn iter_interesting<'a>(&'a self, peer: &'a Bitfield) -> InterestingIter<'a> {
        InterestingIter {
            our_words: &self.words,
            peer_words: &peer.words,
            word_idx: 0,
            current_mask: 0,
            size_bits: self.size_bits.min(peer.size_bits),
        }
    }

    /// Finds the index of the first set bit in `[first, last)`.
    /// Returns `last` if no set bit is found.
    /// Operates at 64 bits per iteration using hardware `leading_zeros()`.
    pub fn find_first_set(&self, first: usize, last: usize) -> usize {
        let last = last.min(self.size_bits);
        if first >= last {
            return last;
        }

        let start_word = first / 64;
        let end_word = last.div_ceil(64);

        for w_idx in start_word..end_word {
            let mut word = self.words[w_idx];
            if word == 0 {
                continue;
            }

            // Mask off bits before `first` in the start word
            if w_idx == start_word {
                let rem = first % 64;
                if rem > 0 {
                    let mask = !0u64 >> rem;
                    word &= mask;
                }
            }

            // Mask off bits at or after `last` in the end word
            let word_end_bit = (w_idx + 1) * 64;
            if word_end_bit > last {
                let excess = word_end_bit - last;
                if excess < 64 {
                    let mask = !0u64 << excess;
                    word &= mask;
                } else {
                    word = 0;
                }
            }

            if word != 0 {
                let bit_in_word = word.leading_zeros() as usize;
                let found_idx = w_idx * 64 + bit_in_word;
                if found_idx < last {
                    return found_idx;
                }
            }
        }

        last
    }
}

pub struct InterestingIter<'a> {
    our_words: &'a [u64],
    peer_words: &'a [u64],
    word_idx: usize,
    current_mask: u64,
    size_bits: usize,
}

impl<'a> Iterator for InterestingIter<'a> {
    type Item = usize;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if self.current_mask != 0 {
                let lz = self.current_mask.leading_zeros() as usize;
                let bit_idx = (self.word_idx - 1) * 64 + lz;
                // Clear the bit we just found
                self.current_mask &= !(1u64 << (63 - lz));
                if bit_idx < self.size_bits {
                    return Some(bit_idx);
                }
                return None;
            }

            let max_words = self.our_words.len().min(self.peer_words.len());
            if self.word_idx >= max_words {
                return None;
            }

            let our_w = self.our_words[self.word_idx];
            let peer_w = self.peer_words[self.word_idx];
            self.word_idx += 1;

            let interesting = peer_w & !our_w;
            if interesting != 0 {
                self.current_mask = interesting;
            }
        }
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
        let mut bf = Bitfield::new(200);
        assert_eq!(bf.find_first_set(0, 200), 200);

        bf.set(5);
        bf.set(12);
        bf.set(65);
        bf.set(129);

        assert_eq!(bf.find_first_set(0, 200), 5);
        assert_eq!(bf.find_first_set(6, 200), 12);
        assert_eq!(bf.find_first_set(13, 200), 65);
        assert_eq!(bf.find_first_set(66, 200), 129);
        assert_eq!(bf.find_first_set(130, 200), 200);
    }

    #[test]
    fn test_interesting_operations() {
        let mut our = Bitfield::new(130);
        let mut peer = Bitfield::new(130);

        assert!(!our.has_interesting(&peer));
        assert_eq!(our.count_interesting(&peer), 0);

        peer.set(10);
        peer.set(70);
        peer.set(128);

        assert!(our.has_interesting(&peer));
        assert_eq!(our.count_interesting(&peer), 3);

        let items: Vec<usize> = our.iter_interesting(&peer).collect();
        assert_eq!(items, vec![10, 70, 128]);

        // When we also have 70:
        our.set(70);
        assert_eq!(our.count_interesting(&peer), 2);
        let items2: Vec<usize> = our.iter_interesting(&peer).collect();
        assert_eq!(items2, vec![10, 128]);
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
