pub const INVALID_PIECE_INDEX: u32 = u32::MAX;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Piece {
    index: u32,
    offset: u32,
    length: u32,
}

impl Piece {
    pub fn new(index: u32, offset: u32, length: u32) -> Self {
        Self {
            index,
            offset,
            length,
        }
    }

    pub fn is_valid(&self) -> bool {
        self.index != INVALID_PIECE_INDEX
    }

    pub fn index(&self) -> u32 {
        self.index
    }

    pub fn set_index(&mut self, index: u32) {
        self.index = index;
    }

    pub fn offset(&self) -> u32 {
        self.offset
    }

    pub fn set_offset(&mut self, offset: u32) {
        self.offset = offset;
    }

    pub fn length(&self) -> u32 {
        self.length
    }

    pub fn set_length(&mut self, length: u32) {
        self.length = length;
    }
}
