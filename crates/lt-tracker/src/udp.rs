use bytes::{Buf, BufMut, BytesMut};
use lt_net::addr::{AddressList, SocketAddress};
use thiserror::Error;

pub const MAGIC_CONNECTION_ID: u64 = 0x0000_0417_2710_1980;

pub const ACTION_CONNECT: u32 = 0;
pub const ACTION_ANNOUNCE: u32 = 1;
pub const ACTION_SCRAPE: u32 = 2;
pub const ACTION_ERROR: u32 = 3;

#[derive(Debug, Error)]
pub enum UdpTrackerError {
    #[error("Packet too short: required {expected}, got {actual}")]
    PacketTooShort { expected: usize, actual: usize },
    #[error("Unknown or invalid action: {0}")]
    InvalidAction(u32),
    #[error("Invalid magic connection ID: expected {MAGIC_CONNECTION_ID:#x}, got {0:#x}")]
    InvalidMagicConnectionId(u64),
    #[error("Invalid info_hash size")]
    InvalidHashSize,
    #[error("Invalid UTF-8 error message: {0}")]
    Utf8Error(#[from] std::string::FromUtf8Error),
    #[error("Tracker returned an error: {0}")]
    TrackerError(String),
    #[error("Malformed tracker response: {0}")]
    MalformedResponse(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ConnectRequest {
    pub connection_id: u64,
    pub transaction_id: u32,
}

impl ConnectRequest {
    pub fn new(transaction_id: u32) -> Self {
        Self {
            connection_id: MAGIC_CONNECTION_ID,
            transaction_id,
        }
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut buf = BytesMut::with_capacity(16);
        buf.put_u64(self.connection_id);
        buf.put_u32(ACTION_CONNECT);
        buf.put_u32(self.transaction_id);
        buf.to_vec()
    }

    pub fn decode(mut data: &[u8]) -> Result<Self, UdpTrackerError> {
        if data.len() < 16 {
            return Err(UdpTrackerError::PacketTooShort {
                expected: 16,
                actual: data.len(),
            });
        }
        let connection_id = data.get_u64();
        let action = data.get_u32();
        if action != ACTION_CONNECT {
            return Err(UdpTrackerError::InvalidAction(action));
        }
        let transaction_id = data.get_u32();
        Ok(Self {
            connection_id,
            transaction_id,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ConnectResponse {
    pub transaction_id: u32,
    pub connection_id: u64,
}

impl ConnectResponse {
    pub fn new(transaction_id: u32, connection_id: u64) -> Self {
        Self {
            transaction_id,
            connection_id,
        }
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut buf = BytesMut::with_capacity(16);
        buf.put_u32(ACTION_CONNECT);
        buf.put_u32(self.transaction_id);
        buf.put_u64(self.connection_id);
        buf.to_vec()
    }

    pub fn decode(mut data: &[u8]) -> Result<Self, UdpTrackerError> {
        if data.len() < 16 {
            return Err(UdpTrackerError::PacketTooShort {
                expected: 16,
                actual: data.len(),
            });
        }
        let action = data.get_u32();
        if action != ACTION_CONNECT {
            return Err(UdpTrackerError::InvalidAction(action));
        }
        let transaction_id = data.get_u32();
        let connection_id = data.get_u64();
        Ok(Self {
            transaction_id,
            connection_id,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AnnounceRequest {
    pub connection_id: u64,
    pub transaction_id: u32,
    pub info_hash: [u8; 20],
    pub peer_id: [u8; 20],
    pub downloaded: u64,
    pub left: u64,
    pub uploaded: u64,
    pub event: u32,
    pub ip_address: u32,
    pub key: u32,
    pub num_want: i32,
    pub port: u16,
}

impl AnnounceRequest {
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = BytesMut::with_capacity(98);
        buf.put_u64(self.connection_id);
        buf.put_u32(ACTION_ANNOUNCE);
        buf.put_u32(self.transaction_id);
        buf.put_slice(&self.info_hash);
        buf.put_slice(&self.peer_id);
        buf.put_u64(self.downloaded);
        buf.put_u64(self.left);
        buf.put_u64(self.uploaded);
        buf.put_u32(self.event);
        buf.put_u32(self.ip_address);
        buf.put_u32(self.key);
        buf.put_i32(self.num_want);
        buf.put_u16(self.port);
        buf.to_vec()
    }

    pub fn decode(mut data: &[u8]) -> Result<Self, UdpTrackerError> {
        if data.len() < 98 {
            return Err(UdpTrackerError::PacketTooShort {
                expected: 98,
                actual: data.len(),
            });
        }
        let connection_id = data.get_u64();
        let action = data.get_u32();
        if action != ACTION_ANNOUNCE {
            return Err(UdpTrackerError::InvalidAction(action));
        }
        let transaction_id = data.get_u32();
        let mut info_hash = [0u8; 20];
        data.copy_to_slice(&mut info_hash);
        let mut peer_id = [0u8; 20];
        data.copy_to_slice(&mut peer_id);
        let downloaded = data.get_u64();
        let left = data.get_u64();
        let uploaded = data.get_u64();
        let event = data.get_u32();
        let ip_address = data.get_u32();
        let key = data.get_u32();
        let num_want = data.get_i32();
        let port = data.get_u16();

        Ok(Self {
            connection_id,
            transaction_id,
            info_hash,
            peer_id,
            downloaded,
            left,
            uploaded,
            event,
            ip_address,
            key,
            num_want,
            port,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AnnounceResponse {
    pub transaction_id: u32,
    pub interval: u32,
    pub leechers: u32,
    pub seeders: u32,
    pub peers: AddressList,
}

impl AnnounceResponse {
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = BytesMut::with_capacity(20 + self.peers.len() * 6);
        buf.put_u32(ACTION_ANNOUNCE);
        buf.put_u32(self.transaction_id);
        buf.put_u32(self.interval);
        buf.put_u32(self.leechers);
        buf.put_u32(self.seeders);
        for sa in &self.peers.0 {
            if let SocketAddress::Inet(v4) = sa {
                buf.put_slice(&v4.ip().octets());
                buf.put_u16(v4.port());
            } else if let SocketAddress::Inet6(v6) = sa {
                buf.put_slice(&v6.ip().octets());
                buf.put_u16(v6.port());
            }
        }
        buf.to_vec()
    }

    pub fn decode(mut data: &[u8]) -> Result<Self, UdpTrackerError> {
        if data.len() < 20 {
            return Err(UdpTrackerError::PacketTooShort {
                expected: 20,
                actual: data.len(),
            });
        }
        let action = data.get_u32();
        if action != ACTION_ANNOUNCE {
            return Err(UdpTrackerError::InvalidAction(action));
        }
        let transaction_id = data.get_u32();
        let interval = data.get_u32();
        let leechers = data.get_u32();
        let seeders = data.get_u32();

        let mut peers = AddressList::new();
        let remaining = &data[..];
        // If remaining is multiple of 6, IPv4 compact
        if remaining.len() % 6 == 0 {
            peers.parse_address_compact(remaining);
        } else if remaining.len() % 18 == 0 {
            peers.parse_address_compact_ipv6(remaining);
        }

        Ok(Self {
            transaction_id,
            interval,
            leechers,
            seeders,
            peers,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScrapeRequest {
    pub connection_id: u64,
    pub transaction_id: u32,
    pub info_hashes: Vec<[u8; 20]>,
}

impl ScrapeRequest {
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = BytesMut::with_capacity(16 + self.info_hashes.len() * 20);
        buf.put_u64(self.connection_id);
        buf.put_u32(ACTION_SCRAPE);
        buf.put_u32(self.transaction_id);
        for hash in &self.info_hashes {
            buf.put_slice(hash);
        }
        buf.to_vec()
    }

    pub fn decode(mut data: &[u8]) -> Result<Self, UdpTrackerError> {
        if data.len() < 16 {
            return Err(UdpTrackerError::PacketTooShort {
                expected: 16,
                actual: data.len(),
            });
        }
        let connection_id = data.get_u64();
        let action = data.get_u32();
        if action != ACTION_SCRAPE {
            return Err(UdpTrackerError::InvalidAction(action));
        }
        let transaction_id = data.get_u32();

        let mut info_hashes = Vec::new();
        while data.remaining() >= 20 {
            let mut h = [0u8; 20];
            data.copy_to_slice(&mut h);
            info_hashes.push(h);
        }

        Ok(Self {
            connection_id,
            transaction_id,
            info_hashes,
        })
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ScrapeFileStats {
    pub seeders: u32,
    pub completed: u32,
    pub leechers: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScrapeResponse {
    pub transaction_id: u32,
    pub stats: Vec<ScrapeFileStats>,
}

impl ScrapeResponse {
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = BytesMut::with_capacity(8 + self.stats.len() * 12);
        buf.put_u32(ACTION_SCRAPE);
        buf.put_u32(self.transaction_id);
        for stat in &self.stats {
            buf.put_u32(stat.seeders);
            buf.put_u32(stat.completed);
            buf.put_u32(stat.leechers);
        }
        buf.to_vec()
    }

    pub fn decode(mut data: &[u8]) -> Result<Self, UdpTrackerError> {
        if data.len() < 8 {
            return Err(UdpTrackerError::PacketTooShort {
                expected: 8,
                actual: data.len(),
            });
        }
        let action = data.get_u32();
        if action != ACTION_SCRAPE {
            return Err(UdpTrackerError::InvalidAction(action));
        }
        let transaction_id = data.get_u32();

        let mut stats = Vec::new();
        while data.remaining() >= 12 {
            let seeders = data.get_u32();
            let completed = data.get_u32();
            let leechers = data.get_u32();
            stats.push(ScrapeFileStats {
                seeders,
                completed,
                leechers,
            });
        }

        Ok(Self {
            transaction_id,
            stats,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ErrorResponse {
    pub transaction_id: u32,
    pub message: String,
}

impl ErrorResponse {
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = BytesMut::with_capacity(8 + self.message.len());
        buf.put_u32(ACTION_ERROR);
        buf.put_u32(self.transaction_id);
        buf.put_slice(self.message.as_bytes());
        buf.to_vec()
    }

    pub fn decode(mut data: &[u8]) -> Result<Self, UdpTrackerError> {
        if data.len() < 8 {
            return Err(UdpTrackerError::PacketTooShort {
                expected: 8,
                actual: data.len(),
            });
        }
        let action = data.get_u32();
        if action != ACTION_ERROR {
            return Err(UdpTrackerError::InvalidAction(action));
        }
        let transaction_id = data.get_u32();
        let message = String::from_utf8(data.to_vec())?;
        Ok(Self {
            transaction_id,
            message,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    #[test]
    fn test_connect_roundtrip() {
        let req = ConnectRequest::new(0x12345678);
        let encoded = req.encode();
        assert_eq!(encoded.len(), 16);
        let decoded = ConnectRequest::decode(&encoded).unwrap();
        assert_eq!(req, decoded);

        let resp = ConnectResponse::new(0x12345678, 0xfedcba9876543210);
        let encoded = resp.encode();
        assert_eq!(encoded.len(), 16);
        let decoded = ConnectResponse::decode(&encoded).unwrap();
        assert_eq!(resp, decoded);
    }

    #[test]
    fn test_announce_roundtrip() {
        let req = AnnounceRequest {
            connection_id: 0x9988776655443322,
            transaction_id: 0x11223344,
            info_hash: [0xaa; 20],
            peer_id: [0xbb; 20],
            downloaded: 1000,
            left: 2000,
            uploaded: 3000,
            event: 2,
            ip_address: 0,
            key: 42,
            num_want: -1,
            port: 6881,
        };
        let encoded = req.encode();
        assert_eq!(encoded.len(), 98);
        let decoded = AnnounceRequest::decode(&encoded).unwrap();
        assert_eq!(req, decoded);

        let mut resp = AnnounceResponse {
            transaction_id: 0x11223344,
            interval: 1800,
            leechers: 5,
            seeders: 10,
            peers: AddressList::new(),
        };
        resp.peers
            .push(SocketAddress::new_inet(Ipv4Addr::new(127, 0, 0, 1), 6881));
        resp.peers.push(SocketAddress::new_inet(
            Ipv4Addr::new(192, 168, 1, 10),
            8080,
        ));

        let encoded = resp.encode();
        assert_eq!(encoded.len(), 20 + 12);
        let decoded = AnnounceResponse::decode(&encoded).unwrap();
        assert_eq!(resp.transaction_id, decoded.transaction_id);
        assert_eq!(resp.interval, decoded.interval);
        assert_eq!(resp.leechers, decoded.leechers);
        assert_eq!(resp.seeders, decoded.seeders);
        assert_eq!(resp.peers.len(), decoded.peers.len());
        assert_eq!(resp.peers[0], decoded.peers[0]);
        assert_eq!(resp.peers[1], decoded.peers[1]);
    }

    #[test]
    fn test_scrape_roundtrip() {
        let req = ScrapeRequest {
            connection_id: 0x1111222233334444,
            transaction_id: 0x55667788,
            info_hashes: vec![[0x11; 20], [0x22; 20]],
        };
        let encoded = req.encode();
        assert_eq!(encoded.len(), 16 + 40);
        let decoded = ScrapeRequest::decode(&encoded).unwrap();
        assert_eq!(req, decoded);

        let resp = ScrapeResponse {
            transaction_id: 0x55667788,
            stats: vec![
                ScrapeFileStats {
                    seeders: 10,
                    completed: 100,
                    leechers: 2,
                },
                ScrapeFileStats {
                    seeders: 20,
                    completed: 200,
                    leechers: 4,
                },
            ],
        };
        let encoded = resp.encode();
        assert_eq!(encoded.len(), 8 + 24);
        let decoded = ScrapeResponse::decode(&encoded).unwrap();
        assert_eq!(resp, decoded);
    }

    #[test]
    fn test_error_roundtrip() {
        let err = ErrorResponse {
            transaction_id: 0x12345678,
            message: "Connection limit reached".to_string(),
        };
        let encoded = err.encode();
        assert_eq!(encoded.len(), 8 + 24);
        let decoded = ErrorResponse::decode(&encoded).unwrap();
        assert_eq!(err, decoded);
    }
}
