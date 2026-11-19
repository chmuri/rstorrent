//! Integration tests for the RSS manager: full feed -> refresh -> match ->
//! record flows against a manual `TcpListener` mock HTTP server, plus
//! scheduling, manual downloads, dry-runs and persistence across restarts.

use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rt_rss::manager::RssManager;
use rt_rss::store::Store;
use rt_rss::{
    DownloadResult, DownloadStatus, Feed, FeedPatch, ItemFilter, RssConfig, RssError, RssRule,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;

const HASH: &str = "0123456789abcdef0123456789abcdef01234567";
const HASH2: &str = "fedcba9876543210fedcba9876543210fedcba98";

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(name: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("rt_rss_test_manager_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Self { path }
    }

    fn store_path(&self) -> PathBuf {
        self.path.join("rss.json")
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

fn manager(dir: &TempDir) -> RssManager {
    RssManager::new(dir.store_path(), RssConfig::default()).unwrap()
}

async fn read_request_head(socket: &mut TcpStream) -> String {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 1024];
    loop {
        let read = socket.read(&mut chunk).await.unwrap();
        if read == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..read]);
        if buf.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
    }
    String::from_utf8_lossy(&buf).into_owned()
}

fn http_response(status: &str, extra_headers: &str, body: &str) -> String {
    format!(
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n{extra_headers}\r\n{body}",
        body.len()
    )
}

/// Serves one canned response per connection, in order. Returns the feed
/// URL and a handle yielding the received request heads.
async fn spawn_server(replies: Vec<String>) -> (String, JoinHandle<Vec<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/rss.xml", listener.local_addr().unwrap());
    let handle = tokio::spawn(async move {
        let mut heads = Vec::new();
        for reply in replies {
            let (mut socket, _) = listener.accept().await.unwrap();
            heads.push(read_request_head(&mut socket).await);
            let _ = socket.write_all(reply.as_bytes()).await;
        }
        heads
    });
    (url, handle)
}

async fn finish(handle: JoinHandle<Vec<String>>) -> Vec<String> {
    tokio::time::timeout(Duration::from_secs(10), handle)
        .await
        .expect("mock server did not finish")
        .unwrap()
}

fn xml_two_items() -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0"><channel><title>Test Feed</title>
<item><title>My Show S01E01 1080p</title><guid>show-1</guid>
<pubDate>Wed, 15 Nov 2023 06:00:00 GMT</pubDate>
<enclosure url="https://example.com/t/1.torrent" length="1500000000" type="application/x-bittorrent"/></item>
<item><title>Other Movie 720p</title><guid>movie-1</guid>
<pubDate>Tue, 14 Nov 2023 06:00:00 GMT</pubDate>
<enclosure url="magnet:?xt=urn:btih:{HASH}&amp;dn=movie" type="application/x-bittorrent"/></item>
</channel></rss>"#
    )
}

fn xml_three_items() -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0"><channel><title>Test Feed</title>
<item><title>My Show S01E01 1080p</title><guid>show-1</guid>
<pubDate>Wed, 15 Nov 2023 06:00:00 GMT</pubDate>
<enclosure url="https://example.com/t/1.torrent" length="1500000000" type="application/x-bittorrent"/></item>
<item><title>Other Movie 720p</title><guid>movie-1</guid>
<pubDate>Tue, 14 Nov 2023 06:00:00 GMT</pubDate>
<enclosure url="magnet:?xt=urn:btih:{HASH}&amp;dn=movie" type="application/x-bittorrent"/></item>
<item><title>My Show S01E02 1080p</title><guid>show-2</guid>
<pubDate>Thu, 16 Nov 2023 06:00:00 GMT</pubDate>
<enclosure url="https://example.com/t/2.torrent" length="2000000000" type="application/x-bittorrent"/></item>
</channel></rss>"#
    )
}

fn xml_five_items() -> String {
    r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0"><channel><title>Five Feed</title>
<item><title>Alpha 1080p</title><guid>alpha-1</guid><pubDate>Wed, 15 Nov 2023 06:00:00 GMT</pubDate>
<enclosure url="https://example.com/t/a.torrent" type="application/x-bittorrent"/></item>
<item><title>Beta 720p</title><guid>beta-1</guid><pubDate>Tue, 14 Nov 2023 06:00:00 GMT</pubDate>
<enclosure url="https://example.com/t/b.torrent" type="application/x-bittorrent"/></item>
<item><title>Gamma SHOW rip</title><guid>gamma-1</guid><pubDate>Mon, 13 Nov 2023 06:00:00 GMT</pubDate>
<enclosure url="https://example.com/t/c.torrent" type="application/x-bittorrent"/></item>
<item><title>Delta cam</title><guid>delta-1</guid><pubDate>Sun, 12 Nov 2023 06:00:00 GMT</pubDate>
<enclosure url="https://example.com/t/d.torrent" type="application/x-bittorrent"/></item>
<item><title>Epsilon</title><guid>epsilon-1</guid><pubDate>Sat, 11 Nov 2023 06:00:00 GMT</pubDate>
<enclosure url="https://example.com/t/e.torrent" type="application/x-bittorrent"/></item>
</channel></rss>"#
        .to_string()
}

