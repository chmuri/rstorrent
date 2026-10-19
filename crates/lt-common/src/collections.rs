use std::collections::VecDeque;

/// An unordered vector where element removal is $O(1)$ via swap-remove.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct UnorderedVector<T> {
    data: Vec<T>,
}

impl<T> UnorderedVector<T> {
    pub fn new() -> Self {
        Self { data: Vec::new() }
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            data: Vec::with_capacity(capacity),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    pub fn len(&self) -> usize {
        self.data.len()
    }

    pub fn clear(&mut self) {
        self.data.clear();
    }

    pub fn push(&mut self, item: T) {
        self.data.push(item);
    }

    pub fn pop(&mut self) -> Option<T> {
        self.data.pop()
    }

    pub fn swap_remove(&mut self, index: usize) -> T {
        self.data.swap_remove(index)
    }

    pub fn as_slice(&self) -> &[T] {
        &self.data
    }

    pub fn as_mut_slice(&mut self) -> &mut [T] {
        &mut self.data
    }
}

/// A fixed-size array of $N$ double-ended queues (buckets), used by libtorrent
/// for prioritized transfer queues and rate-limiter buckets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueueBuckets<T, const N: usize> {
    queues: [VecDeque<T>; N],
}

impl<T, const N: usize> Default for QueueBuckets<T, N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T, const N: usize> QueueBuckets<T, N> {
    pub fn new() -> Self {
        Self {
            queues: std::array::from_fn(|_| VecDeque::new()),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.queues.iter().all(|q| q.is_empty())
    }

    pub fn queue_size(&self, idx: usize) -> usize {
        self.queues[idx].len()
    }

    pub fn queue_empty(&self, idx: usize) -> bool {
        self.queues[idx].is_empty()
    }

    pub fn front(&self, idx: usize) -> Option<&T> {
        self.queues[idx].front()
    }

    pub fn front_mut(&mut self, idx: usize) -> Option<&mut T> {
        self.queues[idx].front_mut()
    }

    pub fn back(&self, idx: usize) -> Option<&T> {
        self.queues[idx].back()
    }

    pub fn back_mut(&mut self, idx: usize) -> Option<&mut T> {
        self.queues[idx].back_mut()
    }

    pub fn push_front(&mut self, idx: usize, value: T) {
        self.queues[idx].push_front(value);
    }

    pub fn push_back(&mut self, idx: usize, value: T) {
        self.queues[idx].push_back(value);
    }

    pub fn pop_front(&mut self, idx: usize) -> Option<T> {
        self.queues[idx].pop_front()
    }

    pub fn pop_back(&mut self, idx: usize) -> Option<T> {
        self.queues[idx].pop_back()
    }

    pub fn queue(&self, idx: usize) -> &VecDeque<T> {
        &self.queues[idx]
    }

    pub fn queue_mut(&mut self, idx: usize) -> &mut VecDeque<T> {
        &mut self.queues[idx]
    }

    pub fn clear_queue(&mut self, idx: usize) {
        self.queues[idx].clear();
    }

    /// Destroys/removes elements in `idx` within the range `[start, end)`.
    pub fn destroy_range(&mut self, idx: usize, start: usize, end: usize) -> usize {
        if start >= end || start >= self.queues[idx].len() {
            return 0;
        }
        let end = end.min(self.queues[idx].len());
        let count = end - start;
        self.queues[idx].drain(start..end);
        count
    }

    /// Moves elements in queue `src_idx` from range `[start, end)` to the back of queue `dst_idx`.
    pub fn move_to(&mut self, src_idx: usize, start: usize, end: usize, dst_idx: usize) {
        if src_idx == dst_idx || start >= end || start >= self.queues[src_idx].len() {
            return;
        }
        let end = end.min(self.queues[src_idx].len());
        let drained: Vec<T> = self.queues[src_idx].drain(start..end).collect();
        self.queues[dst_idx].extend(drained);
    }

    /// Moves all elements from `src_idx` to `dst_idx`.
    pub fn move_all_to(&mut self, src_idx: usize, dst_idx: usize) {
        if src_idx == dst_idx {
            return;
        }
        while let Some(item) = self.queues[src_idx].pop_front() {
            self.queues[dst_idx].push_back(item);
        }
    }

    /// Finds the index of the first element in queue `idx` matching predicate.
    pub fn find_in_queue<F>(&self, idx: usize, mut pred: F) -> Option<usize>
    where
        F: FnMut(&T) -> bool,
    {
        self.queues[idx].iter().position(|item| pred(item))
    }

    /// Finds the first matching element across all queues in order 0..N.
    /// Returns `Some((queue_index, element_index))` or `None`.
    pub fn find_in_any<F>(&self, mut pred: F) -> Option<(usize, usize)>
    where
        F: FnMut(&T) -> bool,
    {
        for (q_idx, q) in self.queues.iter().enumerate() {
            if let Some(pos) = q.iter().position(|item| pred(item)) {
                return Some((q_idx, pos));
            }
        }
        None
    }
}

/// A prioritized layered queue with 8 logarithmic rarity layers,
/// used by libtorrent's ChunkSelector to pop rarest pieces first.
#[derive(Debug, Clone)]
pub struct PartialQueue {
    data: Vec<u32>,
    max_layer_size: usize,
    index: usize,
    ceiling: usize,
    layers: [(usize, usize); Self::NUM_LAYERS],
}

impl Default for PartialQueue {
    fn default() -> Self {
        Self::new()
    }
}

impl PartialQueue {
    pub const NUM_LAYERS: usize = 8;

