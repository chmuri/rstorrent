use lt_common::bitfield::Bitfield;
use lt_common::object::Object;

use crate::error::StorageError;
use crate::file::Priority;

/// File was never created by the client (`~0` in the legacy format).
pub const MTIME_UNCHECKED: i64 = -1;
/// File exists but must not be created (`~1` in the legacy format).
pub const MTIME_SKIPPED: i64 = -2;
/// Resume data predates uncertain-piece tracking and must be rechecked
/// (`~2` in the legacy format).
pub const MTIME_RECHECK: i64 = -3;
/// Resume data was written while the torrent was still downloading, so the
/// file contents may not be fully synced (`~3` in the legacy format).
pub const MTIME_DOWNLOADING: i64 = -4;

/// The in-memory representation of a `.libtorrent_resume` file.
///
/// The structure intentionally mirrors the legacy libtorrent fast-resume
/// schema. Unknown keys (for example `trackers`, `peers`, or future fields) are
/// ignored while parsing, and missing optional keys fall back to safe defaults
/// so an older or newer client can always read the file.
#[derive(Debug, Clone, Default)]
pub struct ResumeData {
    pub bitfield: Option<Bitfield>,
    pub file_priorities: Vec<Priority>,
    pub file_mtimes: Vec<i64>,
    pub completed_bytes: u64,
}

impl ResumeData {
    pub fn new() -> Self {
        Self::default()
    }

    /// Convert ResumeData to Bencode Object compatible with `.libtorrent_resume`.
    ///
    /// The bitfield is always written as a byte string, which both the legacy
    /// libtorrent reader and newer clients understand. Uniform integer
    /// bitfields produced by older clients are still accepted by
    /// [`ResumeData::from_object`].
    pub fn to_object(&self) -> Object {
        let mut map = Object::create_map();

        if let Some(ref bf) = self.bitfield {
            let _ = map.insert_key(b"bitfield", Object::create_string(bf.as_bytes()));
        }

        let file_count = self.file_mtimes.len().max(self.file_priorities.len());
        if file_count > 0 {
            let mut files_list = Object::create_list();
            for idx in 0..file_count {
                let mut file_dict = Object::create_map();
                let mtime = self
                    .file_mtimes
                    .get(idx)
                    .copied()
                    .unwrap_or(MTIME_UNCHECKED);
                let prio = self
                    .file_priorities
                    .get(idx)
                    .copied()
                    .unwrap_or(Priority::Normal);
                let _ = file_dict.insert_key(b"mtime", Object::create_value(mtime));
                let _ = file_dict.insert_key(b"priority", Object::create_value(prio as i64));
                let _ = files_list.insert_back(file_dict);
            }
            let _ = map.insert_key(b"files", files_list);
        }

        let _ = map.insert_key(
            b"completed",
            Object::create_value(self.completed_bytes as i64),
        );

        map
    }

    /// Parse ResumeData from a Bencode Object.
    ///
    /// Prefer [`ResumeData::from_object_with_piece_count`] when the torrent
    /// metadata is available: it restores the exact piece count for
    /// non-byte-aligned bitfields and understands uniform integer bitfields.
    pub fn from_object(obj: &Object) -> Result<Self, StorageError> {
        Self::parse_object(obj, None)
    }

    /// Parse ResumeData and validate the bitfield against the torrent's piece
    /// count.
    ///
    /// A malformed or size-mismatched bitfield is discarded (the rest of the
    /// resume data is still returned) rather than failing the whole load. This
    /// matches the legacy behaviour where an invalid bitfield simply means the
    /// affected torrent has to be rechecked.
    pub fn from_object_with_piece_count(
        obj: &Object,
        num_pieces: usize,
    ) -> Result<Self, StorageError> {
        Self::parse_object(obj, Some(num_pieces))
    }