fn xml_zeta() -> String {
    r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0"><channel><title>Zeta Feed</title>
<item><title>Zeta 1080p</title><guid>zeta-1</guid><pubDate>Thu, 16 Nov 2023 06:00:00 GMT</pubDate>
<enclosure url="https://example.com/t/z.torrent" type="application/x-bittorrent"/></item>
</channel></rss>"#
        .to_string()
}

fn show_rule() -> RssRule {
    RssRule {
        id: 0,
        name: "show".to_string(),
        enabled: true,
        feed_ids: Vec::new(),
        priority: 0,
        match_title: Some("show.*1080p".to_string()),
        exclude_title: None,
        min_size_bytes: None,
        max_size_bytes: None,
        episode: None,
        save_path: Some("/downloads/show".to_string()),
        label: Some("show".to_string()),
        add_paused: true,
        cooldown_secs: None,
    }
}

fn all_items() -> ItemFilter {
    ItemFilter {
        feed_id: None,
        search: None,
        limit: 100,
        offset: 0,
    }
}

fn patch_enabled(enabled: bool) -> FeedPatch {
    FeedPatch {
        url: None,
        alias: None,
        enabled: Some(enabled),
        refresh_interval_secs: None,
    }
}

fn stored_feed(id: u64, url: String, interval: u64, last_fetch: Option<u64>) -> Feed {
    Feed {
        id,
        url,
        alias: None,
        enabled: true,
        refresh_interval_secs: interval,
        last_fetch_at: last_fetch,
        last_success_at: None,
        last_error: None,
        etag: None,
        last_modified: None,
        created_at: 1_000_000,
    }
}

fn item_id_by_guid(manager: &RssManager, guid: &str) -> u64 {
    let (items, _) = manager.items(&all_items());
    items
        .iter()
        .find(|item| item.guid == guid)
        .unwrap_or_else(|| panic!("item {guid} not stored"))
        .id
}

#[tokio::test]
async fn refresh_matches_rule_then_dedups_second_run() {
    let dir = TempDir::new("refresh_dedup");
    let reply = http_response("200 OK", "ETag: \"v1\"\r\n", &xml_two_items());
    let (url, server) = spawn_server(vec![reply.clone(), reply]).await;
    let manager = manager(&dir);
    let feed = manager
        .add_feed(&url, Some("test".to_string()), None)
        .unwrap();
    let rule = manager.add_rule(show_rule()).unwrap();

    let outcome = manager.refresh_feed(feed.id).await.unwrap();
    assert!(outcome.error.is_none());
    assert_eq!(outcome.new_items, 2);
    assert_eq!(outcome.matches.len(), 1);
    let matched = &outcome.matches[0];
    assert_eq!(matched.rule_id, rule.id);
    assert_eq!(matched.item.title, "My Show S01E01 1080p");
    assert_eq!(matched.action.save_path.as_deref(), Some("/downloads/show"));
    assert_eq!(matched.action.label.as_deref(), Some("show"));
    assert!(matched.action.paused);

    let (items, total) = manager.items(&all_items());
    assert_eq!(total, 2);
    // Newest publication first.
    assert_eq!(items[0].guid, "show-1");
    assert_eq!(items[0].size_bytes, Some(1_500_000_000));
    assert_eq!(items[0].published_at, Some(1_700_028_000));
    assert_eq!(
        items[0].torrent_url.as_deref(),
        Some("https://example.com/t/1.torrent")
    );
    assert_eq!(items[1].guid, "movie-1");
    assert_eq!(items[1].info_hash.as_deref(), Some(HASH));
    assert!(items.iter().all(|item| item.download.is_none()));

    // Same document again: guid dedup means no new items and no matches.
    let second = manager.refresh_feed(feed.id).await.unwrap();
    assert!(second.error.is_none());
    assert_eq!(second.new_items, 0);
    assert!(second.matches.is_empty());
    assert_eq!(manager.items(&all_items()).1, 2);

    let heads = finish(server).await;
    assert!(heads[1].to_lowercase().contains("if-none-match: \"v1\""));
    let feeds = manager.feeds();
    assert!(feeds[0].last_fetch_at.is_some());
    assert!(feeds[0].last_success_at.is_some());
    assert_eq!(feeds[0].etag.as_deref(), Some("\"v1\""));
    assert!(feeds[0].last_error.is_none());
}

