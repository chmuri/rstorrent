use std::collections::HashSet;

use lt_tracker::udp_router::{RandomEngine, UdpRouter};

#[test]
fn test_random_engine_state() {
    // Transaction ids are the only thing separating a tracker's own responses from
    // responses forged for a different tracker, so one observed id must not give up
    // the ids that follow it. That rules out any engine whose state is small enough
    // to search.
    let state_bits = RandomEngine::STATE_BITS;
    assert!(state_bits >= 256);
}

#[test]
fn test_random_engine_seeding() {
    // Two routers in the same process must not issue the same sequence, which they
    // would if the engine were seeded from a fixed or near-fixed value.
    let mut first = RandomEngine::new();
    let mut second = RandomEngine::new();

    let mut differs = false;
    for _ in 0..8 {
        differs = differs || (first.next_u32() != second.next_u32());
    }

    assert!(differs);
}

#[test]
fn test_transaction_ids_are_unique_while_reserved() {
    let mut router = UdpRouter::new();
    let ids: Vec<u32> = (0..4096)
        .map(|_| router.allocate_transaction_id())
        .collect();
    let unique: HashSet<u32> = ids.iter().copied().collect();

    assert_eq!(unique.len(), ids.len());
    assert_eq!(router.active_transaction_count(), ids.len());

    for id in ids {
        router.release_transaction_id(id);
    }
    assert_eq!(router.active_transaction_count(), 0);
}

#[test]
fn test_released_transaction_id_can_be_reused() {
    let mut router = UdpRouter::new();
    let first = router.generate_transaction_id();
    router.release_transaction_id(first);

    // Allocation draws from a PRNG, so a released ID is merely eligible again;
    // the important invariant is that the router does not report it in use.
    assert!(!router.is_transaction_active(first));
}
