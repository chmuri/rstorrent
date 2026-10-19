//! Engine-level scrape/UDP integration checks.
//!
//! These exercise the same tracker controller and router path that
//! `DownloadNetworkCoordinator` uses, without starting a full download.

use std::time::Duration;

use lt_common::hash_string::HashString;
use lt_tracker::client::{scrape_udp_with_router, ScrapeStats};
use lt_tracker::controller::TrackerController;
use lt_tracker::list::TrackerList;
use lt_tracker::udp::{
    ConnectRequest, ConnectResponse, ScrapeFileStats, ScrapeRequest, ScrapeResponse,
};
use lt_tracker::udp_router::UdpRouter;
use tokio::net::UdpSocket;

fn id(byte: u8) -> HashString {
    HashString::new([byte; 20])
}

#[tokio::test]
async fn udp_scrape_applies_stats_and_schedules_next_scrape() {
    let info_hash = id(0x91);
    let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let port = socket.local_addr().unwrap().port();

    let server = tokio::spawn(async move {
        let mut buffer = [0u8; 2048];

        let (read, client) = socket.recv_from(&mut buffer).await.unwrap();
        let connect = ConnectRequest::decode(&buffer[..read]).unwrap();
        socket
            .send_to(
                &ConnectResponse::new(connect.transaction_id, 0x1020_3040_5060_7080).encode(),
                client,
            )
            .await
            .unwrap();

        let (read, _) = socket.recv_from(&mut buffer).await.unwrap();
        let scrape = ScrapeRequest::decode(&buffer[..read]).unwrap();
        socket
            .send_to(
                &ScrapeResponse {
                    transaction_id: scrape.transaction_id,
                    stats: vec![ScrapeFileStats {
                        seeders: 12,
                        completed: 34,
                        leechers: 56,
                    }],
                }
                .encode(),
                client,
            )
            .await
            .unwrap();
    });

    let mut tracker_list = TrackerList::new();
    tracker_list.insert_url(0, &format!("udp://127.0.0.1:{port}/announce"), false);
    assert!(tracker_list.at(0).unwrap().is_scrapable());

    let mut controller = TrackerController::new(tracker_list);
    // Do not enable the announce controller: `do_scrape` intentionally skips a
    // tier that already has an outstanding announce.
    controller.scrape_request(0);
    controller.do_scrape();

    let mut router = UdpRouter::new();
    let stats = tokio::time::timeout(
        Duration::from_secs(10),
        scrape_udp_with_router(
            controller.tracker_list.at(0).unwrap().url(),
            info_hash,
            &mut router,
        ),
    )
    .await
    .expect("UDP scrape timed out")
    .unwrap();

    assert_eq!(
        stats,
        ScrapeStats {
            seeders: 12,
            completed: 34,
            leechers: 56,
        }
    );

    controller.receive_scrape_stats(0, stats);

    let state = controller.tracker_list.at(0).unwrap().state();
    assert_eq!(state.scrape_complete, 12);
    assert_eq!(state.scrape_incomplete, 56);
    assert_eq!(state.scrape_downloaded, 34);
    assert_eq!(state.counters.scrape_counter, 1);
    assert!(!controller.is_scrape_in_flight());
    assert!(controller.is_scrape_queued());
    assert_eq!(router.active_transaction_count(), 0);

    server.await.unwrap();
}