#[tokio::test]
async fn not_modified_keeps_items_and_validators() {
    let dir = TempDir::new("not_modified");
    let reply = http_response(
        "200 OK",
        "ETag: \"v1\"\r\nLast-Modified: Wed, 15 Nov 2023 06:00:00 GMT\r\n",
        &xml_two_items(),
    );
    let (url, server) = spawn_server(vec![reply, http_response("304 Not Modified", "", "")]).await;
    let manager = manager(&dir);
    let feed = manager.add_feed(&url, None, None).unwrap();

    let first = manager.refresh_feed(feed.id).await.unwrap();
    assert_eq!(first.new_items, 2);
    let second = manager.refresh_feed(feed.id).await.unwrap();
    assert!(second.error.is_none());
    assert_eq!(second.new_items, 0);
    assert!(second.matches.is_empty());
    assert_eq!(manager.items(&all_items()).1, 2);

    let feeds = manager.feeds();
    assert_eq!(feeds[0].etag.as_deref(), Some("\"v1\""));
    assert_eq!(
        feeds[0].last_modified.as_deref(),
        Some("Wed, 15 Nov 2023 06:00:00 GMT")
    );
    assert!(feeds[0].last_fetch_at.is_some());
    assert!(feeds[0].last_success_at.is_some());
    assert!(feeds[0].last_error.is_none());

    let heads = finish(server).await;
    let head = heads[1].to_lowercase();
    assert!(head.contains("if-none-match: \"v1\""));
    assert!(head.contains("if-modified-since: wed, 15 nov 2023 06:00:00 gmt"));
}

#[tokio::test]
async fn refresh_due_respects_interval_jitter_and_default() {
    let dir = TempDir::new("due_jitter");
    let reply = http_response("200 OK", "", &xml_two_items());
    let (url, server) =
        spawn_server(vec![reply.clone(), reply.clone(), reply.clone(), reply]).await;
    // Seed the store with fixed timestamps so scheduling assertions do not
    // race the wall clock: feeds 1/2 override the interval (100s), feed 3
    // uses the config default (1800s). Jitter is id % 60.
    let base = 1_000_000u64;
    let mut store = Store::default();
    store
        .feeds
        .push(stored_feed(1, format!("{url}?a"), 100, Some(base)));
    store
        .feeds
        .push(stored_feed(2, format!("{url}?b"), 100, Some(base)));
    store
        .feeds
        .push(stored_feed(3, format!("{url}?c"), 0, Some(base)));
    store.next_feed_id = 4;
    store.save(&dir.store_path()).unwrap();
    let manager = manager(&dir);

    assert!(manager.refresh_due(base + 100).await.is_empty());
    // 101 >= 100 + 1: feed 1 due, feed 2 (jitter 2) and feed 3 not yet.
    let due = manager.refresh_due(base + 101).await;
    assert_eq!(due.len(), 1);
    assert_eq!(due[0].feed_id, 1);
    assert_eq!(due[0].new_items, 2);
    // Feed 1 was just refreshed at real time, so only feed 2 is due now.
    let due = manager.refresh_due(base + 102).await;
    assert_eq!(due.len(), 1);
    assert_eq!(due[0].feed_id, 2);
    // Config default 1800 + jitter 3 = 1803.
    assert!(manager.refresh_due(base + 1802).await.is_empty());
    let due = manager.refresh_due(base + 1803).await;
    assert_eq!(due.len(), 1);
    assert_eq!(due[0].feed_id, 3);

    // A never-fetched feed is due regardless of `now`.
    let fresh = manager.add_feed(&format!("{url}?d"), None, None).unwrap();
    assert_eq!(fresh.id, 4);
    let due = manager.refresh_due(base).await;
    assert_eq!(due.len(), 1);
    assert_eq!(due[0].feed_id, 4);

    // Disabled feeds are never due.
    for id in [1, 2, 3, 4] {
        manager.update_feed(id, patch_enabled(false)).unwrap();
    }
    assert!(manager.refresh_due(base + 1_000_000).await.is_empty());
    assert_eq!(finish(server).await.len(), 4);
}

