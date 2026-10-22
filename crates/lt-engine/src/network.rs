use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use lt_common::bitfield::Bitfield;
use lt_common::hash_string::HashString;
use lt_common::object::Object;
use lt_crypto::rc4::Rc4;
use lt_crypto::sha1::sha1_hash;
use lt_dht::controller::DhtHandle;
use lt_dht::search::{announce_peer, get_peers};
use lt_net::addr::{AddressList, SocketAddress};
use lt_protocol::connection::PeerConnection;
use lt_protocol::connection::PeerDirection;
use lt_protocol::encryption::EncryptionPolicy;
use lt_protocol::extensions::{
    extension_handshake_payload, ut_metadata_data_payload, ut_metadata_reject_payload,
    ut_metadata_request_payload, ut_pex_payload, UtMetadataMessage, UtPexMessage,
};
use lt_protocol::handshake::HandshakeMessage;
use lt_protocol::message::PeerMessage;
use lt_protocol::mse::{initiate as mse_initiate, MseError, MseSession};
use lt_protocol::request_list::{
    BlockTransfer, RequestList, DEFAULT_BLOCK_SIZE, DEFAULT_PEER_REQUEST_QUEUE,
    MAX_PEER_REQUEST_QUEUE,
};
use lt_tracker::client::{
    announce_http_detailed, announce_udp_detailed_with_router, scrape_with_router, AnnounceParams,
};
use lt_tracker::state::TrackerEvent;
use lt_tracker::udp_router::UdpRouter;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinSet;
use tokio::time::{sleep, timeout};
use tracing::{debug, info, warn};

use crate::download::Download;
use crate::peer_pool::{
    max_connecting_for, normalize_socket_addr, PeerPool, PeerPoolHandle, PeerSource,
};
use crate::resource_manager::GroupChokeChanges;

/// Numeric peer identifier used by the choke queues and delegator.
pub fn peer_number(peer_id: &HashString) -> usize {
    ((peer_id.as_bytes()[0] as usize) << 8) | peer_id.as_bytes()[1] as usize
}

/// Per-peer outgoing request pipeline statistics.
///
/// These are published by every live peer worker and can be read by the
/// `p.*` command surface (for example to expose the request queue depth in
/// the peer list).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PeerRequestStats {
    /// Requests written to the peer and not yet satisfied or cancelled.
    pub queue_depth: usize,
    /// Maximum pipeline size negotiated with the peer (`reqq`, default 250).
    pub queue_limit: usize,
    /// Outstanding endgame duplicate requests for this peer.
    pub endgame_duplicates: usize,
    /// Whether the peer has choked our download pipeline.
    pub choked: bool,
}

type PeerRequestKey = (HashString, HashString);

fn peer_request_metrics() -> &'static Mutex<HashMap<PeerRequestKey, PeerRequestStats>> {
    static METRICS: OnceLock<Mutex<HashMap<PeerRequestKey, PeerRequestStats>>> = OnceLock::new();
    METRICS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn publish_peer_request_stats(
    info_hash: HashString,
    peer_id: HashString,
    request_list: &RequestList,
    choked: bool,
) {
    let stats = PeerRequestStats {
        queue_depth: request_list.queue_depth(),
        queue_limit: request_list.queue_limit(),
        endgame_duplicates: request_list.endgame_count(),
        choked,
    };
    peer_request_metrics()
        .lock()
        .unwrap()
        .insert((info_hash, peer_id), stats);
}

fn remove_peer_request_stats(info_hash: &HashString, peer_id: &HashString) {
    peer_request_metrics()
        .lock()
        .unwrap()
        .remove(&(*info_hash, *peer_id));
}

/// Returns pipeline statistics for one peer, if a worker is currently serving
/// it.
pub fn peer_request_stats(
    info_hash: &HashString,
    peer_id: &HashString,
) -> Option<PeerRequestStats> {
    peer_request_metrics()
        .lock()
        .unwrap()
        .get(&(*info_hash, *peer_id))
        .copied()
}

/// Returns the current outgoing request queue depth for one peer.
pub fn peer_request_queue_depth(info_hash: &HashString, peer_id: &HashString) -> Option<usize> {
    peer_request_stats(info_hash, peer_id).map(|stats| stats.queue_depth)
}

/// Snapshot of all live peers' request pipelines, for multicall/metric views.
pub fn peer_request_stats_snapshot() -> Vec<(HashString, HashString, PeerRequestStats)> {
    peer_request_metrics()
        .lock()
        .unwrap()
        .iter()
        .map(|(&(info_hash, peer_id), &stats)| (info_hash, peer_id, stats))
        .collect()
}

/// LTEP ids we advertise. Incoming extended messages use *our* ids, while
/// messages we send use the ids learned from the remote handshake.
const LOCAL_METADATA_ID: u8 = 1;
const LOCAL_PEX_ID: u8 = 2;

/// Largest peer list sent in one PEX message. Keeping messages well below the
/// 64 KiB wire limit also limits how many addresses a single peer can inject.
const MAX_PEX_PEERS_PER_MESSAGE: usize = 50;
/// Period between unsolicited PEX updates.
const PEX_INTERVAL: Duration = Duration::from_secs(60);
/// How long a `ut_metadata` request may stay in flight before it is assigned
/// to another peer.
const METADATA_REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

/// Process-wide PEX counters, exposed for `d.*`/diagnostic surfaces.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PexStatistics {
    pub added: u64,
    pub rejected: u64,
    pub private_blocked: u64,
}

fn pex_added() -> &'static AtomicU64 {
    static VALUE: AtomicU64 = AtomicU64::new(0);
    &VALUE
}

fn pex_rejected() -> &'static AtomicU64 {
    static VALUE: AtomicU64 = AtomicU64::new(0);
    &VALUE
}

fn pex_private_blocked() -> &'static AtomicU64 {
    static VALUE: AtomicU64 = AtomicU64::new(0);
    &VALUE
}

pub fn pex_statistics() -> PexStatistics {
    PexStatistics {
        added: pex_added().load(Ordering::Relaxed),
        rejected: pex_rejected().load(Ordering::Relaxed),
        private_blocked: pex_private_blocked().load(Ordering::Relaxed),
    }
}

/// Reset diagnostics counters (tests and `dht.statistics`-style resets).
pub fn reset_pex_statistics() {
    pex_added().store(0, Ordering::Relaxed);
    pex_rejected().store(0, Ordering::Relaxed);
    pex_private_blocked().store(0, Ordering::Relaxed);
}

#[derive(Clone, Debug, Default)]
struct PeerFastPieces {
    allowed_fast: HashSet<u32>,
    suggested: HashSet<u32>,
}

fn peer_fast_state() -> &'static Mutex<HashMap<PeerRequestKey, PeerFastPieces>> {
    static STATE: OnceLock<Mutex<HashMap<PeerRequestKey, PeerFastPieces>>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn record_allowed_fast(info_hash: &HashString, peer_id: &HashString, index: u32) {
    let mut state = peer_fast_state().lock().unwrap();
    state
        .entry((*info_hash, *peer_id))
        .or_default()
        .allowed_fast
        .insert(index);
}

fn record_suggested_piece(info_hash: &HashString, peer_id: &HashString, index: u32) {
    let mut state = peer_fast_state().lock().unwrap();
    state
        .entry((*info_hash, *peer_id))
        .or_default()
        .suggested
        .insert(index);
}

fn remove_fast_state(info_hash: &HashString, peer_id: &HashString) {
    peer_fast_state()
        .lock()
        .unwrap()
        .remove(&(*info_hash, *peer_id));
}

/// Returns the Fast-extension pieces advertised by a peer. The first vector
/// contains `allowed_fast` pieces, the second `suggest` pieces.
pub fn peer_fast_pieces(
    info_hash: &HashString,
    peer_id: &HashString,
) -> Option<(Vec<u32>, Vec<u32>)> {
    let state = peer_fast_state().lock().unwrap();
    let peer = state.get(&(*info_hash, *peer_id))?;
    let mut allowed: Vec<u32> = peer.allowed_fast.iter().copied().collect();
    let mut suggested: Vec<u32> = peer.suggested.iter().copied().collect();
    allowed.sort_unstable();
    suggested.sort_unstable();
    Some((allowed, suggested))
}

/// Raw bencoded `info` dictionaries we can serve over `ut_metadata`.
fn local_metadata_cache() -> &'static Mutex<HashMap<HashString, Arc<Vec<u8>>>> {
    static CACHE: OnceLock<Mutex<HashMap<HashString, Arc<Vec<u8>>>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Registers verified local metadata so it can be served to peers. The bytes
/// must hash to `info_hash`; invalid input is ignored.
pub fn register_local_metadata(info_hash: HashString, bytes: Vec<u8>) -> bool {
    if bytes.is_empty() || bytes.len() > lt_protocol::metadata::MAX_METADATA_SIZE {
        return false;
    }
    if HashString::new(sha1_hash(&bytes)) != info_hash {
        return false;
    }
    local_metadata_cache()
        .lock()
        .unwrap()
        .insert(info_hash, Arc::new(bytes));
    true
}

pub fn local_metadata(info_hash: &HashString) -> Option<Arc<Vec<u8>>> {
    local_metadata_cache()
        .lock()
        .unwrap()
        .get(info_hash)
        .cloned()
}

/// Reconstructs the bencoded `info` dictionary from a parsed metadata value.
/// Torrents with unknown extension keys cannot be reconstructed byte-for-byte;
/// such metadata is only cached when the rebuilt bytes verify against the
/// info-hash.
fn reconstruct_info_bytes(download: &Download) -> Option<Vec<u8>> {
    let metadata = download.metadata();
    if metadata.is_meta_download || metadata.piece_length == 0 || metadata.pieces.is_empty() {
        return None;
    }

    let mut info = Object::create_map();
    info.insert_key(
        "name",
        Object::create_string(metadata.name.as_bytes().to_vec()),
    )
    .ok()?;
    info.insert_key(
        "piece length",
        Object::create_value(metadata.piece_length as i64),
    )
    .ok()?;
    info.insert_key("pieces", Object::create_string(metadata.pieces.clone()))
        .ok()?;
    if metadata.is_private {
        info.insert_key("private", Object::create_value(1i64))
            .ok()?;
    }

    if metadata.is_multi_file {
        let mut files = Object::create_list();
        for entry in &metadata.files {
            let mut components = entry.path.components().to_vec();
            // `TorrentMetadata` folds the torrent name into every file path.
            if components.first().map(String::as_str) == Some(metadata.name.as_str()) {
                components.remove(0);
            }
            let mut path = Object::create_list();
            for component in components {
                path.insert_back(Object::create_string(component.into_bytes()))
                    .ok()?;
            }
            let mut file = Object::create_map();
            file.insert_key("length", Object::create_value(entry.length as i64))
                .ok()?;
            file.insert_key("path", path).ok()?;
            files.insert_back(file).ok()?;
        }
        info.insert_key("files", files).ok()?;
    } else {
        info.insert_key("length", Object::create_value(metadata.total_size as i64))
            .ok()?;
    }

    lt_common::object::bencode::encode(&info).ok()
}

