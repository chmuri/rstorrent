use std::cmp::{max, min};

/// A disjoint, sorted collection of half-open intervals `[first, second)`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Ranges<T: Ord + Copy> {
    ranges: Vec<(T, T)>,
}

impl<T: Ord + Copy> Ranges<T> {
    pub fn new() -> Self {
        Self { ranges: Vec::new() }
    }

    pub fn is_empty(&self) -> bool {
        self.ranges.is_empty()
    }

    pub fn len(&self) -> usize {
        self.ranges.len()
    }

    pub fn clear(&mut self) {
        self.ranges.clear();
    }

    pub fn as_slice(&self) -> &[(T, T)] {
        &self.ranges
    }

    pub fn iter(&self) -> std::slice::Iter<'_, (T, T)> {
        self.ranges.iter()
    }


    pub fn first(&self) -> Option<&(T, T)> {
        self.ranges.first()
    }

    pub fn back(&self) -> Option<&(T, T)> {
        self.ranges.last()
    }

    /// Insert interval `[first, second)` into the ranges collection.
    /// If `first >= second`, no-op.
    pub fn insert(&mut self, mut first: T, mut second: T) {
        if first >= second {
            return;
        }

        let first_idx = self
            .ranges
            .iter()
            .position(|&(_, v_second)| first <= v_second);

        match first_idx {
            None => {
                self.ranges.push((first, second));
            }
            Some(idx) => {
                if second < self.ranges[idx].0 {
                    self.ranges.insert(idx, (first, second));
                } else {
                    first = min(first, self.ranges[idx].0);
                    second = max(second, self.ranges[idx].1);

                    let mut last_idx = idx;
                    while last_idx < self.ranges.len() && second >= self.ranges[last_idx].0 {
                        second = max(second, self.ranges[last_idx].1);
                        last_idx += 1;
                    }

                    self.ranges[idx] = (first, second);
                    if last_idx > idx + 1 {
                        self.ranges.drain((idx + 1)..last_idx);
                    }
                }
            }
        }
    }

    /// Erase interval `[first, second)` from the ranges collection.
    pub fn erase(&mut self, first: T, second: T) {
        if first >= second || self.ranges.is_empty() {
            return;
        }

        let first_idx = match self
            .ranges
            .iter()
            .position(|&(_, v_second)| first < v_second)
        {
            Some(i) => i,
            None => return,
        };

        let last_idx = self
            .ranges
            .iter()
            .position(|&(_, v_second)| second < v_second)
            .unwrap_or(self.ranges.len());

        if first_idx == last_idx {
            let elem = self.ranges[first_idx];
            if first > elem.0 {
                self.ranges[first_idx] = (second, elem.1);
                self.ranges.insert(first_idx, (elem.0, first));
            } else if second > elem.0 {
                self.ranges[first_idx].0 = second;
            }
        } else {
            let mut remove_start = first_idx;
            if first > self.ranges[first_idx].0 {
                self.ranges[first_idx].1 = first;
                remove_start += 1;
            }

            if last_idx < self.ranges.len() && second > self.ranges[last_idx].0 {
                self.ranges[last_idx].0 = second;
            }

            if remove_start < last_idx {
                self.ranges.drain(remove_start..last_idx);
            }
        }
    }

    /// Finds the index of the first range where `index < range.1`.
    pub fn find(&self, index: T) -> Option<usize> {
        self.ranges.iter().position(|&(_, end)| index < end)
    }

    /// Checks if a single point `index` is covered by any range in this collection.
    pub fn has(&self, index: T) -> bool {
        match self.find(index) {
            Some(idx) => index >= self.ranges[idx].0,
            None => false,
        }
    }
}

impl<T> Ranges<T>
where
    T: Ord + Copy + std::ops::Sub<Output = T> + TryInto<usize>,
{
    /// Calculates the total intersecting distance of interval `[first, second)`
    /// with all existing ranges in this collection.
    pub fn intersect_distance(&self, first: T, second: T) -> usize {
        if first >= second || self.ranges.is_empty() {
            return 0;
        }

        let first_idx = match self.find(first) {
            Some(i) => i,
            None => return 0,
        };

        if second <= self.ranges[first_idx].0 {
            return 0;
        }

        let mut dist: usize = 0;
        let mut idx = first_idx;

        let start = max(first, self.ranges[idx].0);
        let end = min(second, self.ranges[idx].1);
        if end > start {
            let diff: usize = (end - start).try_into().unwrap_or(0);
            dist += diff;
        }
        idx += 1;

        while idx < self.ranges.len() && second > self.ranges[idx].0 {
            let cur_start = self.ranges[idx].0;
            let cur_end = min(second, self.ranges[idx].1);
            if cur_end > cur_start {
                let diff: usize = (cur_end - cur_start).try_into().unwrap_or(0);
                dist += diff;
            }
            idx += 1;
        }

        dist
    }
}

