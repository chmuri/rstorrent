use std::time::{SystemTime, UNIX_EPOCH};

use lt_common::bitfield::Bitfield;
use lt_common::hash_string::HashString;
use lt_common::rate::Rate;
use lt_net::addr::SocketAddress;

use crate::client_list::{ClientInfo, ClientList};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PeerDirection {
    #[default]
    Outgoing,
    Incoming,
}

impl PeerDirection {
    pub fn is_incoming(self) -> bool {
        self == PeerDirection::Incoming
    }

    pub fn is_outgoing(self) -> bool {
        self == PeerDirection::Outgoing
    }
}

/// A cheap, flag-only view of a peer's connection state.
///
/// This mirrors the bit flags stored by libtorrent's `PeerInfo`. It is
/// intentionally a plain struct so callers do not need to depend on a
/// `bitflags` type to inspect or compare peer state.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PeerFlags {
    pub connected: bool,
    pub incoming: bool,
    pub handshake: bool,
    /// Blocked for initial seeding (BEP 16).
    pub blocked: bool,
    pub restart: bool,
    pub unwanted: bool,
    pub preferred: bool,
    pub snubbed: bool,
    pub banned: bool,
    pub encrypted: bool,
    pub obfuscated: bool,
    pub choked: bool,
    pub interested: bool,
    pub peer_choked: bool,
    pub peer_interested: bool,
}

impl PeerFlags {
    /// Converts the flags to the legacy `PeerInfo::flag_*` bit layout.
    ///
    /// The extra flags (`snubbed`, `banned`, encryption and choke state) use
    /// bits above the legacy range so the value is still useful to callers
    /// that only compare against the C++ constants.
    pub fn bits(self) -> u32 {
        let mut bits = 0;
        if self.connected {
            bits |= 1 << 0;
        }
        if self.incoming {
            bits |= 1 << 1;
        }
        if self.handshake {
            bits |= 1 << 2;
        }
        if self.blocked {
            bits |= 1 << 3;
        }
        if self.restart {
            bits |= 1 << 4;
        }
        if self.unwanted {
            bits |= 1 << 5;
        }
        if self.preferred {
            bits |= 1 << 6;
        }
        if self.snubbed {
            bits |= 1 << 7;
        }
        if self.banned {
            bits |= 1 << 8;
        }
        if self.encrypted {
            bits |= 1 << 9;
        }
        if self.obfuscated {
            bits |= 1 << 10;
        }
        if self.choked {
            bits |= 1 << 11;
        }
        if self.interested {
            bits |= 1 << 12;
        }
        if self.peer_choked {
            bits |= 1 << 13;
        }
        if self.peer_interested {
            bits |= 1 << 14;
        }
        bits
    }
}

/// An owned snapshot of a connected peer.
///
/// `PeerConnection` is the live session object. `PeerInfo` is the stable,
/// serialisable view used by the command layer and UI; obtaining one never
/// borrows the connection list, so it remains valid after peers are added or
/// removed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PeerInfo {
    pub peer_id: HashString,
    pub socket_address: SocketAddress,
    pub client: ClientInfo,
    pub direction: PeerDirection,
    pub is_connected: bool,
    pub is_handshake: bool,
    pub is_blocked: bool,
    pub is_restart: bool,
    pub is_unwanted: bool,
    pub is_preferred: bool,
    pub is_snubbed: bool,
    pub is_banned: bool,
    pub is_encrypted: bool,
    pub is_obfuscated: bool,
    pub is_choked: bool,
    pub is_interested: bool,
    pub peer_choked: bool,
    pub peer_interested: bool,
    pub uploaded: u64,
    pub downloaded: u64,
    pub rate_up: u64,
    pub rate_down: u64,
    pub failed_counter: u32,
    pub transfer_counter: u32,
    pub last_connection: u64,
    pub last_handshake: u64,
    pub listen_port: u16,
    pub options: [u8; 8],
    /// Outstanding block requests for this peer, when known by the caller.
    pub queue_depth: usize,
    pub bitfield: Bitfield,
}

impl Default for PeerInfo {
    fn default() -> Self {
        Self::new(HashString::default(), SocketAddress::default())
    }
}

