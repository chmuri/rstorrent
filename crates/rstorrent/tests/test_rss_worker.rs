//! Integration tests for the RSS daemon glue (`rstorrent::rss`): feed refresh
//! -> rule match -> torrent added to the session-backed download list, manual
//! magnet downloads, failure recording, cross-feed dedup, and the background
//! worker's refresh/shutdown lifecycle.
//!
//! HTTP is mocked with hand-rolled `TcpListener` servers (one canned reply per
//! connection); torrent fixtures are built in code.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rstorrent::rss::{spawn_worker, RssService};
use rt_core::session::SessionManager;
use rt_core::DownloadList;
use rt_rss::manager::RssManager;
use rt_rss::store::Store;
use rt_rss::{DownloadResult, DownloadStatus, ItemFilter, RssConfig, RssError, RssRule};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;
use tokio::task::JoinHandle;

const MAGNET_HASH: &str = "0123456789abcdef0123456789abcdef01234567";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Self-cleaning temporary directory, unique per test invocation.
struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(label: &str) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let id = COUNTER.fetch_add(1, Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!(
            "rstorrent_rss_worker_{}_{}_{}",
            label,
            std::process::id(),
            id
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("create test dir");
        Self { path }
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// Builds a minimal, valid single-file torrent for `payload`.
fn make_torrent(name: &str, payload: &[u8]) -> Vec<u8> {
    const PIECE_LENGTH: usize = 16384;

    let mut pieces = Vec::new();
    for chunk in payload.chunks(PIECE_LENGTH) {
        pieces.extend_from_slice(&lt_crypto::sha1::sha1_hash(chunk));
    }

    let mut out = Vec::new();
    out.push(b'd');
    out.extend_from_slice(b"4:infod");
    out.extend_from_slice(format!("6:lengthi{}e", payload.len()).as_bytes());
    out.extend_from_slice(format!("4:name{}:{}", name.len(), name).as_bytes());
    out.extend_from_slice(format!("12:piece lengthi{}e", PIECE_LENGTH).as_bytes());
    out.extend_from_slice(format!("6:pieces{}:", pieces.len()).as_bytes());
    out.extend_from_slice(&pieces);
    out.extend_from_slice(b"ee");
    out
}

/// Info hash of a `make_torrent` fixture: SHA1 over the bencoded info dict
/// (everything after the `4:info` key, minus the outer dict's closing `e`).
fn torrent_info_hash(torrent: &[u8]) -> String {
    let marker = b"4:info";
    let start = torrent
        .windows(marker.len())
        .position(|window| window == marker)
        .expect("fixture has an info dict")
        + marker.len();
    let info = &torrent[start..torrent.len() - 1];
    lt_crypto::sha1::sha1_hash(info)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn http_response(status: &str, body: &[u8]) -> Vec<u8> {
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let mut out = head.into_bytes();
    out.extend_from_slice(body);
    out
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

/// Serves one canned response per connection, in order. Returns the base URL
/// and a handle yielding the received request heads.
async fn spawn_server(replies: Vec<Vec<u8>>) -> (String, JoinHandle<Vec<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let handle = tokio::spawn(async move {
        let mut heads = Vec::new();
        for reply in replies {
            let (mut socket, _) = listener.accept().await.unwrap();
            heads.push(read_request_head(&mut socket).await);
            let _ = socket.write_all(&reply).await;
        }
        heads
    });
    (base, handle)
}

async fn finish(handle: JoinHandle<Vec<String>>) -> Vec<String> {
    tokio::time::timeout(Duration::from_secs(10), handle)
        .await
        .expect("mock server did not finish")
        .unwrap()
}

/// Long-lived mock server counting accepted connections, so a test can assert
/// that *no* request was made. Stopped via the returned watch sender.
async fn spawn_counting_server(
    reply: Vec<u8>,
) -> (
    String,
    Arc<AtomicUsize>,
    watch::Sender<bool>,
    JoinHandle<()>,
) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let counter = Arc::new(AtomicUsize::new(0));
    let (stop_tx, mut stop_rx) = watch::channel(false);
    let count = Arc::clone(&counter);
    let handle = tokio::spawn(async move {
        loop {
            tokio::select! {
                accepted = listener.accept() => {
                    let (mut socket, _) = accepted.unwrap();
                    count.fetch_add(1, Ordering::SeqCst);
                    let _ = read_request_head(&mut socket).await;
                    let _ = socket.write_all(&reply).await;
                }
                _ = stop_rx.changed() => break,
            }
        }
    });
    (base, counter, stop_tx, handle)
}

fn active_session(dir: &TempDir) -> Arc<Mutex<SessionManager>> {
    let session_dir = dir.path.join("session");
    std::fs::create_dir_all(&session_dir).unwrap();
    let mut sm = SessionManager::new();
    sm.set_path(session_dir.to_str().unwrap()).unwrap();
    sm.start().unwrap();
    Arc::new(Mutex::new(sm))
}

fn service(
    dir: &TempDir,
    list: Arc<RwLock<DownloadList>>,
    sm: Arc<Mutex<SessionManager>>,
    config: RssConfig,
) -> Arc<RssService> {
    RssService::new(dir.path.join("rss.json"), config, list, sm, None).unwrap()
}

fn feed_xml(torrent_url: &str, guid: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0"><channel><title>RSS Test</title>
<item><title>My Show S01E01 1080p</title><guid>{guid}</guid>
<pubDate>Wed, 15 Nov 2023 06:00:00 GMT</pubDate>
<enclosure url="{torrent_url}" length="1500000000" type="application/x-bittorrent"/></item>
</channel></rss>"#
    )
}

fn magnet_xml() -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0"><channel><title>RSS Test</title>
<item><title>Other Movie 720p</title><guid>movie-1</guid>
<pubDate>Tue, 14 Nov 2023 06:00:00 GMT</pubDate>
<enclosure url="magnet:?xt=urn:btih:{MAGNET_HASH}&amp;dn=movie" type="application/x-bittorrent"/></item>
</channel></rss>"#
    )
}

