use lt_common::object::{encode, Object};
use lt_net::addr::SocketAddress;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExtensionType {
    Handshake = 0,
    UtPex = 1,
    UtMetadata = 2,
    Invalid = 3,
}

pub struct DownloadInfo {
    pub pex_count: Arc<AtomicU32>,
}

impl Default for DownloadInfo {
    fn default() -> Self {
        Self::new()
    }
}

impl DownloadInfo {
    pub fn new() -> Self {
        Self {
            pex_count: Arc::new(AtomicU32::new(0)),
        }
    }

    pub fn size_pex(&self) -> u32 {
        self.pex_count.load(Ordering::SeqCst)
    }
}

#[derive(Default)]
pub struct DownloadMain {
    pub info: DownloadInfo,
}

impl DownloadMain {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn info(&self) -> &DownloadInfo {
        &self.info
    }
}

pub struct ProtocolExtension {
    pex_counter: Option<Arc<AtomicU32>>,
    pex_enabled: bool,
    pex_released: bool,
    metadata_enabled: bool,
    remote_pex_id: u8,
    remote_metadata_id: u8,
}

impl Default for ProtocolExtension {
    fn default() -> Self {
        Self::new()
    }
}

impl ProtocolExtension {
    pub const HANDSHAKE: ExtensionType = ExtensionType::Handshake;
    pub const UT_PEX: ExtensionType = ExtensionType::UtPex;
    pub const UT_METADATA: ExtensionType = ExtensionType::UtMetadata;

    pub const METADATA_PIECE_SIZE: usize = 16384; // 16 KB

    pub fn new() -> Self {
        Self {
            pex_counter: None,
            pex_enabled: false,
            pex_released: false,
            metadata_enabled: false,
            remote_pex_id: 0,
            remote_metadata_id: 0,
        }
    }

    pub fn set_info(&mut self, _peer_info: Option<()>, download: &DownloadMain) {
        self.pex_counter = Some(download.info.pex_count.clone());
    }

    pub fn set_local_enabled(&mut self, ext: ExtensionType) {
        match ext {
            ExtensionType::UtPex => {
                if !self.pex_enabled {
                    self.pex_enabled = true;
                    self.pex_released = false;
                    if let Some(ref counter) = self.pex_counter {
                        counter.fetch_add(1, Ordering::SeqCst);
                    }
                }
            }
            ExtensionType::UtMetadata => {
                self.metadata_enabled = true;
            }
            _ => {}
        }
    }

    pub fn unset_local_enabled(&mut self, ext: ExtensionType) {
        match ext {
            ExtensionType::UtPex => {
                if self.pex_enabled && !self.pex_released {
                    self.pex_released = true;
                    if let Some(ref counter) = self.pex_counter {
                        counter.fetch_sub(1, Ordering::SeqCst);
                    }
                }
                self.pex_enabled = false;
            }
            ExtensionType::UtMetadata => {
                self.metadata_enabled = false;
            }
            _ => {}
        }
    }

    pub fn cleanup(&mut self) {
        if self.pex_enabled && !self.pex_released {
            self.pex_released = true;
            if let Some(ref counter) = self.pex_counter {
                counter.fetch_sub(1, Ordering::SeqCst);
            }
        }
    }

    pub fn is_local_enabled(&self, ext: ExtensionType) -> bool {
        match ext {
            ExtensionType::UtPex => self.pex_enabled,
            ExtensionType::UtMetadata => self.metadata_enabled,
            _ => false,
        }
    }

    pub fn set_remote_id(&mut self, ext: ExtensionType, id: u8) {
        match ext {
            ExtensionType::UtPex => self.remote_pex_id = id,
            ExtensionType::UtMetadata => self.remote_metadata_id = id,
            _ => {}
        }
    }

    pub fn remote_id(&self, ext: ExtensionType) -> u8 {
        match ext {
            ExtensionType::UtPex => self.remote_pex_id,
            ExtensionType::UtMetadata => self.remote_metadata_id,
            _ => 0,
        }
    }