#[tokio::test]
async fn take_manual_rule_action_default_and_already_processed() {
    let dir = TempDir::new("take_manual");
    let (url, server) = spawn_server(vec![http_response("200 OK", "", &xml_three_items())]).await;
    let manager = manager(&dir);
    let feed = manager.add_feed(&url, None, None).unwrap();
    let mut rule = show_rule();
    rule.cooldown_secs = Some(3600);
    let rule = manager.add_rule(rule).unwrap();
    manager.refresh_feed(feed.id).await.unwrap();
    let show1 = item_id_by_guid(&manager, "show-1");
    let show2 = item_id_by_guid(&manager, "show-2");
    let movie = item_id_by_guid(&manager, "movie-1");

    let (item, action, rule_id) = manager.take_manual(show1).unwrap();
    assert_eq!(item.id, show1);
    assert_eq!(rule_id, Some(rule.id));
    assert_eq!(action.save_path.as_deref(), Some("/downloads/show"));
    assert_eq!(action.label.as_deref(), Some("show"));
    assert!(action.paused);

    // No rule matches the movie: default action, no rule.
    let (_, action, rule_id) = manager.take_manual(movie).unwrap();
    assert!(rule_id.is_none());
    assert!(action.save_path.is_none());
    assert!(action.label.is_none());
    assert!(!action.paused);

    // Manual hand-off ignores cooldowns armed by an earlier download.
    manager.record_download(
        show1,
        Some(rule.id),
        DownloadResult::Added(HASH.to_string()),
    );
    let (_, action, rule_id) = manager.take_manual(show2).unwrap();
    assert_eq!(rule_id, Some(rule.id));
    assert_eq!(action.save_path.as_deref(), Some("/downloads/show"));

    // Processed items cannot be taken again; unknown ids are NotFound.
    assert!(matches!(
        manager.take_manual(show1),
        Err(RssError::ItemAlreadyProcessed(id)) if id == show1
    ));
    assert!(matches!(
        manager.take_manual(9999),
        Err(RssError::NotFound("item", 9999))
    ));
    finish(server).await;
}

#[tokio::test]
async fn record_download_history_cooldown_and_statuses() {
    let dir = TempDir::new("record");
    let (url, server) = spawn_server(vec![
        http_response("200 OK", "", &xml_two_items()),
        http_response("200 OK", "", &xml_three_items()),
    ])
    .await;
    let manager = manager(&dir);
    let feed = manager.add_feed(&url, None, None).unwrap();
    let mut rule = show_rule();
    rule.cooldown_secs = Some(3600);
    let rule = manager.add_rule(rule).unwrap();
    let outcome = manager.refresh_feed(feed.id).await.unwrap();
    assert_eq!(outcome.matches.len(), 1);
    let show1 = item_id_by_guid(&manager, "show-1");
    let movie = item_id_by_guid(&manager, "movie-1");

    manager.record_download(
        show1,
        Some(rule.id),
        DownloadResult::Added(HASH.to_string()),
    );
    let (items, _) = manager.items(&all_items());
    let show = items.iter().find(|item| item.id == show1).unwrap();
    let record = show.download.as_ref().unwrap();
    assert_eq!(record.status, DownloadStatus::Added);
    assert_eq!(record.torrent_hash.as_deref(), Some(HASH));
    assert_eq!(record.rule_id, Some(rule.id));
    assert!(record.error.is_none());
    assert!(record.at > 0);
    let history = manager.history(10);
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].item_id, show1);
    assert_eq!(history[0].feed_id, feed.id);
    assert_eq!(history[0].title, "My Show S01E01 1080p");
    assert_eq!(history[0].rule_id, Some(rule.id));
    assert_eq!(history[0].status, DownloadStatus::Added);
    assert_eq!(history[0].torrent_hash.as_deref(), Some(HASH));
    let cooldown = Store::load(&dir.store_path()).unwrap().cooldowns;
    assert_eq!(cooldown.get(&rule.id), Some(&record.at));

    // The armed cooldown suppresses the next matching item.
    let second = manager.refresh_feed(feed.id).await.unwrap();
    assert_eq!(second.new_items, 1);
    assert!(second.matches.is_empty());
    let show2 = item_id_by_guid(&manager, "show-2");

    // Manual Failed result: error recorded, no hash, no cooldown change.
    manager.record_download(movie, None, DownloadResult::Failed("boom".to_string()));
    let (items, _) = manager.items(&all_items());
    let movie_item = items.iter().find(|item| item.id == movie).unwrap();
    let record = movie_item.download.as_ref().unwrap();
    assert_eq!(record.status, DownloadStatus::Failed);
    assert!(record.torrent_hash.is_none());
    assert_eq!(record.error.as_deref(), Some("boom"));
    assert!(record.rule_id.is_none());

    // Duplicate result keeps the cooldown untouched.
    let before = Store::load(&dir.store_path()).unwrap().cooldowns;
    manager.record_download(
        show2,
        Some(rule.id),
        DownloadResult::Duplicate(HASH2.to_string()),
    );
    let after = Store::load(&dir.store_path()).unwrap().cooldowns;
    assert_eq!(before.get(&rule.id), after.get(&rule.id));
    let (items, _) = manager.items(&all_items());
    let show2_item = items.iter().find(|item| item.id == show2).unwrap();
    let record = show2_item.download.as_ref().unwrap();
    assert_eq!(record.status, DownloadStatus::Duplicate);
    assert_eq!(record.torrent_hash.as_deref(), Some(HASH2));

    // Newest first: show2 (Duplicate), movie (Failed), show1 (Added).
    let history = manager.history(10);
    assert_eq!(history.len(), 3);
    assert_eq!(history[0].item_id, show2);
    assert_eq!(history[0].status, DownloadStatus::Duplicate);
    assert_eq!(history[1].item_id, movie);
    assert_eq!(history[1].status, DownloadStatus::Failed);
    assert_eq!(history[1].error.as_deref(), Some("boom"));
    assert_eq!(history[2].item_id, show1);

    // Unknown item ids are ignored without touching history.
    manager.record_download(9999, None, DownloadResult::Failed("x".to_string()));
    assert_eq!(manager.history(10).len(), 3);
    finish(server).await;
}

