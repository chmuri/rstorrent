pub mod dh;
pub mod rc4;
pub mod sha1;

pub use dh::{
    mse_deobfuscate_hash, mse_key_a, mse_key_b, mse_req1, mse_req2, mse_req3, DiffieHellman,
    DH_GENERATOR, DH_PRIME, DH_PRIME_LENGTH, VC_DATA, VC_LENGTH,
};
pub use rc4::Rc4;
pub use sha1::{sha1_hash, sha1_salt};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sha1_vectors() {
        assert_eq!(
            sha1_hash(b""),
            [
                0xda, 0x39, 0xa3, 0xee, 0x5e, 0x6b, 0x4b, 0x0d, 0x32, 0x55, 0xbf, 0xef, 0x95, 0x60,
                0x18, 0x90, 0xaf, 0xd8, 0x07, 0x09
            ]
        );

        assert_eq!(
            sha1_hash(b"The quick brown fox jumps over the lazy dog"),
            [
                0x2f, 0xd4, 0xe1, 0xc6, 0x7a, 0x2d, 0x28, 0xfc, 0xed, 0x84, 0x9e, 0xe1, 0xbb, 0x76,
                0xe7, 0x39, 0x1b, 0x93, 0xeb, 0x12
            ]
        );
    }

    #[test]
    fn test_rc4_roundtrip() {
        let key = b"SecretKey1234567";
        let mut cipher_enc = Rc4::new(key);
        let mut cipher_dec = Rc4::new(key);

        let original = b"Hello BitTorrent Protocol Encryption / MSE!".to_vec();
        let mut buffer = original.clone();

        cipher_enc.crypt(&mut buffer);
        assert_ne!(buffer, original);

        cipher_dec.crypt(&mut buffer);
        assert_eq!(buffer, original);
    }

    #[test]
    fn test_rc4_discard() {
        let key = b"KeyForDiscard123";
        let mut c1 = Rc4::new(key);
        let mut c2 = Rc4::new(key);

        c1.discard(1024);

        let mut dummy = [0u8; 1024];
        c2.crypt(&mut dummy);

        let mut msg1 = b"Testing discard sync".to_vec();
        let mut msg2 = msg1.clone();

        c1.crypt(&mut msg1);
        c2.crypt(&mut msg2);

        assert_eq!(msg1, msg2);
    }

    #[test]
    fn test_diffie_hellman_exchange() {
        let mut alice = DiffieHellman::new_mse();
        let mut bob = DiffieHellman::new_mse();

        assert!(alice.is_valid());
        assert!(bob.is_valid());

        let alice_pub = alice.pub_key_bytes();
        let bob_pub = bob.pub_key_bytes();

        assert_eq!(alice_pub.len(), 96);
        assert_eq!(bob_pub.len(), 96);

        assert!(alice.compute_secret(&bob_pub));
        assert!(bob.compute_secret(&alice_pub));

        let alice_sec = alice.secret().expect("alice secret");
        let bob_sec = bob.secret().expect("bob secret");

        assert_eq!(alice_sec.len(), 96);
        assert_eq!(bob_sec.len(), 96);
        assert_eq!(alice_sec, bob_sec);
    }

    #[test]
    fn test_mse_key_derivation_and_deobfuscation() {
        let s = [42u8; 96];
        let skey = [7u8; 20];

        let key_a = mse_key_a(&s, &skey);
        let key_b = mse_key_b(&s, &skey);
        assert_ne!(key_a, key_b);

        let req1 = mse_req1(&s);
        let req2 = mse_req2(&skey);
        let req3 = mse_req3(&s);
        assert_ne!(req1, req2);
        assert_ne!(req2, req3);

        // Test deobfuscation
        let mut obfuscated = req2;
        for i in 0..20 {
            obfuscated[i] ^= req3[i];
        }

        mse_deobfuscate_hash(&mut obfuscated, &s);
        assert_eq!(obfuscated, req2);
    }
}
