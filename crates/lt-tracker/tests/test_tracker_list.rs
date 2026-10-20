use lt_net::addr::{AddressList, SocketAddress};
use lt_tracker::http::uri_generate_scrape_url;
use lt_tracker::list::TrackerList;
use lt_tracker::state::{TrackerEvent, TrackerFlags};
use lt_tracker::tracker::Tracker;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

#[test]
fn test_basic() {
    let mut tracker_list = TrackerList::new();
    let tracker_0 = Tracker::new(0, "http://1".to_string());
    tracker_list.insert(tracker_0);

    assert_eq!(tracker_list.at(0).unwrap().url(), "http://1");
    assert_eq!(tracker_list.end_group(0) - tracker_list.begin_group(0), 1);
    assert!(tracker_list.has_usable());
}

#[test]
fn test_enable() {
    let mut tracker_list = TrackerList::new();
    let enabled_counter = Arc::new(AtomicU32::new(0));
    let disabled_counter = Arc::new(AtomicU32::new(0));

    let ec = enabled_counter.clone();
    tracker_list.on_tracker_enabled = Some(Box::new(move |_| {
        ec.fetch_add(1, Ordering::SeqCst);
    }));

    let dc = disabled_counter.clone();
    tracker_list.on_tracker_disabled = Some(Box::new(move |_| {
        dc.fetch_add(1, Ordering::SeqCst);
    }));

    tracker_list.insert(Tracker::new(0, "http://1".to_string()));
    tracker_list.insert(Tracker::new(1, "http://2".to_string()));
    assert_eq!(enabled_counter.load(Ordering::SeqCst), 2);
    assert_eq!(disabled_counter.load(Ordering::SeqCst), 0);

    tracker_list.enable_tracker(0);
    tracker_list.enable_tracker(1);
    assert_eq!(enabled_counter.load(Ordering::SeqCst), 2);
    assert_eq!(disabled_counter.load(Ordering::SeqCst), 0);

    tracker_list.disable_tracker(0);
    tracker_list.enable_tracker(1);
    assert_eq!(enabled_counter.load(Ordering::SeqCst), 2);
    assert_eq!(disabled_counter.load(Ordering::SeqCst), 1);

    tracker_list.disable_tracker(1);
    tracker_list.disable_tracker(0);
    assert_eq!(enabled_counter.load(Ordering::SeqCst), 2);
    assert_eq!(disabled_counter.load(Ordering::SeqCst), 2);

    tracker_list.enable_tracker(0);
    tracker_list.enable_tracker(1);
    tracker_list.enable_tracker(0);
    tracker_list.enable_tracker(1);
    assert_eq!(enabled_counter.load(Ordering::SeqCst), 4);
    assert_eq!(disabled_counter.load(Ordering::SeqCst), 2);
}

#[test]
fn test_tracker_flags() {
    let mut tracker_list = TrackerList::new();

    tracker_list.insert(Tracker::new(0, "".to_string()));
    tracker_list.insert(Tracker::new_with_flags(
        0,
        "".to_string(),
        TrackerFlags::empty(),
    ));
    tracker_list.insert(Tracker::new_with_flags(
        0,
        "".to_string(),
        TrackerFlags::ENABLED,
    ));
    tracker_list.insert(Tracker::new_with_flags(
        0,
        "".to_string(),
        TrackerFlags::EXTRA_TRACKER,
    ));
    tracker_list.insert(Tracker::new_with_flags(
        0,
        "".to_string(),
        TrackerFlags::ENABLED | TrackerFlags::EXTRA_TRACKER,
    ));

    assert_eq!(
        tracker_list.at(0).unwrap().state().flags.bits() & 0xff,
        TrackerFlags::ENABLED.bits()
    );
    assert_eq!(tracker_list.at(1).unwrap().state().flags.bits() & 0xff, 0);
    assert_eq!(
        tracker_list.at(2).unwrap().state().flags.bits() & 0xff,
        TrackerFlags::ENABLED.bits()
    );
    assert_eq!(
        tracker_list.at(3).unwrap().state().flags.bits() & 0xff,
        TrackerFlags::EXTRA_TRACKER.bits()
    );
    assert_eq!(
        tracker_list.at(4).unwrap().state().flags.bits() & 0xff,
        (TrackerFlags::ENABLED | TrackerFlags::EXTRA_TRACKER).bits()
    );

    tracker_list.clear();
    assert_eq!(tracker_list.size(), 0);
}

#[test]
fn test_find_url() {
    let mut tracker_list = TrackerList::new();

    tracker_list.insert(Tracker::new(0, "http://1".to_string()));
    tracker_list.insert(Tracker::new(0, "http://2".to_string()));
    tracker_list.insert(Tracker::new(1, "http://3".to_string()));

    assert!(tracker_list.find_url("http://").is_none());
    assert_eq!(tracker_list.find_url("http://1"), Some(0));
    assert_eq!(tracker_list.find_url("http://2"), Some(1));
    assert_eq!(tracker_list.find_url("http://3"), Some(2));
}