/// Returns the verified, bencoded `info` dictionary for a download.
///
/// The bytes are taken from the process-wide metadata cache when available.
/// Otherwise the dictionary is rebuilt from the parsed metadata and only
/// returned if the rebuilt bytes still hash to the torrent's info-hash.
///
/// Callers must not re-encode [`lt_engine::constructor::TorrentMetadata`]
/// themselves: unknown extension keys would change the info-hash. This helper
/// is the only safe source of raw `info` bytes for session persistence.
pub fn download_info_bytes(download: &Download) -> Option<Arc<Vec<u8>>> {
    ensure_local_metadata(download)
}

/// Ensures the process-wide metadata cache has an entry for this download.
/// The rebuilt dictionary is only inserted if its SHA-1 matches the info-hash.
fn ensure_local_metadata(download: &Download) -> Option<Arc<Vec<u8>>> {
    let info_hash = *download.info_hash();
    if let Some(metadata) = local_metadata(&info_hash) {
        return Some(metadata);
    }

    let bytes = reconstruct_info_bytes(download)?;
    if HashString::new(sha1_hash(&bytes)) != info_hash {
        return None;
    }
    register_local_metadata(info_hash, bytes.clone());
    local_metadata(&info_hash).or(Some(Arc::new(bytes)))
}

fn down_choked_peers() -> &'static Mutex<HashSet<PeerRequestKey>> {
    static DOWN_CHOKED: OnceLock<Mutex<HashSet<PeerRequestKey>>> = OnceLock::new();
    DOWN_CHOKED.get_or_init(|| Mutex::new(HashSet::new()))
}

fn set_peer_down_choked(info_hash: &HashString, peer_id: &HashString, choked: bool) {
    let mut peers = down_choked_peers().lock().unwrap();
    if choked {
        peers.insert((*info_hash, *peer_id));
    } else {
        peers.remove(&(*info_hash, *peer_id));
    }
}

/// Whether the resource manager currently choke-groups this peer's download
/// slot. Such a peer must not receive new requests even if it remote-unchoked
/// us.
pub fn is_peer_down_choked(info_hash: &HashString, peer_id: &HashString) -> bool {
    down_choked_peers()
        .lock()
        .unwrap()
        .contains(&(*info_hash, *peer_id))
}

/// Block size to use for a peer. BitTorrent blocks are at most 16 KiB, and a
/// short final piece uses its actual length from the delegator.
fn negotiated_block_size(download: &Download) -> u32 {
    let piece_length = download.metadata().piece_length;
    if piece_length == 0 {
        DEFAULT_BLOCK_SIZE
    } else {
        piece_length.min(DEFAULT_BLOCK_SIZE)
    }
}

fn update_peer_flags(
    download: &Arc<Mutex<Download>>,
    peer_id: &HashString,
    update: impl FnOnce(&mut lt_protocol::connection::PeerConnection),
) {
    let mut dl = download.lock().unwrap();
    if let Some(peer) = dl.connection_list_mut().find_by_id_mut(peer_id) {
        update(peer);
    }
}

/// True when this exact block has already been accepted into the delegator.
/// Used to ignore late endgame duplicates without double-counting bytes.
fn block_already_received(download: &Download, piece_index: u32, block_offset: u32) -> bool {
    download
        .delegator()
        .transfers()
        .find(piece_index)
        .and_then(|block_list| {
            block_list
                .blocks()
                .iter()
                .find(|block| block.piece().offset() == block_offset)
        })
        .map(|block| block.is_finished())
        .unwrap_or(false)
}

/// Applies one group's upload/download choke decisions to a live download.
///
/// The function never indexes outside the connection list and only cancels
/// blocks for peers that actually exist, so a stale queue entry from a
/// previous tick cannot panic the peer worker. Returns the number of peers
/// whose upload choke state changed and the number of cancelled peers.
pub fn apply_choke_changes_to_download(
    download: &mut Download,
    change: &GroupChokeChanges,
) -> (usize, usize) {
    let mut upload_changes = 0;

    for peer in download.connection_list_mut().iter_mut() {
        let number = peer_number(&peer.peer_id);
        if change.up_unchoked.contains(&number) {
            if peer.is_choked {
                peer.is_choked = false;
                upload_changes += 1;
            }
        } else if change.up_choked.contains(&number) && !peer.is_choked {
            peer.is_choked = true;
            upload_changes += 1;
        }
    }

    // Remember group download-choke decisions so the peer workers stop
    // requesting even if the remote peer has not choked us.
    let info_hash = *download.info_hash();
    let down_states: Vec<(HashString, bool)> = download
        .connection_list()
        .iter()
        .filter_map(|peer| {
            let number = peer_number(&peer.peer_id);
            if change.down_choked.contains(&number) {
                Some((peer.peer_id, true))
            } else if change.down_unchoked.contains(&number) {
                Some((peer.peer_id, false))
            } else {
                None
            }
        })
        .collect();

    for (peer_id, choked) in down_states {
        set_peer_down_choked(&info_hash, &peer_id, choked);
    }

    if change.down_choked.is_empty() {
        return (upload_changes, 0);
    }

    let choked: Vec<HashString> = download
        .connection_list()
        .iter()
        .filter(|peer| change.down_choked.contains(&peer_number(&peer.peer_id)))
        .map(|peer| peer.peer_id)
        .collect();

    for peer_id in &choked {
        let _ = download.cancel_peer(peer_id);
    }

    (upload_changes, choked.len())
}

/// Coordinates network discovery and peer connections for a single torrent download.
pub struct DownloadNetworkCoordinator {
    download: Arc<Mutex<Download>>,
    our_peer_id: HashString,
    listen_port: u16,
    dht: Option<DhtHandle>,
    pool: PeerPoolHandle,
    encryption_policy: Arc<Mutex<EncryptionPolicy>>,
    shutdown_tx: watch::Sender<bool>,
    shutdown_rx: watch::Receiver<bool>,
}

impl DownloadNetworkCoordinator {
    pub fn new(
        download: Arc<Mutex<Download>>,
        our_peer_id: HashString,
        listen_port: u16,
        dht: Option<DhtHandle>,
    ) -> Self {
        Self::new_with_policy(
            download,
            our_peer_id,
            listen_port,
            dht,
            EncryptionPolicy::allow(),
        )
    }

    /// Creates a coordinator with an explicit encryption policy.
    pub fn new_with_policy(
        download: Arc<Mutex<Download>>,
        our_peer_id: HashString,
        listen_port: u16,
        dht: Option<DhtHandle>,
        encryption_policy: EncryptionPolicy,
    ) -> Self {
        Self::new_with_shared_policy(
            download,
            our_peer_id,
            listen_port,
            dht,
            Arc::new(Mutex::new(encryption_policy)),
        )
    }

    /// Creates a coordinator whose policy can be updated while it is running.
    pub fn new_with_shared_policy(
        download: Arc<Mutex<Download>>,
        our_peer_id: HashString,
        listen_port: u16,
        dht: Option<DhtHandle>,
        encryption_policy: Arc<Mutex<EncryptionPolicy>>,
    ) -> Self {
        let max_peers = download.lock().unwrap().client_config().max_peers() as usize;
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        Self {
            download,
            our_peer_id,
            listen_port,
            dht,
            pool: Arc::new(Mutex::new(PeerPool::new(max_peers))),
            encryption_policy,
            shutdown_tx,
            shutdown_rx,
        }
    }

    /// Returns the shared encryption policy handle so the runtime can update
    /// it when `protocol.encryption.*` changes.
    pub fn encryption_policy_handle(&self) -> Arc<Mutex<EncryptionPolicy>> {
        Arc::clone(&self.encryption_policy)
    }

    /// Returns the shared peer pool for this download.
    ///
    /// The handle is useful for inbound listeners and for exposing pool
    /// statistics through commands or the UI.
    pub fn peer_pool(&self) -> PeerPoolHandle {
        Arc::clone(&self.pool)
    }

    /// Spawns the background network tasks (tracker announcer and peer workers).
    pub fn spawn(self) -> watch::Sender<bool> {
        let download = Arc::clone(&self.download);
        let our_peer_id = self.our_peer_id;
        let listen_port = self.listen_port;
        let dht = self.dht.clone();
        let pool = self.pool;
        let encryption_policy = Arc::clone(&self.encryption_policy);
        let mut shutdown_rx = self.shutdown_rx;
        let shutdown_tx = self.shutdown_tx;

        // Periodically drain asynchronous disk-write completions. This keeps
        // `is_done`/seeding state accurate even when no peer traffic arrives.
        let poll_download = Arc::clone(&download);
        let mut poll_shutdown = shutdown_rx.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_millis(100));
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