fn show_rule(save_path: &str) -> RssRule {
    RssRule {
        tags: Vec::new(),
        throttle_name: None,
        seed_limits: None,
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
        save_path: Some(save_path.to_string()),
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

fn item_id_by_guid(rss: &RssManager, guid: &str) -> u64 {
    let (items, _) = rss.items(&all_items());
    items
        .iter()
        .find(|item| item.guid == guid)
        .unwrap_or_else(|| panic!("item {guid} not stored"))
        .id
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn refresh_feed_adds_matched_torrent() {
    let dir = TempDir::new("refresh_add");
    let torrent = make_torrent("rss-file", b"rss integration payload");
    let hash = torrent_info_hash(&torrent);

    let (torrent_base, torrent_server) =
        spawn_server(vec![http_response("200 OK", &torrent)]).await;
    let torrent_url = format!("{torrent_base}/file.torrent");
    let (feed_base, feed_server) = spawn_server(vec![http_response(
        "200 OK",
        feed_xml(&torrent_url, "show-1").as_bytes(),
    )])
    .await;

    let list = Arc::new(RwLock::new(DownloadList::new()));
    let sm = active_session(&dir);
    let svc = service(&dir, Arc::clone(&list), sm, RssConfig::default());
    let save_path = dir.path.join("data");
    let feed = svc
        .rss
        .add_feed(&format!("{feed_base}/rss.xml"), None, None)
        .unwrap();
    let rule = svc
        .rss
        .add_rule(show_rule(save_path.to_str().unwrap()))
        .unwrap();

    let outcome = svc.refresh_feed(feed.id).await.unwrap();
    assert!(outcome.error.is_none());
    assert_eq!(outcome.new_items, 1);
    assert_eq!(outcome.matches.len(), 1);
    assert_eq!(outcome.matches[0].rule_id, rule.id);

    {
        let guard = list.read().unwrap();
        assert_eq!(guard.len(), 1);
        let download = guard.find_hex(&hash).expect("torrent must be in the list");
        let dl = download.read().unwrap();
        assert_eq!(dl.directory(), save_path.to_str().unwrap());
        assert_eq!(dl.custom_field(1), "show");
        assert!(
            !dl.is_active(),
            "add_paused rule must not start the torrent"
        );
    }

    let history = svc.rss.history(10);
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].status, DownloadStatus::Added);
    assert_eq!(history[0].torrent_hash.as_deref(), Some(hash.as_str()));
    assert_eq!(history[0].rule_id, Some(rule.id));

    // The store and the session directory both carry the new download.
    let store = Store::load(&dir.path.join("rss.json")).unwrap();
    assert_eq!(store.items.len(), 1);
    let record = store.items[0].download.as_ref().unwrap();
    assert_eq!(record.status, DownloadStatus::Added);
    assert_eq!(record.torrent_hash.as_deref(), Some(hash.as_str()));
    // Session file names use the upper-case info hash (rTorrent convention).
    assert!(dir
        .path
        .join("session")
        .join(format!("{}.torrent", hash.to_uppercase()))
        .exists());

    finish(feed_server).await;
    finish(torrent_server).await;
}

#[tokio::test]
async fn second_feed_same_torrent_is_duplicate() {
    let dir = TempDir::new("dup_feed");
    let torrent = make_torrent("dup-file", b"dup payload");
    let hash = torrent_info_hash(&torrent);
    let reply = http_response("200 OK", &torrent);
    let (torrent_base, torrent_server) = spawn_server(vec![reply.clone(), reply]).await;
    let torrent_url = format!("{torrent_base}/file.torrent");
    let xml_a = feed_xml(&torrent_url, "a-1");
    let xml_b = feed_xml(&torrent_url, "b-1");
    let (feed_base, feed_server) = spawn_server(vec![
        http_response("200 OK", xml_a.as_bytes()),
        http_response("200 OK", xml_b.as_bytes()),
        http_response("200 OK", xml_a.as_bytes()),
    ])
    .await;

    let list = Arc::new(RwLock::new(DownloadList::new()));
    let sm = active_session(&dir);
    let svc = service(&dir, Arc::clone(&list), sm, RssConfig::default());
    let feed_a = svc
        .rss
        .add_feed(&format!("{feed_base}/rss.xml?a"), None, None)
        .unwrap();
    let feed_b = svc
        .rss
        .add_feed(&format!("{feed_base}/rss.xml?b"), None, None)
        .unwrap();
    svc.rss
        .add_rule(show_rule(dir.path.join("data").to_str().unwrap()))
        .unwrap();

    let first = svc.refresh_feed(feed_a.id).await.unwrap();
    assert_eq!(first.matches.len(), 1);
    assert_eq!(list.read().unwrap().len(), 1);

    // Different guid, same torrent: the engine dedups by info hash.
    let second = svc.refresh_feed(feed_b.id).await.unwrap();
    assert_eq!(second.new_items, 1);
    assert_eq!(second.matches.len(), 1);
    assert_eq!(list.read().unwrap().len(), 1);
    let history = svc.rss.history(10);
    assert_eq!(history.len(), 2);
    assert_eq!(history[0].status, DownloadStatus::Duplicate);
    assert_eq!(history[0].torrent_hash.as_deref(), Some(hash.as_str()));
    assert_eq!(history[1].status, DownloadStatus::Added);

    // Re-refreshing the same feed adds nothing (guid dedup).
    let third = svc.refresh_feed(feed_a.id).await.unwrap();
    assert_eq!(third.new_items, 0);
    assert!(third.matches.is_empty());
    assert_eq!(list.read().unwrap().len(), 1);
    assert_eq!(svc.rss.history(10).len(), 2);

    finish(feed_server).await;
    finish(torrent_server).await;
}

#[tokio::test]
async fn magnet_item_manual_download_adds_placeholder() {
    let dir = TempDir::new("magnet_manual");
    let (feed_base, feed_server) =
        spawn_server(vec![http_response("200 OK", magnet_xml().as_bytes())]).await;
    let list = Arc::new(RwLock::new(DownloadList::new()));
    let sm = Arc::new(Mutex::new(SessionManager::new()));
    let svc = service(&dir, Arc::clone(&list), sm, RssConfig::default());
    let feed = svc
        .rss
        .add_feed(&format!("{feed_base}/rss.xml"), None, None)
        .unwrap();

    let outcome = svc.refresh_feed(feed.id).await.unwrap();
    assert_eq!(outcome.new_items, 1);
    assert!(outcome.matches.is_empty());

    // No rule matches: the manual default action applies and starts the
    // magnet placeholder.
    let item_id = item_id_by_guid(&svc.rss, "movie-1");
    let result = svc.download_item(item_id).await.unwrap();
    assert!(matches!(result, DownloadResult::Added(ref h) if h == MAGNET_HASH));
    {
        let guard = list.read().unwrap();
        assert_eq!(guard.len(), 1);
        let download = guard.find_hex(MAGNET_HASH).unwrap();
        assert!(download.read().unwrap().is_active());
    }
    let history = svc.rss.history(10);
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].status, DownloadStatus::Added);
    assert!(history[0].rule_id.is_none());
    assert!(matches!(
        svc.download_item(item_id).await,
        Err(RssError::ItemAlreadyProcessed(id)) if id == item_id
    ));
    finish(feed_server).await;
}

