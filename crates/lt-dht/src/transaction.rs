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
    },
    GetPeers {
        id: HashString,
        token: Vec<u8>,
        values: Vec<[u8; 6]>,
        nodes: Option<Vec<u8>>,
    },
    AnnouncePeer {
        id: HashString,
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
                    KrpcResponse::FindNode { id, nodes } => {
                        resp.insert_key("id", Object::create_string(id.as_slice().to_vec()))
                            .unwrap();
                        resp.insert_key("nodes", Object::create_string(nodes.clone()))
                            .unwrap();
                    }
                    KrpcResponse::GetPeers {
                        id,
                        token,
                        values,
                        nodes,
                    } => {
                        resp.insert_key("id", Object::create_string(id.as_slice().to_vec()))
                            .unwrap();
                        resp.insert_key("token", Object::create_string(token.clone()))
                            .unwrap();
                        if !values.is_empty() {
                            let mut val_list = Object::create_list();
                            for val in values {
                                val_list
                                    .insert_back(Object::create_string(val.to_vec()))
                                    .unwrap();
                            }
                            resp.insert_key("values", val_list).unwrap();
                        }
                        if let Some(ref n) = nodes {
                            resp.insert_key("nodes", Object::create_string(n.clone()))
                                .unwrap();
                        }
                    }
                    KrpcResponse::AnnouncePeer { id } => {
                        resp.insert_key("id", Object::create_string(id.as_slice().to_vec()))
                            .unwrap();
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
                        KrpcQuery::AnnouncePeer {
                            id,
                            info_hash,
                            port,
                            token,
                        }
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
                                        let mut compact = [0u8; 6];
                                        compact.copy_from_slice(s);
                                        values.push(compact);
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
                    Ok((
                        tid,
                        KrpcMessage::Response(KrpcResponse::GetPeers {
                            id,
                            token,
                            values,
                            nodes,
                        }),
                    ))
                } else if let Ok(nodes_obj) = resp.get_key("nodes") {
                    let nodes = nodes_obj
                        .as_string()
                        .map_err(|_| DhtError::Input("'nodes' must be string".into()))?
                        .to_vec();
                    Ok((
                        tid,
                        KrpcMessage::Response(KrpcResponse::FindNode { id, nodes }),
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