    pub fn generate_handshake_message(
        pex_id: u8,
        metadata_id: u8,
        metadata_size: Option<u32>,
    ) -> Vec<u8> {
        let mut root = Object::create_map();
        let mut m = Object::create_map();
        if pex_id > 0 {
            m.insert_key("ut_pex", Object::create_value(pex_id as i64))
                .unwrap();
        }
        if metadata_id > 0 {
            m.insert_key("ut_metadata", Object::create_value(metadata_id as i64))
                .unwrap();
        }
        root.insert_key("m", m).unwrap();
        root.insert_key("reqq", Object::create_value(500)).unwrap();
        root.insert_key("v", Object::create_string(b"rstorrent 0.10.0".to_vec()))
            .unwrap();
        if let Some(size) = metadata_size {
            root.insert_key("metadata_size", Object::create_value(size as i64))
                .unwrap();
        }

        let payload = encode(&root).unwrap();

        // LTEP message: ID 20 (extended), extended message ID 0 (handshake), bencoded payload
        let mut msg = Vec::with_capacity(6 + payload.len());
        let length = (2 + payload.len()) as u32;
        msg.extend_from_slice(&length.to_be_bytes());
        msg.push(20); // BT extended message ID
        msg.push(0); // Handshake extended ID
        msg.extend_from_slice(&payload);
        msg
    }

    pub fn generate_ut_pex_message(
        ext_id: u8,
        added: &[SocketAddress],
        dropped: &[SocketAddress],
    ) -> Vec<u8> {
        // Route through the shared encoder so IPv6 peers are emitted in
        // `added6`/`dropped6` instead of being silently dropped.
        let payload = ut_pex_payload(added, dropped);

        let mut msg = Vec::with_capacity(6 + payload.len());
        let length = (2 + payload.len()) as u32;
        msg.extend_from_slice(&length.to_be_bytes());
        msg.push(20);
        msg.push(ext_id);
        msg.extend_from_slice(&payload);
        msg
    }

    pub fn generate_ut_metadata_request(ext_id: u8, piece: u32) -> Vec<u8> {
        let mut root = Object::create_map();
        root.insert_key("msg_type", Object::create_value(0))
            .unwrap();
        root.insert_key("piece", Object::create_value(piece as i64))
            .unwrap();

        let payload = encode(&root).unwrap();

        let mut msg = Vec::with_capacity(6 + payload.len());
        let length = (2 + payload.len()) as u32;
        msg.extend_from_slice(&length.to_be_bytes());
        msg.push(20);
        msg.push(ext_id);
        msg.extend_from_slice(&payload);
        msg
    }

