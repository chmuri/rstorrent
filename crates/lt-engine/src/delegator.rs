use rustc_hash::FxHashMap;

use crate::chunk_selector::ChunkSelector;
use crate::chunk_statistics::ChunkStatistics;
use lt_common::bitfield::Bitfield;
use lt_common::collections::PartialQueue;
use lt_storage::block::{BlockList, TransferList, DEFAULT_BLOCK_SIZE};
use lt_storage::file::Priority;
use lt_storage::piece::Piece;

/// Peer speed tier for Piece Affinity and dynamic Endgame block stealing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum PeerSpeedClass {
    #[default]
    Medium,
    Slow,
    Fast,
}

/// Peer-specific inputs for [`Delegator::delegate`].
pub struct DelegatePeer<'a> {
    pub peer_id: usize,
    pub peer_bf: &'a Bitfield,
    pub is_seeder: bool,
    pub affinity: Option<u32>,
    pub speed_class: PeerSpeedClass,
    pub is_fastest: bool,
}

/// Shared per-download state borrowed by [`Delegator::delegate`].
pub struct DelegateCtx<'a> {
    pub chunk_selector: &'a mut ChunkSelector,
    pub stats: &'a ChunkStatistics,
    pub peer_cache: &'a mut PartialQueue,
}

/// Delegator orchestrates assigning blocks to peer download pipelines and activates Endgame mode.
#[derive(Debug, Default)]
pub struct Delegator {
    transfers: TransferList,
    piece_speed_class: FxHashMap<u32, PeerSpeedClass>,
    aggressive: bool,
}

impl Delegator {
    pub fn new() -> Self {
        Self {
            transfers: TransferList::new(),
            piece_speed_class: FxHashMap::default(),
            aggressive: false,
        }
    }

    pub fn transfers(&self) -> &TransferList {
        &self.transfers
    }

    pub fn transfers_mut(&mut self) -> &mut TransferList {
        &mut self.transfers
    }

    /// Marks every outstanding transfer for `peer_id` as cancelled, returning
    /// the blocks to the selector.
    pub fn cancel_peer(&mut self, peer_id: usize) -> usize {
        let mut cancelled = 0;

        for block_list in self.transfers.iter_mut() {
            for block in block_list.blocks_mut() {
                for transfer_idx in 0..block.size_all() {
                    let matches = block
                        .get_transfer(transfer_idx)
                        .map(|transfer| transfer.peer_id() == peer_id && !transfer.is_erased())
                        .unwrap_or(false);

                    if matches {
                        block.transfer_dissimilar(transfer_idx);
                        cancelled += 1;
                    }
                }
            }
        }

        cancelled
    }

    /// Cancels one specific block request for `peer_id`.
    pub fn cancel_block(&mut self, peer_id: usize, piece_index: u32, block_offset: u32) -> bool {
        if let Some(block_list) = self.transfers.find_mut(piece_index) {
            for block in block_list.blocks_mut() {
                if block.piece().offset() != block_offset {
                    continue;
                }

                for transfer_idx in 0..block.size_all() {
                    let matches = block
                        .get_transfer(transfer_idx)
                        .map(|transfer| transfer.peer_id() == peer_id && !transfer.is_erased())
                        .unwrap_or(false);

                    if matches {
                        block.transfer_dissimilar(transfer_idx);
                        return true;
                    }
                }
            }
        }

        false
    }

    pub fn is_aggressive(&self) -> bool {
        self.aggressive
    }

    pub fn set_aggressive(&mut self, aggressive: bool) {
        self.aggressive = aggressive;
    }

    /// Dynamically evaluates whether Endgame mode should be activated:
    /// UnrequestedBlocks == 0 && InFlightBlocks <= TotalPipelineCapacity.
    pub fn evaluate_endgame(
        &mut self,
        unrequested_blocks: usize,
        in_flight_blocks: usize,
        total_pipeline_capacity: usize,
    ) -> bool {
        let should_be_aggressive = unrequested_blocks == 0
            && in_flight_blocks > 0
            && in_flight_blocks <= total_pipeline_capacity;
        self.set_aggressive(should_be_aggressive);
        should_be_aggressive
    }