#[tokio::test]
async fn test_rule_dry_run_does_not_persist() {
    let dir = TempDir::new("dry_run");
    let (url, server) = spawn_server(vec![http_response("200 OK", "", &xml_two_items())]).await;
    let manager = manager(&dir);
    let feed = manager.add_feed(&url, None, None).unwrap();
    let rule = manager.add_rule(show_rule()).unwrap();
    manager.refresh_feed(feed.id).await.unwrap();

    let before = std::fs::read(dir.store_path()).unwrap();
    let matched = manager.test_rule(&show_rule(), None, 10).unwrap();
    assert_eq!(matched.len(), 1);
    assert_eq!(matched[0].guid, "show-1");
    assert!(manager.test_rule(&show_rule(), None, 0).unwrap().is_empty());
    assert!(manager
        .test_rule(&show_rule(), Some(999), 10)
        .unwrap()
        .is_empty());
    assert_eq!(
        manager
            .test_rule(&show_rule(), Some(feed.id), 10)
            .unwrap()
            .len(),
        1
    );
    assert_eq!(std::fs::read(dir.store_path()).unwrap(), before);

    let mut invalid = show_rule();
    invalid.match_title = Some("(unclosed".to_string());
    assert!(matches!(
        manager.test_rule(&invalid, None, 10),
        Err(RssError::InvalidRule(_))
    ));

    // Dry-run ignores both cooldowns and the processed flag.
    let show1 = item_id_by_guid(&manager, "show-1");
    manager.record_download(
        show1,
        Some(rule.id),
        DownloadResult::Added(HASH.to_string()),
    );
    let mut cooldown_rule = show_rule();
    cooldown_rule.cooldown_secs = Some(3600);
    assert_eq!(
        manager.test_rule(&cooldown_rule, None, 10).unwrap().len(),
        1
    );
    assert_ne!(std::fs::read(dir.store_path()).unwrap(), before);
    finish(server).await;
}

#[tokio::test]
async fn remove_feed_deletes_items_and_persists() {
    let dir = TempDir::new("remove_feed");
    let (url, server) = spawn_server(vec![http_response("200 OK", "", &xml_two_items())]).await;
    let mgr = manager(&dir);
    let feed = mgr.add_feed(&url, None, None).unwrap();
    mgr.add_rule(show_rule()).unwrap();
    mgr.refresh_feed(feed.id).await.unwrap();
    assert_eq!(mgr.items(&all_items()).1, 2);

    mgr.remove_feed(feed.id).unwrap();
    assert!(mgr.feeds().is_empty());
    assert_eq!(mgr.items(&all_items()).1, 0);
    assert_eq!(mgr.rules().len(), 1);
    assert!(matches!(
        mgr.remove_feed(feed.id),
        Err(RssError::NotFound("feed", id)) if id == feed.id
    ));

    let restarted = manager(&dir);
    assert!(restarted.feeds().is_empty());
    assert_eq!(restarted.items(&all_items()).1, 0);
    finish(server).await;
}

