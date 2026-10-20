use std::str::FromStr;

use lt_common::hash_string::HashString;
use lt_dht::router::{DhtRouter, MAX_TRACKERS, NUM_TRACKER_EVICT, TIMEOUT_TRACKER_EVICT};
use lt_net::addr::SocketAddress;

const ANNOUNCED_INFO_HASHES: usize = 4096;
const UNKNOWN_INFO_HASH: usize = 5000;

fn info_hash_for(index: usize) -> HashString {
    let mut hash = HashString::default();
    hash.clear();
    hash.as_mut_slice()[0] = (index & 0xff) as u8;
    hash.as_mut_slice()[1] = ((index >> 8) & 0xff) as u8;
    hash
}

#[test]
fn test_tracker_count_is_capped() {
    let mut router = DhtRouter::new(HashString::default());
    let mut created = 0;

    for index in 0..ANNOUNCED_INFO_HASHES {
        let hash = info_hash_for(index);
        if router.get_tracker(&hash, true).is_some() {
            created += 1;
        }
    }

    assert!(created < ANNOUNCED_INFO_HASHES);
    assert_eq!(created, MAX_TRACKERS);
}

#[test]
fn test_full_table_evicts_stale_trackers() {
    let after_evict = MAX_TRACKERS - NUM_TRACKER_EVICT;
    let mut router = DhtRouter::new(HashString::default());
    let mut current_time = 1000u32;

    // Fill table: first half announced 20 minutes before second
    for index in 0..MAX_TRACKERS {
        if index == MAX_TRACKERS / 2 {
            current_time += 20 * 60; // 20 minutes
        }
        let tracker = router.get_tracker(&info_hash_for(index), true).unwrap();
        tracker.add_peer(0x0a000001 + index as u32, 6881, current_time);
    }

    assert!(router
        .get_tracker(&info_hash_for(UNKNOWN_INFO_HASH), true)
        .is_none());

    // Advance time by TIMEOUT_TRACKER_EVICT (1 hour)
    current_time += TIMEOUT_TRACKER_EVICT;
    router.evict_stale_trackers_at(current_time);

    assert_eq!(router.get_statistics().num_trackers, after_evict);

    // Verify recent ones are still present
    for index in (MAX_TRACKERS / 2)..MAX_TRACKERS {
        assert!(router.get_tracker(&info_hash_for(index), false).is_some());
    }

    // Now we have space to add unknown_info_hash
    assert!(router
        .get_tracker(&info_hash_for(UNKNOWN_INFO_HASH), true)
        .is_some());

    // Refill table
    let mut index = ANNOUNCED_INFO_HASHES;
    while router.get_statistics().num_trackers < MAX_TRACKERS {
        router.get_tracker(&info_hash_for(index), true);
        index += 1;
    }

    // Trying to evict without enough elapsed time does not evict
    router.evict_stale_trackers_at(current_time);
    assert_eq!(router.get_statistics().num_trackers, MAX_TRACKERS);

    // After period passes, eviction succeeds again
    current_time += TIMEOUT_TRACKER_EVICT;
    router.evict_stale_trackers_at(current_time);
    assert_eq!(router.get_statistics().num_trackers, after_evict);
}

#[test]
fn test_router_tokens() {
    let router = DhtRouter::new(HashString::default());
    let sa = SocketAddress::from_str("127.0.0.1:6881").unwrap();
    let token = router.make_token(&sa);

    assert!(router.token_valid(&token, &sa));

    let other_sa = SocketAddress::from_str("127.0.0.2:6881").unwrap();
    assert!(!router.token_valid(&token, &other_sa));
}
