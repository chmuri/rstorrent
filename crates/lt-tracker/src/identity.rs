//! Client-wide tracker identity values.
//!
//! BEP 7 / BEP 15 announces carry a `key` that lets a tracker recognize the
//! same client across IP changes and restarts. The key must be random but
//! stable for the lifetime of the installation; a hardcoded constant makes
//! every client indistinguishable and breaks per-peer upload accounting on
//! trackers that credit deltas between announces.
//!
//! The daemon loads (or generates and persists) the key at startup and calls
//! [`set_client_key`]. Embedders and tests that never set it get a lazily
//! generated random key, so the value is always usable.

use std::sync::atomic::{AtomicU32, Ordering};

/// Zero means "not initialized yet"; a generated key is always non-zero.
static CLIENT_KEY: AtomicU32 = AtomicU32::new(0);

/// Returns the stable client announce key, generating a random non-zero one
/// on first use when the daemon has not supplied a persisted value.
pub fn client_key() -> u32 {
    let current = CLIENT_KEY.load(Ordering::Relaxed);
    if current != 0 {
        return current;
    }
    let generated = random_key();
    // Another thread may have initialized concurrently; either value is fine
    // as long as every subsequent call observes the same one.
    match CLIENT_KEY.compare_exchange(0, generated, Ordering::Relaxed, Ordering::Relaxed) {
        Ok(_) => generated,
        Err(existing) => existing,
    }
}

/// Installs the persisted client key. A zero value is ignored so a corrupt
/// persisted file cannot erase the identity.
pub fn set_client_key(key: u32) {
    if key != 0 {
        CLIENT_KEY.store(key, Ordering::Relaxed);
    }
}

fn random_key() -> u32 {
    let mut bytes = [0u8; 4];
    if getrandom::fill(&mut bytes).is_ok() {
        let value = u32::from_ne_bytes(bytes);
        if value != 0 {
            return value;
        }
    }
    // Fallback that still guarantees a non-zero key.
    0x5253_0001
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_is_nonzero_and_stable_within_process() {
        let first = client_key();
        assert_ne!(first, 0);
        assert_eq!(first, client_key());
    }

    #[test]
    fn set_overrides_and_zero_is_ignored() {
        set_client_key(0xDEAD_BEEF);
        assert_eq!(client_key(), 0xDEAD_BEEF);
        set_client_key(0);
        assert_eq!(client_key(), 0xDEAD_BEEF);
    }
}