    fn parse_object(obj: &Object, expected_pieces: Option<usize>) -> Result<Self, StorageError> {
        let mut resume = ResumeData::new();

        if let Ok(bf_obj) = obj.get_key(b"bitfield") {
            resume.bitfield = Self::parse_bitfield(bf_obj, expected_pieces);
        }

        if let Ok(files_obj) = obj.get_key(b"files") {
            if let Ok(list) = files_obj.as_list() {
                for file_entry in list {
                    let mtime = file_entry
                        .get_key(b"mtime")
                        .ok()
                        .and_then(|v| v.as_value().ok())
                        .unwrap_or(MTIME_UNCHECKED);

                    // Legacy priority values are 0 (off), 1 (normal) and 2
                    // (high). Anything else is treated as normal so a newer
                    // client's extended priority cannot make an older client
                    // reject the whole file list.
                    let prio = file_entry
                        .get_key(b"priority")
                        .ok()
                        .and_then(|v| v.as_value().ok())
                        .map(|p| match p {
                            0 => Priority::Off,
                            2 => Priority::High,
                            _ => Priority::Normal,
                        })
                        .unwrap_or(Priority::Normal);

                    resume.file_mtimes.push(mtime);
                    resume.file_priorities.push(prio);
                }
            }
        }

        if let Ok(comp_obj) = obj.get_key(b"completed") {
            if let Ok(comp) = comp_obj.as_value() {
                resume.completed_bytes = comp.max(0) as u64;
            }
        }

        Ok(resume)
    }

    fn parse_bitfield(bf_obj: &Object, expected_pieces: Option<usize>) -> Option<Bitfield> {
        // Legacy clients write a byte string for non-uniform bitfields...
        if let Ok(bytes) = bf_obj.as_string() {
            let size_bits = match expected_pieces {
                Some(bits) => {
                    // The tail byte is padded, so the on-disk length must match
                    // the torrent exactly. A mismatch invalidates the saved
                    // progress, not the rest of the resume file.
                    let expected_bytes = (bits + 7) / 8;
                    if bytes.len() != expected_bytes {
                        return None;
                    }
                    bits
                }
                None => bytes.len() * 8,
            };

            return Some(Bitfield::from_bytes(bytes, size_bits));
        }

        // ...and a plain integer for uniform "all set" / "all unset"
        // bitfields. A positive value is the number of completed pieces.
        if let Ok(value) = bf_obj.as_value() {
            if let Some(bits) = expected_pieces {
                if value == bits as i64 {
                    let mut bitfield = Bitfield::new(bits);
                    bitfield.set_all();
                    return Some(bitfield);
                }
                if value == 0 {
                    return Some(Bitfield::new(bits));
                }
                return None;
            }

            if value > 0 && value <= 100_000_000 {
                let mut bitfield = Bitfield::new(value as usize);
                bitfield.set_all();
                return Some(bitfield);
            }
        }

        None
    }

    /// Whether the saved bitfield is usable for a torrent with `num_pieces`
    /// pieces.
    pub fn bitfield_is_valid(&self, num_pieces: usize) -> bool {
        self.bitfield
            .as_ref()
            .map(|bitfield| bitfield.size_bits() == num_pieces)
            .unwrap_or(true)
    }

    /// Recompute `completed_bytes` from the bitfield.
    ///
    /// Returns `true` when the stored value disagreed and was corrected. A
    /// missing bitfield means no progress, so `completed_bytes` is reset to
    /// zero as well.
    pub fn normalize_completed(&mut self, total_size: u64, piece_length: u32) -> bool {
        let expected = match &self.bitfield {
            Some(bitfield) => completed_bytes_for_bitfield(bitfield, total_size, piece_length),
            None => 0,
        };

        if self.completed_bytes != expected {
            self.completed_bytes = expected;
            true
        } else {
            false
        }
    }
}

/// Sum the size of every completed piece in `bitfield`.
pub fn completed_bytes_for_bitfield(
    bitfield: &Bitfield,
    total_size: u64,
    piece_length: u32,
) -> u64 {
    if piece_length == 0 {
        return 0;
    }

    let num_pieces = bitfield.size_bits();
    let piece_length = piece_length as u64;
    let mut completed = 0u64;

    for index in 0..num_pieces {
        if !bitfield.get(index) {
            continue;
        }

        let size = if index + 1 == num_pieces && total_size > 0 {
            total_size.saturating_sub((num_pieces as u64 - 1) * piece_length)
        } else {
            piece_length
        };
        completed = completed.saturating_add(size);
    }

    completed
}

/// Per-tracker settings stored in the legacy resume map.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackerResumeSettings {
    pub url: String,
    pub enabled: bool,
    pub extra_tracker: bool,
    pub group: u32,
}

