//! Iterative Kademlia lookups (BEP 5).
//!
//! The legacy `DhtSearch`/`DhtAnnounce` state machines are reproduced here as
//! async functions on top of [`DhtServer::send_query`]. The lookup queries up
//! to `ALPHA` closest nodes in parallel, merges newly discovered nodes and
//! stops when the candidate set is exhausted or `max_queries` is reached.

use std::collections::{HashMap, HashSet};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use lt_common::hash_string::HashString;
use lt_net::addr::SocketAddress;
use tokio::sync::Mutex;
use tokio::task::JoinSet;

use crate::error::DhtError;
use crate::node::DhtNode;
use crate::router::{xor_distance, DhtRouter};
use crate::server::DhtServer;
use crate::transaction::{KrpcQuery, KrpcResponse};

/// Kademlia concurrency parameter.
pub const ALPHA: usize = 3;
/// Default upper bound on queries per lookup.
pub const MAX_QUERIES: usize = 64;
/// Maximum candidate set retained during a lookup.
const MAX_CANDIDATES: usize = 64;
/// Maximum number of nodes announced to.
const MAX_ANNOUNCE: usize = 8;

struct LookupOutcome {
    peers: Vec<SocketAddress>,
    tokens: Vec<(DhtNode, Vec<u8>)>,
    queries: usize,
}

fn now_seconds() -> u32 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as u32)
        .unwrap_or(0)
}

fn decode_peer(compact: &[u8; 6]) -> Option<SocketAddress> {
    let ip = Ipv4Addr::new(compact[0], compact[1], compact[2], compact[3]);
    let port = u16::from_be_bytes([compact[4], compact[5]]);
    if port == 0 {
        return None;
    }
    Some(SocketAddress::from(SocketAddr::V4(SocketAddrV4::new(
        ip, port,
    ))))
}

/// Decodes the compact node list used by `find_node` and `get_peers`.
pub(crate) fn decode_nodes(bytes: &[u8]) -> Vec<DhtNode> {
    bytes
        .as_chunks::<26>()
        .0
        .iter()
        .filter_map(|chunk| {
            let id = HashString::from_slice(&chunk[0..20])?;
            let ip = Ipv4Addr::new(chunk[20], chunk[21], chunk[22], chunk[23]);
            let port = u16::from_be_bytes([chunk[24], chunk[25]]);
            if port == 0 {
                return None;
            }
            Some(DhtNode::new(
                id,
                SocketAddress::from(SocketAddr::V4(SocketAddrV4::new(ip, port))),
            ))
        })
        .collect()
}

async fn lookup(
    server: Arc<DhtServer>,
    router: Arc<Mutex<DhtRouter>>,
    target: HashString,
    max_queries: usize,
) -> LookupOutcome {
    let own_id = router.lock().await.own_id;

    let mut candidates = router.lock().await.closest_nodes(&target, MAX_CANDIDATES);
    let mut known: HashSet<HashString> = candidates.iter().map(|node| node.id).collect();
    let mut queried: HashSet<HashString> = HashSet::new();
    let mut peers: HashSet<SocketAddress> = HashSet::new();
    let mut tokens: HashMap<HashString, (DhtNode, Vec<u8>)> = HashMap::new();
    let mut queries = 0usize;

    loop {
        if queries >= max_queries {
            break;
        }

        let batch: Vec<DhtNode> = candidates
            .iter()
            .filter(|node| !queried.contains(&node.id))
            .take(ALPHA)
            .cloned()
            .collect();

        if batch.is_empty() {
            break;
        }

        let mut tasks = JoinSet::new();
        for node in batch {
            queried.insert(node.id);
            let server = Arc::clone(&server);

            tasks.spawn(async move {
                let result = match node.address.as_socket_addr() {
                    Some(address) => {
                        server
                            .send_query(
                                address,
                                KrpcQuery::GetPeers {
                                    id: own_id,
                                    info_hash: target,
                                },
                            )
                            .await
                    }
                    None => Err(DhtError::Input("node has no inet address".into())),
                };

                (node, result)
            });
        }

        while let Some(joined) = tasks.join_next().await {
            let (node, result) = match joined {
                Ok(value) => value,
                Err(_) => continue,
            };

            queries += 1;

            match result {
                Ok(KrpcResponse::GetPeers {
                    token,
                    values,
                    nodes: new_nodes,
                    ..
                }) => {
                    router.lock().await.mark_replied(&node.id, now_seconds());

                    for compact in values {
                        if let Some(peer) = decode_peer(&compact) {
                            peers.insert(peer);
                        }
                    }

                    if let Some(bytes) = new_nodes {
                        for discovered in decode_nodes(&bytes) {
                            if known.insert(discovered.id) {
                                router.lock().await.add_node(discovered.clone());
                                candidates.push(discovered);
                            }
                        }
                    }

                    tokens.insert(node.id, (node, token));
                }
                Ok(_) => {}
                Err(_) => {
                    router.lock().await.mark_inactive(&node.id);
                }
            }
        }

        candidates.sort_by_key(|a| xor_distance(&a.id, &target));
        candidates.truncate(MAX_CANDIDATES);
    }

    let mut token_list: Vec<(DhtNode, Vec<u8>)> = tokens.into_values().collect();
    token_list.sort_by_key(|(a, _)| xor_distance(&a.id, &target));

    LookupOutcome {
        peers: peers.into_iter().collect(),
        tokens: token_list,
        queries,
    }
}

/// Performs an iterative `get_peers` lookup for `info_hash`.
pub async fn get_peers(
    server: Arc<DhtServer>,
    router: Arc<Mutex<DhtRouter>>,
    info_hash: HashString,
    max_queries: usize,
) -> Result<Vec<SocketAddress>, DhtError> {
    let outcome = lookup(server, router, info_hash, max_queries).await;
    Ok(outcome.peers)
}

/// Announces `info_hash` on `port` to the closest nodes discovered by a
/// `get_peers` lookup. Returns the number of successful announcements.
pub async fn announce_peer(
    server: Arc<DhtServer>,
    router: Arc<Mutex<DhtRouter>>,
    info_hash: HashString,
    port: u16,
    max_queries: usize,
) -> Result<usize, DhtError> {
    let own_id = router.lock().await.own_id;
    let outcome = lookup(Arc::clone(&server), router, info_hash, max_queries).await;

    let mut tasks = JoinSet::new();
    for (node, token) in outcome.tokens.into_iter().take(MAX_ANNOUNCE) {
        let server = Arc::clone(&server);
        tasks.spawn(async move {
            match node.address.as_socket_addr() {
                Some(address) => server
                    .send_query(
                        address,
                        KrpcQuery::AnnouncePeer {
                            id: own_id,
                            info_hash,
                            port,
                            token,
                        },
                    )
                    .await
                    .is_ok(),
                None => false,
            }
        });
    }

    let mut announced = 0usize;
    while let Some(result) = tasks.join_next().await {
        if matches!(result, Ok(true)) {
            announced += 1;
        }
    }

    Ok(announced)
}

/// Number of queries used by the last lookup (exposed for tests/metrics).
pub async fn lookup_query_count(
    server: Arc<DhtServer>,
    router: Arc<Mutex<DhtRouter>>,
    target: HashString,
    max_queries: usize,
) -> Result<usize, DhtError> {
    let outcome = lookup(server, router, target, max_queries).await;
    Ok(outcome.queries)
}