            loop {
                tokio::select! {
                    _ = poll_shutdown.changed() => break,
                    _ = tick.tick() => {
                        if let Ok(mut dl) = poll_download.try_lock() {
                            let _ = dl.poll_disk_completions();
                        }
                    }
                }
            }
        });

        tokio::spawn(async move {
            let mut is_first_announce = true;
            let mut udp_router = UdpRouter::new();
            let mut connect_tasks: JoinSet<()> = JoinSet::new();
            let mut dht_private_state: Option<bool> = None;
            let mut dht_announced = false;

            info!(
                torrent = %download.lock().unwrap().name(),
                "Starting DownloadNetworkCoordinator..."
            );

            while !*shutdown_rx.borrow() {
                let (
                    info_hash,
                    total_size,
                    downloaded,
                    uploaded,
                    trackers,
                    is_active,
                    is_done,
                    is_private,
                    is_meta_download,
                    max_peers,
                    min_peers,
                    connect_timeout,
                ) = {
                    let dl = download.lock().unwrap();
                    let meta = dl.metadata().clone();
                    // Use the live controller list so extra trackers inserted
                    // at runtime and per-tracker state changes are respected.
                    let tracker_urls: Vec<String> = dl
                        .tracker_controller()
                        .tracker_list
                        .trackers
                        .iter()
                        .map(|tracker| tracker.url().to_string())
                        .collect();
                    (
                        *dl.info_hash(),
                        meta.total_size,
                        dl.downloaded_bytes(),
                        dl.uploaded_bytes(),
                        tracker_urls,
                        dl.state() == &crate::download::DownloadState::Downloading
                            || dl.state() == &crate::download::DownloadState::Seeding,
                        dl.is_done(),
                        meta.is_private,
                        meta.is_meta_download,
                        dl.client_config().max_peers() as usize,
                        dl.client_config().min_peers() as usize,
                        dl.client_config().connect_timeout(),
                    )
                };

                // Private torrents must never leak into the DHT tracker cache
                // or be served through `get_peers`. Magnet downloads only learn
                // privacy once metadata arrives, so keep this in sync instead
                // of deciding once at construction time.
                if let Some(dht) = &dht {
                    if dht_private_state != Some(is_private) {
                        dht.router
                            .lock()
                            .await
                            .set_private_info_hash(info_hash, is_private);
                        dht_private_state = Some(is_private);
                    }
                }

                // Limits may change at runtime through `ClientConfig`; the pool
                // always applies them globally, never per discovery source.
                {
                    let mut p = pool.lock().unwrap();
                    p.set_limits(max_peers, max_connecting_for(max_peers));
                    // A magnet download may discover peers via DHT before the
                    // metadata reveals that the torrent is private.
                    p.enforce_private(is_private);
                    // Never let a lost dial task hold a concurrency slot
                    // forever.
                    p.reap_stale(Instant::now(), connect_timeout + Duration::from_secs(30));
                }

                let dht_lookup_due = is_first_announce;

                // Merge peers learned out-of-band (PEX and future sources).
                let discovered = download.lock().unwrap().take_discovered_peers();
                if !discovered.is_empty() {
                    let added = pool
                        .lock()
                        .unwrap()
                        .merge(discovered, PeerSource::Pex, is_private);
                    if added > 0 {
                        debug!(added, "Merged discovered peers into pool");
                    }
                }

                if is_active {
                    let left = if is_done {
                        0
                    } else {
                        total_size.saturating_sub(downloaded)
                    };
                    let event = if is_first_announce {
                        is_first_announce = false;
                        Some("started".to_string())
                    } else {
                        None
                    };

                    let announce_event = if event.is_some() {
                        TrackerEvent::Started
                    } else {
                        TrackerEvent::None
                    };

                    let params = AnnounceParams {
                        info_hash,
                        peer_id: our_peer_id,
                        port: listen_port,
                        uploaded,
                        downloaded,
                        left,
                        numwant: 50,
                        event,
                    };

                    let cached_time = download.lock().unwrap().tracker_controller().cached_time();

                    for tracker_url in &trackers {
                        if *shutdown_rx.borrow() {
                            break;
                        }

                        // Respect the controller's per-tracker interval and
                        // backoff. A failed tracker is retried only when its
                        // failed_time_next() deadline has been reached.
                        let should_send = {
                            let mut dl = download.lock().unwrap();
                            let controller = dl.tracker_controller_mut();
                            if let Some(index) = controller.tracker_list.find_url(tracker_url) {
                                let due = controller
                                    .tracker_list
                                    .at(index)
                                    .map(|tracker| {
                                        if !tracker.is_usable() {
                                            return false;
                                        }

                                        let state = tracker.state();
                                        // A request queued through the
                                        // controller (manual announce/start)
                                        // must be completed by this loop even
                                        // though it is already marked busy.
                                        let queued_announce = tracker.is_requesting()
                                            && state.latest_event != TrackerEvent::Scrape;
                                        let scheduled = !tracker.is_requesting()
                                            && state.activity_time_next() <= cached_time;

                                        queued_announce || scheduled
                                    })
                                    .unwrap_or(false);

                                if due {
                                    controller.tracker_list.send_event(index, announce_event);
                                    controller
                                        .tracker_list
                                        .at(index)
                                        .map(|tracker| tracker.is_requesting())
                                        .unwrap_or(false)
                                } else {
                                    false
                                }
                            } else {
                                false
                            }
                        };

                        if !should_send {
                            continue;
                        }

                        debug!(url = %tracker_url, "Announcing to tracker...");

                        let result = if tracker_url.starts_with("udp://") {
                            announce_udp_detailed_with_router(tracker_url, &params, &mut udp_router)
                                .await
                        } else if tracker_url.starts_with("http://")
                            || tracker_url.starts_with("https://")
                        {
                            announce_http_detailed(tracker_url, &params).await
                        } else {
                            // Unknown tracker schemes never reach this point in
                            // practice; leave them for the controller's state.
                            continue;
                        };

                        match result {
                            Ok(result) => {
                                info!(
                                    url = %tracker_url,
                                    count = result.peers.len(),
                                    "Discovered peers from tracker"
                                );

                                let added = pool.lock().unwrap().merge(
                                    result.peers.clone(),
                                    PeerSource::Tracker,
                                    is_private,
                                );
                                debug!(added, "Merged tracker peers into pool");

                                let mut addresses = AddressList::new();
                                for peer in &result.peers {
                                    addresses.push(SocketAddress::from(*peer));
                                }

                                let mut dl = download.lock().unwrap();
                                let controller = dl.tracker_controller_mut();
                                if let Some(index) = controller.tracker_list.find_url(tracker_url) {
                                    {
                                        let state = controller
                                            .tracker_list
                                            .at_mut(index)
                                            .unwrap()
                                            .state_mut();
                                        if let Some(interval) = result.interval {
                                            state.set_normal_interval(interval);
                                        }
                                        if let Some(min_interval) = result.min_interval {
                                            state.set_min_interval(min_interval);
                                        }
                                        if let (Some(complete), Some(incomplete)) =
                                            (result.complete, result.incomplete)
                                        {
                                            state.scrape_complete = complete;
                                            state.scrape_incomplete = incomplete;
                                        }
                                    }

                                    controller.receive_success(index, &addresses);
                                }
                            }
                            Err(e) => {
                                debug!(url = %tracker_url, error = %e, "Announce failed");
                                let mut dl = download.lock().unwrap();
                                let controller = dl.tracker_controller_mut();
                                if let Some(index) = controller.tracker_list.find_url(tracker_url) {
                                    controller.receive_failure(index, &e.to_string());
                                }
                            }
                        }
                    }

                    if let Some(dht) = &dht {
                        // A configured but stopped/off controller has no
                        // socket; tracker peers remain the only source.
                        if dht.server.is_active() {
                            let peer_shortage = {
                                let p = pool.lock().unwrap();
                                p.connected_count() + p.connecting_count() < min_peers
                            };

                            if !is_private && (dht_lookup_due || peer_shortage) {
                                // A magnet download has no info dict yet, so
                                // its privacy flag is unknown. Look up peers
                                // to obtain metadata, but never announce an
                                // unverified info-hash to the DHT.
                                if !is_meta_download && !dht_announced {
                                    match announce_peer(
                                        dht.server.clone(),
                                        dht.router.clone(),
                                        info_hash,
                                        listen_port,
                                        16,
                                    )
                                    .await
                                    {
                                        Ok(count) if count > 0 => dht_announced = true,
                                        Ok(_) => {
                                            debug!("DHT announce found no nodes to announce to")
                                        }
                                        Err(err) => {
                                            debug!(error = %err, "DHT announce failed")
                                        }
                                    }
                                }

                                match get_peers(
                                    dht.server.clone(),
                                    dht.router.clone(),
                                    info_hash,
                                    16,
                                )
                                .await
                                {
                                    Ok(peers) => {
                                        info!(count = peers.len(), "Discovered peers from DHT");
                                        let addresses = peers
                                            .into_iter()
                                            .filter_map(|peer| peer.as_socket_addr());
                                        let added = pool.lock().unwrap().merge(
                                            addresses,
                                            PeerSource::Dht,
                                            is_private,
                                        );
                                        debug!(added, "Merged DHT peers into pool");
                                    }
                                    Err(err) => debug!(error = %err, "DHT lookup failed"),
                                }
                            }
                        }
                    }

                    // Single connect scheduler: fill every free slot from the
                    // merged pool, regardless of which source produced the peer.
                    let batch = pool
                        .lock()
                        .unwrap()
                        .next_connect_batch(Instant::now(), usize::MAX);
                    if !batch.is_empty() {
                        debug!(count = batch.len(), "Scheduling peer connection attempts");
                    }

                    for peer_addr in batch {
                        let dl_ref = Arc::clone(&download);
                        let pool_ref = Arc::clone(&pool);
                        let peer_shutdown = shutdown_rx.clone();
                        // Snapshot the live policy when the dial is scheduled;
                        // existing sessions keep the policy they started with.
                        let peer_encryption = *encryption_policy.lock().unwrap();

                        connect_tasks.spawn(async move {
                            run_peer_worker(
                                peer_addr,
                                PeerWorkerContext {
                                    info_hash,
                                    our_peer_id,
                                    download: dl_ref,
                                    shutdown_rx: peer_shutdown,
                                    connect_timeout,
                                    policy: peer_encryption,
                                    pool: Some(pool_ref),
                                },
                            )
                            .await;
                        });
                    }
                }

                // Reap finished connect tasks so the set does not grow without
                // bound during long sessions.
                while connect_tasks.try_join_next().is_some() {}

                // Drive scrapes from the controller's per-tracker schedule.
                // The controller uses the tracker normal interval after a
                // success and the minimum interval after a failure.
                if is_active {
                    let scrape_requests = {
                        let mut dl = download.lock().unwrap();
                        let controller = dl.tracker_controller_mut();

                        if !controller.is_scrape_queued() && !controller.is_scrape_in_flight() {
                            controller.schedule_next_scrape();
                        }

                        if controller.check_scrape_timer() {
                            controller.scrape_requests()
                        } else {
                            Vec::new()
                        }
                    };

                    for index in scrape_requests {
                        let url = download
                            .lock()
                            .unwrap()
                            .tracker_controller()
                            .tracker_list
                            .at(index)
                            .map(|tracker| tracker.url().to_string());

                        let Some(url) = url else {
                            let mut dl = download.lock().unwrap();
                            dl.tracker_controller_mut().receive_scrape_failure(index);
                            continue;
                        };

                        match scrape_with_router(&url, info_hash, &mut udp_router).await {
                            Ok(stats) => {
                                let mut dl = download.lock().unwrap();
                                let controller = dl.tracker_controller_mut();
                                if let Some(index) = controller.tracker_list.find_url(&url) {
                                    controller.receive_scrape_stats(index, stats);
                                } else {
                                    controller.receive_scrape_failure(index);
                                }
                            }
                            Err(err) => {
                                debug!(url = %url, error = %err, "Tracker scrape failed");
                                let mut dl = download.lock().unwrap();
                                let controller = dl.tracker_controller_mut();
                                let index = controller.tracker_list.find_url(&url).unwrap_or(index);

                                if err.is_scrape_unsupported() {
                                    controller.receive_scrape_unsupported(index, &err.to_string());
                                } else {
                                    controller.receive_scrape_failure_with_reason(
                                        index,
                                        &err.to_string(),
                                    );
                                }
                            }
                        }
                    }
                }

                // Check every 15 seconds while the swarm is below `min_peers`,
                // otherwise every 60 seconds.
                let current_peers_count = {
                    let p = pool.lock().unwrap();
                    p.connected_count() + p.connecting_count()
                };
                let base_sleep_secs = if current_peers_count < min_peers {
                    15
                } else {
                    60
                };

                // Wake up in time for an explicitly queued scrape instead of
                // always waiting for the next announce tick.
                let next_scrape = download
                    .lock()
                    .unwrap()
                    .tracker_controller()
                    .seconds_to_next_scrape();
                let sleep_secs = if next_scrape != u32::MAX {
                    base_sleep_secs.min((next_scrape as u64).max(1))
                } else {
                    base_sleep_secs
                };

                tokio::select! {
                    _ = shutdown_rx.changed() => {
                        break;
                    }
                    _ = sleep(Duration::from_secs(sleep_secs)) => {}
                }
            }

            // Give live sessions a brief chance to deregister themselves from
            // the download's connection list, then stop every remaining dial
            // so a stopped or erased download cannot keep connecting.
            let _ = timeout(Duration::from_millis(250), async {
                while connect_tasks.join_next().await.is_some() {}
            })
            .await;
            connect_tasks.abort_all();
            while connect_tasks.join_next().await.is_some() {}

            // Flush verified pieces before the coordinator exits. This blocks,
            // so run it on a blocking pool rather than a reactor worker.
            let flush_download = Arc::clone(&download);
            let _ =
                tokio::task::spawn_blocking(move || flush_download.lock().unwrap().flush_disk())
                    .await;

            info!(
                torrent = %download.lock().unwrap().name(),
                "DownloadNetworkCoordinator stopped."
            );
        });

        shutdown_tx
    }
}

