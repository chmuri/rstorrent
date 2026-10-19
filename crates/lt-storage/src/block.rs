use crate::file::Priority;
use crate::piece::Piece;

pub const DEFAULT_BLOCK_SIZE: u32 = 16384; // 16 KiB standard BitTorrent block size

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BlockState {
    #[default]
    Incomplete,
    Completed,
    Invalid,
}

#[derive(Debug, Clone)]
pub struct BlockTransfer {
    peer_id: usize,
    piece: Piece,
    position: u32,
    is_leader: bool,
    is_erased: bool,
    is_stalled: bool,
}

impl BlockTransfer {
    pub fn new(peer_id: usize, piece: Piece) -> Self {
        Self {
            peer_id,
            piece,
            position: 0,
            is_leader: false,
            is_erased: false,
            is_stalled: false,
        }
    }

    pub fn peer_id(&self) -> usize {
        self.peer_id
    }

    pub fn piece(&self) -> &Piece {
        &self.piece
    }

    pub fn position(&self) -> u32 {
        self.position
    }

    pub fn adjust_position(&mut self, pos: u32) {
        self.position = pos;
    }

    pub fn is_leader(&self) -> bool {
        self.is_leader
    }

    pub fn is_not_leader(&self) -> bool {
        !self.is_leader
    }

    pub fn is_erased(&self) -> bool {
        self.is_erased
    }

    pub fn is_valid(&self) -> bool {
        !self.is_erased
    }

    pub fn is_stalled(&self) -> bool {
        self.is_stalled
    }
}

#[derive(Debug, Clone)]
pub struct Block {
    piece: Piece,
    state: BlockState,
    not_stalled: u32,
    transfers: Vec<BlockTransfer>,
    leader_idx: Option<usize>,
}

impl Block {
    pub fn new(piece: Piece) -> Self {
        Self {
            piece,
            state: BlockState::Incomplete,
            not_stalled: 0,
            transfers: Vec::new(),
            leader_idx: None,
        }
    }

    pub fn piece(&self) -> &Piece {
        &self.piece
    }

    pub fn state(&self) -> BlockState {
        self.state
    }

    pub fn is_finished(&self) -> bool {
        self.state == BlockState::Completed
    }

    pub fn size_not_stalled(&self) -> u32 {
        self.not_stalled
    }

    pub fn is_stalled(&self) -> bool {
        self.not_stalled == 0
    }

    pub fn transfers(&self) -> &[BlockTransfer] {
        &self.transfers
    }

    pub fn size_all(&self) -> usize {
        self.transfers.len()
    }


    pub fn insert(&mut self, peer_id: usize) -> usize {
        let mut transfer = BlockTransfer::new(peer_id, self.piece);
        let idx = self.transfers.len();
        if self.leader_idx.is_none() {
            transfer.is_leader = true;
            self.leader_idx = Some(idx);
        }
        self.transfers.push(transfer);
        self.not_stalled += 1;
        idx
    }

    pub fn transfering(&self, transfer_idx: usize) -> bool {
        self.leader_idx == Some(transfer_idx)
    }

    pub fn transfer_dissimilar(&mut self, transfer_idx: usize) {
        if let Some(t) = self.transfers.get_mut(transfer_idx) {
            if !t.is_erased {
                t.is_erased = true;
                if self.not_stalled > 0 {
                    self.not_stalled -= 1;
                }
            }
        }
    }

    pub fn completed(&mut self, transfer_idx: usize) -> bool {
        if self.leader_idx == Some(transfer_idx) {
            self.state = BlockState::Completed;
            true
        } else {
            false
        }
    }

    pub fn get_transfer(&self, idx: usize) -> Option<&BlockTransfer> {
        self.transfers.get(idx)
    }

    pub fn get_transfer_mut(&mut self, idx: usize) -> Option<&mut BlockTransfer> {
        self.transfers.get_mut(idx)
    }
}

#[derive(Debug)]
pub struct BlockList {
    piece: Piece,
    blocks: Vec<Block>,
    priority: Priority,
    finished: usize,
    failed: u32,
    attempt: u32,
    by_seeder: bool,
}

