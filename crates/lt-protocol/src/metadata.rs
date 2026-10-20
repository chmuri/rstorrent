//! `ut_metadata` (BEP 9) download state machine.
//!
//! Peers transfer the bencoded `info` dictionary in 16 KiB pieces. This type
//! tracks requests, re-assembles pieces and verifies the declared info-hash.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use lt_common::hash_string::HashString;
use lt_crypto::sha1::sha1_hash;
use thiserror::Error;

pub const METADATA_PIECE_SIZE: usize = 16384;
/// Hard upper bound for metadata (defensive against malicious peers).
pub const MAX_METADATA_SIZE: usize = 16 * 1024 * 1024;
/// A request that has not produced data within this window is reassigned.
pub const METADATA_REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, Error, PartialEq, Eq)]
pub enum MetadataError {
    #[error("Metadata size is invalid: {0}")]
    InvalidSize(usize),
    #[error("Piece index {piece} is out of range")]
    InvalidPiece { piece: u32 },
    #[error("Unexpected piece length: expected {expected}, got {actual}")]
    InvalidLength { expected: usize, actual: usize },
    #[error("Metadata size changed from {expected} to {actual}")]
    SizeMismatch { expected: usize, actual: usize },
    #[error("Metadata is incomplete")]
    Incomplete,
    #[error("Metadata info-hash mismatch")]
    HashMismatch,
    #[error("Duplicate metadata piece {piece} differs from the first copy")]
    DuplicateMismatch { piece: u32 },
}

#[derive(Clone, Copy, Debug)]
struct PendingRequest {
    peer: Option<HashString>,
    requested_at: Instant,
}

#[derive(Debug)]
pub struct MetadataDownload {
    info_hash: HashString,
    total_size: usize,
    pieces: Vec<Option<Vec<u8>>>,
    /// Pieces currently in flight and the peer that owns each request.
    pending: HashMap<u32, PendingRequest>,
    /// Peers that explicitly rejected a piece, with the rejection time so a
    /// single-peer swarm can retry after a full timeout.
    rejected_by: HashMap<u32, (HashSet<HashString>, Instant)>,
    received: usize,
}

impl MetadataDownload {
    pub fn new(info_hash: HashString) -> Self {
        Self {
            info_hash,
            total_size: 0,
            pieces: Vec::new(),
            pending: HashMap::new(),
            rejected_by: HashMap::new(),
            received: 0,
        }
    }

    pub fn info_hash(&self) -> &HashString {
        &self.info_hash
    }

    pub fn total_size(&self) -> usize {
        self.total_size
    }

    pub fn is_size_known(&self) -> bool {
        self.total_size > 0
    }

    pub fn piece_count(&self) -> usize {
        self.pieces.len()
    }

    pub fn received_count(&self) -> usize {
        self.received
    }