    pub fn new() -> Self {
        Self {
            data: Vec::new(),
            max_layer_size: 0,
            index: 0,
            ceiling: 0,
            layers: [(0, 0); Self::NUM_LAYERS],
        }
    }

    pub fn enable(&mut self, layer_size: usize) {
        self.max_layer_size = layer_size;
        self.data = vec![0; layer_size * Self::NUM_LAYERS];
        self.clear();
    }

    pub fn disable(&mut self) {
        self.data.clear();
        self.max_layer_size = 0;
        self.layers = [(0, 0); Self::NUM_LAYERS];
    }

    pub fn is_enabled(&self) -> bool {
        self.max_layer_size > 0 && !self.data.is_empty()
    }

    pub fn is_full(&self) -> bool {
        self.ceiling == 0
    }

    pub fn is_layer_full(&self, l: usize) -> bool {
        self.layers[l].1 >= self.max_layer_size
    }

    pub fn max_size(&self) -> usize {
        self.max_layer_size * Self::NUM_LAYERS
    }

    pub fn max_layer_size(&self) -> usize {
        self.max_layer_size
    }

    fn ceiling_of(layer: usize) -> usize {
        (2 << layer) - 1
    }

    pub fn clear(&mut self) {
        if self.data.is_empty() {
            return;
        }
        self.index = 0;
        self.ceiling = Self::ceiling_of(Self::NUM_LAYERS - 1);
        self.layers = [(0, 0); Self::NUM_LAYERS];
    }

    pub fn insert(&mut self, key: u8, value: u32) -> bool {
        let key_sz = key as usize;
        if key_sz >= self.ceiling {
            return false;
        }

        let mut idx = 0;
        while idx < Self::NUM_LAYERS && key_sz >= Self::ceiling_of(idx) {
            idx += 1;
        }
        if idx >= Self::NUM_LAYERS {
            return false;
        }

        self.index = self.index.min(idx);

        if self.is_layer_full(idx) {
            return false;
        }

        let offset = self.max_layer_size * idx + self.layers[idx].1;
        self.data[offset] = value;
        self.layers[idx].1 += 1;

        if self.is_layer_full(idx) {
            self.ceiling = if idx > 0 { Self::ceiling_of(idx - 1) } else { 0 };
        }

        true
    }

    pub fn prepare_pop(&mut self) -> bool {
        while self.index < Self::NUM_LAYERS && self.layers[self.index].0 == self.layers[self.index].1 {
            if self.is_layer_full(self.index) || self.index + 1 == Self::NUM_LAYERS {
                return false;
            }
            self.index += 1;
        }
        self.index < Self::NUM_LAYERS && self.layers[self.index].0 < self.layers[self.index].1
    }

