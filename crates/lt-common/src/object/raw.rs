use crate::object::BencodeError;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RawBencode(pub Vec<u8>);

impl RawBencode {
    pub fn new(data: impl Into<Vec<u8>>) -> Self {
        Self(data.into())
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn size(&self) -> usize {
        self.0.len()
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    pub fn is_value(&self) -> bool {
        self.0.len() >= 3 && self.0[0] == b'i'
    }

    pub fn is_raw_string(&self) -> bool {
        self.0.len() >= 2 && self.0[0].is_ascii_digit()
    }

    pub fn is_raw_list(&self) -> bool {
        self.0.len() >= 2 && self.0[0] == b'l'
    }

    pub fn is_raw_map(&self) -> bool {
        self.0.len() >= 2 && self.0[0] == b'd'
    }

    pub fn as_value_string(&self) -> Result<&[u8], BencodeError> {
        if !self.is_value() || self.0.last() != Some(&b'e') {
            return Err(BencodeError::WrongType {
                expected: "value",
                actual: "non-value",
            });
        }
        Ok(&self.0[1..self.0.len() - 1])
    }

    pub fn as_raw_string(&self) -> Result<RawString, BencodeError> {
        if !self.is_raw_string() {
            return Err(BencodeError::WrongType {
                expected: "string",
                actual: "non-string",
            });
        }
        let colon_pos =
            self.0.iter().position(|&b| b == b':').ok_or_else(|| {
                BencodeError::InvalidData("Invalid bencode in raw_bencode".into())
            })?;
        Ok(RawString(self.0[colon_pos + 1..].to_vec()))
    }

    pub fn as_raw_list(&self) -> Result<RawList, BencodeError> {
        if !self.is_raw_list() || self.0.last() != Some(&b'e') {
            return Err(BencodeError::WrongType {
                expected: "list",
                actual: "non-list",
            });
        }
        Ok(RawList(self.0[1..self.0.len() - 1].to_vec()))
    }

    pub fn as_raw_map(&self) -> Result<RawMap, BencodeError> {
        if !self.is_raw_map() || self.0.last() != Some(&b'e') {
            return Err(BencodeError::WrongType {
                expected: "map",
                actual: "non-map",
            });
        }
        Ok(RawMap(self.0[1..self.0.len() - 1].to_vec()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RawString(pub Vec<u8>);

impl RawString {
    pub fn new(data: impl Into<Vec<u8>>) -> Self {
        Self(data.into())
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn size(&self) -> usize {
        self.0.len()
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    pub fn as_str(&self) -> Result<&str, BencodeError> {
        std::str::from_utf8(&self.0)
            .map_err(|e| BencodeError::InvalidData(format!("Invalid UTF-8 in RawString: {e}")))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RawList(pub Vec<u8>);

impl RawList {
    pub fn new(data: impl Into<Vec<u8>>) -> Self {
        Self(data.into())
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn size(&self) -> usize {
        self.0.len()
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RawMap(pub Vec<u8>);

impl RawMap {
    pub fn new(data: impl Into<Vec<u8>>) -> Self {
        Self(data.into())
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn size(&self) -> usize {
        self.0.len()
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}