/// Reads and decodes the 68-byte BitTorrent handshake.
async fn read_peer_handshake<R>(reader: &mut R, peer_addr: SocketAddr) -> Option<HandshakeMessage>
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut hs_buf = [0u8; 68];

    match timeout(Duration::from_secs(5), reader.read_exact(&mut hs_buf)).await {
        Ok(Ok(_)) => {}
        Ok(Err(err)) => {
            debug!(peer = %peer_addr, error = %err, "Failed to read peer handshake");
            return None;
        }
        Err(_) => {
            debug!(peer = %peer_addr, "Timeout waiting for peer handshake");
            return None;
        }
    }

    match HandshakeMessage::decode(&hs_buf) {
        Ok(handshake) => Some(handshake),
        Err(e) => {
            debug!(peer = %peer_addr, error = %e, "Invalid peer handshake");
            None
        }
    }
}

fn record_connect_failure(pool: &Option<PeerPoolHandle>, peer_addr: SocketAddr) {
    if let Some(pool) = pool {
        pool.lock()
            .unwrap()
            .record_failure(peer_addr, Instant::now());
    }
}

struct OutgoingHandshake {
    stream: TcpStream,
    peer_handshake: HandshakeMessage,
    cipher_enc: Option<Rc4>,
    cipher_dec: Option<Rc4>,
    encrypted: bool,
    obfuscated: bool,
}

async fn dial_peer(peer_addr: SocketAddr, connect_timeout: Duration) -> Option<TcpStream> {
    match timeout(connect_timeout, TcpStream::connect(peer_addr)).await {
        Ok(Ok(stream)) => Some(stream),
        Ok(Err(err)) => {
            debug!(peer = %peer_addr, error = %err, "Connection failed");
            None
        }
        Err(_) => {
            debug!(peer = %peer_addr, "Connection timeout");
            None
        }
    }
}

async fn try_plaintext_handshake(
    peer_addr: SocketAddr,
    info_hash: HashString,
    our_peer_id: HashString,
    connect_timeout: Duration,
    policy: &EncryptionPolicy,
) -> Result<OutgoingHandshake, ()> {
    if !policy.allow_plaintext_handshake() {
        return Err(());
    }

    let mut stream = dial_peer(peer_addr, connect_timeout).await.ok_or(())?;

    let handshake_msg = HandshakeMessage::new(info_hash, our_peer_id);
    match timeout(connect_timeout, stream.write_all(&handshake_msg.encode())).await {
        Ok(Ok(())) => {}
        Ok(Err(err)) => {
            debug!(peer = %peer_addr, error = %err, "Failed to send plaintext handshake");
            return Err(());
        }
        Err(_) => {
            debug!(peer = %peer_addr, "Timeout sending plaintext handshake");
            return Err(());
        }
    }

    let peer_handshake = read_peer_handshake(&mut stream, peer_addr)
        .await
        .ok_or(())?;

    if peer_handshake.info_hash != info_hash {
        debug!(peer = %peer_addr, "Peer info_hash mismatch");
        return Err(());
    }

    Ok(OutgoingHandshake {
        stream,
        peer_handshake,
        cipher_enc: None,
        cipher_dec: None,
        encrypted: false,
        obfuscated: false,
    })
}

async fn try_encrypted_handshake(
    peer_addr: SocketAddr,
    info_hash: HashString,
    our_peer_id: HashString,
    connect_timeout: Duration,
    policy: &EncryptionPolicy,
) -> Result<OutgoingHandshake, MseError> {
    let mut stream = dial_peer(peer_addr, connect_timeout).await.ok_or_else(|| {
        MseError::Io(std::io::Error::new(
            std::io::ErrorKind::ConnectionRefused,
            "could not connect",
        ))
    })?;

    let mut session = mse_initiate(&mut stream, info_hash, our_peer_id, policy).await?;
    let encrypted = session.encrypted_stream;
    let obfuscated = session.is_obfuscated();
    let (cipher_enc, cipher_dec) = session.take_ciphers();
    let peer_handshake = session.peer_handshake;

    Ok(OutgoingHandshake {
        stream,
        peer_handshake,
        cipher_enc,
        cipher_dec,
        encrypted,
        obfuscated,
    })
}

/// Performs the outgoing handshake according to the legacy
/// `HandshakeManager` retry rules.
///
/// - `prefer`/`require` (or a required encrypted stream): MSE first.
/// - `allow`/`disabled`: plaintext first.
/// - A plaintext retry is only armed when policy allows it and no MSE
///   verification value has been validated yet. `require` never downgrades.
async fn outgoing_handshake(
    peer_addr: SocketAddr,
    info_hash: HashString,
    our_peer_id: HashString,
    connect_timeout: Duration,
    policy: &EncryptionPolicy,
) -> Result<OutgoingHandshake, ()> {
    if !policy.is_valid() {
        debug!(peer = %peer_addr, "Invalid encryption policy");
        return Err(());
    }

    if policy.prefer_encrypted_handshake() {
        match try_encrypted_handshake(peer_addr, info_hash, our_peer_id, connect_timeout, policy)
            .await
        {
            Ok(session) => return Ok(session),
            Err(err) => {
                debug!(peer = %peer_addr, error = %err, "MSE handshake failed");

                // Once policy requires encryption, or the failure happened
                // after key/sync verification, never downgrade.
                if policy.require_encrypted_handshake()
                    || !policy.allow_plaintext_handshake()
                    || !err.allows_plaintext_fallback()
                {
                    return Err(());
                }

                debug!(peer = %peer_addr, "Retrying with plaintext handshake");
                return try_plaintext_handshake(
                    peer_addr,
                    info_hash,
                    our_peer_id,
                    connect_timeout,
                    policy,
                )
                .await;
            }
        }
    }

    match try_plaintext_handshake(peer_addr, info_hash, our_peer_id, connect_timeout, policy).await
    {
        Ok(session) => Ok(session),
        Err(()) => {
            if !policy.allow_encrypted_handshake() {
                return Err(());
            }

            debug!(peer = %peer_addr, "Plaintext handshake failed; retrying with MSE");
            try_encrypted_handshake(peer_addr, info_hash, our_peer_id, connect_timeout, policy)
                .await
                .map_err(|err| {
                    debug!(peer = %peer_addr, error = %err, "MSE retry failed");
                })
        }
    }
}

/// Shared state for one outbound peer connection attempt.
struct PeerWorkerContext {
    info_hash: HashString,
    our_peer_id: HashString,
    download: Arc<Mutex<Download>>,
    shutdown_rx: watch::Receiver<bool>,
    connect_timeout: Duration,
    policy: EncryptionPolicy,
    pool: Option<PeerPoolHandle>,
}

/// Runs an outbound peer connection: dials, exchanges handshakes, then runs
/// the shared peer session loop.
async fn run_peer_worker(peer_addr: SocketAddr, ctx: PeerWorkerContext) {
    let PeerWorkerContext {
        info_hash,
        our_peer_id,
        download,
        shutdown_rx,
        connect_timeout,
        policy,
        pool,
    } = ctx;
    debug!(peer = %peer_addr, "Connecting to peer...");

    let session =
        match outgoing_handshake(peer_addr, info_hash, our_peer_id, connect_timeout, &policy).await
        {
            Ok(session) => session,
            Err(()) => {
                record_connect_failure(&pool, peer_addr);
                return;
            }
        };

    debug!(
        peer = %peer_addr,
        peer_id = %session.peer_handshake.peer_id.to_hex(),
        encrypted = session.encrypted,
        obfuscated = session.obfuscated,
        "Peer handshake verified"
    );

    if let Some(pool) = &pool {
        pool.lock()
            .unwrap()
            .record_success(peer_addr, Instant::now());
    }

    let OutgoingHandshake {
        stream,
        peer_handshake,
        cipher_enc,
        cipher_dec,
        encrypted,
        obfuscated,
    } = session;
    let remote_fast = peer_handshake.has_fast();
    let (reader, writer) = stream.into_split();

    run_peer_session(
        reader,
        writer,
        peer_addr,
        peer_handshake.peer_id,
        download,
        shutdown_rx,
        cipher_enc,
        cipher_dec,
        PeerDirection::Outgoing,
        encrypted,
        obfuscated,
        remote_fast,
        pool,
        None,
    )
    .await;
}

/// Runs an accepted inbound connection.
///
/// The caller has already read and validated the peer handshake (and looked up
/// the download by info-hash). This function sends our handshake response and
/// then runs the shared session loop.
pub async fn run_inbound_peer(
    stream: TcpStream,
    handshake: HandshakeMessage,
    our_peer_id: HashString,
    download: Arc<Mutex<Download>>,
    shutdown_rx: watch::Receiver<bool>,
) {
    run_inbound_peer_with_pool(stream, handshake, our_peer_id, download, shutdown_rx, None).await;
}

/// Like [`run_inbound_peer`], but also records the peer in the download's
/// [`PeerPool`](crate::peer_pool::PeerPool).
pub async fn run_inbound_peer_with_pool(
    stream: TcpStream,
    handshake: HandshakeMessage,
    our_peer_id: HashString,
    download: Arc<Mutex<Download>>,
    shutdown_rx: watch::Receiver<bool>,
    pool: Option<PeerPoolHandle>,
) {
    run_inbound_peer_with_pool_timeout(
        stream,
        handshake,
        our_peer_id,
        download,
        shutdown_rx,
        pool,
        None,
    )
    .await;
}

/// Inbound peer session with an explicit request timeout. Primarily useful
/// for deterministic loopback tests; production callers should use
/// [`run_inbound_peer`].
pub async fn run_inbound_peer_with_request_timeout(
    stream: TcpStream,
    handshake: HandshakeMessage,
    our_peer_id: HashString,
    download: Arc<Mutex<Download>>,
    shutdown_rx: watch::Receiver<bool>,
    request_timeout: Duration,
) {
    run_inbound_peer_with_pool_timeout(
        stream,
        handshake,
        our_peer_id,
        download,
        shutdown_rx,
        None,
        Some(request_timeout),
    )
    .await;
}

