//! Regression tests for tracker announce accounting through the real
//! `DownloadNetworkCoordinator` loop.
//!
//! These cover the linuxtracker upload-reporting failure chain:
//! - the first announce carries `event=started` and the live counters,
//! - a completed download transmits `event=completed` (snatched credit),
//! - periodic updates carry NO event and FRESH uploaded/downloaded values,
//! - coordinator shutdown transmits `event=stopped`,
//! - one slow/dead tracker cannot stall announces to healthy ones.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use lt_common::hash_string::HashString;
use lt_crypto::sha1::sha1_hash;
use lt_engine::constructor::TorrentMetadata;
use lt_engine::download::Download;
use lt_engine::network::{set_announce_jitter_max_ms, DownloadNetworkCoordinator};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// Minimal bencoded HTTP tracker that records every announce query string.
struct MockTracker {
    requests: Arc<Mutex<Vec<String>>>,
    port: u16,
    handle: tokio::task::JoinHandle<()>,
}

impl MockTracker {
    async fn start(interval: u64) -> Self {
        Self::start_with_delay(interval, Duration::ZERO).await
    }

    /// Mock tracker that waits `delay` before answering, emulating a slow
    /// tracker or a long permit wait so duplicate-spawn bugs become visible.
    async fn start_with_delay(interval: u64, delay: Duration) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let requests: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let req_clone = Arc::clone(&requests);
        let handle = tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    break;
                };
                let req_clone = Arc::clone(&req_clone);
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 8192];
                    let Ok(n) = socket.read(&mut buf).await else {
                        return;
                    };
                    let text = String::from_utf8_lossy(&buf[..n]).to_string();
                    // First line: GET /announce?... HTTP/1.1
                    if let Some(line) = text.lines().next() {
                        let path = line.split_whitespace().nth(1).unwrap_or("");
                        req_clone.lock().unwrap().push(path.to_string());
                    }
                    if !delay.is_zero() {
                        tokio::time::sleep(delay).await;
                    }
                    let body = format!(
                        "d8:intervali{interval}e12:min intervali{interval}e8:completei5e10:incompletei2e5:peers0:e"
                    );
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = socket.write_all(response.as_bytes()).await;
                    let _ = socket.shutdown().await;
                });
            }
        });
        Self {
            requests,
            port,
            handle,
        }
    }

    fn url(&self) -> String {
        format!("http://127.0.0.1:{}/announce", self.port)
    }

    fn requests(&self) -> Vec<String> {
        self.requests.lock().unwrap().clone()
    }

    /// Waits until a request matching `pred` arrives (or times out).
    async fn wait_for<F: Fn(&str) -> bool>(&self, pred: F, timeout_secs: u64) -> Option<String> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(timeout_secs);
        loop {
            if let Some(found) = self.requests().into_iter().find(|r| pred(r)) {
                return Some(found);
            }
            if tokio::time::Instant::now() >= deadline {
                return None;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
}

fn metadata(announce_url: String) -> TorrentMetadata {
    let piece = vec![0xABu8; 16384];
    let hash = sha1_hash(&piece);
    TorrentMetadata {
        info_hash: HashString::new(sha1_hash(&hash)),
        name: "announce-events".into(),
        piece_length: 16384,
        pieces: hash.to_vec(),
        num_pieces: 1,
        total_size: 16384,
        is_multi_file: false,
        files: Vec::new(),
        announce: Some(announce_url.clone()),
        announce_list: vec![vec![announce_url]],
        is_private: false,
        is_meta_download: false,
        v2: None,
    }
}

fn query_param<'a>(query: &'a str, key: &str) -> Option<&'a str> {
    query.split('&').find_map(|part| {
        let (k, v) = part.split_once('=')?;
        // strip leading "?path?" from the first parameter
        let k = k.rsplit('?').next().unwrap_or(k);
        if k == key {
            Some(v)
        } else {
            None
        }
    })
}