impl<T: Ord + Copy> Ranges<T> {
    /// Creates the union of two `Ranges` collections into a new disjoint `Ranges`.
    pub fn create_union(left: &Self, right: &Self) -> Self {
        if left.is_empty() {
            return right.clone();
        }
        if right.is_empty() {
            return left.clone();
        }

        let mut result = Self::new();
        let mut l_iter = left.ranges.iter().copied().peekable();
        let mut r_iter = right.ranges.iter().copied().peekable();

        while let (Some(&l), Some(&r)) = (l_iter.peek(), r_iter.peek()) {
            let next = if l.0 < r.0 {
                l_iter.next().unwrap()
            } else {
                r_iter.next().unwrap()
            };

            if let Some(last) = result.ranges.last_mut() {
                if next.0 <= last.1 {
                    last.1 = max(next.1, last.1);
                } else {
                    result.ranges.push(next);
                }
            } else {
                result.ranges.push(next);
            }
        }

        for next in l_iter {
            if let Some(last) = result.ranges.last_mut() {
                if next.0 <= last.1 {
                    last.1 = max(next.1, last.1);
                } else {
                    result.ranges.push(next);
                }
            } else {
                result.ranges.push(next);
            }
        }

        for next in r_iter {
            if let Some(last) = result.ranges.last_mut() {
                if next.0 <= last.1 {
                    last.1 = max(next.1, last.1);
                } else {
                    result.ranges.push(next);
                }
            } else {
                result.ranges.push(next);
            }
        }

        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn verify_ranges<T: Ord + Copy>(r: &Ranges<T>) -> bool {
        let s = r.as_slice();
        if s.is_empty() {
            return true;
        }
        for w in s.windows(2) {
            if w[0].1 <= w[0].0 || w[1].1 <= w[1].0 || w[1].0 <= w[0].1 {
                return false;
            }
        }
        s[0].0 < s[0].1
    }

    #[test]
    fn test_intersect() {
        let mut range = Ranges::<i32>::new();
        assert!(verify_ranges(&range));

        assert_eq!(range.intersect_distance(0, 0), 0);
        assert_eq!(range.intersect_distance(0, 10), 0);

        range.insert(0, 5);
        assert!(verify_ranges(&range));

        assert_eq!(range.intersect_distance(0, 5), 5);
        assert_eq!(range.intersect_distance(0, 10), 5);
        assert_eq!(range.intersect_distance(-5, 5), 5);
        assert_eq!(range.intersect_distance(-5, 10), 5);

        assert_eq!(range.intersect_distance(2, 2), 0);
        assert_eq!(range.intersect_distance(1, 4), 3);
        assert_eq!(range.intersect_distance(1, 10), 4);
        assert_eq!(range.intersect_distance(-5, 4), 4);

        range.insert(10, 15);
        assert!(verify_ranges(&range));

        assert_eq!(range.intersect_distance(0, 15), 10);
        assert_eq!(range.intersect_distance(0, 20), 10);
        assert_eq!(range.intersect_distance(-5, 15), 10);
        assert_eq!(range.intersect_distance(-5, 20), 10);

        assert_eq!(range.intersect_distance(2, 12), 5);
        assert_eq!(range.intersect_distance(1, 14), 8);
        assert_eq!(range.intersect_distance(1, 20), 9);
        assert_eq!(range.intersect_distance(-5, 14), 9);
    }

    #[test]
    fn test_create_union() {
        let mut range_1l = Ranges::<i32>::new();
        let mut range_1r = Ranges::<i32>::new();

        // Empty:
        let mut range_1u = Ranges::create_union(&range_1l, &range_1r);
        assert!(verify_ranges(&range_1u));
        assert_eq!(range_1u.intersect_distance(-5, 20), 0);

        range_1l.insert(0, 5);

        // Left one entry:
        range_1u = Ranges::create_union(&range_1l, &range_1r);
        assert!(verify_ranges(&range_1u));
        assert_eq!(range_1u.intersect_distance(-5, 20), 5);

        // Right one entry:
        range_1u = Ranges::create_union(&range_1r, &range_1l);
        assert!(verify_ranges(&range_1u));
        assert_eq!(range_1u.intersect_distance(-5, 20), 5);

        range_1r.insert(10, 15);

        // Both one entry:
        range_1u = Ranges::create_union(&range_1l, &range_1r);
        assert!(verify_ranges(&range_1u));
        assert_eq!(range_1u.intersect_distance(-5, 20), 10);

        range_1r.insert(4, 6);

        // Overlap:
        range_1u = Ranges::create_union(&range_1l, &range_1r);
        assert!(verify_ranges(&range_1u));
        assert_eq!(range_1u.intersect_distance(-5, 20), 11);

        range_1r.insert(9, 10);

        // Boundary:
        range_1u = Ranges::create_union(&range_1l, &range_1r);
        assert!(verify_ranges(&range_1u));
        assert_eq!(range_1u.intersect_distance(-5, 20), 12);

        // Trailing ranges left:
        range_1l.insert(25, 30);
        range_1l.insert(35, 40);

        range_1u = Ranges::create_union(&range_1l, &range_1r);
        assert!(verify_ranges(&range_1u));
        assert_eq!(range_1u.intersect_distance(-5, 50), 22);

        // Trailing ranges right:
        range_1r.insert(37, 45);
        range_1r.insert(50, 55);

        range_1u = Ranges::create_union(&range_1l, &range_1r);
        assert!(verify_ranges(&range_1u));
        assert_eq!(range_1u.intersect_distance(-5, 60), 32);
    }

    #[test]
    fn test_erase() {
        let mut range = Ranges::<i32>::new();
        range.insert(10, 30);
        range.erase(15, 25);
        assert!(verify_ranges(&range));
        assert_eq!(range.as_slice(), &[(10, 15), (25, 30)]);

        range.erase(5, 12);
        assert_eq!(range.as_slice(), &[(12, 15), (25, 30)]);

        range.erase(28, 35);
        assert_eq!(range.as_slice(), &[(12, 15), (25, 28)]);

        range.erase(10, 30);
        assert!(range.is_empty());
    }

    #[test]
    fn test_has() {
        let mut range = Ranges::<i32>::new();
        range.insert(10, 20);
        assert!(!range.has(9));
        assert!(range.has(10));
        assert!(range.has(19));
        assert!(!range.has(20));
    }
}