#[tokio::test]
async fn failed_torrent_fetch_records_failure() {
    let dir = TempDir::new("fetch_fail");
    let (torrent_base, torrent_server) =
        spawn_server(vec![http_response("404 Not Found", b"missing")]).await;
    let torrent_url = format!("{torrent_base}/gone.torrent");
    let (feed_base, feed_server) = spawn_server(vec![http_response(
        "200 OK",
        feed_xml(&torrent_url, "show-1").as_bytes(),
    )])
    .await;

    let list = Arc::new(RwLock::new(DownloadList::new()));
    let sm = Arc::new(Mutex::new(SessionManager::new()));
    let svc = service(&dir, Arc::clone(&list), sm, RssConfig::default());
    let feed = svc
        .rss
        .add_feed(&format!("{feed_base}/rss.xml"), None, None)
        .unwrap();
    svc.rss
        .add_rule(show_rule(dir.path.join("data").to_str().unwrap()))
        .unwrap();

    let outcome = svc.refresh_feed(feed.id).await.unwrap();
    assert!(outcome.error.is_none());
    assert_eq!(outcome.matches.len(), 1);
    assert_eq!(list.read().unwrap().len(), 0);
    let history = svc.rss.history(10);
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].status, DownloadStatus::Failed);
    assert!(history[0].torrent_hash.is_none());
    assert!(history[0].error.as_deref().unwrap().contains("404"));
    finish(feed_server).await;
    finish(torrent_server).await;
}