    /// Number of pieces currently requested from peers.
    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }

    pub fn has_piece(&self, piece: u32) -> bool {
        self.pieces
            .get(piece as usize)
            .map(|data| data.is_some())
            .unwrap_or(false)
    }

    /// Sets the metadata size announced by the peer. Returns `true` if the
    /// size was accepted (or already known and identical).
    pub fn set_size(&mut self, total_size: usize) -> Result<bool, MetadataError> {
        if total_size == 0 || total_size > MAX_METADATA_SIZE {
            return Err(MetadataError::InvalidSize(total_size));
        }

        if self.total_size == total_size {
            return Ok(false);
        }

        if self.total_size != 0 {
            return Err(MetadataError::SizeMismatch {
                expected: self.total_size,
                actual: total_size,
            });
        }

        let count = total_size.div_ceil(METADATA_PIECE_SIZE);
        self.total_size = total_size;
        self.pieces = vec![None; count];
        self.rejected_by.clear();
        Ok(true)
    }

    /// Returns the next missing piece that has not been requested yet and
    /// marks it as requested. This variant does not attribute the request to
    /// a peer and is therefore mostly useful for tests and single-peer use.
    pub fn next_request(&mut self) -> Option<u32> {
        self.next_request_for(None)
    }

    /// Like [`MetadataDownload::next_request`], but records which peer owns
    /// the request so it can be reassigned if that peer disconnects.
    pub fn next_request_for(&mut self, peer: Option<HashString>) -> Option<u32> {
        if self.pieces.is_empty() {
            return None;
        }

        for index in 0..self.pieces.len() {
            let index_u32 = index as u32;
            if self.pieces[index].is_some() || self.pending.contains_key(&index_u32) {
                continue;
            }
            if let Some(peer) = peer {
                let recently_rejected = self
                    .rejected_by
                    .get(&index_u32)
                    .map(|(peers, at)| {
                        peers.contains(&peer) && at.elapsed() < METADATA_REQUEST_TIMEOUT
                    })
                    .unwrap_or(false);
                if recently_rejected {
                    continue;
                }
                if self
                    .rejected_by
                    .get(&index_u32)
                    .map(|(_, at)| at.elapsed() >= METADATA_REQUEST_TIMEOUT)
                    .unwrap_or(false)
                {
                    self.rejected_by.remove(&index_u32);
                }
            }
            self.pending.insert(
                index_u32,
                PendingRequest {
                    peer,
                    requested_at: Instant::now(),
                },
            );
            return Some(index_u32);
        }

        None
    }

    /// Marks a piece as no longer in flight (reject or explicit cancel).
    pub fn mark_rejected(&mut self, piece: u32) -> bool {
        self.pending.remove(&piece).is_some()
    }

    /// Marks a piece rejected by a specific peer. That peer will not be asked
    /// for the piece again, while other peers remain eligible.
    pub fn mark_rejected_by(&mut self, piece: u32, peer: HashString) -> bool {
        let removed = self.pending.remove(&piece).is_some();
        let entry = self
            .rejected_by
            .entry(piece)
            .or_insert_with(|| (HashSet::new(), Instant::now()));
        entry.0.insert(peer);
        entry.1 = Instant::now();
        removed
    }

    /// Requeues every piece currently requested from `peer` (peer disconnect
    /// or a failed session).
    pub fn release_peer(&mut self, peer: &HashString) -> usize {
        let before = self.pending.len();
        self.pending
            .retain(|_, request| request.peer.as_ref() != Some(peer));
        before - self.pending.len()
    }

    /// Expires requests older than `timeout` and returns the affected piece
    /// indices so the caller can request them from another peer.
    pub fn expire_requests(&mut self, now: Instant, timeout: Duration) -> Vec<u32> {
        let mut expired = Vec::new();
        self.pending.retain(|piece, request| {
            if now.duration_since(request.requested_at) >= timeout {
                expired.push(*piece);
                false
            } else {
                true
            }
        });
        expired.sort_unstable();
        expired
    }

    /// Accepts a metadata piece. Returns `true` when all pieces are present.
    pub fn on_data(
        &mut self,
        piece: u32,
        total_size: u32,
        data: &[u8],
    ) -> Result<bool, MetadataError> {
        let total_size = total_size as usize;

        if self.total_size == 0 {
            self.set_size(total_size)?;
        } else if self.total_size != total_size {
            return Err(MetadataError::SizeMismatch {
                expected: self.total_size,
                actual: total_size,
            });
        }

        if piece as usize >= self.pieces.len() {
            return Err(MetadataError::InvalidPiece { piece });
        }

        let offset = piece as usize * METADATA_PIECE_SIZE;
        let expected = (self.total_size - offset).min(METADATA_PIECE_SIZE);
        if data.len() != expected {
            return Err(MetadataError::InvalidLength {
                expected,
                actual: data.len(),
            });
        }

        self.pending.remove(&piece);
        self.rejected_by.remove(&piece);

        match &self.pieces[piece as usize] {
            Some(existing) => {
                // A duplicate from another peer is only acceptable when the
                // bytes are identical; never let a later peer rewrite data.
                if existing.as_slice() != data {
                    return Err(MetadataError::DuplicateMismatch { piece });
                }
            }
            None => {
                self.pieces[piece as usize] = Some(data.to_vec());
                self.received += 1;
            }
        }

        Ok(self.received == self.pieces.len())
    }

    pub fn is_complete(&self) -> bool {
        !self.pieces.is_empty() && self.received == self.pieces.len()
    }

    /// Assembles and verifies the metadata, consuming the pieces on success.
    pub fn verify_and_take(&mut self) -> Result<Vec<u8>, MetadataError> {
        if !self.is_complete() {
            return Err(MetadataError::Incomplete);
        }

        let mut metadata = Vec::with_capacity(self.total_size);
        for piece in &self.pieces {
            metadata.extend_from_slice(piece.as_ref().expect("complete checked above"));
        }

        let digest = sha1_hash(&metadata);
        if HashString::new(digest) != self.info_hash {
            return Err(MetadataError::HashMismatch);
        }

        self.reset();
        Ok(metadata)
    }

    /// Drops every received and pending piece, keeping the info-hash.
    pub fn reset(&mut self) {
        self.pieces.clear();
        self.pending.clear();
        self.rejected_by.clear();
        self.received = 0;
        self.total_size = 0;
    }
}

