use lt_common::hash_string::HashString;
use std::collections::VecDeque;

use crate::chunk_list::ChunkHandle;
use crate::hash_chunk::HashChunk;

pub type SlotHashDone = Box<dyn Fn(ChunkHandle, Option<HashString>) + Send + Sync>;

pub struct HashQueueNode {
    id: usize,
    chunk: HashChunk,
    on_done: SlotHashDone,
}

impl HashQueueNode {
    pub fn new(id: usize, chunk: HashChunk, on_done: SlotHashDone) -> Self {
        Self { id, chunk, on_done }
    }

    pub fn id(&self) -> usize {
        self.id
    }

    pub fn handle(&self) -> &ChunkHandle {
        self.chunk.handle()
    }

    pub fn chunk(&self) -> &HashChunk {
        &self.chunk
    }

    pub fn chunk_mut(&mut self) -> &mut HashChunk {
        &mut self.chunk
    }
}

#[derive(Default)]
pub struct HashQueue {
    queue: VecDeque<HashQueueNode>,
}

impl HashQueue {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn size(&self) -> usize {
        self.queue.len()
    }

    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    pub fn front(&self) -> Option<&HashQueueNode> {
        self.queue.front()
    }

    pub fn back(&self) -> Option<&HashQueueNode> {
        self.queue.back()
    }

    pub fn push_back<F>(&mut self, handle: ChunkHandle, id: usize, data: Vec<u8>, on_done: F)
    where
        F: Fn(ChunkHandle, Option<HashString>) + Send + Sync + 'static,
    {
        let hash_chunk = HashChunk::new(handle, data);
        self.queue.push_back(HashQueueNode::new(id, hash_chunk, Box::new(on_done)));
    }

    pub fn has(&self, id: usize) -> bool {
        self.queue.iter().any(|n| n.id() == id)
    }

    pub fn has_index(&self, id: usize, index: u32) -> bool {
        self.queue.iter().any(|n| n.id() == id && n.handle().index() == index)
    }

    pub fn remove(&mut self, id: usize) {
        let mut remaining = VecDeque::new();
        for node in self.queue.drain(..) {
            if node.id == id {
                (node.on_done)(node.chunk.handle().clone(), None);
            } else {
                remaining.push_back(node);
            }
        }
        self.queue = remaining;
    }

    pub fn clear(&mut self) {
        self.queue.clear();
    }

    /// Performs hashing work on queued chunks
    pub fn work(&mut self) {
        while let Some(mut node) = self.queue.pop_front() {
            let handle = node.chunk.handle().clone();
            let hash = node.chunk.calculate_hash();
            (node.on_done)(handle, Some(hash));
        }
    }
}