#[tokio::test]
async fn worker_refreshes_due_feed_and_shuts_down() {
    let dir = TempDir::new("worker");
    let torrent = make_torrent("worker-file", b"worker payload");
    let hash = torrent_info_hash(&torrent);
    let (torrent_base, torrent_server) =
        spawn_server(vec![http_response("200 OK", &torrent)]).await;
    let torrent_url = format!("{torrent_base}/file.torrent");
    let (feed_base, feed_server) = spawn_server(vec![http_response(
        "200 OK",
        feed_xml(&torrent_url, "show-1").as_bytes(),
    )])
    .await;

    let list = Arc::new(RwLock::new(DownloadList::new()));
    let sm = Arc::new(Mutex::new(SessionManager::new()));
    let svc = service(&dir, Arc::clone(&list), sm, RssConfig::default());
    svc.rss
        .add_feed(&format!("{feed_base}/rss.xml"), None, None)
        .unwrap();
    svc.rss
        .add_rule(show_rule(dir.path.join("data").to_str().unwrap()))
        .unwrap();

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let worker = spawn_worker(Arc::clone(&svc), shutdown_rx);

    // The first tick fires immediately; a never-fetched feed is due.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while list.read().unwrap().is_empty() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "worker did not add the torrent"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(list.read().unwrap().find_hex(&hash).is_some());
    assert_eq!(svc.rss.history(10)[0].status, DownloadStatus::Added);

    shutdown_tx.send(true).unwrap();
    tokio::time::timeout(Duration::from_secs(5), worker)
        .await
        .expect("worker did not stop on shutdown")
        .unwrap();
    // Exactly one fetch per server: no extra tick slipped through.
    assert_eq!(finish(feed_server).await.len(), 1);
    assert_eq!(finish(torrent_server).await.len(), 1);
}