/// Returns the metadata piece at `piece`, or `None` when the index is out of
/// range. Used by the `ut_metadata` server path.
pub fn metadata_piece(metadata: &[u8], piece: u32) -> Option<(u32, &[u8])> {
    let offset = (piece as usize).checked_mul(METADATA_PIECE_SIZE)?;
    if offset >= metadata.len() {
        return None;
    }
    let end = (offset + METADATA_PIECE_SIZE).min(metadata.len());
    Some((metadata.len() as u32, &metadata[offset..end]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(byte: u8) -> HashString {
        HashString::new([byte; 20])
    }

    #[test]
    fn reassembles_out_of_order_and_verifies_hash() {
        let payload: Vec<u8> = (0..40_000).map(|i| (i % 251) as u8).collect();
        let info_hash = HashString::new(sha1_hash(&payload));

        let mut download = MetadataDownload::new(info_hash);
        assert!(download.set_size(payload.len()).unwrap());

        let pieces: Vec<(u32, Vec<u8>)> = payload
            .chunks(METADATA_PIECE_SIZE)
            .enumerate()
            .map(|(index, data)| (index as u32, data.to_vec()))
            .collect();

        // Deliver in reverse order.
        let mut complete = false;
        for (index, data) in pieces.iter().rev() {
            complete = download
                .on_data(*index, payload.len() as u32, data)
                .unwrap();
        }

        assert!(complete);
        assert!(download.is_complete());
        assert_eq!(download.verify_and_take().unwrap(), payload);
        assert!(!download.is_size_known());
    }

    #[test]
    fn rejects_wrong_piece_length_and_hash() {
        let payload = vec![0x11u8; 100];
        let wrong_hash = id(0x99);
        let mut download = MetadataDownload::new(wrong_hash);
        download.set_size(payload.len()).unwrap();

        assert!(matches!(
            download.on_data(0, payload.len() as u32, &payload[..50]),
            Err(MetadataError::InvalidLength { .. })
        ));

        assert!(download.on_data(0, payload.len() as u32, &payload).unwrap());
        assert!(matches!(
            download.verify_and_take(),
            Err(MetadataError::HashMismatch)
        ));
    }

    #[test]
    fn request_tracking_avoids_duplicates() {
        let mut download = MetadataDownload::new(id(0x22));
        download.set_size(METADATA_PIECE_SIZE * 2 + 1).unwrap();

        assert_eq!(download.next_request(), Some(0));
        assert_eq!(download.next_request(), Some(1));
        assert_eq!(download.next_request(), Some(2));
        assert_eq!(download.next_request(), None);

        assert!(download.mark_rejected(1));
        assert_eq!(download.next_request(), Some(1));
    }

    #[test]
    fn rejects_absurd_size() {
        let mut download = MetadataDownload::new(id(0x33));
        assert!(matches!(
            download.set_size(MAX_METADATA_SIZE + 1),
            Err(MetadataError::InvalidSize(_))
        ));
    }

    #[test]
    fn duplicate_piece_must_match() {
        let payload = vec![0x42u8; METADATA_PIECE_SIZE + 10];
        let info_hash = HashString::new(sha1_hash(&payload));
        let mut download = MetadataDownload::new(info_hash);
        download.set_size(payload.len()).unwrap();

        let first = &payload[..METADATA_PIECE_SIZE];
        let second = &payload[METADATA_PIECE_SIZE..];
        assert!(!download.on_data(0, payload.len() as u32, first).unwrap());
        // Identical duplicate is accepted.
        assert!(!download.on_data(0, payload.len() as u32, first).unwrap());
        let mut tampered = first.to_vec();
        tampered[0] ^= 0xff;
        assert!(matches!(
            download.on_data(0, payload.len() as u32, &tampered),
            Err(MetadataError::DuplicateMismatch { piece: 0 })
        ));
        assert!(download.on_data(1, payload.len() as u32, second).unwrap());
        assert_eq!(download.verify_and_take().unwrap(), payload);
    }

    #[test]
    fn expire_and_reassign_requests() {
        let mut download = MetadataDownload::new(id(0x44));
        download.set_size(METADATA_PIECE_SIZE * 3).unwrap();
        let peer_a = id(0xaa);
        let peer_b = id(0xbb);

        assert_eq!(download.next_request_for(Some(peer_a)), Some(0));
        assert_eq!(download.next_request_for(Some(peer_a)), Some(1));
        assert_eq!(download.next_request_for(Some(peer_b)), Some(2));
        assert_eq!(download.pending_count(), 3);

        assert_eq!(download.release_peer(&peer_a), 2);
        assert_eq!(download.pending_count(), 1);
        assert_eq!(download.next_request_for(Some(peer_b)), Some(0));

        let now = Instant::now() + METADATA_REQUEST_TIMEOUT;
        let expired = download.expire_requests(now, METADATA_REQUEST_TIMEOUT);
        assert_eq!(expired, vec![0, 2]);
        assert_eq!(download.pending_count(), 0);
    }

    #[test]
    fn rejected_piece_moves_to_another_peer() {
        let mut download = MetadataDownload::new(id(0x55));
        download.set_size(METADATA_PIECE_SIZE * 2).unwrap();
        let peer_a = id(0xaa);
        let peer_b = id(0xbb);

        assert_eq!(download.next_request_for(Some(peer_a)), Some(0));
        assert_eq!(download.next_request_for(Some(peer_a)), Some(1));
        assert!(download.mark_rejected_by(0, peer_a));

        // The rejecting peer is not asked again immediately, but the piece is
        // still available to a different peer.
        assert_eq!(download.next_request_for(Some(peer_a)), None);
        assert_eq!(download.next_request_for(Some(peer_b)), Some(0));
    }

    #[test]
    fn piece_slicing_uses_total_size_for_last_piece() {
        let metadata = vec![0x55u8; METADATA_PIECE_SIZE + 7];
        let (total, first) = metadata_piece(&metadata, 0).unwrap();
        assert_eq!(total as usize, metadata.len());
        assert_eq!(first.len(), METADATA_PIECE_SIZE);
        let (total, last) = metadata_piece(&metadata, 1).unwrap();
        assert_eq!(total as usize, metadata.len());
        assert_eq!(last.len(), 7);
        assert!(metadata_piece(&metadata, 2).is_none());
    }
}
