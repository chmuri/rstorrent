//! BEP 29: Micro Transport Protocol (uTP) packet format and serialization.

use bytes::{BufMut, Bytes, BytesMut};
use std::fmt;

pub const UTP_HEADER_LEN: usize = 20;
pub const UTP_VERSION: u8 = 1;

/// Type of uTP packet (4-bit field).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum PacketType {
    /// Regular payload data.
    Data = 0,
    /// Connection termination.
    Fin = 1,
    /// State / acknowledgment only (no payload).
    State = 2,
    /// Abrupt connection reset.
    Reset = 3,
    /// Connection initiation.
    Syn = 4,
}

impl PacketType {
    pub fn from_u8(val: u8) -> Option<Self> {
        match val {
            0 => Some(PacketType::Data),
            1 => Some(PacketType::Fin),
            2 => Some(PacketType::State),
            3 => Some(PacketType::Reset),
            4 => Some(PacketType::Syn),
            _ => None,
        }
    }

    pub fn to_u8(self) -> u8 {
        self as u8
    }
}

impl fmt::Display for PacketType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PacketType::Data => write!(f, "ST_DATA"),
            PacketType::Fin => write!(f, "ST_FIN"),
            PacketType::State => write!(f, "ST_STATE"),
            PacketType::Reset => write!(f, "ST_RESET"),
            PacketType::Syn => write!(f, "ST_SYN"),
        }
    }
}

/// uTP Extension headers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UtpExtension {
    /// Selective ACK (type 1). Contains a bitmask of acknowledged packets starting at ack_nr + 2.
    SelectiveAck(Vec<u8>),
    /// Unknown or unhandled extension.
    Unknown(u8, Vec<u8>),
}

/// Base 20-byte uTP header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UtpHeader {
    pub packet_type: PacketType,
    pub version: u8,
    pub extension: u8,
    pub connection_id: u16,
    pub timestamp_micros: u32,
    pub timestamp_diff_micros: u32,
    pub wnd_size: u32,
    pub seq_nr: u16,
    pub ack_nr: u16,
}

/// Complete uTP packet including header, extension chain, and payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UtpPacket {
    pub header: UtpHeader,
    pub extensions: Vec<UtpExtension>,
    pub payload: Bytes,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum PacketError {
    #[error("packet too short: expected at least {0} bytes, got {1}")]
    PacketTooShort(usize, usize),
    #[error("invalid uTP version: expected 1, got {0}")]
    InvalidVersion(u8),
    #[error("unknown packet type: {0}")]
    InvalidPacketType(u8),
    #[error("truncated extension header")]
    TruncatedExtension,
}

impl UtpPacket {
    pub fn new(header: UtpHeader, payload: Bytes) -> Self {
        Self {
            header,
            extensions: Vec::new(),
            payload,
        }
    }

    /// Encode packet into bytes buffer.
    pub fn encode(&self) -> Bytes {
        let mut buf = BytesMut::with_capacity(UTP_HEADER_LEN + self.payload.len() + 32);

        // Header byte 0: [type: 4 bits][version: 4 bits]
        let type_ver = (self.header.packet_type.to_u8() << 4) | (self.header.version & 0x0F);
        buf.put_u8(type_ver);

        // First extension ID (0 if none)
        let first_ext_id = if let Some(first) = self.extensions.first() {
            match first {
                UtpExtension::SelectiveAck(_) => 1,
                UtpExtension::Unknown(id, _) => *id,
            }
        } else {
            0
        };
        buf.put_u8(first_ext_id);

        buf.put_u16(self.header.connection_id);
        buf.put_u32(self.header.timestamp_micros);
        buf.put_u32(self.header.timestamp_diff_micros);
        buf.put_u32(self.header.wnd_size);
        buf.put_u16(self.header.seq_nr);
        buf.put_u16(self.header.ack_nr);

        // Serialize extension chain
        for (i, ext) in self.extensions.iter().enumerate() {
            let next_id = if i + 1 < self.extensions.len() {
                match &self.extensions[i + 1] {
                    UtpExtension::SelectiveAck(_) => 1,
                    UtpExtension::Unknown(id, _) => *id,
                }
            } else {
                0
            };

            buf.put_u8(next_id);
            match ext {
                UtpExtension::SelectiveAck(data) => {
                    buf.put_u8(data.len() as u8);
                    buf.put_slice(data);
                }
                UtpExtension::Unknown(_, data) => {
                    buf.put_u8(data.len() as u8);
                    buf.put_slice(data);
                }
            }
        }

        // Payload
        if !self.payload.is_empty() {
            buf.put_slice(&self.payload);
        }

        buf.freeze()
    }