    /// Delegates up to `max_pieces` blocks for a peer.
    /// Returns the allocated block transfers (piece, offset, length, peer_id).
    pub fn delegate<F>(
        &mut self,
        peer: DelegatePeer<'_>,
        ctx: &mut DelegateCtx<'_>,
        max_pieces: usize,
        chunk_size_fn: F,
    ) -> Vec<Piece>
    where
        F: Fn(u32) -> u32,
    {
        let mut new_transfers = Vec::new();

        // 1. Check affinity (piece currently being downloaded by this peer)
        if let Some(aff_idx) = peer.affinity {
            if let Some(bl) = self.transfers.find_mut(aff_idx) {
                Self::delegate_from_blocklist(&mut new_transfers, max_pieces, bl, peer.peer_id);
                if new_transfers.len() >= max_pieces {
                    return new_transfers;
                }
            }
        }

        // Tier 0: Partial pieces (kawałki napoczęte)
        // Always assign missing blocks from existing in-progress pieces before starting new ones.
        // Piece Affinity: Protect Fast pieces from Slow peer stragglers outside Endgame.
        for bl in self.transfers.iter_mut() {
            if new_transfers.len() >= max_pieces {
                return new_transfers;
            }
            if !peer.peer_bf.get(bl.index() as usize) || bl.priority() == Priority::Off {
                continue;
            }
            if !self.aggressive && peer.speed_class == PeerSpeedClass::Slow {
                if let Some(&cls) = self.piece_speed_class.get(&bl.index()) {
                    if cls == PeerSpeedClass::Fast {
                        continue;
                    }
                }
            }
            Self::delegate_from_blocklist(&mut new_transfers, max_pieces, bl, peer.peer_id);
        }

        if new_transfers.len() >= max_pieces {
            return new_transfers;
        }

        // 2. Prioritize full seeders
        if peer.is_seeder {
            self.delegate_new_chunks(
                &mut new_transfers,
                max_pieces,
                &peer,
                true,
                ctx,
                &chunk_size_fn,
            );
            self.delegate_new_chunks(
                &mut new_transfers,
                max_pieces,
                &peer,
                false,
                ctx,
                &chunk_size_fn,
            );
        }

        if new_transfers.len() >= max_pieces {
            return new_transfers;
        }

        // 3. Create new high-priority pieces
        self.delegate_new_chunks(
            &mut new_transfers,
            max_pieces,
            &peer,
            true,
            ctx,
            &chunk_size_fn,
        );

        if new_transfers.len() >= max_pieces {
            return new_transfers;
        }

        // 4. Create new normal-priority pieces
        self.delegate_new_chunks(
            &mut new_transfers,
            max_pieces,
            &peer,
            false,
            ctx,
            &chunk_size_fn,
        );

        if !self.aggressive || new_transfers.len() >= max_pieces {
            return new_transfers;
        }

        // 7. Endgame (aggressive) mode: duplicate requests across peers for unfinished blocks
        // In Endgame, only fast peers steal/duplicate remaining in-flight blocks to guarantee fast completion.
        if peer.speed_class == PeerSpeedClass::Slow && !peer.is_fastest {
            return new_transfers;
        }

        let mut overlapped = 5;
        for bl in self.transfers.iter_mut() {
            if new_transfers.len() >= max_pieces {
                break;
            }
            if peer.peer_bf.get(bl.index() as usize) && bl.priority() != Priority::Off {
                for block in bl.blocks_mut() {
                    if new_transfers.len() >= max_pieces || block.size_not_stalled() >= overlapped {
                        break;
                    }
                    if !block.is_finished() {
                        let _idx = block.insert(peer.peer_id);
                        new_transfers.push(*block.piece());
                        overlapped = block.size_not_stalled();
                    }
                }
            }
        }

        new_transfers
    }

    fn delegate_new_chunks<F>(
        &mut self,
        transfers: &mut Vec<Piece>,
        max_pieces: usize,
        peer: &DelegatePeer<'_>,
        high_priority: bool,
        ctx: &mut DelegateCtx<'_>,
        chunk_size_fn: &F,
    ) where
        F: Fn(u32) -> u32,
    {
        while transfers.len() < max_pieces {
            let chunk_idx = match ctx.chunk_selector.find(
                peer.peer_bf,
                ctx.peer_cache,
                peer.is_seeder,
                ctx.stats,
            ) {
                Some(idx) => idx,
                None => return,
            };

            ctx.chunk_selector.using_index(chunk_idx);

            let chunk_len = chunk_size_fn(chunk_idx);
            let piece = Piece::new(chunk_idx, 0, chunk_len);

            let bl = self.transfers.insert(piece, DEFAULT_BLOCK_SIZE);
            bl.set_by_seeder(peer.is_seeder);
            bl.set_priority(if high_priority {
                Priority::High
            } else {
                Priority::Normal
            });

            self.piece_speed_class.insert(chunk_idx, peer.speed_class);

            Self::delegate_from_blocklist(transfers, max_pieces, bl, peer.peer_id);
        }
    }