    pub fn pop(&mut self) -> Option<u32> {
        if self.index >= Self::NUM_LAYERS || self.layers[self.index].0 >= self.layers[self.index].1 {
            return None;
        }
        let pos = self.index * self.max_layer_size + self.layers[self.index].0;
        self.layers[self.index].0 += 1;
        Some(self.data[pos])
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_unordered_vector() {
        let mut uv = UnorderedVector::new();
        assert!(uv.is_empty());
        uv.push(10);
        uv.push(20);
        uv.push(30);
        assert_eq!(uv.len(), 3);

        // swap_remove element 0 (10) -> element 2 (30) moves to index 0
        let removed = uv.swap_remove(0);
        assert_eq!(removed, 10);
        assert_eq!(uv.as_slice(), &[30, 20]);
    }

    #[test]
    fn test_queue_buckets_basic() {
        let mut buckets = QueueBuckets::<i32, 2>::new();
        assert_eq!(buckets.queue_size(0), 0);
        assert_eq!(buckets.queue_size(1), 0);
        assert!(buckets.is_empty());

        buckets.push_front(0, 0);
        assert_eq!(buckets.queue_size(0), 1);
        buckets.push_back(0, 0);
        assert_eq!(buckets.queue_size(0), 2);
        assert!(!buckets.is_empty());

        buckets.push_front(1, 0);
        assert_eq!(buckets.queue_size(1), 1);
        buckets.push_back(1, 0);
        assert_eq!(buckets.queue_size(1), 2);
        assert!(!buckets.is_empty());

        buckets.pop_front(0);
        assert_eq!(buckets.queue_size(0), 1);
        assert_eq!(buckets.queue_size(1), 2);
        buckets.pop_back(0);
        assert_eq!(buckets.queue_size(0), 0);
        assert_eq!(buckets.queue_size(1), 2);
        assert!(!buckets.is_empty());

        buckets.pop_front(1);
        assert_eq!(buckets.queue_size(0), 0);
        assert_eq!(buckets.queue_size(1), 1);
        buckets.pop_back(1);
        assert_eq!(buckets.queue_size(0), 0);
        assert_eq!(buckets.queue_size(1), 0);
        assert!(buckets.is_empty());
    }

    fn fill_buckets(buckets: &mut QueueBuckets<i32, 2>, s_0: i32, s_1: i32) {
        for i in 0..s_0 {
            buckets.push_back(0, i);
        }
        for i in 0..s_1 {
            buckets.push_back(1, s_0 + i);
        }
    }

    #[test]
    fn test_queue_buckets_erase() {
        let mut buckets = QueueBuckets::<i32, 2>::new();
        fill_buckets(&mut buckets, 10, 5);
        assert_eq!(buckets.queue_size(0), 10);
        assert_eq!(buckets.queue_size(1), 5);

        let destroyed = buckets.destroy_range(0, 3, 7);
        assert_eq!(destroyed, 4);
        assert_eq!(buckets.queue_size(0), 6);
        assert_eq!(buckets.queue_size(1), 5);

        let destroyed2 = buckets.destroy_range(1, 0, 5);
        assert_eq!(destroyed2, 5);
        assert_eq!(buckets.queue_size(0), 6);
        assert_eq!(buckets.queue_size(1), 0);
    }

    #[test]
    fn test_queue_buckets_find() {
        let mut buckets = QueueBuckets::<i32, 2>::new();
        fill_buckets(&mut buckets, 10, 5);

        assert_eq!(buckets.find_in_queue(0, |&v| v == 0), Some(0));
        assert_eq!(buckets.find_in_queue(0, |&v| v == 10), None);
        assert_eq!(buckets.find_in_queue(1, |&v| v == 10), Some(0));

        assert_eq!(buckets.find_in_any(|&v| v == 0), Some((0, 0)));
        assert_eq!(buckets.find_in_any(|&v| v == 10), Some((1, 0)));
        assert_eq!(buckets.find_in_any(|&v| v == 20), None);
    }

    #[test]
    fn test_queue_buckets_move_range() {
        let mut buckets = QueueBuckets::<i32, 2>::new();
        fill_buckets(&mut buckets, 10, 5);

        buckets.move_to(0, 3, 7, 1);
        assert_eq!(buckets.queue_size(0), 6);
        assert_eq!(buckets.queue_size(1), 9);
    }

    #[test]
    fn test_partial_queue() {
        let mut pq = PartialQueue::new();
        assert!(!pq.is_enabled());
        pq.enable(4);
        assert!(pq.is_enabled());
        assert!(!pq.is_full());

        // Insert key 0 (layer 0) and key 2 (layer 1) and key 5 (layer 2)
        assert!(pq.insert(0, 100));
        assert!(pq.insert(2, 102));
        assert!(pq.insert(5, 105));
        assert!(pq.insert(0, 101));

        assert!(pq.prepare_pop());
        // Layer 0 items should pop first
        assert_eq!(pq.pop(), Some(100));
        assert_eq!(pq.pop(), Some(101));

        // Then layer 1 items
        assert!(pq.prepare_pop());
        assert_eq!(pq.pop(), Some(102));

        // Then layer 2 items
        assert!(pq.prepare_pop());
        assert_eq!(pq.pop(), Some(105));

        assert!(!pq.prepare_pop());
        assert_eq!(pq.pop(), None);
    }
}

