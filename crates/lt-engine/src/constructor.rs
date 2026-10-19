use lt_common::hash_string::HashString;
use lt_common::object::Object;
use lt_common::path::Path;
use lt_crypto::sha1::sha1_hash;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConstructorError {
    #[error("Invalid magnet URI: {0}")]
    InvalidMagnet(String),
    #[error("Missing or invalid info dictionary")]
    InvalidInfoDict,
    #[error("Invalid piece length: {0}")]
    InvalidPieceLength(i64),
    #[error("Pieces byte length {0} is not a multiple of 20")]
    InvalidPiecesLength(usize),
    #[error("Torrent must have either 'length' or 'files' entry")]
    MissingFilesEntry,
    #[error("Invalid file entry: {0}")]
    InvalidFileEntry(String),
    #[error("Torrent has zero length")]
    ZeroLengthTorrent,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TorrentFileEntry {
    pub path: Path,
    pub length: u64,
    pub offset: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TorrentMetadata {
    pub info_hash: HashString,
    pub name: String,
    pub piece_length: u32,
    pub pieces: Vec<u8>,
    pub num_pieces: usize,
    pub total_size: u64,
    pub is_multi_file: bool,
    pub files: Vec<TorrentFileEntry>,
    pub announce: Option<String>,
    pub announce_list: Vec<Vec<String>>,
    pub is_private: bool,
    pub is_meta_download: bool,
}

impl TorrentMetadata {
    pub fn piece_hash(&self, index: usize) -> Option<[u8; 20]> {
        if index < self.num_pieces {
            let start = index * 20;
            let end = start + 20;
            let mut h = [0u8; 20];
            h.copy_from_slice(&self.pieces[start..end]);
            Some(h)
        } else {
            None
        }
    }
}

pub struct DownloadConstructor;

impl DownloadConstructor {
    pub fn sanitize_name(name: &str) -> String {
        name.replace('/', "_")
    }

    fn get_str<'a>(obj: &'a Object, key: &str) -> Option<&'a str> {
        obj.get_key_string(key).ok().and_then(|b| std::str::from_utf8(b).ok())
    }

    fn get_int(obj: &Object, key: &str) -> Option<i64> {
        obj.get_key_value(key).ok()
    }

    fn get_bytes<'a>(obj: &'a Object, key: &str) -> Option<&'a [u8]> {
        obj.get_key_string(key).ok()
    }

    pub fn from_object(b: &Object) -> Result<TorrentMetadata, ConstructorError> {
        let info_obj = b.get_key("info").map_err(|_| ConstructorError::InvalidInfoDict)?;

        // Calculate info hash: serialize the info dictionary back to bencode and hash it
        let raw_info_bytes = lt_common::object::bencode::encode(info_obj)
            .map_err(|_| ConstructorError::InvalidInfoDict)?;
        let info_hash = HashString::new(sha1_hash(&raw_info_bytes));

        let name = if let Some(n_utf8) = Self::get_str(info_obj, "name.utf-8") {
            Self::sanitize_name(n_utf8)
        } else if let Some(n) = Self::get_str(info_obj, "name") {
            Self::sanitize_name(n)
        } else {
            "unnamed".to_string()
        };

        let is_meta_download = Self::get_int(info_obj, "meta_download")
            .map(|v| v != 0)
            .unwrap_or(false);

        let piece_length = if is_meta_download {
            1
        } else {
            let pl = Self::get_int(info_obj, "piece length")
                .ok_or(ConstructorError::InvalidPieceLength(0))?;
            if pl < 1024 || pl > (512 * 1024 * 1024) {
                return Err(ConstructorError::InvalidPieceLength(pl));
            }
            pl as u32
        };

        let pieces_bytes = Self::get_bytes(info_obj, "pieces")
            .ok_or(ConstructorError::InvalidInfoDict)?
            .to_vec();

        if pieces_bytes.len() % 20 != 0 {
            return Err(ConstructorError::InvalidPiecesLength(pieces_bytes.len()));
        }
        let num_pieces = pieces_bytes.len() / 20;

        let mut files = Vec::new();
        let mut total_size = 0u64;
        let mut is_multi_file = false;

        if let Some(single_len) = Self::get_int(info_obj, "length") {
            if single_len < 0 {
                return Err(ConstructorError::InvalidFileEntry("Negative length".into()));
            }
            let len = single_len as u64;
            let mut p = Path::new();
            p.push(&name);
            files.push(TorrentFileEntry {
                path: p,
                length: len,
                offset: 0,
            });
            total_size = len;
        } else if let Ok(files_list) = info_obj.get_key_list("files") {
            is_multi_file = true;
            let mut current_offset = 0u64;

            for file_obj in files_list {
                let file_len = Self::get_int(file_obj, "length")
                    .ok_or_else(|| ConstructorError::InvalidFileEntry("Missing length".into()))?;
                if file_len < 0 {
                    return Err(ConstructorError::InvalidFileEntry("Negative length".into()));
                }
                let len = file_len as u64;

                let path_list = if let Ok(pl) = file_obj.get_key_list("path.utf-8") {
                    pl
                } else if let Ok(pl) = file_obj.get_key_list("path") {
                    pl
                } else {
                    return Err(ConstructorError::InvalidFileEntry("Missing path".into()));
                };

                let mut p = Path::new();
                p.push(&name);
                for component in path_list {
                    if let Ok(b) = component.as_string() {
                        if let Ok(s) = std::str::from_utf8(b) {
                            p.push(&Self::sanitize_name(s));
                        }
                    }
                }

                files.push(TorrentFileEntry {
                    path: p,
                    length: len,
                    offset: current_offset,
                });
                current_offset += len;
            }
            total_size = current_offset;
        } else if !is_meta_download {
            return Err(ConstructorError::MissingFilesEntry);
        }

        if total_size == 0 && !is_meta_download {
            return Err(ConstructorError::ZeroLengthTorrent);
        }

        let is_private = Self::get_int(info_obj, "private")
            .map(|p| p == 1)
            .unwrap_or(false);

        let announce = Self::get_str(b, "announce").map(|s| s.to_string());
        let mut announce_list = Vec::new();

        if let Ok(tiers) = b.get_key_list("announce-list") {
            for tier in tiers {
                if let Ok(trackers) = tier.as_list() {
                    let tier_urls: Vec<String> = trackers
                        .iter()
                        .filter_map(|t| t.as_string().ok().and_then(|b| std::str::from_utf8(b).ok()).map(|s| s.trim().to_string()))
                        .collect();
                    if !tier_urls.is_empty() {
                        announce_list.push(tier_urls);
                    }
                }
            }
        } else if let Some(ann) = &announce {
            announce_list.push(vec![ann.clone()]);
        }

        Ok(TorrentMetadata {
            info_hash,
            name,
            piece_length,
            pieces: pieces_bytes,
            num_pieces,
            total_size,
            is_multi_file,
            files,
            announce,
            announce_list,
            is_private,
            is_meta_download,
        })
    }

    /// Builds a torrent from the raw (already verified) bencoded `info`
    /// dictionary, e.g. after a successful `ut_metadata` download.
    pub fn from_info_bytes(
        info_bytes: &[u8],
        info_hash: HashString,
        announce: Option<String>,
        announce_list: Vec<Vec<String>>,
    ) -> Result<TorrentMetadata, ConstructorError> {
        let info_obj = lt_common::object::bencode::decode(info_bytes)
            .map_err(|_| ConstructorError::InvalidInfoDict)?;

        let mut root = Object::create_map();
        root.insert_key("info", info_obj)
            .map_err(|_| ConstructorError::InvalidInfoDict)?;

        let mut metadata = Self::from_object(&root)?;
        metadata.info_hash = info_hash;
        metadata.announce = announce;
        metadata.announce_list = announce_list;
        metadata.is_meta_download = false;
        Ok(metadata)
    }

    pub fn from_magnet_uri(uri: &str) -> Result<TorrentMetadata, ConstructorError> {
        if !uri.starts_with("magnet:?") {
            return Err(ConstructorError::InvalidMagnet("Not a magnet URI".into()));
        }

        let query = &uri[8..];
        let mut info_hash_opt = None;
        let mut name = "magnet_download".to_string();
        let mut trackers = Vec::new();

        for param in query.split('&') {
            if let Some((key, val)) = param.split_once('=') {
                if key == "xt" && val.starts_with("urn:btih:") {
                    let hash_str = &val[9..];
                    if hash_str.len() == 40 {
                        // 40-char Hex
                        let hash = HashString::from_hex(hash_str)
                            .ok_or_else(|| ConstructorError::InvalidMagnet("Invalid hex hash".into()))?;
                        info_hash_opt = Some(hash);
                    } else if hash_str.len() == 32 {
                        // 32-char Base32
                        let hash = Self::decode_base32(hash_str)
                            .ok_or_else(|| ConstructorError::InvalidMagnet("Invalid base32 hash".into()))?;
                        info_hash_opt = Some(hash);
                    }
                } else if key == "dn" {
                    name = Self::percent_decode(val);
                } else if key == "tr" {
                    let tracker_url = Self::percent_decode(val);
                    if !tracker_url.is_empty() {
                        trackers.push(tracker_url);
                    }
                }
            }
        }

        let info_hash = info_hash_opt.ok_or_else(|| ConstructorError::InvalidMagnet("Missing xt urn:btih".into()))?;

        let announce = trackers.first().cloned();
        let announce_list = if trackers.is_empty() {
            Vec::new()
        } else {
            vec![trackers]
        };

        Ok(TorrentMetadata {
            info_hash,
            name,
            piece_length: 16384,
            pieces: info_hash.as_bytes().to_vec(),
            num_pieces: 1,
            total_size: 0,
            is_multi_file: false,
            files: Vec::new(),
            announce,
            announce_list,
            is_private: false,
            is_meta_download: true,
        })
    }

    fn percent_decode(s: &str) -> String {
        let mut result = Vec::new();
        let bytes = s.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'%' && i + 2 < bytes.len() {
                if let (Ok(h), Ok(l)) = (
                    std::str::from_utf8(&bytes[i + 1..i + 2]),
                    std::str::from_utf8(&bytes[i + 2..i + 3]),
                ) {
                    if let (Ok(high), Ok(low)) = (u8::from_str_radix(h, 16), u8::from_str_radix(l, 16)) {
                        result.push((high << 4) | low);
                        i += 3;
                        continue;
                    }
                }
            }
            if bytes[i] == b'+' {
                result.push(b' ');
            } else {
                result.push(bytes[i]);
            }
            i += 1;
        }
        String::from_utf8_lossy(&result).to_string()
    }

    fn decode_base32(s: &str) -> Option<HashString> {
        let mut bytes = [0u8; 20];
        let mut byte_idx = 0;
        let mut shift = 11;
        let mut decoded: u16 = 0;

        for &c in s.as_bytes() {
            let val: u16 = match c {
                b'A'..=b'Z' => (c - b'A') as u16,
                b'a'..=b'z' => (c - b'a') as u16,
                b'2'..=b'7' => (26 + c - b'2') as u16,
                _ => return None,
            };

            decoded |= val << shift;
            if shift <= 8 {
                if byte_idx >= 20 {
                    return None;
                }
                bytes[byte_idx] = (decoded >> 8) as u8;
                byte_idx += 1;
                decoded <<= 8;
                shift += 3;
            } else {
                shift -= 5;
            }
        }

        if byte_idx == 20 && shift == 11 {
            Some(HashString::new(bytes))
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_magnet_uri_hex() {
        let uri = "magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567&dn=Ubuntu+ISO&tr=http%3A%2F%2Ftracker.example.com%2Fannounce";
        let meta = DownloadConstructor::from_magnet_uri(uri).unwrap();
        assert_eq!(meta.name, "Ubuntu ISO");
        assert_eq!(meta.info_hash.to_hex(), "0123456789abcdef0123456789abcdef01234567");
        assert_eq!(meta.announce, Some("http://tracker.example.com/announce".into()));
        assert!(meta.is_meta_download);
    }

    #[test]
    fn test_from_info_bytes() {
        let mut info = Object::create_map();
        info.insert_key("name", Object::from("metadata-dir")).unwrap();
        info.insert_key("piece length", Object::from(16384i64)).unwrap();
        info.insert_key("pieces", Object::create_string(vec![0u8; 20])).unwrap();
        info.insert_key("length", Object::from(16384i64)).unwrap();

        let bytes = lt_common::object::bencode::encode(&info).unwrap();
        let info_hash = HashString::new(sha1_hash(&bytes));

        let metadata = DownloadConstructor::from_info_bytes(
            &bytes,
            info_hash,
            Some("http://tracker/announce".into()),
            vec![vec!["http://tracker/announce".into()]],
        )
        .unwrap();

        assert_eq!(metadata.info_hash, info_hash);
        assert_eq!(metadata.name, "metadata-dir");
        assert_eq!(metadata.piece_length, 16384);
        assert_eq!(metadata.num_pieces, 1);
        assert_eq!(metadata.total_size, 16384);
        assert!(!metadata.is_meta_download);
    }

    #[test]
    fn test_magnet_uri_base32() {
        // 32-character base32 hash
        // 20 bytes of 0x00 -> 32 'A's
        let uri = "magnet:?xt=urn:btih:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA&dn=Zeroes";
        let meta = DownloadConstructor::from_magnet_uri(uri).unwrap();
        assert_eq!(meta.name, "Zeroes");
        assert_eq!(meta.info_hash.as_bytes(), &[0u8; 20]);
    }
}