async fn run_inbound_peer_with_pool_timeout(
    stream: TcpStream,
    handshake: HandshakeMessage,
    our_peer_id: HashString,
    download: Arc<Mutex<Download>>,
    shutdown_rx: watch::Receiver<bool>,
    pool: Option<PeerPoolHandle>,
    request_timeout: Option<Duration>,
) {
    let peer_addr = match stream.peer_addr() {
        Ok(addr) => normalize_socket_addr(addr),
        Err(_) => return,
    };

    debug!(peer = %peer_addr, peer_id = %handshake.peer_id.to_hex(), "Accepted inbound peer");

    if let Some(pool) = &pool {
        pool.lock()
            .unwrap()
            .record_inbound(peer_addr, Instant::now());
    }

    let (reader, mut writer) = stream.into_split();

    let remote_fast = handshake.has_fast();
    let response = HandshakeMessage::new(handshake.info_hash, our_peer_id);
    if let Err(e) = writer.write_all(&response.encode()).await {
        debug!(peer = %peer_addr, error = %e, "Failed to send handshake response");
        if let Some(pool) = &pool {
            pool.lock()
                .unwrap()
                .record_disconnected(peer_addr, Instant::now());
        }
        return;
    }

    run_peer_session(
        reader,
        writer,
        peer_addr,
        handshake.peer_id,
        download,
        shutdown_rx,
        None,
        None,
        PeerDirection::Incoming,
        false,
        false,
        remote_fast,
        pool,
        request_timeout,
    )
    .await;
}

/// Runs an inbound peer whose MSE handshake has already completed.
pub async fn run_inbound_mse_peer(
    stream: TcpStream,
    session: MseSession,
    download: Arc<Mutex<Download>>,
    shutdown_rx: watch::Receiver<bool>,
) {
    run_inbound_mse_peer_with_pool(stream, session, download, shutdown_rx, None).await;
}

/// Like [`run_inbound_mse_peer`], but also records the peer in the download's
/// [`PeerPool`](crate::peer_pool::PeerPool).
pub async fn run_inbound_mse_peer_with_pool(
    stream: TcpStream,
    mut session: MseSession,
    download: Arc<Mutex<Download>>,
    shutdown_rx: watch::Receiver<bool>,
    pool: Option<PeerPoolHandle>,
) {
    let peer_addr = match stream.peer_addr() {
        Ok(addr) => normalize_socket_addr(addr),
        Err(_) => return,
    };

    let peer_id = session.peer_id;
    let remote_fast = session.peer_handshake.has_fast();
    let encrypted = session.encrypted_stream;
    let obfuscated = session.is_obfuscated();
    let (cipher_enc, cipher_dec) = session.take_ciphers();
    let (reader, writer) = stream.into_split();

    debug!(
        peer = %peer_addr,
        peer_id = %peer_id.to_hex(),
        encrypted,
        obfuscated,
        "Inbound MSE peer ready"
    );

    if let Some(pool) = &pool {
        pool.lock()
            .unwrap()
            .record_inbound(peer_addr, Instant::now());
    }

    run_peer_session(
        reader,
        writer,
        peer_addr,
        peer_id,
        download,
        shutdown_rx,
        cipher_enc,
        cipher_dec,
        PeerDirection::Incoming,
        encrypted,
        obfuscated,
        remote_fast,
        pool,
        None,
    )
    .await;
}

/// Peer ids whose wire stream is RC4 encrypted (`p.is_encrypted`).
pub fn encrypted_peer_ids(download: &Download) -> Vec<HashString> {
    download
        .connection_list()
        .iter()
        .filter(|peer| peer.is_encrypted)
        .map(|peer| peer.peer_id)
        .collect()
}

/// Peer ids that completed an MSE handshake (`p.is_obfuscated`). This
/// includes peers whose negotiated stream is plaintext.
pub fn obfuscated_peer_ids(download: &Download) -> Vec<HashString> {
    download
        .connection_list()
        .iter()
        .filter(|peer| peer.is_obfuscated)
        .map(|peer| peer.peer_id)
        .collect()
}

/// Requests up to four `ut_metadata` pieces from the peer.
async fn request_metadata_pieces(
    download: &Arc<Mutex<Download>>,
    tx: &mpsc::Sender<PeerMessage>,
    remote_metadata_id: u8,
    peer_id: HashString,
) {
    if remote_metadata_id == 0 {
        return;
    }

    let requests = {
        let mut download = download.lock().unwrap();
        let Some(metadata) = download.metadata_download_mut() else {
            return;
        };

        let mut requests = Vec::new();
        for _ in 0..4 {
            match metadata.next_request_for(Some(peer_id)) {
                Some(piece) => requests.push(piece),
                None => break,
            }
        }
        requests
    };

    for piece in requests {
        let _ = tx
            .send(PeerMessage::Extended {
                id: remote_metadata_id,
                payload: ut_metadata_request_payload(piece),
            })
            .await;
    }
}

#[derive(Default)]
struct RemoteExtensionHandshake {
    metadata_id: Option<u8>,
    metadata_size: Option<u32>,
    pex_id: Option<u8>,
    request_queue: Option<usize>,
}

fn parse_extension_handshake(payload: &[u8]) -> RemoteExtensionHandshake {
    let Ok((object, _)) = lt_common::object::bencode::read_bencode(payload) else {
        return RemoteExtensionHandshake::default();
    };

    let mut handshake = RemoteExtensionHandshake::default();

    if let Ok(map) = object.get_key_map("m") {
        if let Some(value) = map.get(b"ut_metadata".as_slice()) {
            if let Ok(id) = value.as_value() {
                handshake.metadata_id = Some(id as u8);
            }
        }
        if let Some(value) = map.get(b"ut_pex".as_slice()) {
            if let Ok(id) = value.as_value() {
                handshake.pex_id = Some(id as u8);
            }
        }
    }

    handshake.metadata_size = object
        .get_key_value("metadata_size")
        .ok()
        .map(|size| size as u32);

    // BEP 10 `reqq`: number of outstanding requests the peer is willing to
    // accept. Clamp to the protocol maximum so a broken value cannot make us
    // build an unbounded pipeline.
    handshake.request_queue = object
        .get_key_value("reqq")
        .ok()
        .and_then(|value| usize::try_from(value).ok())
        .map(|value| value.clamp(1, MAX_PEER_REQUEST_QUEUE));

    handshake
}

/// Calls the delegator and registers the returned blocks in the peer's
/// authoritative request list. Blocks that cannot be registered (queue full,
/// duplicate not allowed) are immediately returned to the delegator.
fn fill_request_pipeline(
    download: &Arc<Mutex<Download>>,
    peer_id: &HashString,
    info_hash: HashString,
    peer_unchoked: bool,
    request_list: &mut RequestList,
    now: Instant,
) -> Vec<BlockTransfer> {
    if !peer_unchoked || is_peer_down_choked(&info_hash, peer_id) {
        return Vec::new();
    }

    let slots = request_list.available_slots();
    if slots == 0 {
        return Vec::new();
    }

    let (pieces, aggressive) = {
        let mut dl = download.lock().unwrap();
        if dl.is_done() {
            return Vec::new();
        }
        let aggressive = dl.delegator().is_aggressive();
        let pieces = dl.delegate_for_peer(peer_id, slots);
        (pieces, aggressive)
    };

    let mut pieces = pieces;
    // Prefer pieces the peer advertised through Fast `suggest`/`allowed_fast`.
    // Ordering only; the delegator remains the authority on what may be
    // requested.
    if let Some((allowed, suggested)) = peer_fast_pieces(&info_hash, peer_id) {
        let preferred: HashSet<u32> = allowed.into_iter().chain(suggested).collect();
        if !preferred.is_empty() {
            pieces.sort_by_key(|piece| !preferred.contains(&piece.index()));
        }
    }

    let mut accepted = Vec::new();
    let mut unregistered = Vec::new();

    for piece in pieces {
        let transfer = BlockTransfer::new(piece.index(), piece.offset(), piece.length());
        let duplicate = request_list.contains_outstanding(transfer);

        // Outside endgame a block may only be outstanding once per peer. In
        // endgame the request list itself caps the number of duplicates.
        if duplicate && !aggressive {
            unregistered.push(transfer);
            continue;
        }

        let registered = if duplicate {
            request_list.register_endgame(transfer, now)
        } else {
            request_list.register(transfer, now)
        };

        if registered {
            accepted.push(transfer);
        } else {
            unregistered.push(transfer);
        }
    }

    if !unregistered.is_empty() {
        let mut dl = download.lock().unwrap();
        for transfer in unregistered {
            let _ = dl.cancel_block(peer_id, transfer.piece_index, transfer.block_offset);
        }
    }

    accepted
}

async fn send_requests(tx: &mpsc::Sender<PeerMessage>, transfers: &[BlockTransfer]) {
    for transfer in transfers {
        if tx
            .send(PeerMessage::Request {
                index: transfer.piece_index,
                begin: transfer.block_offset,
                length: transfer.block_length,
            })
            .await
            .is_err()
        {
            break;
        }
    }
}

/// Drops requests for blocks that were completed while they were in flight
/// (typically endgame duplicates won by another peer) and tells the remote
/// peer not to send them.
async fn cancel_completed_requests(
    download: &Arc<Mutex<Download>>,
    request_list: &mut RequestList,
    tx: &mpsc::Sender<PeerMessage>,
) -> usize {
    let completed: Vec<BlockTransfer> = {
        let dl = download.lock().unwrap();
        request_list
            .outstanding()
            .filter(|record| dl.has_piece(record.transfer.piece_index))
            .map(|record| record.transfer)
            .collect()
    };

    let mut cancelled = 0;
    let mut seen = HashSet::new();
    for transfer in completed {
        if !seen.insert((transfer.piece_index, transfer.block_offset)) {
            continue;
        }
        let removed = request_list.take_block_at(transfer.piece_index, transfer.block_offset);
        cancelled += removed.len();
        let _ = tx
            .send(PeerMessage::Cancel {
                index: transfer.piece_index,
                begin: transfer.block_offset,
                length: transfer.block_length,
            })
            .await;
    }

    cancelled
}

/// Fills free request-queue slots from the delegator and writes the new
/// requests, then refreshes the peer metrics snapshot.
async fn refill_requests(
    download: &Arc<Mutex<Download>>,
    peer_id: &HashString,
    info_hash: HashString,
    peer_unchoked: bool,
    request_list: &mut RequestList,
    tx: &mpsc::Sender<PeerMessage>,
) {
    let requests = fill_request_pipeline(
        download,
        peer_id,
        info_hash,
        peer_unchoked,
        request_list,
        Instant::now(),
    );
    send_requests(tx, &requests).await;
    publish_peer_request_stats(info_hash, *peer_id, request_list, !peer_unchoked);
}

/// Expires overdue requests, returning the blocks to the delegator so they
/// can be retried or reassigned to another peer.
async fn expire_requests(
    download: &Arc<Mutex<Download>>,
    peer_id: &HashString,
    request_list: &mut RequestList,
    tx: &mpsc::Sender<PeerMessage>,
    now: Instant,
) -> usize {
    let expired = request_list.expire(now);
    let count = expired.len();

    for record in expired {
        let transfer = record.transfer;
        {
            let mut dl = download.lock().unwrap();
            if !dl.has_piece(transfer.piece_index) {
                let _ = dl.cancel_block(peer_id, transfer.piece_index, transfer.block_offset);
            }
        }
        let _ = tx
            .send(PeerMessage::Cancel {
                index: transfer.piece_index,
                begin: transfer.block_offset,
                length: transfer.block_length,
            })
            .await;
    }

    count
}