#[tokio::test]
async fn announce_events_and_fresh_counters_reach_tracker() {
    set_announce_jitter_max_ms(0);
    let tracker = MockTracker::start(600).await;
    let meta = metadata(tracker.url());

    let mut dl = Download::new(meta);
    dl.set_disk_thread(None);
    dl.open().unwrap();
    dl.try_start().unwrap();
    // Seed some upload accounting before the first announce.
    dl.record_upload(4096);
    let download = Arc::new(Mutex::new(dl));

    let peer_id = HashString::new([0x77; 20]);
    let coordinator = DownloadNetworkCoordinator::new(Arc::clone(&download), peer_id, 50000, None);
    let stop_tx = coordinator.spawn();

    // 1) First announce: event=started, live uploaded value, stable key, port.
    let started = tracker
        .wait_for(|q| q.contains("event=started"), 15)
        .await
        .expect("started announce must arrive");
    assert_eq!(query_param(&started, "uploaded"), Some("4096"));
    assert_eq!(query_param(&started, "port"), Some("50000"));
    assert_eq!(query_param(&started, "left"), Some("16384"));
    let key = query_param(&started, "key").expect("key parameter present");
    assert_ne!(key, "12345678", "hardcoded legacy key must not be used");

    // 2) Complete the download -> event=completed must be transmitted.
    //    Wait until the started announce was applied (tracker in use), otherwise
    //    send_completed_event() has no in-use tracker to queue the event on.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        let in_use = download
            .lock()
            .unwrap()
            .tracker_controller()
            .tracker_list
            .at(0)
            .map(|t| t.is_in_use())
            .unwrap_or(false);
        if in_use {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "started announce success must be applied"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    {
        let mut dl = download.lock().unwrap();
        let payload = vec![0xABu8; 16384];
        assert!(dl.receive_block(0, 0, &payload).unwrap());
        assert!(dl.is_seeder());
        // The peer worker normally records downloaded bytes; simulate that here.
        dl.set_downloaded_bytes(16384);
    }
    let completed = tracker
        .wait_for(|q| q.contains("event=completed"), 15)
        .await
        .expect("completed announce must arrive (snatched credit)");
    assert_eq!(query_param(&completed, "left"), Some("0"));

    // 3) Periodic update: no event, FRESH counters. Wait until the completed
    //    announce result was applied (otherwise send_event rejects the update
    //    while the previous request is still marked in flight, and the success
    //    would overwrite the rewound timestamp).
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        let applied = {
            let dl = download.lock().unwrap();
            let t = dl.tracker_controller().tracker_list.at(0).unwrap();
            t.state().counters.success_counter >= 2 && !t.is_requesting()
        };
        if applied {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "completed announce success must be applied"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    // Make the tracker due again by rewinding its last success, bump upload
    // accounting, and queue a plain update event (the same per-tracker mark
    // `send_update_event` produces).
    {
        let mut dl = download.lock().unwrap();
        dl.record_upload(8192);
        let controller = dl.tracker_controller_mut();
        let now = controller.cached_time();
        {
            let state = controller.tracker_list.at_mut(0).unwrap().state_mut();
            state.counters.success_time_last = now.saturating_sub(700);
            state.counters.failed_counter = 0;
        }
        controller
            .tracker_list
            .send_event(0, lt_tracker::TrackerEvent::None);
    }
    let update = tracker
        .wait_for(
            |q| !q.contains("event=") && query_param(q, "uploaded") == Some("12288"),
            20,
        )
        .await
        .expect("periodic update with fresh uploaded counters must arrive");
    assert_eq!(query_param(&update, "downloaded"), Some("16384"));
    assert_eq!(query_param(&update, "key"), Some(key));

    // 4) Shutdown -> event=stopped (best effort, bounded).
    let _ = stop_tx.send(true);
    let stopped = tracker
        .wait_for(|q| q.contains("event=stopped"), 15)
        .await
        .expect("stopped announce must arrive on coordinator shutdown");
    assert_eq!(query_param(&stopped, "uploaded"), Some("12288"));

    tracker.handle.abort();
    set_announce_jitter_max_ms(30_000);
}

#[tokio::test]
async fn dead_udp_tracker_does_not_stall_http_announces() {
    set_announce_jitter_max_ms(0);
    let tracker = MockTracker::start(600).await;
    let mut meta = metadata(tracker.url());
    // Tier 0: a black-hole UDP tracker (nothing listens on this port).
    // Tier 1: the healthy HTTP mock tracker.
    meta.announce_list = vec![
        vec!["udp://127.0.0.1:1/announce".to_string()],
        vec![tracker.url()],
    ];
    meta.announce = Some("udp://127.0.0.1:1/announce".to_string());

    let mut dl = Download::new(meta);
    dl.set_disk_thread(None);
    dl.open().unwrap();
    dl.try_start().unwrap();
    let download = Arc::new(Mutex::new(dl));

    let peer_id = HashString::new([0x78; 20]);
    let coordinator = DownloadNetworkCoordinator::new(Arc::clone(&download), peer_id, 50000, None);
    let stop_tx = coordinator.spawn();

    // The HTTP announce must succeed promptly even though the UDP tracker in
    // the same session is unreachable (bounded retries, concurrent tasks).
    let started = tracker
        .wait_for(|q| q.contains("event=started"), 20)
        .await
        .expect("healthy HTTP tracker must not be starved by a dead UDP tracker");
    assert_eq!(query_param(&started, "port"), Some("50000"));

    let _ = stop_tx.send(true);
    tracker.handle.abort();
    set_announce_jitter_max_ms(30_000);
}

#[tokio::test]
async fn slow_tracker_is_not_announced_twice_while_in_flight() {
    // Regression: the selection used to re-spawn any REQUESTING tracker on
    // every wake-up (~1s while queued work exists). A request waiting several
    // seconds for a permit/response was therefore multiplied into many
    // concurrent announces of the same event (observed as success_counter
    // bursts of 20+ within one interval on production trackers).
    set_announce_jitter_max_ms(0);
    let tracker = MockTracker::start_with_delay(600, Duration::from_secs(4)).await;
    let meta = metadata(tracker.url());

    let mut dl = Download::new(meta);
    dl.set_disk_thread(None);
    dl.open().unwrap();
    dl.try_start().unwrap();
    let download = Arc::new(Mutex::new(dl));

    let peer_id = HashString::new([0x79; 20]);
    let coordinator = DownloadNetworkCoordinator::new(Arc::clone(&download), peer_id, 50000, None);
    let stop_tx = coordinator.spawn();

    // Wait long enough for several loop wake-ups while the first announce is
    // still in flight (4s response delay), then a bit past completion.
    tokio::time::sleep(Duration::from_secs(7)).await;
    let started_count = tracker
        .requests()
        .iter()
        .filter(|q| q.contains("event=started"))
        .count();
    assert_eq!(
        started_count,
        1,
        "exactly one started announce may be in flight/completed, got {}: {:?}",
        started_count,
        tracker.requests()
    );

    let _ = stop_tx.send(true);
    tracker.handle.abort();
    set_announce_jitter_max_ms(30_000);
}