#[tokio::test]
async fn items_filter_search_and_pagination() {
    let dir = TempDir::new("items_query");
    let (url, server) = spawn_server(vec![
        http_response("200 OK", "", &xml_five_items()),
        http_response("200 OK", "", &xml_zeta()),
    ])
    .await;
    let manager = manager(&dir);
    let feed_a = manager.add_feed(&format!("{url}?a"), None, None).unwrap();
    let feed_b = manager.add_feed(&format!("{url}?b"), None, None).unwrap();
    let outcomes = manager.refresh_all().await;
    assert_eq!(outcomes.len(), 2);
    assert!(outcomes.iter().all(|outcome| outcome.error.is_none()));

    let (page, total) = manager.items(&all_items());
    assert_eq!(total, 6);
    assert_eq!(page[0].guid, "zeta-1");

    let scoped = ItemFilter {
        feed_id: Some(feed_b.id),
        ..all_items()
    };
    let (page, total) = manager.items(&scoped);
    assert_eq!(total, 1);
    assert_eq!(page[0].guid, "zeta-1");

    let search = ItemFilter {
        search: Some("SHOW".to_string()),
        ..all_items()
    };
    let (page, total) = manager.items(&search);
    assert_eq!(total, 1);
    assert_eq!(page[0].guid, "gamma-1");

    let search = ItemFilter {
        search: Some("1080P".to_string()),
        ..all_items()
    };
    let (page, total) = manager.items(&search);
    assert_eq!(total, 2);
    assert_eq!(page[0].guid, "zeta-1");
    assert_eq!(page[1].guid, "alpha-1");

    let page_filter = |offset: usize| ItemFilter {
        feed_id: Some(feed_a.id),
        limit: 2,
        offset,
        ..all_items()
    };
    let (page, total) = manager.items(&page_filter(0));
    assert_eq!(total, 5);
    let guids: Vec<&str> = page.iter().map(|item| item.guid.as_str()).collect();
    assert_eq!(guids, ["alpha-1", "beta-1"]);
    let (page, _) = manager.items(&page_filter(2));
    let guids: Vec<&str> = page.iter().map(|item| item.guid.as_str()).collect();
    assert_eq!(guids, ["gamma-1", "delta-1"]);
    let (page, _) = manager.items(&page_filter(4));
    assert_eq!(page.len(), 1);
    assert_eq!(page[0].guid, "epsilon-1");
    let (page, total) = manager.items(&page_filter(5));
    assert_eq!(total, 5);
    assert!(page.is_empty());
    finish(server).await;
}

#[tokio::test]
async fn persistence_survives_restart() {
    let dir = TempDir::new("restart");
    let (url, server) = spawn_server(vec![http_response(
        "200 OK",
        "ETag: \"v1\"\r\n",
        &xml_two_items(),
    )])
    .await;
    let feed_id;
    let rule_id;
    let show1;
    {
        let manager = manager(&dir);
        let feed = manager
            .add_feed(&url, Some("kept".to_string()), Some(120))
            .unwrap();
        let rule = manager.add_rule(show_rule()).unwrap();
        let outcome = manager.refresh_feed(feed.id).await.unwrap();
        assert_eq!(outcome.matches.len(), 1);
        show1 = item_id_by_guid(&manager, "show-1");
        manager.record_download(
            show1,
            Some(rule.id),
            DownloadResult::Added(HASH.to_string()),
        );
        manager.set_config(RssConfig {
            default_refresh_interval_secs: 60,
            ..RssConfig::default()
        });
        feed_id = feed.id;
        rule_id = rule.id;
    }

    // A fresh manager on the same file recovers the full state.
    let manager = manager(&dir);
    let feeds = manager.feeds();
    assert_eq!(feeds.len(), 1);
    assert_eq!(feeds[0].id, feed_id);
    assert_eq!(feeds[0].alias.as_deref(), Some("kept"));
    assert_eq!(feeds[0].refresh_interval_secs, 120);
    assert_eq!(feeds[0].etag.as_deref(), Some("\"v1\""));
    assert!(feeds[0].last_fetch_at.is_some());
    assert!(feeds[0].last_success_at.is_some());
    assert_eq!(manager.rules().len(), 1);
    assert_eq!(manager.rules()[0].id, rule_id);
    let (items, total) = manager.items(&all_items());
    assert_eq!(total, 2);
    let show = items.iter().find(|item| item.id == show1).unwrap();
    let record = show.download.as_ref().unwrap();
    assert_eq!(record.status, DownloadStatus::Added);
    assert_eq!(record.torrent_hash.as_deref(), Some(HASH));
    assert_eq!(record.rule_id, Some(rule_id));
    assert_eq!(manager.history(10).len(), 1);
    assert!(matches!(
        manager.take_manual(show1),
        Err(RssError::ItemAlreadyProcessed(id)) if id == show1
    ));
    assert!(Store::load(&dir.store_path())
        .unwrap()
        .cooldowns
        .contains_key(&rule_id));
    // The config itself is not part of the rss.json schema.
    assert_eq!(
        manager.config().default_refresh_interval_secs,
        RssConfig::default().default_refresh_interval_secs
    );
    finish(server).await;
}