#[tokio::test]
async fn worker_with_disabled_config_does_not_fetch() {
    let dir = TempDir::new("worker_disabled");
    let (feed_base, counter, stop_tx, server) =
        spawn_counting_server(http_response("200 OK", magnet_xml().as_bytes())).await;
    let list = Arc::new(RwLock::new(DownloadList::new()));
    let sm = Arc::new(Mutex::new(SessionManager::new()));
    let config = RssConfig {
        enabled: false,
        ..RssConfig::default()
    };
    let svc = service(&dir, Arc::clone(&list), sm, config);
    svc.rss
        .add_feed(&format!("{feed_base}/rss.xml"), None, None)
        .unwrap();

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let worker = spawn_worker(Arc::clone(&svc), shutdown_rx);
    // Give the immediate first tick (and then some) a chance to run.
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(counter.load(Ordering::SeqCst), 0);
    assert_eq!(list.read().unwrap().len(), 0);
    assert!(svc.rss.feeds()[0].last_fetch_at.is_none());

    shutdown_tx.send(true).unwrap();
    tokio::time::timeout(Duration::from_secs(5), worker)
        .await
        .expect("worker did not stop on shutdown")
        .unwrap();
    stop_tx.send(true).unwrap();
    tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .expect("counting server did not stop")
        .unwrap();
}

#[tokio::test]
async fn refresh_due_and_refresh_all_cover_every_feed() {
    let dir = TempDir::new("due_all");
    let torrent_a = make_torrent("file-a", b"payload a");
    let torrent_b = make_torrent("file-b", b"payload b");
    let hash_a = torrent_info_hash(&torrent_a);
    let hash_b = torrent_info_hash(&torrent_b);
    let (torrent_base, torrent_server) = spawn_server(vec![
        http_response("200 OK", &torrent_a),
        http_response("200 OK", &torrent_b),
    ])
    .await;
    let xml_a = feed_xml(&format!("{torrent_base}/a.torrent"), "a-1");
    let xml_b = feed_xml(&format!("{torrent_base}/b.torrent"), "b-1");
    let (feed_base, feed_server) = spawn_server(vec![
        http_response("200 OK", xml_a.as_bytes()),
        http_response("200 OK", xml_b.as_bytes()),
        http_response("200 OK", xml_a.as_bytes()),
        http_response("200 OK", xml_b.as_bytes()),
    ])
    .await;

    let list = Arc::new(RwLock::new(DownloadList::new()));
    let sm = Arc::new(Mutex::new(SessionManager::new()));
    let svc = service(&dir, Arc::clone(&list), sm, RssConfig::default());
    svc.rss
        .add_feed(&format!("{feed_base}/rss.xml?a"), None, None)
        .unwrap();
    svc.rss
        .add_feed(&format!("{feed_base}/rss.xml?b"), None, None)
        .unwrap();
    svc.rss
        .add_rule(show_rule(dir.path.join("data").to_str().unwrap()))
        .unwrap();

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let due = svc.refresh_due(now).await;
    assert_eq!(due.len(), 2);
    assert!(due.iter().all(|outcome| outcome.error.is_none()));
    {
        let guard = list.read().unwrap();
        assert_eq!(guard.len(), 2);
        assert!(guard.find_hex(&hash_a).is_some());
        assert!(guard.find_hex(&hash_b).is_some());
    }

    // refresh_all re-fetches both feeds; guid dedup keeps the list stable.
    let all = svc.refresh_all().await;
    assert_eq!(all.len(), 2);
    assert!(all
        .iter()
        .all(|outcome| outcome.new_items == 0 && outcome.matches.is_empty()));
    assert_eq!(list.read().unwrap().len(), 2);
    assert_eq!(svc.rss.history(10).len(), 2);
    finish(feed_server).await;
    finish(torrent_server).await;
}
