use lt_common::hash_string::HashString;
use lt_net::addr::{AddressList, SocketAddress};
use thiserror::Error;

use crate::connection::{PeerConnection, PeerInfo};

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ConnectionListError {
    #[error("Input error: {0}")]
    InputError(&'static str),
    #[error("Internal error: {0}")]
    InternalError(&'static str),
}

pub struct ConnectionList {
    connections: Vec<PeerConnection>,
    min_size: usize,
    max_size: usize,
    /// Whether `max_size` was chosen explicitly. Downloads use this to let a
    /// runtime `ClientConfig` limit replace the legacy default without
    /// overriding a caller that deliberately configured the list.
    max_size_explicit: bool,
    change_counter: u32,
}

impl Default for ConnectionList {
    fn default() -> Self {
        Self::new()
    }
}

impl ConnectionList {
    pub const MAX_ALLOWED_PEERS: usize = 1 << 16; // 65536

    pub fn new() -> Self {
        Self {
            connections: Vec::new(),
            min_size: 0,
            max_size: 100,
            max_size_explicit: false,
            change_counter: 0,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.connections.is_empty()
    }

    pub fn empty(&self) -> bool {
        self.connections.is_empty()
    }

    pub fn size(&self) -> usize {
        self.connections.len()
    }

    /// Alias for [`ConnectionList::size`].
    pub fn peer_count(&self) -> usize {
        self.size()
    }

    pub fn change_counter(&self) -> u32 {
        self.change_counter
    }

    pub fn min_size(&self) -> usize {
        self.min_size
    }

    pub fn set_min_size(&mut self, v: usize) -> Result<(), ConnectionListError> {
        if v > Self::MAX_ALLOWED_PEERS {
            return Err(ConnectionListError::InputError(
                "Min peer connections must be between 0 and 2^16.",
            ));
        }
        self.min_size = v;
        Ok(())
    }

    pub fn max_size(&self) -> usize {
        self.max_size
    }

    pub fn set_max_size(&mut self, v: usize) -> Result<(), ConnectionListError> {
        if v > Self::MAX_ALLOWED_PEERS {
            return Err(ConnectionListError::InputError(
                "Max peer connections must be between 0 and 2^16.",
            ));
        }
        // Match libtorrent: zero means "no explicit limit".
        self.max_size = if v == 0 { Self::MAX_ALLOWED_PEERS } else { v };
        self.max_size_explicit = true;
        Ok(())
    }

    /// Whether [`ConnectionList::set_max_size`] has been called.
    pub fn max_size_explicit(&self) -> bool {
        self.max_size_explicit
    }

    /// Applies a runtime limit without marking the list as explicitly
    /// configured, so later `ClientConfig` changes can replace it.
    pub fn set_max_size_auto(&mut self, v: usize) -> Result<(), ConnectionListError> {
        if v > Self::MAX_ALLOWED_PEERS {
            return Err(ConnectionListError::InputError(
                "Max peer connections must be between 0 and 2^16.",
            ));
        }
        self.max_size = if v == 0 { Self::MAX_ALLOWED_PEERS } else { v };
        Ok(())
    }

    /// Inserts a connection if the configured maximum has not been reached.
    ///
    /// Returns an error when the list is full. Use [`ConnectionList::push`]
    /// when a boolean result is more convenient.
    pub fn try_push(&mut self, conn: PeerConnection) -> Result<(), ConnectionListError> {
        if self.connections.len() >= self.max_size {
            return Err(ConnectionListError::InputError(
                "Maximum peer connections reached.",
            ));
        }
        self.push_unchecked(conn);
        Ok(())
    }

    /// Inserts a connection, respecting [`ConnectionList::max_size`].
    ///
    /// Returns whether the connection was inserted. This keeps the historical
    /// `push` entry point while making the limit enforceable on every path.
    pub fn push(&mut self, conn: PeerConnection) -> bool {
        if self.connections.len() >= self.max_size {
            return false;
        }
        self.push_unchecked(conn);
        true
    }

    /// Inserts a connection without checking the configured maximum.
    ///
    /// Intended for tests and for callers that enforce a stricter policy
    /// before touching the list.
    pub fn push_unchecked(&mut self, conn: PeerConnection) {
        self.connections.push(conn);
        self.change_counter = self.change_counter.wrapping_add(1);
    }

    /// True when the list has fewer peers than the configured minimum.
    pub fn below_min_size(&self) -> bool {
        self.connections.len() < self.min_size
    }

    /// Alias for [`ConnectionList::below_min_size`].
    pub fn needs_peers(&self) -> bool {
        self.below_min_size()
    }

    pub fn erase(
        &mut self,
        peer_id: Option<&HashString>,
        _flags: i32,
    ) -> Result<(), ConnectionListError> {
        let id = match peer_id {
            Some(id) => id,
            None => {
                return Err(ConnectionListError::InternalError(
                    "ConnectionList::erase(...) received a null pointer.",
                ))
            }
        };

        if let Some(pos) = self.connections.iter().position(|c| c.peer_id == *id) {
            self.connections.remove(pos);
            self.change_counter = self.change_counter.wrapping_add(1);
            Ok(())
        } else {
            Err(ConnectionListError::InternalError(
                "ConnectionList::erase(...) peer not found.",
            ))
        }
    }

    /// Removes the peer at `sa`, returning its id when present.
    pub fn erase_by_address(&mut self, sa: &SocketAddress) -> Option<HashString> {
        let pos = self
            .connections
            .iter()
            .position(|c| c.socket_address == *sa)?;
        let removed = self.connections.remove(pos);
        self.change_counter = self.change_counter.wrapping_add(1);
        Some(removed.peer_id)
    }

    pub fn contains_id(&self, id: &HashString) -> bool {
        self.find_by_id(id).is_some()
    }

    pub fn contains_address(&self, sa: &SocketAddress) -> bool {
        self.find_by_address(sa).is_some()
    }

    pub fn find_by_id(&self, id: &HashString) -> Option<&PeerConnection> {
        self.connections.iter().find(|c| c.peer_id == *id)
    }

    pub fn iter(&self) -> std::slice::Iter<'_, PeerConnection> {
        self.connections.iter()
    }

    pub fn iter_mut(&mut self) -> std::slice::IterMut<'_, PeerConnection> {
        self.connections.iter_mut()
    }

    /// Iterates over owned [`PeerInfo`] snapshots.
    ///
    /// The snapshots are produced lazily, so callers that use the command layer
    /// do not hold a borrow of the connection list while formatting results.
    pub fn info_iter(&self) -> impl Iterator<Item = PeerInfo> + '_ {
        self.connections.iter().map(PeerConnection::info)
    }

    /// Collects owned snapshots of every connected peer.
    pub fn infos(&self) -> Vec<PeerInfo> {
        self.info_iter().collect()
    }

    /// Alias for [`ConnectionList::infos`].
    pub fn peer_infos(&self) -> Vec<PeerInfo> {
        self.infos()
    }

    /// Alias for [`ConnectionList::info_iter`].
    pub fn peer_info_iter(&self) -> impl Iterator<Item = PeerInfo> + '_ {
        self.info_iter()
    }

    /// Alias for [`ConnectionList::info_iter`].
    pub fn iter_infos(&self) -> impl Iterator<Item = PeerInfo> + '_ {
        self.info_iter()
    }

    /// Returns a snapshot for `id`.
    pub fn info(&self, id: &HashString) -> Option<PeerInfo> {
        self.find_by_id(id).map(PeerConnection::info)
    }

    /// Alias for [`ConnectionList::info`].
    pub fn peer_info(&self, id: &HashString) -> Option<PeerInfo> {
        self.info(id)
    }

    /// Returns a snapshot for the peer at `sa`.
    pub fn info_by_address(&self, sa: &SocketAddress) -> Option<PeerInfo> {
        self.find_by_address(sa).map(PeerConnection::info)
    }

    pub fn find_by_id_mut(&mut self, id: &HashString) -> Option<&mut PeerConnection> {
        self.connections.iter_mut().find(|c| c.peer_id == *id)
    }

    pub fn find_by_address(&self, sa: &SocketAddress) -> Option<&PeerConnection> {
        self.connections.iter().find(|c| c.socket_address == *sa)
    }

    pub fn find_by_address_mut(&mut self, sa: &SocketAddress) -> Option<&mut PeerConnection> {
        self.connections
            .iter_mut()
            .find(|c| c.socket_address == *sa)
    }

    pub fn set_difference(&mut self, address_list: &mut AddressList) {
        self.connections
            .sort_by(|a, b| a.socket_address.cmp(&b.socket_address));
        self.change_counter = self.change_counter.wrapping_add(1);

        address_list.0.retain(|sa| {
            self.connections
                .binary_search_by(|c| c.socket_address.cmp(sa))
                .is_err()
        });
    }

    pub fn erase_seeders(&mut self) {
        let before = self.connections.len();
        self.connections.retain(|c| c.is_not_seeder());
        if self.connections.len() != before {
            self.change_counter = self.change_counter.wrapping_add(1);
        }
    }
}
