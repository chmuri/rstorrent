use num_bigint::BigUint;
use num_traits::{One, Zero};

pub const DH_PRIME_LENGTH: usize = 96;
pub const DH_GENERATOR_LENGTH: usize = 1;
pub const VC_LENGTH: usize = 8;
pub const VC_DATA: [u8; VC_LENGTH] = [0u8; VC_LENGTH];

pub const DH_PRIME: [u8; DH_PRIME_LENGTH] = [
    0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xC9, 0x0F, 0xDA, 0xA2,
    0x21, 0x68, 0xC2, 0x34, 0xC4, 0xC6, 0x62, 0x8B, 0x80, 0xDC, 0x1C, 0xD1,
    0x29, 0x02, 0x4E, 0x08, 0x8A, 0x67, 0xCC, 0x74, 0x02, 0x0B, 0xBE, 0xA6,
    0x3B, 0x13, 0x9B, 0x22, 0x51, 0x4A, 0x08, 0x79, 0x8E, 0x34, 0x04, 0xDD,
    0xEF, 0x95, 0x19, 0xB3, 0xCD, 0x3A, 0x43, 0x1B, 0x30, 0x2B, 0x0A, 0x6D,
    0xF2, 0x5F, 0x14, 0x37, 0x4F, 0xE1, 0x35, 0x6D, 0x6D, 0x51, 0xC2, 0x45,
    0xE4, 0x85, 0xB5, 0x76, 0x62, 0x5E, 0x7E, 0xC6, 0xF4, 0x4C, 0x42, 0xE9,
    0xA6, 0x3A, 0x36, 0x21, 0x00, 0x00, 0x00, 0x00, 0x00, 0x09, 0x05, 0x63,
];

pub const DH_GENERATOR: [u8; DH_GENERATOR_LENGTH] = [2];

#[derive(Debug, Clone)]
pub struct DiffieHellman {
    prime: BigUint,
    generator: BigUint,
    priv_key: BigUint,
    pub_key: BigUint,
    secret: Option<Vec<u8>>,
    size: usize,
}

impl DiffieHellman {
    pub fn new(prime_bytes: &[u8], generator_bytes: &[u8]) -> Self {
        let prime = BigUint::from_bytes_be(prime_bytes);
        let generator = BigUint::from_bytes_be(generator_bytes);
        let size = prime_bytes.len();

        let mut priv_bytes = [0u8; 96];
        getrandom::getrandom(&mut priv_bytes).expect("getrandom failed");
        let mut priv_key = BigUint::from_bytes_be(&priv_bytes);
        if priv_key >= prime {
            priv_key %= &prime;
        }
        if priv_key < BigUint::from(2u32) {
            priv_key += BigUint::from(2u32);
        }

        let pub_key = generator.modpow(&priv_key, &prime);

        Self {
            prime,
            generator,
            priv_key,
            pub_key,
            secret: None,
            size,
        }
    }

    pub fn new_mse() -> Self {
        Self::new(&DH_PRIME, &DH_GENERATOR)
    }

    pub fn is_valid(&self) -> bool {
        !self.pub_key.is_zero() && self.pub_key < self.prime
    }

    pub fn prime(&self) -> &BigUint {
        &self.prime
    }

    pub fn generator(&self) -> &BigUint {
        &self.generator
    }

    pub fn size(&self) -> usize {
        self.size
    }

    pub fn pub_key_bytes(&self) -> Vec<u8> {
        let mut dest = vec![0u8; self.size];
        self.store_pub_key(&mut dest);
        dest
    }

    pub fn store_pub_key(&self, dest: &mut [u8]) {
        dest.fill(0);
        let bytes = self.pub_key.to_bytes_be();
        if dest.len() >= bytes.len() {
            let offset = dest.len() - bytes.len();
            dest[offset..].copy_from_slice(&bytes);
        }
    }

    pub fn compute_secret(&mut self, peer_pub_bytes: &[u8]) -> bool {
        let peer_pub = BigUint::from_bytes_be(peer_pub_bytes);
        if peer_pub.is_zero() || peer_pub >= self.prime || peer_pub.is_one() {
            return false;
        }

        let shared = peer_pub.modpow(&self.priv_key, &self.prime);
        let bytes = shared.to_bytes_be();
        let mut secret = vec![0u8; self.size];
        if secret.len() >= bytes.len() {
            let offset = secret.len() - bytes.len();
            secret[offset..].copy_from_slice(&bytes);
        } else {
            return false;
        }

        self.secret = Some(secret);
        true
    }

    pub fn secret(&self) -> Option<&[u8]> {
        self.secret.as_deref()
    }
}

// MSE Key Derivation functions
use crate::sha1::sha1_salt;

pub fn mse_key_a(s: &[u8], s_key: &[u8]) -> [u8; 20] {
    sha1_salt(&[b"keyA", s, s_key])
}

pub fn mse_key_b(s: &[u8], s_key: &[u8]) -> [u8; 20] {
    sha1_salt(&[b"keyB", s, s_key])
}

pub fn mse_req1(s: &[u8]) -> [u8; 20] {
    sha1_salt(&[b"req1", s])
}

pub fn mse_req2(s_key: &[u8]) -> [u8; 20] {
    sha1_salt(&[b"req2", s_key])
}

pub fn mse_req3(s: &[u8]) -> [u8; 20] {
    sha1_salt(&[b"req3", s])
}

pub fn mse_deobfuscate_hash(obfuscated: &mut [u8; 20], s: &[u8]) {
    let req3 = mse_req3(s);
    for i in 0..20 {
        obfuscated[i] ^= req3[i];
    }
}
