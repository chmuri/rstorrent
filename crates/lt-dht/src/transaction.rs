use lt_common::hash_string::HashString;
use lt_common::object::Object;

use crate::error::DhtError;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KrpcQuery {
    Ping {
        id: HashString,
    },
    FindNode {
        id: HashString,
        target: HashString,
    },
    GetPeers {
        id: HashString,
        info_hash: HashString,
    },
    AnnouncePeer {
        id: HashString,
        info_hash: HashString,
        port: u16,
        token: Vec<u8>,
        implied_port: bool,
    },
    SampleInfohashes {
        id: HashString,
        target: HashString,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KrpcResponse {
    Ping {
        id: HashString,
    },
    FindNode {
        id: HashString,
        nodes: Vec<u8>,
        nodes6: Vec<u8>,
    },
    GetPeers {
        id: HashString,
        token: Vec<u8>,
        values: Vec<Vec<u8>>,
        values6: Vec<Vec<u8>>,
        nodes: Option<Vec<u8>>,
        nodes6: Option<Vec<u8>>,
    },
    AnnouncePeer {
        id: HashString,
    },
    SampleInfohashes {
        id: HashString,
        interval: i64,
        num: i64,
        nodes: Option<Vec<u8>>,
        nodes6: Option<Vec<u8>>,
        samples: Vec<u8>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KrpcError {
    pub code: i64,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KrpcMessage {
    Query(KrpcQuery),
    Response(KrpcResponse),
    Error(KrpcError),
}

impl KrpcMessage {
    pub fn encode(&self, transaction_id: &[u8]) -> Result<Vec<u8>, DhtError> {
        let mut root = Object::create_map();
        root.insert_key("t", Object::create_string(transaction_id.to_vec()))
            .unwrap();

        match self {
            KrpcMessage::Query(q) => {
                root.insert_key("y", Object::create_string(b"q".to_vec()))
                    .unwrap();
                root.insert_key(
                    "v",
                    Object::create_string(
                        lt_common::version::DHT_CLIENT_VERSION.as_bytes().to_vec(),
                    ),
                )
                .unwrap();
                let mut args = Object::create_map();

                match q {
                    KrpcQuery::Ping { id } => {
                        root.insert_key("q", Object::create_string(b"ping".to_vec()))
                            .unwrap();
                        args.insert_key("id", Object::create_string(id.as_slice().to_vec()))
                            .unwrap();
                    }
                    KrpcQuery::FindNode { id, target } => {
                        root.insert_key("q", Object::create_string(b"find_node".to_vec()))
                            .unwrap();
                        args.insert_key("id", Object::create_string(id.as_slice().to_vec()))
                            .unwrap();
                        args.insert_key(
                            "target",
                            Object::create_string(target.as_slice().to_vec()),
                        )
                        .unwrap();
                    }
                    KrpcQuery::GetPeers { id, info_hash } => {
                        root.insert_key("q", Object::create_string(b"get_peers".to_vec()))
                            .unwrap();
                        args.insert_key("id", Object::create_string(id.as_slice().to_vec()))
                            .unwrap();
                        args.insert_key(
                            "info_hash",
                            Object::create_string(info_hash.as_slice().to_vec()),
                        )
                        .unwrap();
                    }
                    KrpcQuery::AnnouncePeer {
                        id,
                        info_hash,
                        port,
                        token,
                        implied_port,
                    } => {
                        root.insert_key("q", Object::create_string(b"announce_peer".to_vec()))
                            .unwrap();
                        args.insert_key("id", Object::create_string(id.as_slice().to_vec()))
                            .unwrap();
                        args.insert_key(
                            "info_hash",
                            Object::create_string(info_hash.as_slice().to_vec()),
                        )
                        .unwrap();
                        args.insert_key("port", Object::create_value(*port as i64))
                            .unwrap();
                        args.insert_key("token", Object::create_string(token.clone()))
                            .unwrap();
                        if *implied_port {
                            args.insert_key("implied_port", Object::create_value(1))
                                .unwrap();
                        }
                    }
                    KrpcQuery::SampleInfohashes { id, target } => {
                        root.insert_key("q", Object::create_string(b"sample_infohashes".to_vec()))
                            .unwrap();
                        args.insert_key("id", Object::create_string(id.as_slice().to_vec()))
                            .unwrap();
                        args.insert_key(
                            "target",
                            Object::create_string(target.as_slice().to_vec()),
                        )
                        .unwrap();
                    }
                }
                root.insert_key("a", args).unwrap();
            }
            KrpcMessage::Response(r) => {
                root.insert_key("y", Object::create_string(b"r".to_vec()))
                    .unwrap();
                let mut resp = Object::create_map();

                match r {
                    KrpcResponse::Ping { id } => {
                        resp.insert_key("id", Object::create_string(id.as_slice().to_vec()))
                            .unwrap();
                    }
                    KrpcResponse::FindNode { id, nodes, nodes6 } => {
                        resp.insert_key("id", Object::create_string(id.as_slice().to_vec()))
                            .unwrap();
                        resp.insert_key("nodes", Object::create_string(nodes.clone()))
                            .unwrap();
                        if !nodes6.is_empty() {
                            resp.insert_key("nodes6", Object::create_string(nodes6.clone()))
                                .unwrap();
                        }
                    }
                    KrpcResponse::GetPeers {
                        id,
                        token,
                        values,
                        values6,
                        nodes,
                        nodes6,
                    } => {
                        resp.insert_key("id", Object::create_string(id.as_slice().to_vec()))
                            .unwrap();
                        resp.insert_key("token", Object::create_string(token.clone()))
                            .unwrap();
                        if !values.is_empty() {
                            let mut val_list = Object::create_list();
                            for val in values {
                                val_list
                                    .insert_back(Object::create_string(val.clone()))
                                    .unwrap();
                            }
                            resp.insert_key("values", val_list).unwrap();
                        }
                        if !values6.is_empty() {
                            let mut val_list = Object::create_list();
                            for val in values6 {
                                val_list
                                    .insert_back(Object::create_string(val.clone()))
                                    .unwrap();
                            }
                            resp.insert_key("values6", val_list).unwrap();
                        }
                        if let Some(ref n) = nodes {
                            resp.insert_key("nodes", Object::create_string(n.clone()))
                                .unwrap();
                        }
                        if let Some(ref n) = nodes6 {
                            if !n.is_empty() {
                                resp.insert_key("nodes6", Object::create_string(n.clone()))
                                    .unwrap();
                            }
                        }
                    }
                    KrpcResponse::AnnouncePeer { id } => {
                        resp.insert_key("id", Object::create_string(id.as_slice().to_vec()))
                            .unwrap();
                    }
                    KrpcResponse::SampleInfohashes {
                        id,
                        interval,
                        num,
                        nodes,
                        nodes6,
                        samples,
                    } => {
                        resp.insert_key("id", Object::create_string(id.as_slice().to_vec()))
                            .unwrap();
                        resp.insert_key("interval", Object::create_value(*interval))
                            .unwrap();
                        resp.insert_key("num", Object::create_value(*num)).unwrap();
                        resp.insert_key("samples", Object::create_string(samples.clone()))
                            .unwrap();
                        if let Some(ref n) = nodes {
                            resp.insert_key("nodes", Object::create_string(n.clone()))
                                .unwrap();
                        }
                        if let Some(ref n) = nodes6 {
                            if !n.is_empty() {
                                resp.insert_key("nodes6", Object::create_string(n.clone()))
                                    .unwrap();
                            }
                        }
                    }
                }
                root.insert_key("r", resp).unwrap();
            }
            KrpcMessage::Error(e) => {
                root.insert_key("y", Object::create_string(b"e".to_vec()))
                    .unwrap();
                let mut err_list = Object::create_list();
                err_list.insert_back(Object::create_value(e.code)).unwrap();
                err_list
                    .insert_back(Object::create_string(e.message.as_bytes().to_vec()))
                    .unwrap();
                root.insert_key("e", err_list).unwrap();
            }
        }

        let mut out = Vec::new();
        lt_common::object::write_bencode(&root, &mut out, 0)
            .map_err(|e| DhtError::Internal(format!("Failed to bencode KRPC message: {}", e)))?;
        Ok(out)
    }

    pub fn decode(bytes: &[u8]) -> Result<(Vec<u8>, KrpcMessage), DhtError> {
        let (obj, _) = lt_common::object::read_bencode(bytes)
            .map_err(|e| DhtError::Input(format!("Failed to parse Bencode: {}", e)))?;

        let tid = obj
            .get_key("t")
            .map_err(|_| DhtError::Input("Missing transaction ID 't'".into()))?
            .as_string()
            .map_err(|_| DhtError::Input("'t' must be string".into()))?
            .to_vec();

        let y = obj
            .get_key("y")
            .map_err(|_| DhtError::Input("Missing message type 'y'".into()))?
            .as_string()
            .map_err(|_| DhtError::Input("'y' must be string".into()))?;

        match y {
            b"q" => {
                let q_type = obj
                    .get_key("q")
                    .map_err(|_| DhtError::Input("Missing query type 'q'".into()))?
                    .as_string()
                    .map_err(|_| DhtError::Input("'q' must be string".into()))?;

                let args = obj
                    .get_key("a")
                    .map_err(|_| DhtError::Input("Missing arguments 'a'".into()))?;

                let id = Self::extract_id(args, "id")?;

                let query = match q_type {
                    b"ping" => KrpcQuery::Ping { id },
                    b"find_node" => {
                        let target = Self::extract_id(args, "target")?;
                        KrpcQuery::FindNode { id, target }
                    }
                    b"get_peers" => {
                        let info_hash = Self::extract_id(args, "info_hash")?;
                        KrpcQuery::GetPeers { id, info_hash }
                    }
                    b"announce_peer" => {
                        let info_hash = Self::extract_id(args, "info_hash")?;
                        let port = args
                            .get_key("port")
                            .map_err(|_| DhtError::Input("Missing 'port'".into()))?
                            .as_value()
                            .map_err(|_| DhtError::Input("'port' must be int".into()))?
                            as u16;
                        let token = args
                            .get_key("token")
                            .map_err(|_| DhtError::Input("Missing 'token'".into()))?
                            .as_string()
                            .map_err(|_| DhtError::Input("'token' must be string".into()))?
                            .to_vec();
                        // BEP 5: when set, the announcer's source UDP port is
                        // authoritative and the announced `port` is ignored.
                        let implied_port = args
                            .get_key("implied_port")
                            .ok()
                            .and_then(|v| v.as_value().ok())
                            .map(|v| v != 0)
                            .unwrap_or(false);
                        KrpcQuery::AnnouncePeer {
                            id,
                            info_hash,
                            port,
                            token,
                            implied_port,
                        }
                    }
                    b"sample_infohashes" => {
                        let target = Self::extract_id(args, "target")?;
                        KrpcQuery::SampleInfohashes { id, target }
                    }
                    other => {
                        return Err(DhtError::Input(format!(
                            "Unknown query type: {:?}",
                            String::from_utf8_lossy(other)
                        )));
                    }
                };

                Ok((tid, KrpcMessage::Query(query)))
            }
            b"r" => {
                let resp = obj
                    .get_key("r")
                    .map_err(|_| DhtError::Input("Missing response dictionary 'r'".into()))?;

                let id = Self::extract_id(resp, "id")?;

                if let Ok(token_obj) = resp.get_key("token") {
                    let token = token_obj
                        .as_string()
                        .map_err(|_| DhtError::Input("'token' must be string".into()))?
                        .to_vec();

                    let mut values = Vec::new();
                    if let Ok(val_obj) = resp.get_key("values") {
                        if let Ok(val_list) = val_obj.as_list() {
                            for item in val_list {
                                if let Ok(s) = item.as_string() {
                                    if s.len() == 6 {
                                        values.push(s.to_vec());
                                    }
                                }
                            }
                        }
                    }
                    let mut values6 = Vec::new();
                    if let Ok(val_obj) = resp.get_key("values6") {
                        if let Ok(val_list) = val_obj.as_list() {
                            for item in val_list {
                                if let Ok(s) = item.as_string() {
                                    if s.len() == 18 {
                                        values6.push(s.to_vec());
                                    }
                                }
                            }
                        }
                    }

                    let nodes = resp
                        .get_key("nodes")
                        .ok()
                        .and_then(|n| n.as_string().ok())
                        .map(|s| s.to_vec());
                    let nodes6 = resp
                        .get_key("nodes6")
                        .ok()
                        .and_then(|n| n.as_string().ok())
                        .map(|s| s.to_vec());
                    Ok((
                        tid,
                        KrpcMessage::Response(KrpcResponse::GetPeers {
                            id,
                            token,
                            values,
                            values6,
                            nodes,
                            nodes6,
                        }),
                    ))
                } else if resp.get_key("samples").is_ok() {
                    let interval = resp
                        .get_key("interval")
                        .ok()
                        .and_then(|v| v.as_value().ok())
                        .unwrap_or(0);
                    let num = resp
                        .get_key("num")
                        .ok()
                        .and_then(|v| v.as_value().ok())
                        .unwrap_or(0);
                    let samples = resp
                        .get_key("samples")
                        .ok()
                        .and_then(|s| s.as_string().ok())
                        .map(|s| s.to_vec())
                        .unwrap_or_default();
                    let nodes = resp
                        .get_key("nodes")
                        .ok()
                        .and_then(|n| n.as_string().ok())
                        .map(|s| s.to_vec());
                    let nodes6 = resp
                        .get_key("nodes6")
                        .ok()
                        .and_then(|n| n.as_string().ok())
                        .map(|s| s.to_vec());
                    Ok((
                        tid,
                        KrpcMessage::Response(KrpcResponse::SampleInfohashes {
                            id,
                            interval,
                            num,
                            nodes,
                            nodes6,
                            samples,
                        }),
                    ))
                } else if resp.get_key("nodes").is_ok() || resp.get_key("nodes6").is_ok() {
                    let nodes = resp
                        .get_key("nodes")
                        .ok()
                        .and_then(|n| n.as_string().ok())
                        .map(|s| s.to_vec())
                        .unwrap_or_default();
                    let nodes6 = resp
                        .get_key("nodes6")
                        .ok()
                        .and_then(|n| n.as_string().ok())
                        .map(|s| s.to_vec())
                        .unwrap_or_default();
                    Ok((
                        tid,
                        KrpcMessage::Response(KrpcResponse::FindNode { id, nodes, nodes6 }),
                    ))
                } else {
                    Ok((tid, KrpcMessage::Response(KrpcResponse::Ping { id })))
                }
            }
            b"e" => {
                let err_obj = obj
                    .get_key("e")
                    .map_err(|_| DhtError::Input("Missing error list 'e'".into()))?;
                let list = err_obj
                    .as_list()
                    .map_err(|_| DhtError::Input("'e' must be list".into()))?;
                if list.len() < 2 {
                    return Err(DhtError::Input("Error list 'e' too short".into()));
                }
                let code = list[0].as_value().unwrap_or(201);
                let message =
                    String::from_utf8_lossy(list[1].as_string().unwrap_or(b"")).into_owned();
                Ok((tid, KrpcMessage::Error(KrpcError { code, message })))
            }
            other => Err(DhtError::Input(format!(
                "Unknown message category 'y': {:?}",
                String::from_utf8_lossy(other)
            ))),
        }
    }

    fn extract_id(obj: &Object, key: &str) -> Result<HashString, DhtError> {
        let bytes = obj
            .get_key(key)
            .map_err(|_| DhtError::Input(format!("Missing key '{}'", key)))?
            .as_string()
            .map_err(|_| DhtError::Input(format!("'{}' must be string", key)))?;

        if bytes.len() != 20 {
            return Err(DhtError::Input(format!(
                "'{}' must be exactly 20 bytes",
                key
            )));
        }

        let mut id = HashString::default();
        id.as_mut_slice().copy_from_slice(bytes);
        Ok(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queries_advertise_the_client_version() {
        let query = KrpcMessage::Query(KrpcQuery::Ping {
            id: HashString::new([7u8; 20]),
        });
        let bytes = query.encode(b"aa").expect("query should encode");
        let object = lt_common::object::bencode::decode(&bytes).expect("query should decode");

        assert_eq!(
            object.get_key_string("v").expect("v key"),
            lt_common::version::DHT_CLIENT_VERSION.as_bytes()
        );
    }

    #[test]
    fn find_node_response_round_trips_nodes6() {
        let id = HashString::new([3u8; 20]);
        let nodes = vec![0x11u8; 26];
        let nodes6 = vec![0x22u8; 38];
        let msg = KrpcMessage::Response(KrpcResponse::FindNode {
            id,
            nodes: nodes.clone(),
            nodes6: nodes6.clone(),
        });
        let bytes = msg.encode(b"tt").expect("encode");
        let (tid, decoded) = KrpcMessage::decode(&bytes).expect("decode");
        assert_eq!(tid, b"tt");
        match decoded {
            KrpcMessage::Response(KrpcResponse::FindNode {
                id: rid,
                nodes: rn,
                nodes6: rn6,
            }) => {
                assert_eq!(rid, id);
                assert_eq!(rn, nodes);
                assert_eq!(rn6, nodes6);
            }
            _ => panic!("unexpected response variant"),
        }
    }

    #[test]
    fn get_peers_response_round_trips_values6_and_nodes6() {
        let id = HashString::new([4u8; 20]);
        let token = b"tok".to_vec();
        let values = vec![vec![1, 2, 3, 4, 0x1a, 0xe1]];
        let values6 = vec![vec![0x33u8; 18]];
        let nodes6 = vec![0x44u8; 38];
        let msg = KrpcMessage::Response(KrpcResponse::GetPeers {
            id,
            token: token.clone(),
            values: values.clone(),
            values6: values6.clone(),
            nodes: None,
            nodes6: Some(nodes6.clone()),
        });
        let bytes = msg.encode(b"gg").expect("encode");
        let (_, decoded) = KrpcMessage::decode(&bytes).expect("decode");
        match decoded {
            KrpcMessage::Response(KrpcResponse::GetPeers {
                id: rid,
                token: rt,
                values: rv,
                values6: rv6,
                nodes: rn,
                nodes6: rn6,
            }) => {
                assert_eq!(rid, id);
                assert_eq!(rt, token);
                assert_eq!(rv, values);
                assert_eq!(rv6, values6);
                assert!(rn.is_none());
                assert_eq!(rn6, Some(nodes6));
            }
            _ => panic!("unexpected response variant"),
        }
    }

    #[test]
    fn sample_infohashes_round_trips_samples_num_interval() {
        let id = HashString::new([5u8; 20]);
        let target = HashString::new([6u8; 20]);
        let query = KrpcMessage::Query(KrpcQuery::SampleInfohashes { id, target });
        let bytes = query.encode(b"ss").expect("encode query");
        let (tid, decoded) = KrpcMessage::decode(&bytes).expect("decode query");
        assert_eq!(tid, b"ss");
        match decoded {
            KrpcMessage::Query(KrpcQuery::SampleInfohashes {
                id: qid,
                target: qt,
            }) => {
                assert_eq!(qid, id);
                assert_eq!(qt, target);
            }
            _ => panic!("unexpected query variant"),
        }

        let samples = vec![0x77u8; 40];
        let nodes6 = vec![0x88u8; 38];
        let msg = KrpcMessage::Response(KrpcResponse::SampleInfohashes {
            id,
            interval: 60,
            num: 123,
            nodes: None,
            nodes6: Some(nodes6.clone()),
            samples: samples.clone(),
        });
        let bytes = msg.encode(b"sr").expect("encode response");
        let (_, decoded) = KrpcMessage::decode(&bytes).expect("decode response");
        match decoded {
            KrpcMessage::Response(KrpcResponse::SampleInfohashes {
                id: rid,
                interval,
                num,
                nodes,
                nodes6: rn6,
                samples: rs,
            }) => {
                assert_eq!(rid, id);
                assert_eq!(interval, 60);
                assert_eq!(num, 123);
                assert!(nodes.is_none());
                assert_eq!(rn6, Some(nodes6));
                assert_eq!(rs, samples);
            }
            _ => panic!("unexpected response variant"),
        }
    }
}