#[test]
fn test_can_scrape() {
    let mut tracker_list = TrackerList::new();

    tracker_list.insert_url(0, "http://example.com/announce", false);
    assert!(tracker_list.at(0).unwrap().is_scrapable());
    assert_eq!(
        uri_generate_scrape_url(tracker_list.at(0).unwrap().url()).unwrap(),
        "http://example.com/scrape"
    );

    tracker_list.insert_url(0, "http://example.com/x/announce", false);
    assert!(tracker_list.at(1).unwrap().is_scrapable());
    assert_eq!(
        uri_generate_scrape_url(tracker_list.at(1).unwrap().url()).unwrap(),
        "http://example.com/x/scrape"
    );

    tracker_list.insert_url(0, "http://example.com/announce.php", false);
    assert!(tracker_list.at(2).unwrap().is_scrapable());
    assert_eq!(
        uri_generate_scrape_url(tracker_list.at(2).unwrap().url()).unwrap(),
        "http://example.com/scrape.php"
    );

    tracker_list.insert_url(0, "http://example.com/a", false);
    assert!(!tracker_list.at(3).unwrap().is_scrapable());

    tracker_list.insert_url(0, "http://example.com/announce?x2%0644", false);
    assert!(tracker_list.at(4).unwrap().is_scrapable());
    assert_eq!(
        uri_generate_scrape_url(tracker_list.at(4).unwrap().url()).unwrap(),
        "http://example.com/scrape?x2%0644"
    );

    tracker_list.insert_url(0, "http://example.com/announce?x=2/4", false);
    assert!(!tracker_list.at(5).unwrap().is_scrapable());

    tracker_list.insert_url(0, "http://example.com/x%064announce", false);
    assert!(!tracker_list.at(6).unwrap().is_scrapable());
}

#[test]
fn test_new_peers() {
    let mut tracker_list = TrackerList::new();
    tracker_list.insert(Tracker::new(0, "http://1".to_string()));

    assert_eq!(tracker_list.at(0).unwrap().state().latest_new_peers, 0);
    assert_eq!(tracker_list.at(0).unwrap().state().latest_sum_peers, 0);

    let mut peers = AddressList::new();
    for i in 0..20 {
        peers.push(SocketAddress::new_inet(
            std::net::Ipv4Addr::new(127, 0, 0, 1),
            8000 + i,
        ));
    }

    tracker_list.send_event(0, TrackerEvent::None);
    tracker_list.receive_success(0, &peers, 10, 100);
    assert_eq!(tracker_list.at(0).unwrap().state().latest_new_peers, 10);
    assert_eq!(tracker_list.at(0).unwrap().state().latest_sum_peers, 20);

    tracker_list.send_event(0, TrackerEvent::None);
    tracker_list.receive_failure(0, 105);
    assert_eq!(tracker_list.at(0).unwrap().state().latest_new_peers, 10);
    assert_eq!(tracker_list.at(0).unwrap().state().latest_sum_peers, 20);

    tracker_list.clear_stats();
    assert_eq!(tracker_list.at(0).unwrap().state().latest_new_peers, 0);
    assert_eq!(tracker_list.at(0).unwrap().state().latest_sum_peers, 0);
}

#[test]
fn test_has_active() {
    let mut tracker_list = TrackerList::new();
    tracker_list.insert(Tracker::new(0, "http://0.0".to_string()));
    tracker_list.insert(Tracker::new(0, "http://0.1".to_string()));
    tracker_list.insert(Tracker::new(1, "http://1.0".to_string()));

    assert!(!tracker_list.has_active());
    assert!(!tracker_list.has_active_not_scrape());

    tracker_list.send_event(0, TrackerEvent::None);
    assert!(tracker_list.has_active());
    assert!(tracker_list.has_active_not_scrape());

    let empty_peers = AddressList::new();
    tracker_list.receive_success(0, &empty_peers, 0, 100);
    assert!(!tracker_list.has_active());
    assert!(!tracker_list.has_active_not_scrape());

    tracker_list.send_event(2, TrackerEvent::None);
    assert!(tracker_list.has_active());
    tracker_list.receive_success(2, &empty_peers, 0, 101);
    assert!(!tracker_list.has_active());

    // Multiple active
    tracker_list.send_event(0, TrackerEvent::None);
    tracker_list.send_event(1, TrackerEvent::None);
    assert!(tracker_list.has_active());
    tracker_list.receive_success(0, &empty_peers, 0, 102);
    assert!(tracker_list.has_active());
    tracker_list.receive_success(1, &empty_peers, 0, 103);
    assert!(!tracker_list.has_active());

    // Scrape
    tracker_list
        .at_mut(0)
        .unwrap()
        .state_mut()
        .set_scrapable(true);
    tracker_list.send_scrape(0);
    assert!(tracker_list.has_active());
    assert!(!tracker_list.has_active_not_scrape());
    tracker_list.receive_scrape_success(0, 104);
    assert!(!tracker_list.has_active());
}

#[test]
fn test_cycle_and_promote() {
    let mut tracker_list = TrackerList::new();
    tracker_list.insert(Tracker::new(0, "http://0.0".to_string()));
    tracker_list.insert(Tracker::new(0, "http://0.1".to_string()));
    tracker_list.insert(Tracker::new(0, "http://0.2".to_string()));

    assert_eq!(tracker_list.at(0).unwrap().url(), "http://0.0");
    assert_eq!(tracker_list.at(1).unwrap().url(), "http://0.1");
    assert_eq!(tracker_list.at(2).unwrap().url(), "http://0.2");

    tracker_list.cycle_group(0);
    assert_eq!(tracker_list.at(0).unwrap().url(), "http://0.1");
    assert_eq!(tracker_list.at(1).unwrap().url(), "http://0.2");
    assert_eq!(tracker_list.at(2).unwrap().url(), "http://0.0");

    // Promote index 2 ("http://0.0") to front of group 0
    tracker_list.promote(2);
    assert_eq!(tracker_list.at(0).unwrap().url(), "http://0.0");
}
