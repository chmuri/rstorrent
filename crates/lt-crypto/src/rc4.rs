/// RC4 stream cipher compatible with OpenSSL and BitTorrent MSE / PE
#[derive(Clone, Debug)]
pub struct Rc4 {
    state: [u8; 256],
    i: u8,
    j: u8,
}

impl Rc4 {
    /// Initialize RC4 state with a key of arbitrary length (1..=256 bytes)
    pub fn new(key: &[u8]) -> Self {
        assert!(!key.is_empty(), "RC4 key cannot be empty");
        let mut rc4 = Self {
            state: [0u8; 256],
            i: 0,
            j: 0,
        };
        rc4.set_key(key);
        rc4
    }

    pub fn set_key(&mut self, key: &[u8]) {
        assert!(!key.is_empty(), "RC4 key cannot be empty");
        for (idx, val) in self.state.iter_mut().enumerate() {
            *val = idx as u8;
        }

        let mut j: u8 = 0;
        let key_len = key.len();
        for i in 0..256 {
            let si = self.state[i];
            let k = key[i % key_len];
            j = j.wrapping_add(si).wrapping_add(k);
            self.state.swap(i, j as usize);
        }

        self.i = 0;
        self.j = 0;
    }

    /// In-place encryption or decryption
    pub fn crypt(&mut self, data: &mut [u8]) {
        for byte in data.iter_mut() {
            self.i = self.i.wrapping_add(1);
            let si = self.state[self.i as usize];
            self.j = self.j.wrapping_add(si);
            let sj = self.state[self.j as usize];
            self.state.swap(self.i as usize, self.j as usize);

            let t = si.wrapping_add(sj) as usize;
            let k = self.state[t];
            *byte ^= k;
        }
    }

    /// Convenience alias for crypt
    pub fn apply_keystream(&mut self, data: &mut [u8]) {
        self.crypt(data);
    }

    /// Discard the first `count` bytes of the keystream (essential for MSE 1024-byte discard)
    pub fn discard(&mut self, count: usize) {
        for _ in 0..count {
            self.i = self.i.wrapping_add(1);
            let si = self.state[self.i as usize];
            self.j = self.j.wrapping_add(si);
            self.state.swap(self.i as usize, self.j as usize);
        }
    }
}