#[tokio::test]
async fn feed_and_rule_crud_validation() {
    let dir = TempDir::new("crud");
    let manager = manager(&dir);

    assert!(matches!(
        manager.add_feed("file:///tmp/x", None, None),
        Err(RssError::InvalidUrl(_))
    ));
    assert!(matches!(
        manager.add_feed("not a url", None, None),
        Err(RssError::InvalidUrl(_))
    ));
    let feed = manager
        .add_feed("https://example.com/a.xml", Some("a".to_string()), Some(60))
        .unwrap();
    assert_eq!(feed.id, 1);
    assert!(feed.enabled);
    assert_eq!(feed.refresh_interval_secs, 60);
    assert!(feed.created_at > 0);
    assert!(matches!(
        manager.add_feed("https://example.com/a.xml", None, None),
        Err(RssError::DuplicateFeed(_))
    ));

    assert!(matches!(
        manager.update_feed(
            feed.id,
            FeedPatch {
                url: Some("ftp://example.com/x".to_string()),
                ..patch_enabled(true)
            }
        ),
        Err(RssError::InvalidUrl(_))
    ));
    // Setting the feed's own URL is not a duplicate.
    manager
        .update_feed(
            feed.id,
            FeedPatch {
                url: Some("https://example.com/a.xml".to_string()),
                ..patch_enabled(true)
            },
        )
        .unwrap();
    let other = manager
        .add_feed("https://example.com/b.xml", None, None)
        .unwrap();
    assert!(matches!(
        manager.update_feed(
            other.id,
            FeedPatch {
                url: Some("https://example.com/a.xml".to_string()),
                ..patch_enabled(true)
            }
        ),
        Err(RssError::DuplicateFeed(_))
    ));
    let updated = manager
        .update_feed(
            feed.id,
            FeedPatch {
                url: None,
                alias: Some("renamed".to_string()),
                enabled: Some(false),
                refresh_interval_secs: Some(300),
            },
        )
        .unwrap();
    assert_eq!(updated.url, "https://example.com/a.xml");
    assert_eq!(updated.alias.as_deref(), Some("renamed"));
    assert!(!updated.enabled);
    assert_eq!(updated.refresh_interval_secs, 300);
    assert!(matches!(
        manager.update_feed(99, patch_enabled(true)),
        Err(RssError::NotFound("feed", 99))
    ));

    let mut invalid = show_rule();
    invalid.match_title = Some("(unclosed".to_string());
    assert!(matches!(
        manager.add_rule(invalid),
        Err(RssError::InvalidRule(_))
    ));
    let mut incoming = show_rule();
    incoming.id = 999;
    let rule = manager.add_rule(incoming).unwrap();
    assert_eq!(rule.id, 1);
    let mut replacement = show_rule();
    replacement.id = 555;
    replacement.name = "renamed rule".to_string();
    let updated = manager.update_rule(rule.id, replacement).unwrap();
    assert_eq!(updated.id, rule.id);
    assert_eq!(updated.name, "renamed rule");
    assert!(matches!(
        manager.update_rule(99, show_rule()),
        Err(RssError::NotFound("rule", 99))
    ));
    manager.remove_rule(rule.id).unwrap();
    assert!(matches!(
        manager.remove_rule(rule.id),
        Err(RssError::NotFound("rule", id)) if id == rule.id
    ));

    assert_eq!(manager.feeds().len(), 2);
    assert!(manager.rules().is_empty());
    let custom = RssConfig {
        enabled: false,
        default_refresh_interval_secs: 60,
        max_items_per_feed: 10,
        request_timeout_secs: 5,
        max_response_bytes: 1024,
    };
    manager.set_config(custom);
    assert_eq!(manager.config().default_refresh_interval_secs, 60);
    assert!(!manager.config().enabled);
    assert_eq!(manager.config().max_items_per_feed, 10);
}