    fn delegate_from_blocklist(
        transfers: &mut Vec<Piece>,
        max_pieces: usize,
        bl: &mut BlockList,
        peer_id: usize,
    ) {
        for block in bl.blocks_mut() {
            if transfers.len() >= max_pieces {
                return;
            }
            if !block.is_finished() && block.is_stalled() && block.size_all() == 0 {
                let _idx = block.insert(peer_id);
                transfers.push(*block.piece());
            }
        }

        if transfers.len() >= max_pieces {
            return;
        }

        for block in bl.blocks_mut() {
            if transfers.len() >= max_pieces {
                return;
            }
            if !block.is_finished() && block.is_stalled() {
                let _idx = block.insert(peer_id);
                transfers.push(*block.piece());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_delegator_basic() {
        let mut delegator = Delegator::new();
        let mut selector = ChunkSelector::new(4);
        let stats = ChunkStatistics::new(4);
        let mut peer_cache = PartialQueue::new();

        let mut peer_bf = Bitfield::new(4);
        peer_bf.set(1);

        let delegated = delegator.delegate(
            DelegatePeer {
                peer_id: 100,
                peer_bf: &peer_bf,
                is_seeder: false,
                affinity: None,
                speed_class: PeerSpeedClass::Medium,
                is_fastest: false,
            },
            &mut DelegateCtx {
                chunk_selector: &mut selector,
                stats: &stats,
                peer_cache: &mut peer_cache,
            },
            2,
            |_| 32768, // 32KB = 2 blocks
        );

        assert_eq!(delegated.len(), 2);
        assert_eq!(delegated[0].index(), 1);
        assert_eq!(delegated[0].offset(), 0);
        assert_eq!(delegated[0].length(), 16384);
        assert_eq!(delegated[1].index(), 1);
        assert_eq!(delegated[1].offset(), 16384);
        assert_eq!(delegated[1].length(), 16384);

        assert_eq!(delegator.transfers().len(), 1);
    }

    #[test]
    fn test_piece_affinity_and_endgame() {
        let mut delegator = Delegator::new();
        let mut selector = ChunkSelector::new(4);
        let stats = ChunkStatistics::new(4);
        let mut peer_cache = PartialQueue::new();

        let mut peer_fast_bf = Bitfield::new(4);
        peer_fast_bf.set(2);

        // Fast peer starts piece 2
        let delegated_fast = delegator.delegate(
            DelegatePeer {
                peer_id: 1,
                peer_bf: &peer_fast_bf,
                is_seeder: false,
                affinity: None,
                speed_class: PeerSpeedClass::Fast,
                is_fastest: true,
            },
            &mut DelegateCtx {
                chunk_selector: &mut selector,
                stats: &stats,
                peer_cache: &mut peer_cache,
            },
            1,
            |_| 32768, // 2 blocks total, so 1 block remains
        );
        assert_eq!(delegated_fast.len(), 1);

        // Slow peer tries to pick piece 2; outside Endgame, piece affinity protects it
        let mut peer_slow_bf = Bitfield::new(4);
        peer_slow_bf.set(2);
        let mut slow_cache = PartialQueue::new();
        let delegated_slow = delegator.delegate(
            DelegatePeer {
                peer_id: 2,
                peer_bf: &peer_slow_bf,
                is_seeder: false,
                affinity: None,
                speed_class: PeerSpeedClass::Slow,
                is_fastest: false,
            },
            &mut DelegateCtx {
                chunk_selector: &mut selector,
                stats: &stats,
                peer_cache: &mut slow_cache,
            },
            1,
            |_| 32768,
        );
        // Slow peer was not assigned the remaining block from Fast peer's piece
        assert_eq!(delegated_slow.len(), 0);

        // When evaluating endgame:
        assert!(delegator.evaluate_endgame(0, 1, 10));
        assert!(delegator.is_aggressive());
    }
}
