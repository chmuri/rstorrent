use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ProtocolError {
    #[error("ProtocolBuffer out of range consume length: {0}")]
    OutOfRangeConsume(isize),
    #[error("ProtocolBuffer read beyond end of buffer")]
    ReadBeyondEnd,
    #[error("ProtocolBuffer write beyond capacity")]
    WriteBeyondCapacity,
}

#[derive(Clone, Debug)]
pub struct ProtocolBuffer<const N: usize> {
    buffer: [u8; N],
    position: usize,
    end: usize,
}

impl<const N: usize> Default for ProtocolBuffer<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> ProtocolBuffer<N> {
    pub const fn new() -> Self {
        Self {
            buffer: [0u8; N],
            position: 0,
            end: 0,
        }
    }

    pub fn reset(&mut self) {
        self.position = 0;
        self.end = 0;
    }

    pub fn reset_position(&mut self) {
        self.position = 0;
    }

    pub fn consume(&mut self, v: isize) -> Result<bool, ProtocolError> {
        if v < 0 || (v as usize) > self.remaining() {
            return Err(ProtocolError::OutOfRangeConsume(v));
        }

        self.position += v as usize;
        Ok(self.remaining() == 0)
    }

    pub fn set_end(&mut self, end: usize) {
        self.end = end.min(N);
    }

    pub fn remaining(&self) -> usize {
        if self.end >= self.position {
            self.end - self.position
        } else {
            0
        }
    }

    pub fn reserved(&self) -> usize {
        N
    }

    pub fn reserved_left(&self) -> usize {
        N.saturating_sub(self.end)
    }

    pub fn size_position(&self) -> usize {
        self.position
    }

    pub fn size_end(&self) -> usize {
        self.end
    }

    pub fn as_slice(&self) -> &[u8] {
        &self.buffer[self.position..self.end]
    }

    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        &mut self.buffer[self.position..self.end]
    }

    pub fn read_8(&mut self) -> Result<u8, ProtocolError> {
        if self.remaining() < 1 {
            return Err(ProtocolError::ReadBeyondEnd);
        }
        let b = self.buffer[self.position];
        self.position += 1;
        Ok(b)
    }

    pub fn peek_8(&self) -> Result<u8, ProtocolError> {
        if self.remaining() < 1 {
            return Err(ProtocolError::ReadBeyondEnd);
        }
        Ok(self.buffer[self.position])
    }

    pub fn read_16(&mut self) -> Result<u16, ProtocolError> {
        if self.remaining() < 2 {
            return Err(ProtocolError::ReadBeyondEnd);
        }
        let v = u16::from_be_bytes([self.buffer[self.position], self.buffer[self.position + 1]]);
        self.position += 2;
        Ok(v)
    }

    pub fn peek_16(&self) -> Result<u16, ProtocolError> {
        if self.remaining() < 2 {
            return Err(ProtocolError::ReadBeyondEnd);
        }
        Ok(u16::from_be_bytes([
            self.buffer[self.position],
            self.buffer[self.position + 1],
        ]))
    }

    pub fn read_32(&mut self) -> Result<u32, ProtocolError> {
        if self.remaining() < 4 {
            return Err(ProtocolError::ReadBeyondEnd);
        }
        let v = u32::from_be_bytes([
            self.buffer[self.position],
            self.buffer[self.position + 1],
            self.buffer[self.position + 2],
            self.buffer[self.position + 3],
        ]);
        self.position += 4;
        Ok(v)
    }

    pub fn peek_32(&self) -> Result<u32, ProtocolError> {
        if self.remaining() < 4 {
            return Err(ProtocolError::ReadBeyondEnd);
        }
        Ok(u32::from_be_bytes([
            self.buffer[self.position],
            self.buffer[self.position + 1],
            self.buffer[self.position + 2],
            self.buffer[self.position + 3],
        ]))
    }

    pub fn read_64(&mut self) -> Result<u64, ProtocolError> {
        if self.remaining() < 8 {
            return Err(ProtocolError::ReadBeyondEnd);
        }
        let mut bytes = [0u8; 8];
        bytes.copy_from_slice(&self.buffer[self.position..self.position + 8]);
        self.position += 8;
        Ok(u64::from_be_bytes(bytes))
    }

    pub fn peek_64(&self) -> Result<u64, ProtocolError> {
        if self.remaining() < 8 {
            return Err(ProtocolError::ReadBeyondEnd);
        }
        let mut bytes = [0u8; 8];
        bytes.copy_from_slice(&self.buffer[self.position..self.position + 8]);
        Ok(u64::from_be_bytes(bytes))
    }

    pub fn write_8(&mut self, v: u8) -> Result<(), ProtocolError> {
        if self.reserved_left() < 1 {
            return Err(ProtocolError::WriteBeyondCapacity);
        }
        self.buffer[self.end] = v;
        self.end += 1;
        Ok(())
    }

    pub fn write_16(&mut self, v: u16) -> Result<(), ProtocolError> {
        if self.reserved_left() < 2 {
            return Err(ProtocolError::WriteBeyondCapacity);
        }
        let bytes = v.to_be_bytes();
        self.buffer[self.end..self.end + 2].copy_from_slice(&bytes);
        self.end += 2;
        Ok(())
    }

    pub fn write_32(&mut self, v: u32) -> Result<(), ProtocolError> {
        if self.reserved_left() < 4 {
            return Err(ProtocolError::WriteBeyondCapacity);
        }
        let bytes = v.to_be_bytes();
        self.buffer[self.end..self.end + 4].copy_from_slice(&bytes);
        self.end += 4;
        Ok(())
    }

    pub fn write_64(&mut self, v: u64) -> Result<(), ProtocolError> {
        if self.reserved_left() < 8 {
            return Err(ProtocolError::WriteBeyondCapacity);
        }
        let bytes = v.to_be_bytes();
        self.buffer[self.end..self.end + 8].copy_from_slice(&bytes);
        self.end += 8;
        Ok(())
    }

    pub fn write_slice(&mut self, src: &[u8]) -> Result<(), ProtocolError> {
        if self.reserved_left() < src.len() {
            return Err(ProtocolError::WriteBeyondCapacity);
        }
        self.buffer[self.end..self.end + src.len()].copy_from_slice(src);
        self.end += src.len();
        Ok(())
    }

    pub fn read_slice(&mut self, dest: &mut [u8]) -> Result<(), ProtocolError> {
        if self.remaining() < dest.len() {
            return Err(ProtocolError::ReadBeyondEnd);
        }
        dest.copy_from_slice(&self.buffer[self.position..self.position + dest.len()]);
        self.position += dest.len();
        Ok(())
    }

    pub fn move_unused(&mut self) {
        let rem = self.remaining();
        if rem > 0 && self.position > 0 {
            self.buffer.copy_within(self.position..self.end, 0);
        }
        self.position = 0;
        self.end = rem;
    }
}
