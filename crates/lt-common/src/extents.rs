use std::collections::BTreeMap;
use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ExtentsError {
    #[error("nothing defined for specified key")]
    NotDefined,
}

/// A map of key ranges `[start, end]` (inclusive) to mapped values.
/// Used for IP filtering tables, chunk extents, etc.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Extents<K: Ord + Copy, V: Clone + PartialEq> {
    range_map: BTreeMap<K, (K, V)>,
}

impl<K: Ord + Copy, V: Clone + PartialEq> Extents<K, V> {
    pub fn new() -> Self {
        Self {
            range_map: BTreeMap::new(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.range_map.is_empty()
    }

    pub fn len(&self) -> usize {
        self.range_map.len()
    }

    pub fn clear(&mut self) {
        self.range_map.clear();
    }

    /// Insert interval `[address_start, address_end]` mapping to `value`.
    pub fn insert(&mut self, address_start: K, address_end: K, value: V) {
        // Collect keys to delete or check for total overlap
        let mut to_delete = Vec::new();
        let mut ignore_due_to_total_overlap = false;

        // Iterate over candidates that could overlap with [address_start, address_end]
        // Since intervals are indexed by start, an entry starting before or at address_end might overlap.
        for (&start, &(end, ref val)) in self.range_map.iter() {
            if start > address_end {
                break;
            }

            let overlaps = start <= address_end && end >= address_start;
            let different_val = *val != value;

            let do_delete_due_to_overlap = overlaps && different_val;
            let do_delete_due_to_total_overlap = address_start <= start && address_end >= end;

            if do_delete_due_to_overlap || do_delete_due_to_total_overlap {
                to_delete.push(start);
            } else if start <= address_start && end >= address_end {
                ignore_due_to_total_overlap = true;
            }
        }

        for k in to_delete {
            self.range_map.remove(&k);
        }

        if !ignore_due_to_total_overlap {
            self.range_map.insert(address_start, (address_end, value));
        }
    }

    /// Checks if any range covers or intersects with `[address_start, address_end]`.
    pub fn defined_range(&self, address_start: K, address_end: K) -> bool {
        for (&start, &(end, _)) in self.range_map.iter() {
            if start > address_end {
                break;
            }
            if start <= address_end && end >= address_start {
                return true;
            }
        }
        false
    }

    /// Checks if a single address is covered by any range in extents.
    pub fn defined(&self, address: K) -> bool {
        self.defined_range(address, address)
    }

    /// Retrieves the start key of the matching range for `[address_start, address_end]`.
    pub fn get_matching_key(&self, address_start: K, address_end: K) -> Result<K, ExtentsError> {
        for (&start, &(end, _)) in self.range_map.iter() {
            if start > address_end {
                break;
            }
            if start <= address_end && end >= address_start {
                return Ok(start);
            }
        }
        Err(ExtentsError::NotDefined)
    }

    /// Retrieves the value at the specified range, or returns error if not defined.
    pub fn at_range(&self, address_start: K, address_end: K) -> Result<V, ExtentsError> {
        for (&start, &(end, ref val)) in self.range_map.iter() {
            if start > address_end {
                break;
            }
            if start <= address_end && end >= address_start {
                return Ok(val.clone());
            }
        }
        Err(ExtentsError::NotDefined)
    }

    /// Retrieves the value at the specified point, or returns error if not defined.
    pub fn at(&self, address: K) -> Result<V, ExtentsError> {
        self.at_range(address, address)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn verify_extent_data(extent: &Extents<u32, i32>, idx: &[u32], val: &[i32]) -> bool {
        let mut idx_pos = 0;
        let mut val_pos = 0;

        while idx[idx_pos] != idx[idx_pos + 1] {
            for i in idx[idx_pos]..idx[idx_pos + 1] {
                match extent.at(i) {
                    Ok(v) if v == val[val_pos] => {}
                    _ => return false,
                }
            }
            idx_pos += 1;
            val_pos += 1;
        }

        true
    }

    #[test]
    fn test_basic() {
        let mut extent_1 = Extents::<u32, i32>::new();
        extent_1.insert(0, 255, 0);

        let idx_empty = [0, 256, 256];
        let val_empty = [0];
        assert!(verify_extent_data(&extent_1, &idx_empty, &val_empty));
        assert_eq!(extent_1.at(0), Ok(0));
        assert_eq!(extent_1.at(255), Ok(0));

        // Borders test:
        extent_1.insert(0, 0, 1);
        extent_1.insert(255, 255, 1);
        extent_1.insert(1, 254, 0);

        assert_eq!(extent_1.at(0), Ok(1));
        assert_eq!(extent_1.at(255), Ok(1));

        let idx_basic_1 = [0, 1, 255, 256, 256];
        let val_basic_1 = [1, 0, 1];
        assert!(verify_extent_data(&extent_1, &idx_basic_1, &val_basic_1));
    }

    #[test]
    fn test_empty() {
        let extent_1 = Extents::<u32, i32>::new();

        assert!(!extent_1.defined(0));
        assert!(!extent_1.defined(!0u32));
        assert!(!extent_1.defined_range(0, !0u32));

        assert_eq!(extent_1.at(0), Err(ExtentsError::NotDefined));
        assert_eq!(
            extent_1.get_matching_key(0, 0),
            Err(ExtentsError::NotDefined)
        );
    }
}
