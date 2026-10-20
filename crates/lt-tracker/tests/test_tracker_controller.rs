use lt_net::addr::AddressList;
use lt_tracker::controller::{ControllerFlags, TrackerController};
use lt_tracker::list::TrackerList;
use lt_tracker::state::TrackerEvent;
use lt_tracker::tracker::Tracker;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

#[test]
fn test_controller_lifecycle() {
    let mut list = TrackerList::new();
    list.insert(Tracker::new(0, "http://1".to_string()));

    let mut controller = TrackerController::new(list);
    assert!(!controller.is_active());
    assert!(!controller.is_timeout_queued());

    controller.enable(0);
    assert!(controller.is_active());
    assert!(controller.flags.contains(ControllerFlags::ACTIVE));

    controller.disable();
    assert!(!controller.is_active());
    assert!(!controller.is_timeout_queued());
}

#[test]
fn test_events() {
    let mut list = TrackerList::new();
    list.insert(Tracker::new(0, "http://1".to_string()));
    list.insert(Tracker::new(0, "http://2".to_string()));

    let mut controller = TrackerController::new(list);
    controller.enable(0);

    // Start event
    controller.send_start_event();
    assert!(controller.flags.contains(ControllerFlags::SEND_START));
    assert_eq!(
        controller.tracker_list.at(0).unwrap().state().latest_event,
        TrackerEvent::Started
    );
    assert!(controller.is_promiscuous_mode());
    assert!(controller.is_timeout_queued());

    // Completed event
    let empty_peers = AddressList::new();
    controller.receive_success(0, &empty_peers);
    assert!(controller.tracker_list.at(0).unwrap().is_in_use());

    controller.send_completed_event();
    assert!(controller.flags.contains(ControllerFlags::SEND_COMPLETED));
    assert_eq!(
        controller.tracker_list.at(0).unwrap().state().latest_event,
        TrackerEvent::Completed
    );

    // Stop event
    controller.send_stop_event();
    assert!(controller.flags.contains(ControllerFlags::SEND_STOP));
    assert_eq!(
        controller.tracker_list.at(0).unwrap().state().latest_event,
        TrackerEvent::Stopped
    );
}

#[test]
fn test_exponential_backoff() {
    let mut list = TrackerList::new();
    list.insert(Tracker::new(0, "http://1".to_string()));

    let mut controller = TrackerController::new(list);
    controller.enable(0);

    let timeout_counter = Arc::new(AtomicU32::new(0));
    let tc = timeout_counter.clone();
    controller.on_timeout = Some(Box::new(move || {
        tc.fetch_add(1, Ordering::SeqCst);
    }));

    // 1st failure: shift = 0 -> 5s
    controller.receive_failure(0, "Connection refused");
    assert!(controller.is_failure_mode());
    assert_eq!(controller.seconds_to_next_timeout(), 5);

    controller.add_cached_time(5);
    assert_eq!(timeout_counter.load(Ordering::SeqCst), 1);

    // 2nd failure: shift = 1 -> 10s
    controller.receive_failure(0, "Connection refused");
    assert_eq!(controller.seconds_to_next_timeout(), 10);

    controller.add_cached_time(10);
    assert_eq!(timeout_counter.load(Ordering::SeqCst), 2);

    // 3rd failure: shift = 2 -> 20s
    controller.receive_failure(0, "Connection refused");
    assert_eq!(controller.seconds_to_next_timeout(), 20);

    // Success resets failure mode and restores normal interval (1800s)
    controller
        .tracker_list
        .at_mut(0)
        .unwrap()
        .state_mut()
        .set_normal_interval(1800);
    let empty_peers = AddressList::new();
    controller.receive_success(0, &empty_peers);
    assert!(!controller.is_failure_mode());
    assert_eq!(controller.seconds_to_next_timeout(), 1800);
}

#[test]
fn test_scrape_scheduling() {
    let mut list = TrackerList::new();
    list.insert(Tracker::new(0, "http://example.com/announce".to_string()));

    let mut controller = TrackerController::new(list);
    // Do not enable the announce controller: enabling queues an announce, and
    // `do_scrape` intentionally skips a tier with an outstanding announce.

    assert!(!controller.is_scrape_queued());

    controller.scrape_request(60);
    assert!(controller.is_scrape_queued());
    assert_eq!(controller.seconds_to_next_scrape(), 60);

    controller.add_cached_time(60);
    assert!(!controller.is_scrape_queued());
    assert_eq!(
        controller.tracker_list.at(0).unwrap().state().latest_event,
        TrackerEvent::Scrape
    );
    assert!(controller.tracker_list.at(0).unwrap().is_requesting());

    controller.receive_scrape_success(0);
    assert!(!controller.tracker_list.at(0).unwrap().is_requesting());
    assert_eq!(
        controller
            .tracker_list
            .at(0)
            .unwrap()
            .state()
            .counters
            .scrape_counter,
        1
    );
}
