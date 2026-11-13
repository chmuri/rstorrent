//! Iterative Kademlia lookups (BEP 5).
//!
//! The legacy `DhtSearch`/`DhtAnnounce` state machines are reproduced here as
//! async functions on top of [`DhtServer::send_query`]. The lookup queries up
//! to `ALPHA` closest nodes in parallel, merges newly discovered nodes and
//! stops when the candidate set is exhausted or `max_queries` is reached.

use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6};
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

/// Decodes a compact peer: 6 bytes for IPv4 (`values`), 18 for IPv6
/// (`values6`, BEP 32).
fn decode_peer(compact: &[u8]) -> Option<SocketAddress> {
    let (ip, port_bytes) = match compact.len() {
        6 => (
            IpAddr::V4(Ipv4Addr::new(
                compact[0], compact[1], compact[2], compact[3],
            )),
            [compact[4], compact[5]],
        ),
        18 => {
            let mut octets = [0u8; 16];
            octets.copy_from_slice(&compact[0..16]);
            (
                IpAddr::V6(Ipv6Addr::from(octets)),
                [compact[16], compact[17]],
            )
        }
        _ => return None,
    };
    let port = u16::from_be_bytes(port_bytes);
    if port == 0 {
        return None;
    }
    let addr = match ip {
        IpAddr::V4(v4) => SocketAddr::V4(SocketAddrV4::new(v4, port)),
        IpAddr::V6(v6) => SocketAddr::V6(SocketAddrV6::new(v6, port, 0, 0)),
    };
    Some(SocketAddress::from(addr))
}

/// Decodes the compact IPv6 node list (`nodes6`, BEP 32): 38 bytes each.
pub(crate) fn decode_nodes6(bytes: &[u8]) -> Vec<DhtNode> {
    bytes
        .as_chunks::<38>()
        .0
        .iter()
        .filter_map(|chunk| {
            let id = HashString::from_slice(&chunk[0..20])?;
            let mut octets = [0u8; 16];
            octets.copy_from_slice(&chunk[20..36]);
            let ip = Ipv6Addr::from(octets);
            let port = u16::from_be_bytes([chunk[36], chunk[37]]);
            if port == 0 {
                return None;
            }
            Some(DhtNode::new(
                id,
                SocketAddress::from(SocketAddr::V6(SocketAddrV6::new(ip, port, 0, 0))),
            ))
        })
        .collect()
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

fn dht_lookup_semaphore() -> &'static tokio::sync::Semaphore {
    static SEMAPHORE: std::sync::OnceLock<tokio::sync::Semaphore> = std::sync::OnceLock::new();
    SEMAPHORE.get_or_init(|| tokio::sync::Semaphore::new(32))
}

async fn lookup(
    server: Arc<DhtServer>,
    router: Arc<Mutex<DhtRouter>>,
    target: HashString,
    max_queries: usize,
) -> LookupOutcome {
    let _permit = match dht_lookup_semaphore().acquire().await {
        Ok(permit) => permit,
        Err(_) => {
            return LookupOutcome {
                peers: Vec::new(),
                tokens: Vec::new(),
                queries: 0,
            };
        }
    };
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
                    values6,
                    nodes: new_nodes,
                    nodes6: new_nodes6,
                    ..
                }) => {
                    router.lock().await.mark_replied(&node.id, now_seconds());

                    for compact in values.iter().chain(values6.iter()) {
                        if let Some(peer) = decode_peer(compact) {
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
                    if let Some(bytes) = new_nodes6 {
                        for discovered in decode_nodes6(&bytes) {
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
                            implied_port: false,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_peer_handles_ipv4_and_ipv6() {
        let v4 = decode_peer(&[1, 2, 3, 4, 0x1a, 0xe1]).expect("v4 peer");
        assert_eq!(v4.ip(), Some(IpAddr::V4(Ipv4Addr::new(1, 2, 3, 4))));
        assert_eq!(v4.port(), 6881);

        let mut compact = vec![0x20u8; 16];
        compact.extend_from_slice(&6881u16.to_be_bytes());
        let v6 = decode_peer(&compact).expect("v6 peer");
        assert!(v6.is_inet6());
        assert_eq!(v6.port(), 6881);

        assert!(decode_peer(&[1, 2, 3]).is_none());
        assert!(decode_peer(&[1, 2, 3, 4, 0, 0]).is_none());
    }

    #[test]
    fn decode_nodes6_parses_38_byte_entries() {
        let mut buf = Vec::new();
        buf.extend_from_slice(&[9u8; 20]);
        buf.extend_from_slice(&[0x20u8; 16]);
        buf.extend_from_slice(&6881u16.to_be_bytes());
        let nodes = decode_nodes6(&buf);
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].id, HashString::new([9u8; 20]));
        assert!(nodes[0].address.is_inet6());
        assert_eq!(nodes[0].address.port(), 6881);
        assert!(decode_nodes6(&[0u8; 37]).is_empty());
    }
}