    pub fn generate_ut_metadata_data(
        ext_id: u8,
        piece: u32,
        total_size: u32,
        data: &[u8],
    ) -> Vec<u8> {
        let mut root = Object::create_map();
        root.insert_key("msg_type", Object::create_value(1))
            .unwrap();
        root.insert_key("piece", Object::create_value(piece as i64))
            .unwrap();
        root.insert_key("total_size", Object::create_value(total_size as i64))
            .unwrap();

        let payload = encode(&root).unwrap();

        let mut msg = Vec::with_capacity(6 + payload.len() + data.len());
        let length = (2 + payload.len() + data.len()) as u32;
        msg.extend_from_slice(&length.to_be_bytes());
        msg.push(20);
        msg.push(ext_id);
        msg.extend_from_slice(&payload);
        msg.extend_from_slice(data);
        msg
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ExtensionError {
    #[error("Invalid ut_metadata payload: {0}")]
    InvalidPayload(String),
    #[error("Bencode error: {0}")]
    Bencode(#[from] lt_common::object::BencodeError),
    #[error("Unsupported ut_metadata message type: {0}")]
    UnsupportedMessage(i64),
}

/// Parsed `ut_metadata` extension message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UtMetadataMessage {
    Request {
        piece: u32,
    },
    Data {
        piece: u32,
        total_size: u32,
        data: Vec<u8>,
    },
    Reject {
        piece: u32,
    },
}

impl UtMetadataMessage {
    pub fn parse(payload: &[u8]) -> Result<Self, ExtensionError> {
        let (object, consumed) = lt_common::object::bencode::read_bencode(payload)?;

        let msg_type = object
            .get_key_value("msg_type")
            .map_err(|_| ExtensionError::InvalidPayload("missing msg_type".into()))?;
        let piece = object
            .get_key_value("piece")
            .map_err(|_| ExtensionError::InvalidPayload("missing piece".into()))?
            as u32;

        match msg_type {
            0 => Ok(UtMetadataMessage::Request { piece }),
            1 => {
                let total_size = object
                    .get_key_value("total_size")
                    .map_err(|_| ExtensionError::InvalidPayload("missing total_size".into()))?
                    as u32;

                if consumed > payload.len() {
                    return Err(ExtensionError::InvalidPayload(
                        "truncated metadata payload".into(),
                    ));
                }

                Ok(UtMetadataMessage::Data {
                    piece,
                    total_size,
                    data: payload[consumed..].to_vec(),
                })
            }
            2 => Ok(UtMetadataMessage::Reject { piece }),
            other => Err(ExtensionError::UnsupportedMessage(other)),
        }
    }
}

/// Bencoded payload for a `ut_metadata` request (without the framing bytes).
pub fn ut_metadata_request_payload(piece: u32) -> Vec<u8> {
    let mut root = Object::create_map();
    root.insert_key("msg_type", Object::create_value(0))
        .unwrap();
    root.insert_key("piece", Object::create_value(piece as i64))
        .unwrap();
    encode(&root).unwrap_or_default()
}

/// Bencoded payload for a `ut_metadata` data message (without the framing bytes).
pub fn ut_metadata_data_payload(piece: u32, total_size: u32, data: &[u8]) -> Vec<u8> {
    let mut root = Object::create_map();
    root.insert_key("msg_type", Object::create_value(1))
        .unwrap();
    root.insert_key("piece", Object::create_value(piece as i64))
        .unwrap();
    root.insert_key("total_size", Object::create_value(total_size as i64))
        .unwrap();

    let mut payload = encode(&root).unwrap_or_default();
    payload.extend_from_slice(data);
    payload
}

/// Bencoded payload for a `ut_metadata` reject message.
pub fn ut_metadata_reject_payload(piece: u32) -> Vec<u8> {
    let mut root = Object::create_map();
    root.insert_key("msg_type", Object::create_value(2))
        .unwrap();
    root.insert_key("piece", Object::create_value(piece as i64))
        .unwrap();
    encode(&root).unwrap_or_default()
}

/// Bencoded payload for the LTEP handshake (without the framing bytes).
pub fn extension_handshake_payload(
    pex_id: u8,
    metadata_id: u8,
    metadata_size: Option<u32>,
) -> Vec<u8> {
    let mut root = Object::create_map();
    let mut m = Object::create_map();
    if pex_id > 0 {
        m.insert_key("ut_pex", Object::create_value(pex_id as i64))
            .unwrap();
    }
    if metadata_id > 0 {
        m.insert_key("ut_metadata", Object::create_value(metadata_id as i64))
            .unwrap();
    }
    root.insert_key("m", m).unwrap();
    root.insert_key("reqq", Object::create_value(500)).unwrap();
    root.insert_key("v", Object::create_string(b"rstorrent 0.10.0".to_vec()))
        .unwrap();
    if let Some(size) = metadata_size {
        root.insert_key("metadata_size", Object::create_value(size as i64))
            .unwrap();
    }
    encode(&root).unwrap_or_default()
}

/// Parsed `ut_pex` message.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UtPexMessage {
    pub added: Vec<SocketAddress>,
    pub added6: Vec<SocketAddress>,
    pub dropped: Vec<SocketAddress>,
    pub dropped6: Vec<SocketAddress>,
}

fn decode_compact_v4(bytes: &[u8]) -> Vec<SocketAddress> {
    // A compact list that is not a whole number of entries is malformed.
    // Ignore the whole field rather than accepting a truncated prefix.
    if bytes.len() % 6 != 0 {
        return Vec::new();
    }
    bytes
        .chunks_exact(6)
        .filter_map(|chunk| {
            let port = u16::from_be_bytes([chunk[4], chunk[5]]);
            if port == 0 {
                return None;
            }
            Some(SocketAddress::from(SocketAddr::V4(SocketAddrV4::new(
                Ipv4Addr::new(chunk[0], chunk[1], chunk[2], chunk[3]),
                port,
            ))))
        })
        .collect()
}

fn decode_compact_v6(bytes: &[u8]) -> Vec<SocketAddress> {
    if bytes.len() % 18 != 0 {
        return Vec::new();
    }
    bytes
        .chunks_exact(18)
        .filter_map(|chunk| {
            let mut ip = [0u8; 16];
            ip.copy_from_slice(&chunk[0..16]);
            let port = u16::from_be_bytes([chunk[16], chunk[17]]);
            if port == 0 {
                return None;
            }
            Some(SocketAddress::from(SocketAddr::V6(SocketAddrV6::new(
                Ipv6Addr::from(ip),
                port,
                0,
                0,
            ))))
        })
        .collect()
}

impl UtPexMessage {
    pub fn parse(payload: &[u8]) -> Result<Self, ExtensionError> {
        let (object, _) = lt_common::object::bencode::read_bencode(payload)?;

        let bytes_for = |key: &str| -> Vec<u8> {
            object
                .get_key(key)
                .ok()
                .and_then(|value| value.as_string().ok())
                .map(|bytes| bytes.to_vec())
                .unwrap_or_default()
        };

        Ok(Self {
            added: decode_compact_v4(&bytes_for("added")),
            added6: decode_compact_v6(&bytes_for("added6")),
            dropped: decode_compact_v4(&bytes_for("dropped")),
            dropped6: decode_compact_v6(&bytes_for("dropped6")),
        })
    }