    /// Decode packet from raw byte slice.
    pub fn decode(data: &[u8]) -> Result<Self, PacketError> {
        if data.len() < UTP_HEADER_LEN {
            return Err(PacketError::PacketTooShort(UTP_HEADER_LEN, data.len()));
        }

        let type_ver = data[0];
        let p_type = PacketType::from_u8(type_ver >> 4)
            .ok_or(PacketError::InvalidPacketType(type_ver >> 4))?;
        let version = type_ver & 0x0F;
        if version != UTP_VERSION {
            return Err(PacketError::InvalidVersion(version));
        }

        let first_ext = data[1];
        let connection_id = u16::from_be_bytes([data[2], data[3]]);
        let timestamp_micros = u32::from_be_bytes([data[4], data[5], data[6], data[7]]);
        let timestamp_diff_micros = u32::from_be_bytes([data[8], data[9], data[10], data[11]]);
        let wnd_size = u32::from_be_bytes([data[12], data[13], data[14], data[15]]);
        let seq_nr = u16::from_be_bytes([data[16], data[17]]);
        let ack_nr = u16::from_be_bytes([data[18], data[19]]);

        let header = UtpHeader {
            packet_type: p_type,
            version,
            extension: first_ext,
            connection_id,
            timestamp_micros,
            timestamp_diff_micros,
            wnd_size,
            seq_nr,
            ack_nr,
        };

        let mut offset = UTP_HEADER_LEN;
        let mut cur_ext = first_ext;
        let mut extensions = Vec::new();

        while cur_ext != 0 {
            if offset + 2 > data.len() {
                return Err(PacketError::TruncatedExtension);
            }
            let next_ext = data[offset];
            let ext_len = data[offset + 1] as usize;
            offset += 2;

            if offset + ext_len > data.len() {
                return Err(PacketError::TruncatedExtension);
            }

            let ext_data = data[offset..offset + ext_len].to_vec();
            offset += ext_len;

            match cur_ext {
                1 => extensions.push(UtpExtension::SelectiveAck(ext_data)),
                unknown => extensions.push(UtpExtension::Unknown(unknown, ext_data)),
            }
            cur_ext = next_ext;
        }

        let payload = Bytes::copy_from_slice(&data[offset..]);

        Ok(UtpPacket {
            header,
            extensions,
            payload,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encode_decode_syn() {
        let packet = UtpPacket::new(
            UtpHeader {
                packet_type: PacketType::Syn,
                version: 1,
                extension: 0,
                connection_id: 0x1234,
                timestamp_micros: 1_000_000,
                timestamp_diff_micros: 0,
                wnd_size: 1024 * 1024,
                seq_nr: 1,
                ack_nr: 0,
            },
            Bytes::new(),
        );

        let encoded = packet.encode();
        assert_eq!(encoded.len(), 20);

        let decoded = UtpPacket::decode(&encoded).unwrap();
        assert_eq!(decoded, packet);
    }

    #[test]
    fn test_encode_decode_data_with_sack() {
        let mut packet = UtpPacket::new(
            UtpHeader {
                packet_type: PacketType::Data,
                version: 1,
                extension: 1,
                connection_id: 0xABCD,
                timestamp_micros: 5_555_555,
                timestamp_diff_micros: 42_000,
                wnd_size: 65536,
                seq_nr: 10,
                ack_nr: 9,
            },
            Bytes::from_static(b"hello utp payload world!"),
        );
        packet.extensions.push(UtpExtension::SelectiveAck(vec![
            0b10101010, 0b01010101, 0, 0,
        ]));

        let encoded = packet.encode();
        let decoded = UtpPacket::decode(&encoded).unwrap();
        assert_eq!(decoded.header.packet_type, PacketType::Data);
        assert_eq!(
            decoded.payload,
            Bytes::from_static(b"hello utp payload world!")
        );
        assert_eq!(decoded.extensions.len(), 1);
        match &decoded.extensions[0] {
            UtpExtension::SelectiveAck(bytes) => {
                assert_eq!(bytes, &vec![0b10101010, 0b01010101, 0, 0])
            }
            _ => panic!("Expected SACK extension"),
        }
    }
}
