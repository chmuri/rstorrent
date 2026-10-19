use lt_common::hash_string::HashString;

use crate::node::DhtNode;

pub const NUM_NODES: usize = 8;

#[derive(Clone, Debug)]
pub struct DhtBucket {
    pub begin: HashString,
    pub end: HashString,
    pub nodes: Vec<DhtNode>,
    pub last_changed: u32,
}

impl DhtBucket {
    pub fn new(begin: HashString, end: HashString) -> Self {
        Self {
            begin,
            end,
            nodes: Vec::with_capacity(NUM_NODES),
            last_changed: 0,
        }
    }

    pub fn is_in_range(&self, id: &HashString) -> bool {
        self.begin <= *id && *id <= self.end
    }

    pub fn size(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    pub fn is_full(&self) -> bool {
        self.nodes.len() >= NUM_NODES
    }

    pub fn has_space(&self) -> bool {
        !self.is_full() || self.num_bad() > 0
    }

    pub fn num_good(&self) -> usize {
        self.nodes.iter().filter(|n| n.is_good()).count()
    }

    pub fn num_bad(&self) -> usize {
        self.nodes.iter().filter(|n| n.is_bad()).count()
    }

    pub fn add_node(&mut self, node: DhtNode) {
        self.nodes.push(node);
        self.touch(0);
    }

    pub fn remove_node(&mut self, id: &HashString) -> Option<DhtNode> {
        if let Some(pos) = self.nodes.iter().position(|n| n.id == *id) {
            Some(self.nodes.remove(pos))
        } else {
            None
        }
    }

    pub fn touch(&mut self, now: u32) {
        self.last_changed = now;
    }

    pub fn set_time(&mut self, time: u32) {
        self.last_changed = time;
    }

    pub fn find_replacement_candidate(&self, only_oldest: bool) -> Option<usize> {
        if self.nodes.is_empty() {
            return None;
        }

        let mut oldest_idx = None;
        let mut oldest_time = u32::MAX;

        for (i, node) in self.nodes.iter().enumerate() {
            if node.is_bad() && !only_oldest {
                return Some(i);
            }

            if oldest_idx.is_none() || node.last_seen < oldest_time {
                oldest_time = node.last_seen;
                oldest_idx = Some(i);
            }
        }

        oldest_idx
    }

    pub fn get_mid_point(&self) -> HashString {
        let mut middle = self.end;
        let b_slice = self.begin.as_slice();
        let e_slice = self.end.as_slice();

        for i in 0..HashString::size_data() {
            if b_slice[i] != e_slice[i] {
                let sum = (b_slice[i] as u16) + (e_slice[i] as u16);
                middle.as_mut_slice()[i] = (sum / 2) as u8;
                break;
            }
        }
        middle
    }

    pub fn split(&mut self, _target_id: &HashString) -> DhtBucket {
        let mid_range = self.get_mid_point();
        let mut other = DhtBucket::new(self.begin, mid_range);

        // self.begin = mid_range + 1
        let mut carry = 1u16;
        for i in (0..HashString::size_data()).rev() {
            let sum = (mid_range.as_slice()[i] as u16) + carry;
            self.begin.as_mut_slice()[i] = (sum & 0xFF) as u8;
            carry = sum >> 8;
        }

        // Partition nodes: nodes that fit in `other` range are moved there
        let mut i = 0;
        while i < self.nodes.len() {
            if other.is_in_range(&self.nodes[i].id) {
                let node = self.nodes.remove(i);
                other.nodes.push(node);
            } else {
                i += 1;
            }
        }

        other.last_changed = self.last_changed;
        other
    }

    pub fn full_bucket(&self, buffer: &mut Vec<u8>) {
        for node in &self.nodes {
            node.store_compact(buffer);
        }
    }
}