#[tokio::test]
async fn refresh_http_error_recorded_and_recovers() {
    let dir = TempDir::new("http_error");
    let (url, server) = spawn_server(vec![
        http_response("500 Internal Server Error", "", "oops"),
        http_response("200 OK", "", &xml_two_items()),
    ])
    .await;
    let manager = manager(&dir);
    let feed = manager.add_feed(&url, None, None).unwrap();

    let outcome = manager.refresh_feed(feed.id).await.unwrap();
    assert_eq!(outcome.new_items, 0);
    assert!(outcome.matches.is_empty());
    assert!(outcome.error.as_deref().unwrap().contains("500"));
    let feeds = manager.feeds();
    assert!(feeds[0].last_fetch_at.is_some());
    assert!(feeds[0].last_success_at.is_none());
    assert!(feeds[0].last_error.as_deref().unwrap().contains("500"));
    assert_eq!(manager.items(&all_items()).1, 0);

    // Explicit refresh works even for a disabled feed and clears the error.
    manager.update_feed(feed.id, patch_enabled(false)).unwrap();
    let outcome = manager.refresh_feed(feed.id).await.unwrap();
    assert!(outcome.error.is_none());
    assert_eq!(outcome.new_items, 2);
    let feeds = manager.feeds();
    assert!(feeds[0].last_error.is_none());
    assert!(feeds[0].last_success_at.is_some());
    finish(server).await;
}

#[tokio::test]
async fn refresh_parse_error_recorded() {
    let dir = TempDir::new("parse_error");
    let (url, server) = spawn_server(vec![http_response("200 OK", "", "this is not xml")]).await;
    let manager = manager(&dir);
    let feed = manager.add_feed(&url, None, None).unwrap();
    let outcome = manager.refresh_feed(feed.id).await.unwrap();
    assert_eq!(outcome.new_items, 0);
    assert!(outcome.error.is_some());
    assert_eq!(manager.items(&all_items()).1, 0);
    let feeds = manager.feeds();
    assert!(feeds[0].last_error.is_some());
    assert!(feeds[0].last_fetch_at.is_some());
    assert!(feeds[0].last_success_at.is_none());
    finish(server).await;
}

#[tokio::test]
async fn refresh_due_continues_after_feed_error() {
    let dir = TempDir::new("due_error");
    let (url, server) = spawn_server(vec![http_response("200 OK", "", &xml_two_items())]).await;
    let manager = manager(&dir);
    // Port 1 on loopback refuses connections immediately.
    let bad = manager
        .add_feed("http://127.0.0.1:1/rss.xml", None, None)
        .unwrap();
    let good = manager.add_feed(&url, None, None).unwrap();
    let outcomes = manager.refresh_due(unix_now() + 100_000).await;
    assert_eq!(outcomes.len(), 2);
    assert_eq!(outcomes[0].feed_id, bad.id);
    assert!(outcomes[0].error.is_some());
    assert_eq!(outcomes[0].new_items, 0);
    assert_eq!(outcomes[1].feed_id, good.id);
    assert!(outcomes[1].error.is_none());
    assert_eq!(outcomes[1].new_items, 2);
    let feeds = manager.feeds();
    assert!(feeds.iter().all(|feed| feed.last_fetch_at.is_some()));
    finish(server).await;
}

#[tokio::test]
async fn history_limit_and_clear() {
    let dir = TempDir::new("history");
    let (url, server) = spawn_server(vec![http_response("200 OK", "", &xml_two_items())]).await;
    let manager = manager(&dir);
    let feed = manager.add_feed(&url, None, None).unwrap();
    manager.refresh_feed(feed.id).await.unwrap();
    let show = item_id_by_guid(&manager, "show-1");
    let movie = item_id_by_guid(&manager, "movie-1");

    manager.record_download(show, None, DownloadResult::Added(HASH.to_string()));
    manager.record_download(movie, None, DownloadResult::Duplicate(HASH2.to_string()));
    let newest = manager.history(1);
    assert_eq!(newest.len(), 1);
    assert_eq!(newest[0].item_id, movie);
    let all = manager.history(10);
    assert_eq!(all.len(), 2);
    assert_eq!(all[0].item_id, movie);
    assert_eq!(all[1].item_id, show);

    manager.clear_history();
    assert!(manager.history(10).is_empty());
    assert!(Store::load(&dir.store_path()).unwrap().history.is_empty());
    finish(server).await;
}