impl From<&PeerConnection> for PeerInfo {
    fn from(peer: &PeerConnection) -> Self {
        peer.info()
    }
}

impl PeerInfo {
    pub fn id(&self) -> &HashString {
        &self.peer_id
    }

    pub fn address(&self) -> &SocketAddress {
        &self.socket_address
    }

    pub fn socket_address(&self) -> &SocketAddress {
        &self.socket_address
    }

    pub fn client_info(&self) -> &ClientInfo {
        &self.client
    }

    pub fn id_hex(&self) -> String {
        self.peer_id.to_hex()
    }

    pub fn address_str(&self) -> String {
        self.socket_address.addr_str()
    }

    pub fn port(&self) -> u16 {
        self.socket_address.port()
    }

    pub fn options(&self) -> &[u8; 8] {
        &self.options
    }

    pub fn options_str(&self) -> String {
        self.options
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<Vec<_>>()
            .join("")
    }

    pub fn listen_port(&self) -> u16 {
        self.listen_port
    }

    pub fn queue_depth(&self) -> usize {
        self.queue_depth
    }

    pub fn up_total(&self) -> u64 {
        self.uploaded
    }

    pub fn down_total(&self) -> u64 {
        self.downloaded
    }

    pub fn up_rate(&self) -> u64 {
        self.rate_up
    }

    pub fn down_rate(&self) -> u64 {
        self.rate_down
    }

    /// Snapshots a live connection.
    pub fn from_connection(peer: &PeerConnection) -> Self {
        peer.info()
    }

    /// Creates an empty snapshot for a peer id/address pair.
    pub fn new(peer_id: HashString, socket_address: SocketAddress) -> Self {
        let client = identify_client(&peer_id);
        let listen_port = socket_address.port();
        Self {
            peer_id,
            socket_address,
            client,
            direction: PeerDirection::Outgoing,
            is_connected: true,
            is_handshake: false,
            is_blocked: false,
            is_restart: false,
            is_unwanted: false,
            is_preferred: false,
            is_snubbed: false,
            is_banned: false,
            is_encrypted: false,
            is_obfuscated: false,
            is_choked: true,
            is_interested: false,
            peer_choked: true,
            peer_interested: false,
            uploaded: 0,
            downloaded: 0,
            rate_up: 0,
            rate_down: 0,
            failed_counter: 0,
            transfer_counter: 0,
            last_connection: 0,
            last_handshake: 0,
            listen_port,
            options: [0; 8],
            queue_depth: 0,
            bitfield: Bitfield::default(),
        }
    }

    pub fn is_connected(&self) -> bool {
        self.is_connected
    }

    pub fn is_handshake(&self) -> bool {
        self.is_handshake
    }

    pub fn is_blocked(&self) -> bool {
        self.is_blocked
    }

    pub fn is_restart(&self) -> bool {
        self.is_restart
    }

    pub fn is_unwanted(&self) -> bool {
        self.is_unwanted
    }

    pub fn is_preferred(&self) -> bool {
        self.is_preferred
    }

    pub fn is_snubbed(&self) -> bool {
        self.is_snubbed
    }

    pub fn is_banned(&self) -> bool {
        self.is_banned
    }

    pub fn is_encrypted(&self) -> bool {
        self.is_encrypted
    }

    pub fn is_obfuscated(&self) -> bool {
        self.is_obfuscated
    }

    pub fn is_choked(&self) -> bool {
        self.is_choked
    }

    pub fn is_interested(&self) -> bool {
        self.is_interested
    }

    pub fn is_incoming(&self) -> bool {
        self.direction == PeerDirection::Incoming
    }

    pub fn is_outgoing(&self) -> bool {
        self.direction == PeerDirection::Outgoing
    }

    pub fn flags(&self) -> PeerFlags {
        PeerFlags {
            connected: self.is_connected,
            incoming: self.is_incoming(),
            handshake: self.is_handshake,
            blocked: self.is_blocked,
            restart: self.is_restart,
            unwanted: self.is_unwanted,
            preferred: self.is_preferred,
            snubbed: self.is_snubbed,
            banned: self.is_banned,
            encrypted: self.is_encrypted,
            obfuscated: self.is_obfuscated,
            choked: self.is_choked,
            interested: self.is_interested,
            peer_choked: self.peer_choked,
            peer_interested: self.peer_interested,
        }
    }

