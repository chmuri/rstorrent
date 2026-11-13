use lt_common::hash_string::{Hash256, HashString};
use lt_common::object::Object;
use lt_common::path::Path;
use lt_crypto::sha1::sha1_hash;
use lt_crypto::sha256::sha256_hash;
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
    #[error("Unsupported meta version: {0}")]
    UnsupportedMetaVersion(i64),
    #[error("Invalid file tree: {0}")]
    InvalidFileTree(String),
    #[error("Invalid piece layers: {0}")]
    InvalidPieceLayers(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TorrentFileEntry {
    pub path: Path,
    pub length: u64,
    pub offset: u64,
}

/// A file's BEP52 piece layer: its merkle pieces root and the concatenated
/// layer hashes (one hash per piece), present only for files larger than a piece.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PieceLayer {
    pub pieces_root: [u8; 32],
    pub layers: Vec<[u8; 32]>,
}

/// A v2 file tree leaf: path, byte length and optional merkle pieces root
/// (absent for empty/padding files).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct V2FileEntry {
    pub path: Path,
    pub length: u64,
    pub pieces_root: Option<[u8; 32]>,
}

/// BitTorrent v2 / hybrid metadata parsed from the info dictionary (BEP52).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TorrentV2 {
    /// Full SHA-256 v2 info-hash (32 bytes), truncated to 20 bytes on the wire.
    pub info_hash: Hash256,
    /// True when the torrent carries both v1 `pieces` and a v2 `file tree`.
    pub is_hybrid: bool,
    /// Files in tree order, each aligned to a piece boundary.
    pub files: Vec<V2FileEntry>,
    /// Per-file piece layers, matched to files by pieces root.
    pub piece_layers: Vec<PieceLayer>,
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
    /// BitTorrent v2 / hybrid metadata, present only for `meta version` 2.
    pub v2: Option<TorrentV2>,
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
        obj.get_key_string(key)
            .ok()
            .and_then(|b| std::str::from_utf8(b).ok())
    }

    fn get_int(obj: &Object, key: &str) -> Option<i64> {
        obj.get_key_value(key).ok()
    }

    fn get_bytes<'a>(obj: &'a Object, key: &str) -> Option<&'a [u8]> {
        obj.get_key_string(key).ok()
    }

    pub fn from_object(b: &Object) -> Result<TorrentMetadata, ConstructorError> {
        let info_obj = b
            .get_key("info")
            .map_err(|_| ConstructorError::InvalidInfoDict)?;

        // Calculate info hash: serialize the info dictionary back to bencode and hash it
        let raw_info_bytes = lt_common::object::bencode::encode(info_obj)
            .map_err(|_| ConstructorError::InvalidInfoDict)?;
        let v1_info_hash = HashString::new(sha1_hash(&raw_info_bytes));

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

        // BEP52: check `meta version` first, before any other validation.
        let meta_version = Self::get_int(info_obj, "meta version").unwrap_or(1);
        if meta_version != 1 && meta_version != 2 {
            return Err(ConstructorError::UnsupportedMetaVersion(meta_version));
        }
        let is_v2 = meta_version == 2;

        let piece_length = if is_meta_download {
            1
        } else {
            let pl = Self::get_int(info_obj, "piece length")
                .ok_or(ConstructorError::InvalidPieceLength(0))?;
            if is_v2 {
                // v2 requires a power of two of at least 16 KiB.
                if pl < 16384 || !(pl as u64).is_power_of_two() || pl > 512 * 1024 * 1024 {
                    return Err(ConstructorError::InvalidPieceLength(pl));
                }
            } else if !(1024..=(512 * 1024 * 1024)).contains(&pl) {
                return Err(ConstructorError::InvalidPieceLength(pl));
            }
            pl as u32
        };

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
                        .filter_map(|t| {
                            t.as_string()
                                .ok()
                                .and_then(|b| std::str::from_utf8(b).ok())
                                .map(|s| s.trim().to_string())
                        })
                        .collect();
                    if !tier_urls.is_empty() {
                        announce_list.push(tier_urls);
                    }
                }
            }
        } else if let Some(ann) = &announce {
            announce_list.push(vec![ann.clone()]);
        }

        if is_v2 {
            return Self::from_v2_object(
                b,
                info_obj,
                &raw_info_bytes,
                v1_info_hash,
                name,
                piece_length,
                is_private,
                announce,
                announce_list,
                is_meta_download,
            );
        }

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

        Ok(TorrentMetadata {
            info_hash: v1_info_hash,
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
            v2: None,
        })
    }

    /// Parse a BEP52 v2 / hybrid info dictionary: `file tree`, `piece layers`
    /// and the dual info-hash (v1 SHA-1, v2 SHA-256 truncated to 20 bytes).
    #[allow(clippy::too_many_arguments)]
    fn from_v2_object(
        b: &Object,
        info_obj: &Object,
        raw_info_bytes: &[u8],
        v1_info_hash: HashString,
        name: String,
        piece_length: u32,
        is_private: bool,
        announce: Option<String>,
        announce_list: Vec<Vec<String>>,
        is_meta_download: bool,
    ) -> Result<TorrentMetadata, ConstructorError> {
        let tree = info_obj
            .get_key("file tree")
            .map_err(|_| ConstructorError::InvalidFileTree("missing file tree".into()))?;
        let v2_files = Self::parse_file_tree(tree, &name)?;
        let piece_layers = Self::parse_piece_layers(b, &v2_files)?;
        let v2_info_hash = Hash256::new(sha256_hash(raw_info_bytes));
        let is_hybrid = info_obj.get_key("pieces").is_ok();

        let mut files = Vec::new();
        let mut total_size = 0u64;
        let mut offset = 0u64;
        let mut piece_offset = 0u64;
        for f in &v2_files {
            // Hybrid torrents keep the contiguous v1 byte layout; v2-only
            // torrents lay each file out at a piece boundary (BEP52), so a
            // piece belongs to exactly one file and the storage machinery can
            // reuse the v1 piece-segment mapping.
            let file_offset = if is_hybrid {
                offset
            } else {
                piece_offset * piece_length as u64
            };
            files.push(TorrentFileEntry {
                path: f.path.clone(),
                length: f.length,
                offset: file_offset,
            });
            offset += f.length;
            piece_offset += f.length.div_ceil(piece_length as u64);
            total_size += f.length;
        }

        // Hybrid torrents keep the v1 SHA-1 identity and `pieces`; v2-only
        // torrents use the truncated SHA-256 identity and carry no v1 pieces.
        let (pieces, info_hash, num_pieces) = if is_hybrid {
            let pieces_bytes = Self::get_bytes(info_obj, "pieces")
                .ok_or(ConstructorError::InvalidInfoDict)?
                .to_vec();
            if pieces_bytes.len() % 20 != 0 {
                return Err(ConstructorError::InvalidPiecesLength(pieces_bytes.len()));
            }
            let n = pieces_bytes.len() / 20;
            (pieces_bytes, v1_info_hash, n)
        } else {
            let n: usize = v2_files
                .iter()
                .map(|f| f.length.div_ceil(piece_length as u64) as usize)
                .sum();
            (Vec::new(), v2_info_hash.truncate(), n)
        };

        if total_size == 0 && !is_meta_download {
            return Err(ConstructorError::ZeroLengthTorrent);
        }

        Ok(TorrentMetadata {
            info_hash,
            name,
            piece_length,
            pieces,
            num_pieces,
            total_size,
            is_multi_file: v2_files.len() > 1,
            files,
            announce,
            announce_list,
            is_private,
            is_meta_download,
            v2: Some(TorrentV2 {
                info_hash: v2_info_hash,
                is_hybrid,
                files: v2_files,
                piece_layers,
            }),
        })
    }

    /// Sanitize a v2 path component: BEP52 requires neutralizing '.' and '..'
    /// (directory traversal) in addition to path separators.
    fn sanitize_component(s: &str) -> String {
        if s == "." || s == ".." {
            "_".to_string()
        } else {
            Self::sanitize_name(s)
        }
    }

    fn parse_file_tree(tree: &Object, name: &str) -> Result<Vec<V2FileEntry>, ConstructorError> {
        let mut root = Path::new();
        root.push(name);
        let mut files = Vec::new();
        Self::walk_file_tree(tree, &root, &mut files)?;
        Ok(files)
    }

    fn walk_file_tree(
        node: &Object,
        path: &Path,
        files: &mut Vec<V2FileEntry>,
    ) -> Result<(), ConstructorError> {
        let map = node
            .as_map()
            .map_err(|_| ConstructorError::InvalidFileTree("not a dictionary".into()))?;
        for (key, value) in map {
            let component = std::str::from_utf8(key)
                .map_err(|_| ConstructorError::InvalidFileTree("non-UTF-8 path".into()))?;
            let mut child = path.clone();
            child.push(&Self::sanitize_component(component));
            if !value.is_map() {
                return Err(ConstructorError::InvalidFileTree(
                    "entry is not a dictionary".into(),
                ));
            }
            // A zero-length key "" holds the file attributes; its presence
            // distinguishes a file leaf from a directory.
            if let Ok(attrs) = value.get_key("") {
                let length = attrs
                    .get_key_value("length")
                    .map_err(|_| ConstructorError::InvalidFileTree("missing length".into()))?;
                if length < 0 {
                    return Err(ConstructorError::InvalidFileTree("negative length".into()));
                }
                let pieces_root = match attrs.get_key_string("pieces root") {
                    Ok(bytes) if bytes.len() == 32 => {
                        let mut root = [0u8; 32];
                        root.copy_from_slice(bytes);
                        Some(root)
                    }
                    Ok(_) => {
                        return Err(ConstructorError::InvalidFileTree(
                            "pieces root must be 32 bytes".into(),
                        ));
                    }
                    Err(_) => None,
                };
                files.push(V2FileEntry {
                    path: child,
                    length: length as u64,
                    pieces_root,
                });
            } else {
                Self::walk_file_tree(value, &child, files)?;
            }
        }
        Ok(())
    }

    fn parse_piece_layers(
        b: &Object,
        files: &[V2FileEntry],
    ) -> Result<Vec<PieceLayer>, ConstructorError> {
        let layers_obj = b
            .get_key("piece layers")
            .map_err(|_| ConstructorError::InvalidPieceLayers("missing piece layers".into()))?;
        let map = layers_obj
            .as_map()
            .map_err(|_| ConstructorError::InvalidPieceLayers("not a dictionary".into()))?;
        let mut result = Vec::new();
        for (root_bytes, layers_value) in map {
            if root_bytes.len() != 32 {
                return Err(ConstructorError::InvalidPieceLayers(
                    "root key must be 32 bytes".into(),
                ));
            }
            let mut pieces_root = [0u8; 32];
            pieces_root.copy_from_slice(root_bytes);
            let layers_bytes = layers_value.as_string().map_err(|_| {
                ConstructorError::InvalidPieceLayers("layers must be a string".into())
            })?;
            if layers_bytes.len() % 32 != 0 {
                return Err(ConstructorError::InvalidPieceLayers(
                    "layer length not a multiple of 32".into(),
                ));
            }
            let layers: Vec<[u8; 32]> = layers_bytes
                .chunks(32)
                .map(|c| {
                    let mut h = [0u8; 32];
                    h.copy_from_slice(c);
                    h
                })
                .collect();
            // BEP52: the torrent is invalid if a layer root matches no file.
            if !files.iter().any(|f| f.pieces_root == Some(pieces_root)) {
                return Err(ConstructorError::InvalidPieceLayers(
                    "pieces root does not match any file".into(),
                ));
            }
            result.push(PieceLayer {
                pieces_root,
                layers,
            });
        }
        Ok(result)
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

        // BEP9 transfers only the `info` dictionary; BEP52 `piece layers` live
        // outside it and arrive separately. Supply an empty map so a v2 info
        // dict parses here (layers are filled in later); a full `.torrent`
        // parsed via `from_object` still requires and validates real layers.
        if root.get_key("piece layers").is_err() {
            root.insert_key("piece layers", Object::create_map())
                .map_err(|_| ConstructorError::InvalidInfoDict)?;
        }

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
                        let hash = HashString::from_hex(hash_str).ok_or_else(|| {
                            ConstructorError::InvalidMagnet("Invalid hex hash".into())
                        })?;
                        info_hash_opt = Some(hash);
                    } else if hash_str.len() == 32 {
                        // 32-char Base32
                        let hash = Self::decode_base32(hash_str).ok_or_else(|| {
                            ConstructorError::InvalidMagnet("Invalid base32 hash".into())
                        })?;
                        info_hash_opt = Some(hash);
                    }
                } else if key == "xt" && val.starts_with("urn:btmh:") && info_hash_opt.is_none() {
                    // BEP52 v2: full SHA-256 multihash, truncated to the 20-byte
                    // wire identity. A v1 `btih` (hybrid) always takes precedence.
                    let hash = Self::decode_btmh_multihash(&val[9..]).ok_or_else(|| {
                        ConstructorError::InvalidMagnet("Invalid btmh multihash".into())
                    })?;
                    info_hash_opt = Some(hash.truncate());
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

        let info_hash = info_hash_opt
            .ok_or_else(|| ConstructorError::InvalidMagnet("Missing xt urn:btih/btmh".into()))?;

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
            v2: None,
        })
    }

    /// Splits a magnet URI into `(key, raw_value)` query pairs.
    fn magnet_params(uri: &str) -> Vec<(&str, &str)> {
        let Some(query) = uri.strip_prefix("magnet:?") else {
            return Vec::new();
        };
        query
            .split('&')
            .filter_map(|param| param.split_once('='))
            .collect()
    }

    /// Exact-source (`xs`) mirrors from a magnet URI, percent-decoded and
    /// filtered to HTTP(S) URLs usable as BEP 19 WebSeeds.
    pub fn magnet_web_seeds(uri: &str) -> Vec<String> {
        Self::magnet_params(uri)
            .into_iter()
            .filter(|(key, _)| *key == "xs")
            .map(|(_, val)| Self::percent_decode(val))
            .filter(|url| url.starts_with("http://") || url.starts_with("https://"))
            .collect()
    }

    /// Optional `btms` metadata hash (SHA-1 of the bencoded info dictionary)
    /// advertised by a magnet URI, used to verify `ut_metadata` downloads.
    pub fn magnet_metadata_hash(uri: &str) -> Option<HashString> {
        Self::magnet_params(uri)
            .into_iter()
            .find(|(key, _)| *key == "btms")
            .and_then(|(_, val)| HashString::from_hex(val))
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
                    if let (Ok(high), Ok(low)) =
                        (u8::from_str_radix(h, 16), u8::from_str_radix(l, 16))
                    {
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

    /// Decode a `urn:btmh:` multihash (BEP52 v2 info-hash) into a full `Hash256`.
    ///
    /// The value is a hex-encoded multihash `0x12 0x20 || sha256(info_dict)`
    /// (sha2-256 code `0x12`, digest length `0x20` = 32 bytes). Returns `None`
    /// for a malformed prefix, length or hex encoding.
    pub fn decode_btmh_multihash(hex: &str) -> Option<Hash256> {
        let bytes = Self::decode_hex(hex)?;
        if bytes.len() != 34 || bytes[0] != 0x12 || bytes[1] != 0x20 {
            return None;
        }
        let mut hash = [0u8; 32];
        hash.copy_from_slice(&bytes[2..]);
        Some(Hash256::new(hash))
    }

    /// Full v2 info-hash (`Hash256`) advertised by a magnet's `xt=urn:btmh:`.
    pub fn magnet_v2_hash(uri: &str) -> Option<Hash256> {
        Self::magnet_params(uri)
            .into_iter()
            .find(|(key, val)| *key == "xt" && val.starts_with("urn:btmh:"))
            .and_then(|(_, val)| Self::decode_btmh_multihash(&val[9..]))
    }

    fn decode_hex(s: &str) -> Option<Vec<u8>> {
        if !s.len().is_multiple_of(2) {
            return None;
        }
        let bytes = s.as_bytes();
        let mut out = Vec::with_capacity(bytes.len() / 2);
        for pair in bytes.chunks(2) {
            let hi = (pair[0] as char).to_digit(16)?;
            let lo = (pair[1] as char).to_digit(16)?;
            out.push(((hi << 4) | lo) as u8);
        }
        Some(out)
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
        assert_eq!(
            meta.info_hash.to_hex(),
            "0123456789abcdef0123456789abcdef01234567"
        );
        assert_eq!(
            meta.announce,
            Some("http://tracker.example.com/announce".into())
        );
        assert!(meta.is_meta_download);
    }

    #[test]
    fn test_from_info_bytes() {
        let mut info = Object::create_map();
        info.insert_key("name", Object::from("metadata-dir"))
            .unwrap();
        info.insert_key("piece length", Object::from(16384i64))
            .unwrap();
        info.insert_key("pieces", Object::create_string(vec![0u8; 20]))
            .unwrap();
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

    #[test]
    fn test_magnet_web_seeds_and_btms() {
        let uri = "magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567\
&xs=https%3A%2F%2Fmirror.example.org%2Ffile.iso\
&xs=http://other.example.org/file.iso\
&xs=urn:sha1:YNCKHTQCWBTRNJIV4WNAE52SJUQCZO5C\
&btms=0123456789abcdef0123456789abcdef01234567";
        let seeds = DownloadConstructor::magnet_web_seeds(uri);
        assert_eq!(
            seeds,
            vec![
                "https://mirror.example.org/file.iso",
                "http://other.example.org/file.iso",
            ]
        );
        let hash = DownloadConstructor::magnet_metadata_hash(uri).unwrap();
        assert_eq!(hash.to_hex(), "0123456789abcdef0123456789abcdef01234567");
        assert!(DownloadConstructor::magnet_metadata_hash(
            "magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567"
        )
        .is_none());
        assert!(DownloadConstructor::magnet_web_seeds("not a magnet").is_empty());
    }

    fn sample_v2_hash() -> Hash256 {
        let mut hash = [0u8; 32];
        for (i, b) in hash.iter_mut().enumerate() {
            *b = i as u8;
        }
        Hash256::new(hash)
    }

    #[test]
    fn test_magnet_btmh_v2_truncated_identity() {
        let full = sample_v2_hash();
        let uri = format!("magnet:?xt=urn:btmh:1220{}&dn=V2+File", full.to_hex());
        let meta = DownloadConstructor::from_magnet_uri(&uri).unwrap();
        assert_eq!(meta.info_hash, full.truncate());
        assert_eq!(meta.name, "V2 File");
        assert!(meta.is_meta_download);
        assert_eq!(DownloadConstructor::magnet_v2_hash(&uri), Some(full));
    }

    #[test]
    fn test_magnet_btmh_hybrid_btih_precedence() {
        let full = sample_v2_hash();
        let btih = "0123456789abcdef0123456789abcdef01234567";
        // btmh first, btih second: v1 identity must win for a hybrid magnet.
        let uri = format!(
            "magnet:?xt=urn:btmh:1220{}&xt=urn:btih:{btih}",
            full.to_hex()
        );
        let meta = DownloadConstructor::from_magnet_uri(&uri).unwrap();
        assert_eq!(meta.info_hash.to_hex(), btih);
        // Reverse order: btih first, btmh must not overwrite it.
        let uri = format!(
            "magnet:?xt=urn:btih:{btih}&xt=urn:btmh:1220{}",
            full.to_hex()
        );
        let meta = DownloadConstructor::from_magnet_uri(&uri).unwrap();
        assert_eq!(meta.info_hash.to_hex(), btih);
    }

    #[test]
    fn test_magnet_btmh_rejects_invalid_multihash() {
        let full = sample_v2_hash();
        // 0x21 digest length instead of 0x20.
        let uri = format!("magnet:?xt=urn:btmh:1221{}", full.to_hex());
        assert!(matches!(
            DownloadConstructor::from_magnet_uri(&uri),
            Err(ConstructorError::InvalidMagnet(_))
        ));
    }

    #[test]
    fn test_decode_btmh_multihash_roundtrip() {
        let full = sample_v2_hash();
        let mh = format!("1220{}", full.to_hex());
        assert_eq!(DownloadConstructor::decode_btmh_multihash(&mh), Some(full));
        assert!(DownloadConstructor::decode_btmh_multihash("1220").is_none());
        assert!(
            DownloadConstructor::decode_btmh_multihash(&format!("1221{}", full.to_hex())).is_none()
        );
        assert!(DownloadConstructor::decode_btmh_multihash("zz").is_none());
        assert!(DownloadConstructor::magnet_v2_hash(
            "magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567"
        )
        .is_none());
    }

    /// Build a BEP52 v2 (or hybrid) metainfo with a single file tree leaf.
    fn build_v2(file_len: i64, root: [u8; 32], layers: &[[u8; 32]], hybrid: bool) -> Object {
        let mut attrs = Object::create_map();
        attrs.insert_key("length", Object::from(file_len)).unwrap();
        attrs
            .insert_key("pieces root", Object::create_string(root.to_vec()))
            .unwrap();
        let mut entry = Object::create_map();
        entry.insert_key("", attrs).unwrap();
        let mut tree = Object::create_map();
        tree.insert_key("file.txt", entry).unwrap();

        let mut info = Object::create_map();
        info.insert_key("name", Object::from("test")).unwrap();
        info.insert_key("piece length", Object::from(16384i64))
            .unwrap();
        info.insert_key("meta version", Object::from(2i64)).unwrap();
        info.insert_key("file tree", tree).unwrap();
        if hybrid {
            let n = (file_len / 16384) as usize;
            info.insert_key("pieces", Object::create_string(vec![0u8; 20 * n]))
                .unwrap();
        }

        let mut layer_bytes = Vec::new();
        for l in layers {
            layer_bytes.extend_from_slice(l);
        }
        let mut pl = Object::create_map();
        pl.insert_key(root.to_vec(), Object::create_string(layer_bytes))
            .unwrap();

        let mut meta = Object::create_map();
        meta.insert_key("info", info).unwrap();
        meta.insert_key("piece layers", pl).unwrap();
        meta
    }

    fn info_bytes(meta: &Object) -> Vec<u8> {
        let info = meta.get_key("info").unwrap();
        lt_common::object::bencode::encode(info).unwrap()
    }

    #[test]
    fn test_v2_single_file_parses_tree_layers_and_dual_hash() {
        let root = [0x11u8; 32];
        let layers = [[0x22u8; 32], [0x33u8; 32]];
        let meta = build_v2(32768, root, &layers, false);
        let parsed = DownloadConstructor::from_object(&meta).unwrap();

        let v2 = parsed.v2.expect("v2 metadata present");
        assert!(!v2.is_hybrid);
        assert_eq!(v2.info_hash, Hash256::new(sha256_hash(&info_bytes(&meta))));
        assert_eq!(v2.files.len(), 1);
        assert_eq!(v2.files[0].length, 32768);
        assert_eq!(v2.files[0].pieces_root, Some(root));
        assert_eq!(v2.piece_layers.len(), 1);
        assert_eq!(v2.piece_layers[0].pieces_root, root);
        assert_eq!(v2.piece_layers[0].layers, layers.to_vec());

        // v2-only: primary identity is the truncated SHA-256, no v1 pieces.
        assert_eq!(parsed.info_hash, v2.info_hash.truncate());
        assert!(parsed.pieces.is_empty());
        assert_eq!(parsed.num_pieces, 2);
        assert_eq!(parsed.total_size, 32768);
    }

    #[test]
    fn test_hybrid_keeps_v1_identity_and_pieces() {
        let root = [0x44u8; 32];
        let layers = [[0x55u8; 32], [0x66u8; 32]];
        let meta = build_v2(32768, root, &layers, true);
        let parsed = DownloadConstructor::from_object(&meta).unwrap();

        let v2 = parsed.v2.expect("v2 metadata present");
        assert!(v2.is_hybrid);
        assert_eq!(v2.info_hash, Hash256::new(sha256_hash(&info_bytes(&meta))));
        // Hybrid primary identity is the v1 SHA-1 info-hash.
        assert_eq!(
            parsed.info_hash,
            HashString::new(sha1_hash(&info_bytes(&meta)))
        );
        assert_eq!(parsed.pieces.len(), 40);
        assert_eq!(parsed.num_pieces, 2);
    }

    #[test]
    fn test_v2_only_files_are_piece_aligned() {
        let pl = 16384i64;
        let root1 = [0x01u8; 32];
        let root2 = [0x02u8; 32];
        // file1 spans 1.5 pieces (2 pieces); file2 is exactly one piece.
        let len1 = pl + pl / 2;
        let len2 = pl;
        let mut tree = Object::create_map();
        for (name, len, root) in [("a.bin", len1, root1), ("b.bin", len2, root2)] {
            let mut attrs = Object::create_map();
            attrs.insert_key("length", Object::from(len)).unwrap();
            attrs
                .insert_key("pieces root", Object::create_string(root.to_vec()))
                .unwrap();
            let mut entry = Object::create_map();
            entry.insert_key("", attrs).unwrap();
            tree.insert_key(name, entry).unwrap();
        }
        let mut info = Object::create_map();
        info.insert_key("name", Object::from("test")).unwrap();
        info.insert_key("piece length", Object::from(pl)).unwrap();
        info.insert_key("meta version", Object::from(2i64)).unwrap();
        info.insert_key("file tree", tree).unwrap();
        // Only files larger than a piece carry piece layers (file1: 2 hashes).
        let mut layers = Object::create_map();
        layers
            .insert_key(root1.to_vec(), Object::create_string(vec![0x11u8; 64]))
            .unwrap();
        let mut meta = Object::create_map();
        meta.insert_key("info", info).unwrap();
        meta.insert_key("piece layers", layers).unwrap();

        let parsed = DownloadConstructor::from_object(&meta).unwrap();
        assert_eq!(parsed.num_pieces, 3);
        // Pure v2 lays files out at piece boundaries: file1 occupies 2 pieces,
        // so file2 starts at piece 2 (offset 32768), not at its byte length.
        assert_eq!(parsed.files[0].offset, 0);
        assert_eq!(parsed.files[1].offset, 2 * pl as u64);
        assert_eq!(parsed.files[0].length, len1 as u64);
    }

    #[test]
    fn test_v2_rejects_piece_layers_root_mismatch() {
        let root = [0x11u8; 32];
        let layers = [[0x22u8; 32]];
        let mut meta = build_v2(16384, root, &layers, false);
        // Replace the piece layers key with a root that matches no file.
        let mut pl = Object::create_map();
        pl.insert_key([0x99u8; 32].to_vec(), Object::create_string(vec![0u8; 32]))
            .unwrap();
        meta.insert_key("piece layers", pl).unwrap();
        let err = DownloadConstructor::from_object(&meta).unwrap_err();
        assert!(matches!(err, ConstructorError::InvalidPieceLayers(_)));
    }

    #[test]
    fn test_v2_rejects_unsupported_meta_version() {
        let root = [0x11u8; 32];
        let mut meta = build_v2(16384, root, &[[0x22u8; 32]], false);
        let mut info = meta.get_key("info").unwrap().clone();
        info.insert_key("meta version", Object::from(3i64)).unwrap();
        meta.insert_key("info", info).unwrap();
        let err = DownloadConstructor::from_object(&meta).unwrap_err();
        assert!(matches!(err, ConstructorError::UnsupportedMetaVersion(3)));
    }

    #[test]
    fn test_v2_piece_length_must_be_power_of_two() {
        let root = [0x11u8; 32];
        let mut meta = build_v2(16384, root, &[[0x22u8; 32]], false);
        let mut info = meta.get_key("info").unwrap().clone();
        info.insert_key("piece length", Object::from(49152i64))
            .unwrap();
        meta.insert_key("info", info).unwrap();
        let err = DownloadConstructor::from_object(&meta).unwrap_err();
        assert!(matches!(err, ConstructorError::InvalidPieceLength(49152)));
    }

    /// Round-trip a v2/hybrid info dictionary built from REAL Merkle roots:
    /// encode -> bencode decode/encode (determinism) -> from_info_bytes, then
    /// check the dual identity and that the parsed tree/layers reproduce the
    /// independently computed BEP52 tree.
    #[test]
    fn test_v2_hybrid_info_dict_round_trip_with_real_merkle() {
        use lt_crypto::merkle::{compute_merkle_tree, BLOCK_SIZE};

        let pl = BLOCK_SIZE;
        // Three distinct 16 KiB blocks -> a 3-piece file (multi-piece, so it
        // carries piece layers and exercises the two-level tree).
        let data: Vec<u8> = (1..=3u8).flat_map(|b| vec![b; BLOCK_SIZE]).collect();
        let tree = compute_merkle_tree(&data, pl);
        assert_eq!(tree.piece_layers.len(), 3);

        for hybrid in [false, true] {
            let meta = build_v2(
                data.len() as i64,
                tree.piece_root,
                &tree.piece_layers,
                hybrid,
            );
            let bytes = info_bytes(&meta);

            // bencode decode -> encode must be byte-identical (canonical form).
            let decoded = lt_common::object::bencode::decode(&bytes).unwrap();
            let reencoded = lt_common::object::bencode::encode(&decoded).unwrap();
            assert_eq!(bytes, reencoded, "info dict bencode must round-trip");

            let identity = if hybrid {
                HashString::new(sha1_hash(&bytes))
            } else {
                Hash256::new(sha256_hash(&bytes)).truncate()
            };

            // Full metainfo (info + piece layers): the parsed tree and layers
            // must reproduce the independently computed BEP52 tree.
            let full = DownloadConstructor::from_object(&meta).unwrap();
            let full_v2 = full.v2.expect("v2 metadata present");
            assert_eq!(full.info_hash, identity);
            assert_eq!(full_v2.info_hash, Hash256::new(sha256_hash(&bytes)));
            assert_eq!(full_v2.is_hybrid, hybrid);
            assert_eq!(full_v2.files[0].pieces_root, Some(tree.piece_root));
            assert_eq!(full_v2.piece_layers.len(), 1);
            assert_eq!(full_v2.piece_layers[0].pieces_root, tree.piece_root);
            assert_eq!(full_v2.piece_layers[0].layers, tree.piece_layers);

            // Info-only round-trip (the ut_metadata path): identity and file
            // tree survive, but piece layers are not carried by the info dict.
            let parsed =
                DownloadConstructor::from_info_bytes(&bytes, identity, None, vec![]).unwrap();
            assert_eq!(parsed.info_hash, identity);
            assert_eq!(parsed.num_pieces, 3);
            assert_eq!(parsed.total_size, data.len() as u64);

            let v2 = parsed.v2.expect("v2 metadata present");
            assert_eq!(v2.is_hybrid, hybrid);
            assert_eq!(v2.info_hash, Hash256::new(sha256_hash(&bytes)));
            assert_eq!(v2.files.len(), 1);
            assert_eq!(v2.files[0].length, data.len() as u64);
            assert_eq!(v2.files[0].pieces_root, Some(tree.piece_root));
            assert!(v2.piece_layers.is_empty());

            if hybrid {
                assert_eq!(parsed.pieces.len(), 60);
            } else {
                assert!(parsed.pieces.is_empty());
            }
        }
    }
}
