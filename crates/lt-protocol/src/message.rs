use bytes::{Buf, BufMut, BytesMut};
use thiserror::Error;

pub const MSG_CHOKE: u8 = 0;
pub const MSG_UNCHOKE: u8 = 1;
pub const MSG_INTERESTED: u8 = 2;
pub const MSG_NOT_INTERESTED: u8 = 3;
pub const MSG_HAVE: u8 = 4;
pub const MSG_BITFIELD: u8 = 5;
pub const MSG_REQUEST: u8 = 6;
pub const MSG_PIECE: u8 = 7;
pub const MSG_CANCEL: u8 = 8;
pub const MSG_PORT: u8 = 9;
pub const MSG_SUGGEST_PIECE: u8 = 0x0D;
pub const MSG_HAVE_ALL: u8 = 0x0E;
pub const MSG_HAVE_NONE: u8 = 0x0F;
pub const MSG_REJECT_REQUEST: u8 = 0x10;
pub const MSG_ALLOWED_FAST: u8 = 0x11;
pub const MSG_EXTENDED: u8 = 20;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum MessageError {
    #[error("Message payload too short: expected {expected}, got {actual}")]
    PayloadTooShort { expected: usize, actual: usize },
    #[error("Unknown message ID: {0}")]
    UnknownId(u8),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PeerMessage {
    KeepAlive,
    Choke,
    Unchoke,
    Interested,
    NotInterested,
    Have {
        index: u32,
    },
    Bitfield(Vec<u8>),
    Request {
        index: u32,
        begin: u32,
        length: u32,
    },
    Piece {
        index: u32,
        begin: u32,
        data: Vec<u8>,
    },
    Cancel {
        index: u32,
        begin: u32,
        length: u32,
    },
    Port(u16),
    Extended {
        id: u8,
        payload: Vec<u8>,
    },
    SuggestPiece {
        index: u32,
    },
    HaveAll,
    HaveNone,
    RejectRequest {
        index: u32,
        begin: u32,
        length: u32,
    },
    AllowedFast {
        index: u32,
    },
    Batch(Vec<PeerMessage>),
}

impl PeerMessage {
    /// Encodes the message into an existing `BytesMut` buffer in wire format:
    /// `<4-byte length prefix><1-byte message ID><payload>`.
    /// Zero allocations when the buffer has sufficient capacity.
    pub fn encode_to(&self, buf: &mut BytesMut) {
        match self {
            PeerMessage::KeepAlive => {
                buf.put_u32(0);
            }
            PeerMessage::Choke => {
                buf.put_u32(1);
                buf.put_u8(MSG_CHOKE);
            }
            PeerMessage::Unchoke => {
                buf.put_u32(1);
                buf.put_u8(MSG_UNCHOKE);
            }
            PeerMessage::Interested => {
                buf.put_u32(1);
                buf.put_u8(MSG_INTERESTED);
            }
            PeerMessage::NotInterested => {
                buf.put_u32(1);
                buf.put_u8(MSG_NOT_INTERESTED);
            }
            PeerMessage::Have { index } => {
                buf.put_u32(5);
                buf.put_u8(MSG_HAVE);
                buf.put_u32(*index);
            }
            PeerMessage::Bitfield(data) => {
                buf.put_u32(1 + data.len() as u32);
                buf.put_u8(MSG_BITFIELD);
                buf.put_slice(data);
            }
            PeerMessage::Request {
                index,
                begin,
                length,
            } => {
                buf.put_u32(13);
                buf.put_u8(MSG_REQUEST);
                buf.put_u32(*index);
                buf.put_u32(*begin);
                buf.put_u32(*length);
            }
            PeerMessage::Piece { index, begin, data } => {
                buf.put_u32(9 + data.len() as u32);
                buf.put_u8(MSG_PIECE);
                buf.put_u32(*index);
                buf.put_u32(*begin);
                buf.put_slice(data);
            }
            PeerMessage::Cancel {
                index,
                begin,
                length,
            } => {
                buf.put_u32(13);
                buf.put_u8(MSG_CANCEL);
                buf.put_u32(*index);
                buf.put_u32(*begin);
                buf.put_u32(*length);
            }
            PeerMessage::Port(port) => {
                buf.put_u32(3);
                buf.put_u8(MSG_PORT);
                buf.put_u16(*port);
            }
            PeerMessage::Extended { id, payload } => {
                buf.put_u32(2 + payload.len() as u32);
                buf.put_u8(MSG_EXTENDED);
                buf.put_u8(*id);
                buf.put_slice(payload);
            }
            PeerMessage::SuggestPiece { index } => {
                buf.put_u32(5);
                buf.put_u8(MSG_SUGGEST_PIECE);
                buf.put_u32(*index);
            }
            PeerMessage::HaveAll => {
                buf.put_u32(1);
                buf.put_u8(MSG_HAVE_ALL);
            }
            PeerMessage::HaveNone => {
                buf.put_u32(1);
                buf.put_u8(MSG_HAVE_NONE);
            }
            PeerMessage::RejectRequest {
                index,
                begin,
                length,
            } => {
                buf.put_u32(13);
                buf.put_u8(MSG_REJECT_REQUEST);
                buf.put_u32(*index);
                buf.put_u32(*begin);
                buf.put_u32(*length);
            }
            PeerMessage::AllowedFast { index } => {
                buf.put_u32(5);
                buf.put_u8(MSG_ALLOWED_FAST);
                buf.put_u32(*index);
            }
            PeerMessage::Batch(msgs) => {
                Self::encode_batch(msgs, buf);
            }
        }
    }

    /// Encodes the message into full framed wire format:
    /// `<4-byte length prefix><1-byte message ID><payload>`
    /// For KeepAlive, simply emits 4 zero bytes.
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = BytesMut::with_capacity(32);
        self.encode_to(&mut buf);
        buf.to_vec()
    }

    /// Batches multiple messages into a single `BytesMut` buffer for coalesced flushing.
    pub fn encode_batch(messages: &[PeerMessage], buf: &mut BytesMut) {
        for msg in messages {
            msg.encode_to(buf);
        }
    }

    /// Decodes a message from `id` and `payload` (excluding 4-byte length and 1-byte id).
    pub fn decode(id: u8, mut payload: &[u8]) -> Result<Self, MessageError> {
        match id {
            MSG_CHOKE => Ok(PeerMessage::Choke),
            MSG_UNCHOKE => Ok(PeerMessage::Unchoke),
            MSG_INTERESTED => Ok(PeerMessage::Interested),
            MSG_NOT_INTERESTED => Ok(PeerMessage::NotInterested),
            MSG_HAVE => {
                if payload.len() < 4 {
                    return Err(MessageError::PayloadTooShort {
                        expected: 4,
                        actual: payload.len(),
                    });
                }
                let index = payload.get_u32();
                Ok(PeerMessage::Have { index })
            }
            MSG_BITFIELD => Ok(PeerMessage::Bitfield(payload.to_vec())),
            MSG_REQUEST => {
                if payload.len() < 12 {
                    return Err(MessageError::PayloadTooShort {
                        expected: 12,
                        actual: payload.len(),
                    });
                }
                let index = payload.get_u32();
                let begin = payload.get_u32();
                let length = payload.get_u32();
                Ok(PeerMessage::Request {
                    index,
                    begin,
                    length,
                })
            }
            MSG_PIECE => {
                if payload.len() < 8 {
                    return Err(MessageError::PayloadTooShort {
                        expected: 8,
                        actual: payload.len(),
                    });
                }
                let index = payload.get_u32();
                let begin = payload.get_u32();
                let data = payload.to_vec();
                Ok(PeerMessage::Piece { index, begin, data })
            }
            MSG_CANCEL => {
                if payload.len() < 12 {
                    return Err(MessageError::PayloadTooShort {
                        expected: 12,
                        actual: payload.len(),
                    });
                }
                let index = payload.get_u32();
                let begin = payload.get_u32();
                let length = payload.get_u32();
                Ok(PeerMessage::Cancel {
                    index,
                    begin,
                    length,
                })
            }
            MSG_PORT => {
                if payload.len() < 2 {
                    return Err(MessageError::PayloadTooShort {
                        expected: 2,
                        actual: payload.len(),
                    });
                }
                let port = payload.get_u16();
                Ok(PeerMessage::Port(port))
            }
            MSG_SUGGEST_PIECE => {
                if payload.len() < 4 {
                    return Err(MessageError::PayloadTooShort {
                        expected: 4,
                        actual: payload.len(),
                    });
                }
                Ok(PeerMessage::SuggestPiece {
                    index: payload.get_u32(),
                })
            }
            MSG_HAVE_ALL => Ok(PeerMessage::HaveAll),
            MSG_HAVE_NONE => Ok(PeerMessage::HaveNone),
            MSG_REJECT_REQUEST => {
                if payload.len() < 12 {
                    return Err(MessageError::PayloadTooShort {
                        expected: 12,
                        actual: payload.len(),
                    });
                }
                let index = payload.get_u32();
                let begin = payload.get_u32();
                let length = payload.get_u32();
                Ok(PeerMessage::RejectRequest {
                    index,
                    begin,
                    length,
                })
            }
            MSG_ALLOWED_FAST => {
                if payload.len() < 4 {
                    return Err(MessageError::PayloadTooShort {
                        expected: 4,
                        actual: payload.len(),
                    });
                }
                Ok(PeerMessage::AllowedFast {
                    index: payload.get_u32(),
                })
            }
            MSG_EXTENDED => {
                if payload.is_empty() {
                    return Err(MessageError::PayloadTooShort {
                        expected: 1,
                        actual: 0,
                    });
                }
                let id = payload.get_u8();
                let data = payload.to_vec();
                Ok(PeerMessage::Extended { id, payload: data })
            }
            other => Err(MessageError::UnknownId(other)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encode_decode_messages() {
        let msgs = vec![
            PeerMessage::KeepAlive,
            PeerMessage::Choke,
            PeerMessage::Unchoke,
            PeerMessage::Interested,
            PeerMessage::NotInterested,
            PeerMessage::Have { index: 42 },
            PeerMessage::Bitfield(vec![0b10101010, 0b11110000]),
            PeerMessage::Request {
                index: 5,
                begin: 16384,
                length: 16384,
            },
            PeerMessage::Piece {
                index: 5,
                begin: 16384,
                data: vec![1, 2, 3, 4],
            },
            PeerMessage::Cancel {
                index: 5,
                begin: 16384,
                length: 16384,
            },
            PeerMessage::Port(6881),
            PeerMessage::Extended {
                id: 1,
                payload: vec![10, 20, 30],
            },
            PeerMessage::SuggestPiece { index: 7 },
            PeerMessage::HaveAll,
            PeerMessage::HaveNone,
            PeerMessage::RejectRequest {
                index: 7,
                begin: 0,
                length: 16384,
            },
            PeerMessage::AllowedFast { index: 7 },
        ];

        for msg in msgs {
            let encoded = msg.encode();
            if msg == PeerMessage::KeepAlive {
                assert_eq!(encoded, vec![0, 0, 0, 0]);
                continue;
            }

            let len = u32::from_be_bytes([encoded[0], encoded[1], encoded[2], encoded[3]]) as usize;
            assert_eq!(len, encoded.len() - 4);
            let id = encoded[4];
            let payload = &encoded[5..];
            let decoded = PeerMessage::decode(id, payload).unwrap();
            assert_eq!(msg, decoded);
        }
    }
}