    pub fn flags_bits(&self) -> u32 {
        self.flags().bits()
    }

    pub fn client_version(&self) -> String {
        if self.client.version.is_empty() {
            self.client.name.clone()
        } else {
            format!("{} {}", self.client.name, self.client.version)
        }
    }

    pub fn is_seeder(&self) -> bool {
        !self.bitfield.is_empty() && self.bitfield.is_all_set()
    }

    /// Alias matching the `p.is_seed` command name.
    pub fn is_seed(&self) -> bool {
        self.is_seeder()
    }

    pub fn completed_percent(&self) -> u64 {
        let bits = self.bitfield.size_bits();
        if bits == 0 {
            0
        } else {
            (100 * self.bitfield.size_set() as u64) / bits as u64
        }
    }

    pub fn supports_dht(&self) -> bool {
        self.options[7] & 0x01 != 0
    }

    pub fn supports_extensions(&self) -> bool {
        self.options[5] & 0x10 != 0
    }
}

/// Client identification wrapper that keeps malformed peer ids from reaching
/// the legacy parser's unchecked digit arithmetic.
fn identify_client(peer_id: &HashString) -> ClientInfo {
    let bytes = peer_id.as_bytes();

    // The Mainline-style parser subtracts `b'0'` from the fourth byte without
    // validating it. Reject that shape first so arbitrary wire data cannot
    // underflow (debug builds panic on overflow).
    if bytes.len() >= 8
        && bytes[0] == b'M'
        && bytes[1].is_ascii_digit()
        && bytes[2] == b'-'
        && !bytes[3].is_ascii_digit()
    {
        return ClientInfo::default();
    }

    ClientList::identify(peer_id)
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

#[derive(Clone, Debug)]
pub struct PeerConnection {
    pub peer_id: HashString,
    pub socket_address: SocketAddress,
    pub bitfield: Bitfield,
    pub client: ClientInfo,
    pub direction: PeerDirection,
    pub is_connected: bool,
    pub is_handshake: bool,
    pub is_blocked: bool,
    pub is_restart: bool,
    pub is_unwanted: bool,
    pub is_encrypted: bool,
    pub is_obfuscated: bool,
    pub is_snubbed: bool,
    pub is_banned: bool,
    pub is_preferred: bool,
    pub is_choked: bool,
    pub is_interested: bool,
    pub peer_choked: bool,
    pub peer_interested: bool,
    pub uploaded: u64,
    pub downloaded: u64,
    pub rate_up: Rate,
    pub rate_down: Rate,
    pub failed_counter: u32,
    pub transfer_counter: u32,
    pub last_connection: u64,
    pub last_handshake: u64,
    /// Timestamp when the transport was established. Used by the engine to
    /// enforce connect/handshake timeouts.
    pub connected_at: u64,
    /// Listening port reported by the remote peer, or the port used for an
    /// outgoing connection.
    pub listen_port: u16,
    /// Reserved handshake bytes (DHT/extension support bits).
    pub options: [u8; 8],
}

impl PeerConnection {
    pub fn id(&self) -> &HashString {
        &self.peer_id
    }

    pub fn address(&self) -> &SocketAddress {
        &self.socket_address
    }

    pub fn socket_address(&self) -> &SocketAddress {
        &self.socket_address
    }

    pub fn client_info(&self) -> &ClientInfo {
        &self.client
    }

    pub fn id_hex(&self) -> String {
        self.peer_id.to_hex()
    }

    pub fn address_str(&self) -> String {
        self.socket_address.addr_str()
    }

    pub fn port(&self) -> u16 {
        self.socket_address.port()
    }

    pub fn options(&self) -> &[u8; 8] {
        &self.options
    }

    pub fn options_str(&self) -> String {
        self.options
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<Vec<_>>()
            .join("")
    }

    pub fn listen_port(&self) -> u16 {
        self.listen_port
    }

    pub fn new(peer_id: HashString, sa: SocketAddress, bitfield_size: usize) -> Self {
        Self::new_with_direction(
            peer_id,
            sa,
            bitfield_size,
            PeerDirection::Outgoing,
            false,
        )
    }

    pub fn new_with_direction(
        peer_id: HashString,
        sa: SocketAddress,
        bitfield_size: usize,
        direction: PeerDirection,
        encrypted: bool,
    ) -> Self {
        let now = unix_now();
        let listen_port = sa.port();
        Self {
            peer_id,
            socket_address: sa,
            bitfield: Bitfield::new(bitfield_size),
            client: identify_client(&peer_id),
            direction,
            is_connected: true,
            is_handshake: false,
            is_blocked: false,
            is_restart: false,
            is_unwanted: false,
            is_encrypted: encrypted,
            is_obfuscated: false,
            is_snubbed: false,
            is_banned: false,
            is_preferred: false,
            is_choked: true,
            is_interested: false,
            peer_choked: true,
            peer_interested: false,
            uploaded: 0,
            downloaded: 0,
            rate_up: Rate::new(20),
            rate_down: Rate::new(20),
            failed_counter: 0,
            transfer_counter: 0,
            last_connection: now,
            last_handshake: now,
            connected_at: now,
            listen_port,
            options: [0; 8],
        }
    }

    pub fn is_seeder(&self) -> bool {
        !self.bitfield.is_empty() && self.bitfield.is_all_set()
    }

    pub fn is_not_seeder(&self) -> bool {
        !self.is_seeder()
    }

    pub fn is_connected(&self) -> bool {
        self.is_connected
    }

    pub fn is_handshake(&self) -> bool {
        self.is_handshake
    }

    pub fn is_blocked(&self) -> bool {
        self.is_blocked
    }

    pub fn is_restart(&self) -> bool {
        self.is_restart
    }

    pub fn is_unwanted(&self) -> bool {
        self.is_unwanted
    }

    pub fn is_preferred(&self) -> bool {
        self.is_preferred
    }

    pub fn is_snubbed(&self) -> bool {
        self.is_snubbed
    }

    pub fn is_banned(&self) -> bool {
        self.is_banned
    }

    pub fn is_encrypted(&self) -> bool {
        self.is_encrypted
    }

    pub fn is_obfuscated(&self) -> bool {
        self.is_obfuscated
    }

    pub fn is_choked(&self) -> bool {
        self.is_choked
    }

    pub fn is_interested(&self) -> bool {
        self.is_interested
    }

    pub fn is_incoming(&self) -> bool {
        self.direction == PeerDirection::Incoming
    }

    pub fn is_outgoing(&self) -> bool {
        self.direction == PeerDirection::Outgoing
    }

    pub fn client_version(&self) -> String {
        if self.client.version.is_empty() {
            self.client.name.clone()
        } else {
            format!("{} {}", self.client.name, self.client.version)
        }
    }

    /// Returns a flag-only view of this peer.
    pub fn flags(&self) -> PeerFlags {
        PeerFlags {
            connected: self.is_connected,
            incoming: self.is_incoming(),
            handshake: self.is_handshake,
            blocked: self.is_blocked,
            restart: self.is_restart,
            unwanted: self.is_unwanted,
            preferred: self.is_preferred,
            snubbed: self.is_snubbed,
            banned: self.is_banned,
            encrypted: self.is_encrypted,
            obfuscated: self.is_obfuscated,
            choked: self.is_choked,
            interested: self.is_interested,
            peer_choked: self.peer_choked,
            peer_interested: self.peer_interested,
        }
    }

    pub fn flags_bits(&self) -> u32 {
        self.flags().bits()
    }

    /// Takes an owned snapshot of this peer.
    ///
    /// The current rate is sampled by cloning the rate meter, so this works
    /// with a shared reference and does not perturb accounting.
    pub fn info(&self) -> PeerInfo {
        let mut rate_up = self.rate_up.clone();
        let mut rate_down = self.rate_down.clone();

        PeerInfo {
            peer_id: self.peer_id,
            socket_address: self.socket_address.clone(),
            client: self.client.clone(),
            direction: self.direction,
            is_connected: self.is_connected,
            is_handshake: self.is_handshake,
            is_blocked: self.is_blocked,
            is_restart: self.is_restart,
            is_unwanted: self.is_unwanted,
            is_preferred: self.is_preferred,
            is_snubbed: self.is_snubbed,
            is_banned: self.is_banned,
            is_encrypted: self.is_encrypted,
            is_obfuscated: self.is_obfuscated,
            is_choked: self.is_choked,
            is_interested: self.is_interested,
            peer_choked: self.peer_choked,
            peer_interested: self.peer_interested,
            uploaded: self.uploaded,
            downloaded: self.downloaded,
            rate_up: rate_up.rate(),
            rate_down: rate_down.rate(),
            failed_counter: self.failed_counter,
            transfer_counter: self.transfer_counter,
            last_connection: self.last_connection,
            last_handshake: self.last_handshake,
            listen_port: self.listen_port,
            options: self.options,
            queue_depth: 0,
            bitfield: self.bitfield.clone(),
        }
    }

    /// Alias for [`PeerConnection::info`].
    pub fn snapshot(&self) -> PeerInfo {
        self.info()
    }

    /// Alias for [`PeerConnection::info`].
    pub fn to_info(&self) -> PeerInfo {
        self.info()
    }

    /// Alias for [`PeerConnection::info`].
    pub fn to_peer_info(&self) -> PeerInfo {
        self.info()
    }

    /// Alias for [`PeerConnection::info`].
    pub fn peer_info(&self) -> PeerInfo {
        self.info()
    }

    pub fn set_direction(&mut self, direction: PeerDirection) {
        self.direction = direction;
    }

    pub fn set_snubbed(&mut self, value: bool) {
        self.is_snubbed = value;
    }

    pub fn set_preferred(&mut self, value: bool) {
        self.is_preferred = value;
    }

    pub fn set_banned(&mut self, value: bool) {
        self.is_banned = value;
    }

    pub fn set_unwanted(&mut self, value: bool) {
        self.is_unwanted = value;
    }

    pub fn set_encrypted(&mut self, value: bool) {
        self.is_encrypted = value;
    }

    pub fn set_obfuscated(&mut self, value: bool) {
        self.is_obfuscated = value;
    }

    pub fn set_choked(&mut self, value: bool) {
        self.is_choked = value;
    }

    pub fn set_interested(&mut self, value: bool) {
        self.is_interested = value;
    }

    pub fn set_peer_choked(&mut self, value: bool) {
        self.peer_choked = value;
    }

    pub fn set_peer_interested(&mut self, value: bool) {
        self.peer_interested = value;
    }

    pub fn set_connected(&mut self, value: bool) {
        self.is_connected = value;
        if value {
            self.connected_at = unix_now();
            self.last_connection = self.connected_at;
        }
    }

    /// Marks the peer-wire handshake as complete and clears the handshake
    /// timeout state.
    pub fn mark_handshake_complete(&mut self) {
        self.is_handshake = false;
        self.last_handshake = unix_now();
    }

    /// Marks the peer as currently negotiating its handshake.
    pub fn mark_handshake_started(&mut self) {
        self.is_handshake = true;
        self.connected_at = unix_now();
    }

    pub fn set_failed_counter(&mut self, value: u32) {
        self.failed_counter = value;
    }

    pub fn inc_failed_counter(&mut self) {
        self.failed_counter = self.failed_counter.saturating_add(1);
    }

    pub fn inc_transfer_counter(&mut self) {
        self.transfer_counter = self.transfer_counter.saturating_add(1);
    }

    pub fn dec_transfer_counter(&mut self) {
        self.transfer_counter = self.transfer_counter.saturating_sub(1);
    }

    pub fn supports_dht(&self) -> bool {
        self.options[7] & 0x01 != 0
    }

    pub fn supports_extensions(&self) -> bool {
        self.options[5] & 0x10 != 0
    }

    pub fn add_uploaded(&mut self, bytes: u64) {
        self.uploaded = self.uploaded.saturating_add(bytes);
        self.rate_up.insert(bytes);
    }

    pub fn add_downloaded(&mut self, bytes: u64) {
        self.downloaded = self.downloaded.saturating_add(bytes);
        self.rate_down.insert(bytes);
    }

    pub fn up_rate(&mut self) -> u64 {
        self.rate_up.rate()
    }

    pub fn down_rate(&mut self) -> u64 {
        self.rate_down.rate()
    }
}