    pub fn all_added(&self) -> impl Iterator<Item = &SocketAddress> {
        self.added.iter().chain(self.added6.iter())
    }
}

/// Builds a bencoded `ut_pex` payload (without the extended-message framing).
pub fn ut_pex_payload(added: &[SocketAddress], dropped: &[SocketAddress]) -> Vec<u8> {
    let mut added_v4 = Vec::new();
    let mut added_v6 = Vec::new();
    let mut dropped_v4 = Vec::new();
    let mut dropped_v6 = Vec::new();

    for address in added {
        match address {
            SocketAddress::Inet(v4) => {
                added_v4.extend_from_slice(&v4.ip().octets());
                added_v4.extend_from_slice(&v4.port().to_be_bytes());
            }
            SocketAddress::Inet6(v6) => {
                added_v6.extend_from_slice(&v6.ip().octets());
                added_v6.extend_from_slice(&v6.port().to_be_bytes());
            }
            _ => {}
        }
    }

    for address in dropped {
        match address {
            SocketAddress::Inet(v4) => {
                dropped_v4.extend_from_slice(&v4.ip().octets());
                dropped_v4.extend_from_slice(&v4.port().to_be_bytes());
            }
            SocketAddress::Inet6(v6) => {
                dropped_v6.extend_from_slice(&v6.ip().octets());
                dropped_v6.extend_from_slice(&v6.port().to_be_bytes());
            }
            _ => {}
        }
    }

    let mut root = Object::create_map();
    root.insert_key("added", Object::create_string(added_v4))
        .unwrap();
    root.insert_key("added6", Object::create_string(added_v6))
        .unwrap();
    root.insert_key("dropped", Object::create_string(dropped_v4))
        .unwrap();
    root.insert_key("dropped6", Object::create_string(dropped_v6))
        .unwrap();

    encode(&root).unwrap_or_default()
}

impl Drop for ProtocolExtension {
    fn drop(&mut self) {
        self.cleanup();
    }
}