/// Cancels every outstanding request (on choke or before disconnect) and
/// releases the blocks back to the delegator.
async fn cancel_outstanding_requests(
    download: &Arc<Mutex<Download>>,
    peer_id: &HashString,
    request_list: &mut RequestList,
    tx: &mpsc::Sender<PeerMessage>,
) -> usize {
    let outstanding = request_list.take_outstanding();
    let count = outstanding.len();

    for record in outstanding {
        let transfer = record.transfer;
        {
            let mut dl = download.lock().unwrap();
            if !dl.has_piece(transfer.piece_index) {
                let _ = dl.cancel_block(peer_id, transfer.piece_index, transfer.block_offset);
            }
        }
        let _ = tx
            .send(PeerMessage::Cancel {
                index: transfer.piece_index,
                begin: transfer.block_offset,
                length: transfer.block_length,
            })
            .await;
    }

    count
}

/// True when every bit belonging to a real piece is set; padding bits at the
/// end of the last byte are ignored.
fn bitfield_all_set(bytes: &[u8], num_pieces: usize) -> bool {
    if num_pieces == 0 {
        return false;
    }
    for index in 0..num_pieces {
        let byte = index / 8;
        let bit = 0x80 >> (index % 8);
        match bytes.get(byte) {
            Some(value) if value & bit != 0 => {}
            _ => return false,
        }
    }
    true
}

fn bitfield_all_clear(bytes: &[u8]) -> bool {
    bytes.iter().all(|byte| *byte == 0)
}

fn download_is_private_and_pex_enabled(download: &Arc<Mutex<Download>>) -> (bool, bool) {
    let dl = download.lock().unwrap();
    let metadata = dl.metadata();
    let private = metadata.is_private;
    // Until metadata arrives the privacy flag of a magnet is unknown: do not
    // exchange peer lists for it.
    let pex_allowed = !private && !metadata.is_meta_download && dl.client_config().is_pex_enabled();
    (private, pex_allowed)
}

/// Answers a BEP 9 metadata request from the cached/reconstructed info dict.
async fn serve_metadata_piece(
    download: &Arc<Mutex<Download>>,
    tx: &mpsc::Sender<PeerMessage>,
    extension_id: u8,
    piece: u32,
) {
    let metadata = {
        let dl = download.lock().unwrap();
        ensure_local_metadata(&dl)
    };

    let Some(metadata) = metadata else {
        let _ = tx
            .send(PeerMessage::Extended {
                id: extension_id,
                payload: ut_metadata_reject_payload(piece),
            })
            .await;
        return;
    };

    if let Some((total_size, data)) =
        lt_protocol::metadata::metadata_piece(metadata.as_slice(), piece)
    {
        let _ = tx
            .send(PeerMessage::Extended {
                id: extension_id,
                payload: ut_metadata_data_payload(piece, total_size, data),
            })
            .await;
    } else {
        let _ = tx
            .send(PeerMessage::Extended {
                id: extension_id,
                payload: ut_metadata_reject_payload(piece),
            })
            .await;
    }
}