/// Legacy resume fields that the Rust engine does not need, but that must still
/// round-trip for clients exchanging `.libtorrent_resume` files.
///
/// `ResumeData` itself is kept layout-compatible with the engine (it is built
/// with struct literals), so these optional fields live in a companion codec
/// instead of adding fields to the hot path representation.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResumeExtras {
    /// Per-file `completed` chunk counts. `None` preserves a missing key.
    pub file_completed_chunks: Vec<Option<u64>>,
    pub trackers: Vec<TrackerResumeSettings>,
}

impl ResumeExtras {
    /// Extract legacy-only fields from a resume object. Unknown keys and
    /// malformed entries are ignored.
    pub fn from_object(object: &Object) -> Self {
        let mut extras = ResumeExtras::default();

        if let Ok(files) = object.get_key_list(b"files") {
            for file in files {
                extras.file_completed_chunks.push(
                    file.get_key_value(b"completed")
                        .ok()
                        .map(|completed| completed.max(0) as u64),
                );
            }
        }

        if let Ok(trackers) = object.get_key_map(b"trackers") {
            for (url, tracker) in trackers {
                extras.trackers.push(TrackerResumeSettings {
                    url: String::from_utf8_lossy(url).into_owned(),
                    enabled: tracker
                        .get_key_value(b"enabled")
                        .map(|enabled| enabled != 0)
                        .unwrap_or(true),
                    extra_tracker: tracker
                        .get_key_value(b"extra_tracker")
                        .map(|extra| extra != 0)
                        .unwrap_or(false),
                    group: tracker
                        .get_key_value(b"group")
                        .ok()
                        .filter(|group| *group >= 0 && *group <= u32::MAX as i64)
                        .map(|group| group as u32)
                        .unwrap_or(0),
                });
            }
        }

        extras
    }

