//! Crate `lt-protocol`
//!
//! BitTorrent Peer Wire Protocol (PWP):
//! - Buffer protocol frame handling & bounds checks
//! - Request list and block request queuing (4 buckets: queued, unordered, stalled, choked)
//! - Extension protocol BEP 10 (LTEP), BEP 11 (PEX), BEP 9 (ut_metadata)
//! - Handshake serialization (68 bytes) & MSE/PE protocol encryption (DH 768-bit + RC4)
//! - Peer connections, client identification (ClientList), ConnectionList & InitialSeeding (BEP 16)

pub mod buffer;
pub mod client_list;
pub mod connection;
pub mod encryption;
pub mod extensions;
pub mod handshake;
pub mod handshake_manager;
pub mod initial_seed;
pub mod message;
pub mod metadata;
pub mod mse;
pub mod peer_list;
pub mod request_list;

pub use buffer::{ProtocolBuffer, ProtocolError};
pub use client_list::{ClientInfo, ClientList, ClientType};
pub use connection::{PeerConnection, PeerDirection};
pub use encryption::{CryptoMode, EncryptionPolicy, HandshakeEncryption};
pub use extensions::{
    extension_handshake_payload, ut_metadata_data_payload, ut_metadata_request_payload,
    ut_pex_payload, DownloadInfo, DownloadMain, ExtensionError, ExtensionType, ProtocolExtension,
    UtMetadataMessage, UtPexMessage,
};
pub use handshake::{
    HandshakeError, HandshakeMessage, BT_HANDSHAKE_SIZE, BT_PROTOCOL_LEN, BT_PROTOCOL_STR,
};
pub use handshake_manager::{HandshakeManager, HandshakePhase, HandshakeSession};
pub use initial_seed::InitialSeeding;
pub use message::{MessageError, PeerMessage};
pub use metadata::{MetadataDownload, MetadataError, MAX_METADATA_SIZE, METADATA_PIECE_SIZE};
pub use mse::{
    accept as mse_accept, initiate as mse_initiate, obfuscated_info_hash, MseError, MseSession,
};
pub use peer_list::{ConnectionList, ConnectionListError};
pub use request_list::{BlockTransfer, RequestList};