/// Shared message loop for outbound and inbound peers.
#[allow(clippy::too_many_arguments)]
async fn run_peer_session(
    mut reader: tokio::net::tcp::OwnedReadHalf,
    mut writer: tokio::net::tcp::OwnedWriteHalf,
    peer_addr: SocketAddr,
    peer_id: HashString,
    download: Arc<Mutex<Download>>,
    mut shutdown_rx: watch::Receiver<bool>,
    cipher_enc: Option<Rc4>,
    cipher_dec: Option<Rc4>,
    direction: PeerDirection,
    encrypted: bool,
    obfuscated: bool,
    remote_fast: bool,
    pool: Option<PeerPoolHandle>,
    request_timeout: Option<Duration>,
) {
    let mut cipher_dec = cipher_dec;
    let mut request_tick = tokio::time::interval(Duration::from_secs(1));
    request_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    // 3. Register Peer in Download ConnectionList
    let num_pieces = {
        let mut dl = download.lock().unwrap();
        let n = dl.total_chunks();
        let mut peer_conn = PeerConnection::new_with_direction(
            peer_id,
            SocketAddress::from(peer_addr),
            n,
            direction,
            encrypted,
        );
        peer_conn.is_encrypted = encrypted;
        peer_conn.is_obfuscated = obfuscated;
        let added = dl.add_peer(peer_conn);
        (n, added)
    };

    if !num_pieces.1 {
        debug!(peer = %peer_addr, "Connection rejected by peer limits");
        if let Some(pool) = &pool {
            pool.lock()
                .unwrap()
                .record_disconnected(peer_addr, Instant::now());
        }
        return;
    }
    let mut num_pieces = num_pieces.0;
    let info_hash = *download.lock().unwrap().info_hash();
    // A reconnect must not inherit a stale group-choke marker from a previous
    // session with the same peer id.
    set_peer_down_choked(&info_hash, &peer_id, false);

    // Per-peer request pipeline. The queue limit defaults to 250 and is
    // replaced by the peer's LTEP `reqq` value when its handshake arrives.
    let mut request_list = RequestList::with_queue_limit(DEFAULT_PEER_REQUEST_QUEUE);
    {
        let dl = download.lock().unwrap();
        request_list.set_block_size(negotiated_block_size(&dl));
    }
    if let Some(timeout) = request_timeout {
        request_list.set_request_timeout(timeout);
    }
    publish_peer_request_stats(info_hash, peer_id, &request_list, true);

    // 4. Send our Interested message
    let (tx, mut rx) = mpsc::channel::<PeerMessage>(256);
    let _ = tx.send(PeerMessage::Interested).await;
    update_peer_flags(&download, &peer_id, |peer| peer.is_interested = true);

    // Metadata we can serve to this peer, and whether PEX is allowed for this
    // torrent. Private torrents never advertise or accept PEX.
    let (local_metadata_size, mut pex_allowed) = {
        let dl = download.lock().unwrap();
        (
            ensure_local_metadata(&dl).map(|bytes| bytes.len() as u32),
            !dl.metadata().is_private
                && !dl.metadata().is_meta_download
                && dl.client_config().is_pex_enabled(),
        )
    };

    // LTEP handshake advertising ut_metadata (id 1) and, unless private, PEX
    // (id 2). The remote ids are learned from its reply.
    let _ = tx
        .send(PeerMessage::Extended {
            id: 0,
            payload: extension_handshake_payload(
                if pex_allowed { LOCAL_PEX_ID } else { 0 },
                LOCAL_METADATA_ID,
                local_metadata_size,
            ),
        })
        .await;

    // 4b. Advertise the pieces we can upload. In initial-seeding mode the
    // bitfield stays empty; each peer is offered one piece with a HAVE
    // message instead (BEP 16).
    let (our_bitfield, initial_offer, initial_seeding) = {
        let mut dl = download.lock().unwrap();
        let initial = dl.is_initial_seeding();
        let bitfield = if initial {
            vec![0u8; num_pieces.div_ceil(8)]
        } else {
            dl.completed_bitfield_bytes()
        };
        let offer = if initial {
            dl.initial_seed_offer(&peer_id, None)
        } else {
            None
        };
        // If the peer's bitfield proved that everything is already in the
        // swarm, initial seeding ended while choosing the first offer. Advertise
        // the completed bitfield immediately instead of staying empty.
        let bitfield = if initial && !dl.is_initial_seeding() {
            dl.completed_bitfield_bytes()
        } else {
            bitfield
        };
        (bitfield, offer, initial)
    };
    let bitfield_message = if remote_fast && initial_seeding {
        // BEP 6: `have_none` takes the place of an empty bitfield. The
        // initial-seeding HAVE offer is still sent afterwards.
        PeerMessage::HaveNone
    } else if remote_fast && bitfield_all_set(&our_bitfield, num_pieces) {
        PeerMessage::HaveAll
    } else if remote_fast && bitfield_all_clear(&our_bitfield) {
        PeerMessage::HaveNone
    } else {
        PeerMessage::Bitfield(our_bitfield)
    };
    let _ = tx.send(bitfield_message).await;
    if let Some(index) = initial_offer {
        let _ = tx.send(PeerMessage::Have { index }).await;
    }
    // Start with an upload slot; the choke group can choke us later through
    // `apply_choke_changes_to_download`, which the tick below mirrors on the
    // wire.
    let _ = tx.send(PeerMessage::Unchoke).await;
    update_peer_flags(&download, &peer_id, |peer| peer.is_choked = false);

    let mut remote_metadata_id = 0u8;
    let mut remote_pex_id = 0u8;
    // Peers we have already advertised in a PEX message, used to compute the
    // `added`/`dropped` deltas of later updates.
    let mut pex_sent: HashSet<SocketAddr> = HashSet::new();
    // Send the first update as soon as the remote extension handshake is
    // known, then once per PEX_INTERVAL.
    let mut last_pex_sent = Instant::now()
        .checked_sub(PEX_INTERVAL)
        .unwrap_or_else(Instant::now);
    // Incoming upload requests that have not been cancelled/served yet.
    let mut upload_requests: HashMap<(u32, u32), u32> = HashMap::new();

    // Background task for writing outgoing messages
    let mut write_shutdown = shutdown_rx.clone();
    let mut write_cipher = if encrypted { cipher_enc } else { None };
    let write_task = tokio::spawn(async move {
        while !*write_shutdown.borrow() {
            tokio::select! {
                _ = write_shutdown.changed() => {
                    break;
                }
                msg_opt = rx.recv() => {
                    match msg_opt {
                        Some(msg) => {
                            let mut frame = msg.encode();
                            if let Some(cipher) = write_cipher.as_mut() {
                                cipher.crypt(&mut frame);
                            }

                            if writer.write_all(&frame).await.is_err() {
                                break;
                            }
                        }
                        None => break,
                    }
                }
            }
        }
    });

    // 5. Incoming message framing loop
    let mut len_buf = [0u8; 4];
    let mut peer_unchoked = false;
    let mut local_choked = false;

    loop {
        tokio::select! {
            _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() {
                    break;
                }
            }
            _ = request_tick.tick() => {
                let now = Instant::now();

                // Expire metadata requests whose owner did not answer, then
                // keep the queue filled. Re-filling every tick also picks up
                // pieces released by a peer that disconnected.
                let metadata_pending = {
                    let mut dl = download.lock().unwrap();
                    match dl.metadata_download_mut() {
                        Some(metadata) => {
                            let _ =
                                metadata.expire_requests(now, METADATA_REQUEST_TIMEOUT);
                            metadata.is_size_known() && !metadata.is_complete()
                        }
                        None => false,
                    }
                };
                if metadata_pending && remote_metadata_id != 0 {
                    request_metadata_pieces(&download, &tx, remote_metadata_id, peer_id).await;
                }

                // PEX is best-effort and never allowed for private torrents.
                let (private_now, pex_enabled_now) =
                    download_is_private_and_pex_enabled(&download);
                pex_allowed = pex_enabled_now && !private_now;

                if pex_allowed
                    && remote_pex_id != 0
                    && now.duration_since(last_pex_sent) >= PEX_INTERVAL
                {
                    let current: HashSet<SocketAddr> = match &pool {
                        Some(pool) => pool
                            .lock()
                            .unwrap()
                            .entries()
                            .filter(|entry| entry.state().is_connected())
                            .map(|entry| entry.address())
                            .filter(|address| *address != peer_addr)
                            .take(MAX_PEX_PEERS_PER_MESSAGE)
                            .collect(),
                        None => download
                            .lock()
                            .unwrap()
                            .connection_list()
                            .iter()
                            .filter_map(|peer| peer.socket_address.as_socket_addr())
                            .filter(|address| *address != peer_addr)
                            .take(MAX_PEX_PEERS_PER_MESSAGE)
                            .collect(),
                    };

                    if !current.is_empty() || !pex_sent.is_empty() {
                        let added: Vec<_> = current
                            .difference(&pex_sent)
                            .copied()
                            .map(SocketAddress::from)
                            .collect();
                        let dropped: Vec<_> = pex_sent
                            .difference(&current)
                            .copied()
                            .map(SocketAddress::from)
                            .collect();

                        if !added.is_empty() || !dropped.is_empty() {
                            let _ = tx
                                .send(PeerMessage::Extended {
                                    id: remote_pex_id,
                                    payload: ut_pex_payload(&added, &dropped),
                                })
                                .await;
                        }
                        pex_sent = current;
                    }
                    last_pex_sent = now;
                }

                // Mirror upload choke decisions from the resource manager on
                // the wire.
                let should_choke = download
                    .lock()
                    .unwrap()
                    .connection_list()
                    .find_by_id(&peer_id)
                    .map(|peer| peer.is_choked)
                    .unwrap_or(local_choked);
                if should_choke != local_choked {
                    local_choked = should_choke;
                    let message = if local_choked {
                        PeerMessage::Choke
                    } else {
                        PeerMessage::Unchoke
                    };
                    let _ = tx.send(message).await;
                }

                // Blocks completed by another peer (endgame winner) no longer
                // need to be requested from this peer.
                let _ = cancel_completed_requests(&download, &mut request_list, &tx).await;

                // A group download-choke can arrive between ticks; release
                // anything still outstanding before re-filling.
                if is_peer_down_choked(&info_hash, &peer_id)
                    && request_list.queue_depth() != 0
                {
                    let _ = cancel_outstanding_requests(
                        &download,
                        &peer_id,
                        &mut request_list,
                        &tx,
                    )
                    .await;
                }

                // Expired requests are returned to the delegator so another
                // peer (or this one on a later attempt) can take them.
                let _ = expire_requests(
                    &download,
                    &peer_id,
                    &mut request_list,
                    &tx,
                    now,
                )
                .await;

                let requests = fill_request_pipeline(
                    &download,
                    &peer_id,
                    info_hash,
                    peer_unchoked,
                    &mut request_list,
                    now,
                );
                send_requests(&tx, &requests).await;

                publish_peer_request_stats(
                    info_hash,
                    peer_id,
                    &request_list,
                    !peer_unchoked,
                );
            }
            res = reader.read_exact(&mut len_buf) => {
                if res.is_err() {
                    break;
                }
                if encrypted {
                    if let Some(cipher) = cipher_dec.as_mut() {
                        cipher.crypt(&mut len_buf);
                    }
                }

                let length = u32::from_be_bytes(len_buf) as usize;
                if length == 0 {
                    // Keep-alive frame
                    continue;
                }
                if length > 2 * 1024 * 1024 {
                    // Guard against invalid/corrupt length
                    warn!(peer = %peer_addr, length, "Peer sent oversized message");
                    break;
                }

                let mut payload = vec![0u8; length];
                if reader.read_exact(&mut payload).await.is_err() {
                    break;
                }

                if encrypted {
                    if let Some(cipher) = cipher_dec.as_mut() {
                        cipher.crypt(&mut payload);
                    }
                }

                let id = payload[0];
                let body = &payload[1..];
                debug!(peer = %peer_addr, id, body_len = body.len(), "Received peer wire message");

                match PeerMessage::decode(id, body) {
                    Ok(msg) => match msg {
                        PeerMessage::Choke => {
                            debug!(peer = %peer_addr, "Peer choked us");
                            peer_unchoked = false;
                            update_peer_flags(&download, &peer_id, |peer| {
                                peer.peer_choked = true
                            });

                            // Cancel and requeue outstanding blocks. The peer
                            // may still deliver blocks that were already on
                            // the wire; those are accepted in the Piece arm.
                            let _ = cancel_outstanding_requests(
                                &download,
                                &peer_id,
                                &mut request_list,
                                &tx,
                            )
                            .await;

                            publish_peer_request_stats(
                                info_hash,
                                peer_id,
                                &request_list,
                                true,
                            );
                        }
                        PeerMessage::Unchoke => {
                            debug!(peer = %peer_addr, "Peer unchoked us!");
                            peer_unchoked = true;
                            update_peer_flags(&download, &peer_id, |peer| {
                                peer.peer_choked = false
                            });

                            let requests = fill_request_pipeline(
                                &download,
                                &peer_id,
                                info_hash,
                                peer_unchoked,
                                &mut request_list,
                                Instant::now(),
                            );
                            send_requests(&tx, &requests).await;

                            publish_peer_request_stats(
                                info_hash,
                                peer_id,
                                &request_list,
                                false,
                            );
                        }
                        PeerMessage::Interested => {
                            debug!(peer = %peer_addr, "Peer is interested");
                            update_peer_flags(&download, &peer_id, |peer| {
                                peer.peer_interested = true
                            });
                        }
                        PeerMessage::NotInterested => {
                            debug!(peer = %peer_addr, "Peer is not interested");
                            update_peer_flags(&download, &peer_id, |peer| {
                                peer.peer_interested = false
                            });
                        }
                        PeerMessage::Have { index } => {
                            debug!(peer = %peer_addr, index, "Peer have chunk");
                            let (initial_complete, next_offer) = {
                                let mut dl = download.lock().unwrap();
                                dl.receive_have(&peer_id, index);
                                if dl.is_initial_seeding() {
                                    let complete = dl.initial_seed_seen(index, &peer_id);
                                    let offer = if complete {
                                        None
                                    } else {
                                        dl.initial_seed_offer(&peer_id, Some(index))
                                    };
                                    (complete, offer)
                                } else {
                                    (false, None)
                                }
                            };

                            if initial_complete {
                                let bitfield =
                                    download.lock().unwrap().completed_bitfield_bytes();
                                let _ = tx.send(PeerMessage::Bitfield(bitfield)).await;
                            } else if let Some(next) = next_offer {
                                let _ = tx.send(PeerMessage::Have { index: next }).await;
                            }

                            refill_requests(
                                &download,
                                &peer_id,
                                info_hash,
                                peer_unchoked,
                                &mut request_list,
                                &tx,
                            )
                            .await;
                        }
                        PeerMessage::Bitfield(data) => {
                            debug!(peer = %peer_addr, bytes_len = data.len(), "Peer sent bitfield");
                            let mut bf = Bitfield::new(num_pieces);
                            for (b_idx, &b) in data.iter().enumerate() {
                                for bit in 0..8 {
                                    let idx = b_idx * 8 + bit;
                                    if idx < num_pieces && (b & (0x80 >> bit)) != 0 {
                                        bf.set(idx);
                                    }
                                }
                            }
                            {
                                let mut dl = download.lock().unwrap();
                                dl.receive_bitfield(&peer_id, bf);
                            }

                            refill_requests(
                                &download,
                                &peer_id,
                                info_hash,
                                peer_unchoked,
                                &mut request_list,
                                &tx,
                            )
                            .await;
                        }
                        PeerMessage::Request { index, begin, length } => {
                            let request_key = (index, begin);
                            let queue_full = !upload_requests.contains_key(&request_key)
                                && upload_requests.len() >= MAX_PEER_REQUEST_QUEUE;

                            if queue_full {
                                debug!(
                                    peer = %peer_addr,
                                    index,
                                    begin,
                                    "Ignoring upload request: peer queue full"
                                );
                            } else {
                                upload_requests.insert(request_key, length);

                                let block = {
                                    let mut dl = download.lock().unwrap();
                                    let unchoked = dl
                                        .connection_list()
                                        .find_by_id(&peer_id)
                                        .map(|peer| !peer.is_choked)
                                        .unwrap_or(true);
                                    let allowed = unchoked
                                        && if dl.is_initial_seeding() {
                                            dl.initial_seed_allows(&peer_id, index)
                                        } else {
                                            dl.has_piece(index)
                                        };
                                    if !allowed {
                                        None
                                    } else {
                                        match dl.read_piece_block(index, begin, length) {
                                            Ok(data) => {
                                                dl.record_upload_for_peer(&peer_id, data.len());
                                                Some(data)
                                            }
                                            Err(err) => {
                                                debug!(
                                                    peer = %peer_addr,
                                                    index,
                                                    begin,
                                                    length,
                                                    error = %err,
                                                    "Could not read requested block"
                                                );
                                                None
                                            }
                                        }
                                    }
                                };

                                if let Some(data) = block {
                                    let _ = tx
                                        .send(PeerMessage::Piece { index, begin, data })
                                        .await;
                                }
                                upload_requests.remove(&request_key);
                            }
                        }
                        PeerMessage::Piece { index, begin, data } => {
                            info!(peer = %peer_addr, index, begin, size = data.len(), "Received Piece block!");

                            // Record downloaded bytes and rate on the peer connection
                            {
                                let mut dl = download.lock().unwrap();
                                if let Some(peer) = dl.connection_list_mut().find_by_id_mut(&peer_id) {
                                    peer.add_downloaded(data.len() as u64);
                                }
                            }

                            let received = request_list.take_block_at(index, begin);

                            // All copies except the one we just received are
                            // endgame duplicates sent to this peer; cancel
                            // them so it stops working on the same block.
                            for duplicate in received.iter().skip(1) {
                                let _ = tx
                                    .send(PeerMessage::Cancel {
                                        index: duplicate.transfer.piece_index,
                                        begin: duplicate.transfer.block_offset,
                                        length: duplicate.transfer.block_length,
                                    })
                                    .await;
                            }

                            // A late block from a cancelled/choked request is
                            // still useful. Do not discard it just because it
                            // is no longer tracked.
                            let already_complete = {
                                let dl = download.lock().unwrap();
                                dl.has_piece(index)
                                    || block_already_received(&dl, index, begin)
                            };
                            let mut download_finished = false;
                            if !already_complete {
                                let mut dl = download.lock().unwrap();
                                let completed = dl
                                    .receive_block(index, begin, &data)
                                    .unwrap_or(false);
                                let _ = dl.poll_disk_completions();
                                download_finished = completed && dl.is_done();
                            }

                            if download_finished {
                                update_peer_flags(&download, &peer_id, |peer| {
                                    peer.is_interested = false
                                });
                                let _ = tx.send(PeerMessage::NotInterested).await;
                            }

                            refill_requests(
                                &download,
                                &peer_id,
                                info_hash,
                                peer_unchoked,
                                &mut request_list,
                                &tx,
                            )
                            .await;
                        }
                        PeerMessage::Cancel { index, begin, length } => {
                            // The peer no longer wants an upload block. The
                            // upload path serves requests immediately, so this
                            // is normally a no-op; still drop any tracked copy.
                            debug!(peer = %peer_addr, index, begin, length, "Peer cancelled upload request");
                            upload_requests.remove(&(index, begin));
                        }
                        PeerMessage::HaveAll => {
                            debug!(peer = %peer_addr, "Peer has all pieces");
                            let mut bitfield = Bitfield::new(num_pieces);
                            for index in 0..num_pieces {
                                bitfield.set(index);
                            }
                            {
                                let mut dl = download.lock().unwrap();
                                dl.receive_bitfield(&peer_id, bitfield);
                            }
                            refill_requests(
                                &download,
                                &peer_id,
                                info_hash,
                                peer_unchoked,
                                &mut request_list,
                                &tx,
                            )
                            .await;
                        }
                        PeerMessage::HaveNone => {
                            debug!(peer = %peer_addr, "Peer has no pieces");
                            let bitfield = Bitfield::new(num_pieces);
                            {
                                let mut dl = download.lock().unwrap();
                                dl.receive_bitfield(&peer_id, bitfield);
                            }
                        }
                        PeerMessage::RejectRequest { index, begin, length } => {
                            debug!(peer = %peer_addr, index, begin, length, "Peer rejected request");
                            // Be tolerant of a reject that races a timeout or
                            // echoes a slightly different length.
                            let _ = request_list.take_block_at(index, begin);

                            // Return the block to the delegator so it can be
                            // retried or reassigned to another peer. A peer
                            // that rejects an unchoked request is penalised so
                            // repeated rejects become visible to the choke
                            // heuristics and the retry scheduler.
                            {
                                let mut dl = download.lock().unwrap();
                                if let Some(peer) = dl
                                    .connection_list_mut()
                                    .find_by_id_mut(&peer_id)
                                {
                                    peer.inc_failed_counter();
                                }
                                if !dl.has_piece(index) {
                                    let _ = dl.cancel_block(&peer_id, index, begin);
                                }
                            }

                            refill_requests(
                                &download,
                                &peer_id,
                                info_hash,
                                peer_unchoked,
                                &mut request_list,
                                &tx,
                            )
                            .await;
                        }
                        PeerMessage::SuggestPiece { index } => {
                            debug!(peer = %peer_addr, index, "Peer suggested piece");
                            if index < num_pieces as u32 {
                                record_suggested_piece(&info_hash, &peer_id, index);
                            }
                        }
                        PeerMessage::AllowedFast { index } => {
                            debug!(peer = %peer_addr, index, "Peer allowed fast piece");
                            if index < num_pieces as u32 {
                                record_allowed_fast(&info_hash, &peer_id, index);
                            }
                        }
                        PeerMessage::Extended { id: 0, payload } => {
                            let handshake = parse_extension_handshake(&payload);

                            if let Some(metadata_id) = handshake.metadata_id {
                                remote_metadata_id = metadata_id;
                            }
                            if let Some(pex_id) = handshake.pex_id {
                                remote_pex_id = pex_id;
                            }
                            if let Some(size) = handshake.metadata_size {
                                let mut dl = download.lock().unwrap();
                                if let Some(metadata) = dl.metadata_download_mut() {
                                    if let Err(err) = metadata.set_size(size as usize) {
                                        debug!(
                                            peer = %peer_addr,
                                            size,
                                            error = %err,
                                            "Rejected metadata size"
                                        );
                                    }
                                }
                            }
                            if let Some(request_queue) = handshake.request_queue {
                                request_list.set_queue_limit(request_queue);
                                publish_peer_request_stats(
                                    info_hash,
                                    peer_id,
                                    &request_list,
                                    !peer_unchoked,
                                );
                            }

                            request_metadata_pieces(&download, &tx, remote_metadata_id, peer_id).await;
                        }
                        PeerMessage::Extended { id, payload } if id == LOCAL_METADATA_ID => {
                            // Incoming ut_metadata messages always use the id
                            // *we* advertised, irrespective of the id the
                            // remote chose for its own messages.
                            match UtMetadataMessage::parse(&payload) {
                                Ok(UtMetadataMessage::Request { piece }) => {
                                    serve_metadata_piece(&download, &tx, id, piece).await;
                                }
                                Ok(UtMetadataMessage::Data {
                                    piece,
                                    total_size,
                                    data,
                                }) => {
                                    let complete = {
                                        let mut dl = download.lock().unwrap();
                                        dl.metadata_download_mut()
                                            .and_then(|metadata| {
                                                metadata
                                                    .on_data(piece, total_size, &data)
                                                    .ok()
                                            })
                                            .unwrap_or(false)
                                    };

                                    if complete {
                                        let verified = {
                                            let mut dl = download.lock().unwrap();
                                            let info_hash = *dl.info_hash();
                                            let announce = dl.metadata().announce.clone();
                                            let announce_list =
                                                dl.metadata().announce_list.clone();
                                            let verified = dl
                                                .metadata_download_mut()
                                                .map(|metadata| metadata.verify_and_take());
                                            verified.map(|result| {
                                                (result, info_hash, announce, announce_list)
                                            })
                                        };

                                        match verified {
                                            Some((Ok(bytes), info_hash, announce, announce_list)) => {
                                                let (applied, advertise_pex) = {
                                                    let mut dl = download.lock().unwrap();
                                                    match dl.apply_metadata_bytes(
                                                        &bytes,
                                                        info_hash,
                                                        announce,
                                                        announce_list,
                                                    ) {
                                                        Ok(()) => {
                                                            num_pieces = dl.total_chunks();
                                                            register_local_metadata(
                                                                info_hash,
                                                                bytes.clone(),
                                                            );
                                                            let advertise_pex =
                                                                !dl.metadata().is_private
                                                                    && dl
                                                                        .client_config()
                                                                        .is_pex_enabled();
                                                            (Ok(()), advertise_pex)
                                                        }
                                                        Err(err) => (Err(err), false),
                                                    }
                                                };

                                                match applied {
                                                    Ok(()) => {
                                                        info!(peer = %peer_addr, "Metadata applied");
                                                        let _ = tx
                                                            .send(PeerMessage::Extended {
                                                                id: 0,
                                                                payload:
                                                                    extension_handshake_payload(
                                                                        if advertise_pex {
                                                                            LOCAL_PEX_ID
                                                                        } else {
                                                                            0
                                                                        },
                                                                        LOCAL_METADATA_ID,
                                                                        Some(bytes.len() as u32),
                                                                    ),
                                                            })
                                                            .await;
                                                    }
                                                    Err(err) => {
                                                        warn!(error = %err, "Could not apply metadata")
                                                    }
                                                }
                                            }
                                            Some((Err(err), _, _, _)) => {
                                                warn!(peer = %peer_addr, error = %err, "Metadata verification failed");
                                                // The complete but unverifiable
                                                // set is useless; allow a
                                                // fresh attempt from scratch.
                                                let mut dl = download.lock().unwrap();
                                                if let Some(metadata) = dl.metadata_download_mut() {
                                                    metadata.reset();
                                                }
                                            }
                                            None => {}
                                        }
                                    }

                                    request_metadata_pieces(&download, &tx, remote_metadata_id, peer_id)
                                        .await;
                                }
                                Ok(UtMetadataMessage::Reject { piece }) => {
                                    {
                                        let mut dl = download.lock().unwrap();
                                        if let Some(metadata) = dl.metadata_download_mut() {
                                            metadata.mark_rejected_by(piece, peer_id);
                                        }
                                    }
                                    request_metadata_pieces(&download, &tx, remote_metadata_id, peer_id)
                                        .await;
                                }
                                Err(err) => {
                                    debug!(peer = %peer_addr, error = %err, "Invalid ut_metadata payload");
                                }
                            }
                        }
                        PeerMessage::Extended { id, payload } if id == LOCAL_PEX_ID => {
                            if !pex_allowed || remote_pex_id == 0 {
                                pex_private_blocked().fetch_add(1, Ordering::Relaxed);
                                debug!(peer = %peer_addr, "Ignoring PEX (disabled or private)");
                            } else {
                                match UtPexMessage::parse(&payload) {
                                    Ok(pex) => {
                                        let addresses: Vec<SocketAddr> = pex
                                            .all_added()
                                            .filter_map(|address| address.as_socket_addr())
                                            .collect();
                                        let mut added = 0usize;
                                        let mut rejected = 0usize;
                                        if let Some(pool) = &pool {
                                            let mut pool = pool.lock().unwrap();
                                            for address in addresses {
                                                if pool.add_peer(address, PeerSource::Pex, false)
                                                {
                                                    added += 1;
                                                } else {
                                                    rejected += 1;
                                                }
                                            }
                                        } else {
                                            let mut dl = download.lock().unwrap();
                                            for address in addresses {
                                                dl.add_discovered_peer(address);
                                                added += 1;
                                            }
                                        }
                                        pex_added().fetch_add(added as u64, Ordering::Relaxed);
                                        pex_rejected()
                                            .fetch_add(rejected as u64, Ordering::Relaxed);
                                        if added > 0 {
                                            debug!(peer = %peer_addr, added, "PEX peers merged");
                                        }
                                    }
                                    Err(err) => {
                                        pex_rejected().fetch_add(1, Ordering::Relaxed);
                                        debug!(peer = %peer_addr, error = %err, "Invalid ut_pex payload");
                                    }
                                }
                            }
                        }
                        _ => {}
                    },
                    Err(e) => {
                        debug!(peer = %peer_addr, id, error = ?e, "Failed to decode message");
                    }
                }
            }
        }
    }

    write_task.abort();

    // 6. Return every unfinished block to the delegator and release the
    // chunk handles before removing the peer.
    {
        let outstanding = request_list.take_outstanding();
        let mut dl = download.lock().unwrap();
        // Metadata requests owned by this peer must be retried elsewhere.
        if let Some(metadata) = dl.metadata_download_mut() {
            metadata.release_peer(&peer_id);
        }
        for record in outstanding {
            let transfer = record.transfer;
            if !dl.has_piece(transfer.piece_index) {
                let _ = dl.cancel_block(&peer_id, transfer.piece_index, transfer.block_offset);
            }
        }
        dl.cancel_peer(&peer_id);
        dl.remove_peer(&peer_id);
    }

    request_list.clear();
    remove_peer_request_stats(&info_hash, &peer_id);
    remove_fast_state(&info_hash, &peer_id);
    set_peer_down_choked(&info_hash, &peer_id, false);

    if let Some(pool) = &pool {
        pool.lock()
            .unwrap()
            .record_disconnected(peer_addr, Instant::now());
    }

    debug!(peer = %peer_addr, "Peer disconnected");
}