impl BlockList {
    pub fn new(piece: Piece, block_length: u32) -> Self {
        let bl = if block_length == 0 { DEFAULT_BLOCK_SIZE } else { block_length };
        let mut blocks = Vec::new();

        let mut offset = 0;
        let total_len = piece.length();

        while offset < total_len {
            let len = bl.min(total_len - offset);
            let sub_piece = Piece::new(piece.index(), piece.offset() + offset, len);
            blocks.push(Block::new(sub_piece));
            offset += len;
        }

        Self {
            piece,
            blocks,
            priority: Priority::Normal,
            finished: 0,
            failed: 0,
            attempt: 0,
            by_seeder: false,
        }
    }

    pub fn piece(&self) -> &Piece {
        &self.piece
    }

    pub fn index(&self) -> u32 {
        self.piece.index()
    }

    pub fn len(&self) -> usize {
        self.blocks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }

    pub fn is_all_finished(&self) -> bool {
        !self.blocks.is_empty() && self.finished == self.blocks.len()
    }

    pub fn finished(&self) -> usize {
        self.finished
    }

    pub fn inc_finished(&mut self) {
        self.finished += 1;
    }

    pub fn clear_finished(&mut self) {
        self.finished = 0;
    }

    pub fn priority(&self) -> Priority {
        self.priority
    }

    pub fn set_priority(&mut self, p: Priority) {
        self.priority = p;
    }

    pub fn failed(&self) -> u32 {
        self.failed
    }

    pub fn inc_failed(&mut self) {
        self.failed += 1;
    }

    pub fn attempt(&self) -> u32 {
        self.attempt
    }

    pub fn set_attempt(&mut self, a: u32) {
        self.attempt = a;
    }

    pub fn by_seeder(&self) -> bool {
        self.by_seeder
    }

    pub fn set_by_seeder(&mut self, state: bool) {
        self.by_seeder = state;
    }

    pub fn get(&self, index: usize) -> Option<&Block> {
        self.blocks.get(index)
    }

    pub fn get_mut(&mut self, index: usize) -> Option<&mut Block> {
        self.blocks.get_mut(index)
    }

    pub fn blocks(&self) -> &[Block] {
        &self.blocks
    }

    pub fn blocks_mut(&mut self) -> &mut [Block] {
        &mut self.blocks
    }
}

impl std::ops::Index<usize> for BlockList {
    type Output = Block;
    fn index(&self, index: usize) -> &Self::Output {
        &self.blocks[index]
    }
}

impl std::ops::IndexMut<usize> for BlockList {
    fn index_mut(&mut self, index: usize) -> &mut Self::Output {
        &mut self.blocks[index]
    }
}

#[derive(Debug, Default)]
pub struct TransferList {
    transfers: Vec<BlockList>,
    succeeded_count: u32,
    failed_count: u32,
}

impl TransferList {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.transfers.len()
    }

    pub fn is_empty(&self) -> bool {
        self.transfers.is_empty()
    }

    pub fn clear(&mut self) {
        self.transfers.clear();
    }

    pub fn find(&self, index: u32) -> Option<&BlockList> {
        self.transfers.iter().find(|b| b.index() == index)
    }

    pub fn find_mut(&mut self, index: u32) -> Option<&mut BlockList> {
        self.transfers.iter_mut().find(|b| b.index() == index)
    }

    pub fn insert(&mut self, piece: Piece, block_size: u32) -> &mut BlockList {
        let bl = BlockList::new(piece, block_size);
        self.transfers.push(bl);
        self.transfers.last_mut().unwrap()
    }

    pub fn erase(&mut self, index: u32) -> Option<BlockList> {
        if let Some(pos) = self.transfers.iter().position(|b| b.index() == index) {
            Some(self.transfers.remove(pos))
        } else {
            None
        }
    }

    pub fn iter(&self) -> std::slice::Iter<'_, BlockList> {
        self.transfers.iter()
    }

    pub fn iter_mut(&mut self) -> std::slice::IterMut<'_, BlockList> {
        self.transfers.iter_mut()
    }

    pub fn succeeded_count(&self) -> u32 {
        self.succeeded_count
    }

    pub fn failed_count(&self) -> u32 {
        self.failed_count
    }

    pub fn inc_succeeded(&mut self) {
        self.succeeded_count += 1;
    }

    pub fn inc_failed(&mut self) {
        self.failed_count += 1;
    }
}

