use lt_common::bitfield::Bitfield;
use lt_common::collections::PartialQueue;
use lt_storage::block::{BlockList, TransferList, DEFAULT_BLOCK_SIZE};
use lt_storage::file::Priority;
use lt_storage::piece::Piece;
use crate::chunk_selector::ChunkSelector;
use crate::chunk_statistics::ChunkStatistics;

/// Delegator orchestrates assigning blocks to peer download pipelines and activates Endgame mode.
#[derive(Debug, Default)]
pub struct Delegator {
    transfers: TransferList,
    aggressive: bool,
}

impl Delegator {
    pub fn new() -> Self {
        Self {
            transfers: TransferList::new(),
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
    pub fn cancel_block(
        &mut self,
        peer_id: usize,
        piece_index: u32,
        block_offset: u32,
    ) -> bool {
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

    /// Delegates up to `max_pieces` blocks for a peer.
    /// Returns the allocated block transfers (piece, offset, length, peer_id).
    pub fn delegate<F>(
        &mut self,
        peer_id: usize,
        peer_bf: &Bitfield,
        is_seeder: bool,
        affinity: Option<u32>,
        max_pieces: usize,
        chunk_selector: &mut ChunkSelector,
        stats: &ChunkStatistics,
        peer_cache: &mut PartialQueue,
        chunk_size_fn: F,
    ) -> Vec<Piece>
    where
        F: Fn(u32) -> u32,
    {
        let mut new_transfers = Vec::new();

        // 1. Check affinity (piece currently being downloaded by this peer)
        if let Some(aff_idx) = affinity {
            if let Some(bl) = self.transfers.find_mut(aff_idx) {
                Self::delegate_from_blocklist(&mut new_transfers, max_pieces, bl, peer_id);
                if new_transfers.len() >= max_pieces {
                    return new_transfers;
                }
            }
        }

        // 2. Prioritize full seeders
        if is_seeder {
            for bl in self.transfers.iter_mut() {
                if new_transfers.len() >= max_pieces {
                    return new_transfers;
                }
                if bl.by_seeder() {
                    Self::delegate_from_blocklist(&mut new_transfers, max_pieces, bl, peer_id);
                }
            }

            self.delegate_new_chunks(
                &mut new_transfers,
                max_pieces,
                peer_id,
                peer_bf,
                is_seeder,
                true,
                chunk_selector,
                stats,
                peer_cache,
                &chunk_size_fn,
            );
            self.delegate_new_chunks(
                &mut new_transfers,
                max_pieces,
                peer_id,
                peer_bf,
                is_seeder,
                false,
                chunk_selector,
                stats,
                peer_cache,
                &chunk_size_fn,
            );
        }

        if new_transfers.len() >= max_pieces {
            return new_transfers;
        }

        // 3. Find existing high-priority pieces
        for bl in self.transfers.iter_mut() {
            if new_transfers.len() >= max_pieces {
                return new_transfers;
            }
            if bl.priority() == Priority::High && peer_bf.get(bl.index() as usize) {
                Self::delegate_from_blocklist(&mut new_transfers, max_pieces, bl, peer_id);
            }
        }

        // 4. Create new high-priority pieces
        self.delegate_new_chunks(
            &mut new_transfers,
            max_pieces,
            peer_id,
            peer_bf,
            is_seeder,
            true,
            chunk_selector,
            stats,
            peer_cache,
            &chunk_size_fn,
        );

        if new_transfers.len() >= max_pieces {
            return new_transfers;
        }

        // 5. Find existing normal-priority pieces
        for bl in self.transfers.iter_mut() {
            if new_transfers.len() >= max_pieces {
                return new_transfers;
            }
            if bl.priority() == Priority::Normal && peer_bf.get(bl.index() as usize) {
                Self::delegate_from_blocklist(&mut new_transfers, max_pieces, bl, peer_id);
            }
        }

        // 6. Create new normal-priority pieces
        self.delegate_new_chunks(
            &mut new_transfers,
            max_pieces,
            peer_id,
            peer_bf,
            is_seeder,
            false,
            chunk_selector,
            stats,
            peer_cache,
            &chunk_size_fn,
        );

        if !self.aggressive || new_transfers.len() >= max_pieces {
            return new_transfers;
        }

        // 7. Endgame (aggressive) mode: duplicate requests across peers for unfinished blocks
        let mut overlapped = 5;
        for bl in self.transfers.iter_mut() {
            if new_transfers.len() >= max_pieces {
                break;
            }
            if peer_bf.get(bl.index() as usize) && bl.priority() != Priority::Off {
                for block in bl.blocks_mut() {
                    if new_transfers.len() >= max_pieces || block.size_not_stalled() >= overlapped {
                        break;
                    }
                    if !block.is_finished() {
                        let _idx = block.insert(peer_id);
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
        peer_id: usize,
        peer_bf: &Bitfield,
        is_seeder: bool,
        high_priority: bool,
        chunk_selector: &mut ChunkSelector,
        stats: &ChunkStatistics,
        peer_cache: &mut PartialQueue,
        chunk_size_fn: &F,
    ) where
        F: Fn(u32) -> u32,
    {
        while transfers.len() < max_pieces {
            let chunk_idx = match chunk_selector.find(peer_bf, peer_cache, is_seeder, stats) {
                Some(idx) => idx,
                None => return,
            };

            chunk_selector.using_index(chunk_idx);

            let chunk_len = chunk_size_fn(chunk_idx);
            let piece = Piece::new(chunk_idx, 0, chunk_len);

            let bl = self.transfers.insert(piece, DEFAULT_BLOCK_SIZE);
            bl.set_by_seeder(is_seeder);
            bl.set_priority(if high_priority {
                Priority::High
            } else {
                Priority::Normal
            });

            Self::delegate_from_blocklist(transfers, max_pieces, bl, peer_id);
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
            100,
            &peer_bf,
            false,
            None,
            2,
            &mut selector,
            &stats,
            &mut peer_cache,
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
}