    /// Merge these values into an existing resume object, preserving unrelated
    /// keys.
    pub fn write_into(&self, object: &mut Object) {
        if !self.file_completed_chunks.is_empty() {
            let mut entries: Vec<Object> = object
                .get_key_list(b"files")
                .map(|files| files.to_vec())
                .unwrap_or_default();

            if entries.len() < self.file_completed_chunks.len() {
                entries.resize_with(self.file_completed_chunks.len(), Object::create_map);
            }

            for (index, completed) in self.file_completed_chunks.iter().enumerate() {
                let Some(completed) = completed else {
                    continue;
                };
                if !entries[index].is_map() {
                    entries[index] = Object::create_map();
                }
                let _ = entries[index]
                    .insert_key(b"completed", Object::create_value(*completed as i64));
            }

            let mut files = Object::create_list();
            for entry in entries {
                let _ = files.insert_back(entry);
            }
            let _ = object.insert_key(b"files", files);
        }

        if !self.trackers.is_empty() {
            let mut trackers = object
                .get_key_map(b"trackers")
                .map(|trackers| trackers.clone())
                .unwrap_or_default();

            for tracker in &self.trackers {
                let mut entry = Object::create_map();
                let _ = entry.insert_key(
                    b"enabled",
                    Object::create_value(if tracker.enabled { 1 } else { 0 }),
                );
                if tracker.extra_tracker {
                    let _ = entry.insert_key(b"extra_tracker", Object::create_value(1));
                    let _ = entry.insert_key(b"group", Object::create_value(tracker.group as i64));
                }
                trackers.insert(tracker.url.clone().into_bytes(), entry);
            }

            let _ = object.insert_key(b"trackers", Object::from(trackers));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resume_round_trip_preserves_all_fields() {
        let mut bitfield = Bitfield::new(10);
        bitfield.set(0);
        bitfield.set(3);
        bitfield.set(9);

        let data = ResumeData {
            bitfield: Some(bitfield.clone()),
            file_priorities: vec![Priority::High, Priority::Off, Priority::Normal],
            file_mtimes: vec![100, MTIME_RECHECK, 300],
            completed_bytes: 12345,
        };

        let object = data.to_object();
        let restored = ResumeData::from_object(&object).unwrap();

        assert_eq!(restored.bitfield.unwrap().as_bytes(), bitfield.as_bytes());
        assert_eq!(
            restored.file_priorities,
            vec![Priority::High, Priority::Off, Priority::Normal]
        );
        assert_eq!(restored.file_mtimes, vec![100, MTIME_RECHECK, 300]);
        assert_eq!(restored.completed_bytes, 12345);
    }

    #[test]
    fn priorities_without_mtimes_are_preserved() {
        let data = ResumeData {
            bitfield: None,
            file_priorities: vec![Priority::High, Priority::Normal],
            file_mtimes: Vec::new(),
            completed_bytes: 0,
        };

        let restored = ResumeData::from_object(&data.to_object()).unwrap();
        assert_eq!(
            restored.file_priorities,
            vec![Priority::High, Priority::Normal]
        );
    }

    #[test]
    fn non_byte_aligned_bitfield_uses_expected_piece_count() {
        let mut bitfield = Bitfield::new(10);
        bitfield.set(0);
        bitfield.set(9);

        let data = ResumeData {
            bitfield: Some(bitfield),
            ..ResumeData::new()
        };
        let object = data.to_object();

        // Without a piece count the codec can only infer whole bytes.
        let raw = ResumeData::from_object(&object).unwrap();
        assert_eq!(raw.bitfield.unwrap().size_bits(), 16);

        let exact = ResumeData::from_object_with_piece_count(&object, 10).unwrap();
        let exact = exact.bitfield.unwrap();
        assert_eq!(exact.size_bits(), 10);
        assert!(exact.get(0));
        assert!(exact.get(9));
    }

    #[test]
    fn mismatched_bitfield_is_discarded_but_files_are_kept() {
        let mut bitfield = Bitfield::new(8);
        bitfield.set_all();
        let data = ResumeData {
            bitfield: Some(bitfield),
            file_priorities: vec![Priority::High],
            file_mtimes: vec![123],
            completed_bytes: 8,
        };

        let restored = ResumeData::from_object_with_piece_count(&data.to_object(), 9).unwrap();
        assert!(restored.bitfield.is_none());
        assert_eq!(restored.file_priorities, vec![Priority::High]);
        assert_eq!(restored.file_mtimes, vec![123]);
    }

    #[test]
    fn legacy_integer_bitfields_are_supported() {
        let mut object = Object::create_map();
        object
            .insert_key(b"bitfield", Object::create_value(10))
            .unwrap();

        let all_set = ResumeData::from_object_with_piece_count(&object, 10).unwrap();
        let bitfield = all_set.bitfield.unwrap();
        assert_eq!(bitfield.size_bits(), 10);
        assert!(bitfield.is_all_set());

        object
            .insert_key(b"bitfield", Object::create_value(0))
            .unwrap();
        let all_unset = ResumeData::from_object_with_piece_count(&object, 10).unwrap();
        let bitfield = all_unset.bitfield.unwrap();
        assert_eq!(bitfield.size_bits(), 10);
        assert!(bitfield.is_all_unset());
    }

    #[test]
    fn unknown_keys_and_trackers_are_ignored() {
        let mut object = Object::create_map();
        object
            .insert_key(b"bitfield", Object::create_string(vec![0x80]))
            .unwrap();
        object
            .insert_key(b"future_key", Object::create_value(7))
            .unwrap();

        let mut trackers = Object::create_map();
        let mut tracker = Object::create_map();
        tracker
            .insert_key(b"enabled", Object::create_value(0))
            .unwrap();
        tracker
            .insert_key(b"extra_tracker", Object::create_value(1))
            .unwrap();
        trackers
            .insert_key(b"http://tracker/announce", tracker)
            .unwrap();
        object.insert_key(b"trackers", trackers).unwrap();

        let resume = ResumeData::from_object_with_piece_count(&object, 8).unwrap();
        assert!(resume.bitfield.is_some());
        assert!(resume.file_mtimes.is_empty());
    }

    #[test]
    fn completed_bytes_are_recomputed_from_bitfield() {
        let mut bitfield = Bitfield::new(3);
        bitfield.set(0);
        bitfield.set(2);

        let mut resume = ResumeData {
            bitfield: Some(bitfield),
            completed_bytes: 1,
            ..ResumeData::new()
        };

        // Last (short) piece: total 20 bytes, 8-byte pieces -> 8 + 4 = 12.
        assert!(resume.normalize_completed(20, 8));
        assert_eq!(resume.completed_bytes, 12);
        assert!(!resume.normalize_completed(20, 8));
    }
}
