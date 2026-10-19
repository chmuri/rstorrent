//! Priority ranges shared by the chunk selector and file commands.

use lt_common::ranges::Ranges;

#[derive(Debug, Clone, Default)]
pub struct DownloadData {
    high_priority: Ranges<u32>,
    normal_priority: Ranges<u32>,
}

impl DownloadData {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn high_priority(&self) -> &Ranges<u32> {
        &self.high_priority
    }

    pub fn normal_priority(&self) -> &Ranges<u32> {
        &self.normal_priority
    }

    pub fn set_priorities(&mut self, high: Ranges<u32>, normal: Ranges<u32>) {
        self.high_priority = high;
        self.normal_priority = normal;
    }

    pub fn has(&self, index: u32) -> bool {
        self.high_priority.has(index) || self.normal_priority.has(index)
    }

    pub fn insert_high(&mut self, first: u32, last: u32) {
        self.high_priority.insert(first, last);
    }

    pub fn insert_normal(&mut self, first: u32, last: u32) {
        self.normal_priority.insert(first, last);
    }

    pub fn remove_high(&mut self, first: u32, last: u32) {
        self.high_priority.erase(first, last);
    }

    pub fn remove_normal(&mut self, first: u32, last: u32) {
        self.normal_priority.erase(first, last);
    }

    pub fn clear(&mut self) {
        self.high_priority.clear();
        self.normal_priority.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn priority_ranges_are_inclusive() {
        let mut data = DownloadData::new();
        data.insert_high(2, 5);
        data.insert_normal(10, 13);

        assert!(data.has(2));
        assert!(data.has(4));
        assert!(!data.has(5));
        assert!(data.has(12));
        assert!(!data.has(13));

        data.remove_high(3, 5);
        assert!(data.has(2));
        assert!(!data.has(3));
        assert!(!data.has(4));
    }
}
