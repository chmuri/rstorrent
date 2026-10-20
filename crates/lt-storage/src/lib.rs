pub mod block;
pub mod chunk;
pub mod chunk_iterator;
pub mod chunk_list;
pub mod chunk_manager;
pub mod chunk_part;
pub mod chunk_utils;
pub mod disk_thread;
pub mod download_data;
pub mod error;
pub mod file;
pub mod file_list;
pub mod file_list_iterator;
pub mod file_manager;
pub mod file_status_cache;
pub mod file_utils;
pub mod hash_check_queue;
pub mod hash_chunk;
pub mod hash_queue;
pub mod hash_torrent;
pub mod memory_chunk;
pub mod memory_manager;
pub mod mmap_safe;
pub mod piece;
pub mod resume;
pub mod socket_file;
pub mod writer;

pub use block::{Block, BlockList, BlockState, BlockTransfer, TransferList, DEFAULT_BLOCK_SIZE};
pub use chunk::Chunk;
pub use chunk_iterator::ChunkIterator;
pub use chunk_list::{
    ChunkHandle, ChunkList, ChunkListNode, ChunkSyncFlags, GetFlags, ReleaseFlags,
};
pub use chunk_manager::{ChunkManager, ALLOCATE_DONT_LOG, ALLOCATE_REVERT_LOG};
pub use chunk_part::{ChunkPart, MappedType};
pub use disk_thread::{
    global_disk_thread, shutdown_global_disk_thread, DiskThread, DiskWriteHandle,
    DEFAULT_DISK_QUEUE, DEFAULT_IDLE_TIMEOUT, DEFAULT_MAX_OPEN_FILES,
};
pub use error::StorageError;
pub use file::{File, FileFlags, Priority};
pub use file_list::FileList;
pub use file_manager::FileManager;
pub use file_status_cache::{FileStatus, FileStatusCache};
pub use hash_check_queue::{HashCheckQueue, FLAG_NEED, FLAG_WILL_NEED};
pub use hash_chunk::HashChunk;
pub use hash_queue::{HashQueue, HashQueueNode};
pub use hash_torrent::{HashResult, HashTorrent};
pub use memory_chunk::MemoryChunk;
pub use memory_manager::{memory_manager, MemoryManager};
pub use mmap_safe::{
    allocate_file, system_page_size, Advice, MapFlags, MappedMemory, Prot, SyncFlags,
};
pub use piece::Piece;
pub use resume::{ResumeData, MTIME_RECHECK, MTIME_SKIPPED, MTIME_UNCHECKED};
pub use socket_file::{FallocateFlags, OpenFlags, SocketFile, INVALID_FD};
pub use writer::{write_piece_to_disk, TargetFileInfo};

#[cfg(test)]
mod tests {
    use super::*;
    use lt_common::bitfield::Bitfield;
    use lt_common::hash_string::HashString;
    use std::fs;
    use std::sync::{Arc, Mutex};

    #[test]
    fn test_memory_chunk_alignment_math() {
        let ps = MemoryChunk::page_size();
        assert!(ps >= 4096);

        // Simulated chunk with page_align = 100, length = 5000
        let vec = vec![0u8; ps * 3];
        let inner = std::sync::Arc::new(MappedMemory::from_vec(vec));
        let chunk = MemoryChunk::from_mapped(inner, 100, 5000, Prot::READ | Prot::WRITE).unwrap();

        assert_eq!(chunk.page_align(), 100);
        assert_eq!(chunk.size(), 5000);
        assert!(chunk.is_valid());
        assert!(chunk.is_readable());
        assert!(chunk.is_writable());
        assert!(!chunk.is_exec());

        // pages touched: (5000 + 100 + ps - 1) / ps
        let expected_pages = (5000 + 100 + ps - 1) / ps;
        assert_eq!(chunk.pages_touched(0, 5000), expected_pages);

        // size_aligned
        let total = 100 + 5000;
        let expected_aligned = ((total + ps - 1) / ps) * ps;
        assert_eq!(chunk.size_aligned(), expected_aligned);

        // align_pair: offset becomes page-aligned (0), len becomes len + (offset % ps) = 1000 + 300 = 1300
        let mut off = 200;
        let mut len = 1000;
        chunk.align_pair(&mut off, &mut len);
        assert_eq!(off % ps, 0);
        assert_eq!(len, 1300);
    }

    #[test]
    fn test_memory_chunk_buffer_read_write() {
        let mut chunk = MemoryChunk::from_vec(vec![1, 2, 3, 4, 5], Prot::READ | Prot::WRITE);
        assert_eq!(chunk.as_slice(), &[1, 2, 3, 4, 5]);

        chunk.as_mut_slice()[0] = 42;
        assert_eq!(chunk.as_slice(), &[42, 2, 3, 4, 5]);

        assert!(chunk.is_incore(0, 5).unwrap());
    }

    #[test]
    fn test_anonymous_padding_chunk() {
        let mut chunk =
            SocketFile::create_padding_chunk(8192, Prot::READ | Prot::WRITE, MapFlags::SHARED)
                .unwrap();

        assert!(chunk.is_valid());
        assert_eq!(chunk.size(), 8192);

        let slice = chunk.as_mut_slice();
        slice[0] = 0xAA;
        slice[8191] = 0xBB;

        assert_eq!(chunk.as_slice()[0], 0xAA);
        assert_eq!(chunk.as_slice()[8191], 0xBB);

        chunk.advise(0, 8192, Advice::Normal).unwrap();
    }

    #[test]
    fn test_socket_file_lifecycle_and_mmap() {
        let temp_dir = std::env::temp_dir();
        let file_path = temp_dir.join(format!("rtorrent_test_file_{}", std::process::id()));

        // Cleanup if existing
        let _ = fs::remove_file(&file_path);

        let mut file = SocketFile::new();
        assert!(!file.is_open());

        file.open(
            &file_path,
            Prot::READ | Prot::WRITE,
            OpenFlags::CREATE | OpenFlags::TRUNCATE,
            Some(0o600),
        )
        .unwrap();

        assert!(file.is_open());

        // Allocate / extend file size to 16KB
        let target_size = 16 * 1024;
        file.allocate(target_size, FallocateFlags::FALLOCATE)
            .unwrap();
        assert_eq!(file.size().unwrap(), target_size);

        // Map a chunk at offset 100 with length 4000
        let mut chunk = file
            .create_chunk(100, 4000, Prot::READ | Prot::WRITE, MapFlags::SHARED)
            .unwrap();

        assert!(chunk.is_valid());
        assert_eq!(chunk.size(), 4000);
        assert_eq!(chunk.page_align(), 100);

        // Write data into the memory chunk
        let data = chunk.as_mut_slice();
        data[0] = 0xDE;
        data[1] = 0xAD;
        data[2] = 0xBE;
        data[3] = 0xEF;

        // Sync to disk
        chunk.sync(0, 4000, SyncFlags::SYNC).unwrap();

        // Check is_incore
        assert!(chunk.is_incore(0, 4000).unwrap());

        // Close socket file and unmap chunk
        drop(chunk);
        file.close();

        // Read directly from disk to verify data was flushed
        let file_bytes = fs::read(&file_path).unwrap();
        assert_eq!(file_bytes[100], 0xDE);
        assert_eq!(file_bytes[101], 0xAD);
        assert_eq!(file_bytes[102], 0xBE);
        assert_eq!(file_bytes[103], 0xEF);

        // Cleanup test file
        let _ = fs::remove_file(&file_path);
    }

    #[test]
    fn test_memory_chunk_edge_cases() {
        let mut chunk = MemoryChunk::new();
        assert!(!chunk.is_valid());
        assert_eq!(chunk.size(), 0);
        assert!(chunk.is_empty());
        assert_eq!(chunk.as_slice(), &[]);
        assert_eq!(chunk.as_mut_slice(), &mut []);
        assert!(!chunk.is_valid_range(0, 10));

        let mut populated = MemoryChunk::from_vec(vec![1, 2, 3], Prot::READ);
        assert!(populated.is_valid());
        assert!(populated.is_readable());
        assert!(!populated.is_writable());
        assert_eq!(populated.as_mut_slice(), &mut []); // not writable

        populated.clear();
        assert!(!populated.is_valid());
        assert_eq!(populated.size(), 0);
    }

    #[test]
    fn test_socket_file_errors_and_resize() {
        let mut file = SocketFile::new();
        assert!(!file.is_open());
        assert!(matches!(file.size(), Err(StorageError::FileNotOpen)));
        assert!(matches!(file.set_size(100), Err(StorageError::FileNotOpen)));
        assert!(matches!(
            file.allocate(100, FallocateFlags::NONE),
            Err(StorageError::FileNotOpen)
        ));
        assert!(matches!(
            file.create_chunk(0, 10, Prot::READ, MapFlags::SHARED),
            Err(StorageError::FileNotOpen)
        ));

        let temp_dir = std::env::temp_dir();
        let file_path = temp_dir.join(format!("rtorrent_test_resize_{}", std::process::id()));
        let _ = fs::remove_file(&file_path);

        // Open with no protection flags -> error
        assert!(matches!(
            file.open(&file_path, Prot::NONE, OpenFlags::CREATE, None),
            Err(StorageError::NoProtectionFlags)
        ));

        // Open successfully
        file.open(
            &file_path,
            Prot::READ | Prot::WRITE,
            OpenFlags::CREATE | OpenFlags::TRUNCATE,
            Some(0o644),
        )
        .unwrap();

        // Resize via set_size (ftruncate)
        file.set_size(2048).unwrap();
        assert_eq!(file.size().unwrap(), 2048);

        // create_chunk beyond file size returns empty invalid chunk
        let invalid_chunk = file
            .create_chunk(3000, 100, Prot::READ, MapFlags::SHARED)
            .unwrap();
        assert!(!invalid_chunk.is_valid());

        file.close();
        let _ = fs::remove_file(&file_path);
    }

    #[test]
    fn test_chunk_multi_part_read_write() {
        let mut chunk = Chunk::new();
        assert!(chunk.is_empty());
        assert_eq!(chunk.chunk_size(), 0);

        // Add Part 1 (1000 bytes)
        let part1 = MemoryChunk::from_vec(vec![0u8; 1000], Prot::READ | Prot::WRITE);
        chunk.push_back(MappedType::Static, part1);

        // Add Part 2 (2000 bytes)
        let part2 = MemoryChunk::from_vec(vec![0u8; 2000], Prot::READ | Prot::WRITE);
        chunk.push_back(MappedType::Static, part2);

        assert_eq!(chunk.chunk_size(), 3000);
        assert_eq!(chunk.parts().len(), 2);
        assert!(chunk.is_readable());
        assert!(chunk.is_writable());

        // Write a payload of 1500 bytes spanning across part 1 (500 bytes) and part 2 (1000 bytes)
        let mut payload = vec![0u8; 1500];
        for (i, b) in payload.iter_mut().enumerate() {
            *b = (i % 256) as u8;
        }

        chunk.from_buffer(&payload, 500, 1500).unwrap();

        // Verify with compare_buffer
        assert!(chunk.compare_buffer(&payload, 500, 1500).unwrap());

        // Verify with to_buffer
        let mut read_back = vec![0u8; 1500];
        chunk.to_buffer(&mut read_back, 500, 1500).unwrap();
        assert_eq!(read_back, payload);

        // Verify part 1 and part 2 slices directly
        let p1_slice = chunk.parts()[0].chunk().as_slice();
        assert_eq!(&p1_slice[500..1000], &payload[0..500]);
        let p2_slice = chunk.parts()[1].chunk().as_slice();
        assert_eq!(&p2_slice[0..1000], &payload[500..1500]);
    }

    #[test]
    fn test_chunk_buffer_rejects_wrapping_range() {
        // Port of CppUnit test_chunk::test_buffer_rejects_wrapping_range
        let mut chunk = Chunk::new();
        let mem = MemoryChunk::from_vec(vec![b'x'; 4096], Prot::READ | Prot::WRITE);
        chunk.push_back(MappedType::Mmap, mem);

        let position = 2048u32;
        let length = u32::MAX - 1791; // 2048 + length wraps to 256 in 32-bit arithmetic

        assert_eq!(position.wrapping_add(length), 256);

        let mut buffer = [0u8; 4096];

        assert!(matches!(
            chunk.to_buffer(&mut buffer, position, length),
            Err(StorageError::InvalidRange { .. })
        ));
        assert!(matches!(
            chunk.from_buffer(&buffer, position, length),
            Err(StorageError::InvalidRange { .. })
        ));
        assert!(matches!(
            chunk.compare_buffer(&buffer, position, length),
            Err(StorageError::InvalidRange { .. })
        ));
    }

    #[test]
    fn test_chunk_iterator_and_address_lookup() {
        let mut chunk = Chunk::new();
        let p1 = MemoryChunk::from_vec(vec![10, 20, 30], Prot::READ | Prot::WRITE);
        let p2 = MemoryChunk::from_vec(vec![40, 50, 60, 70], Prot::READ | Prot::WRITE);
        chunk.push_back(MappedType::Static, p1);
        chunk.push_back(MappedType::Static, p2);

        // Test at_position and at_memory
        let p1_idx = chunk.at_position(2).unwrap();
        assert_eq!(p1_idx, 0);
        let p2_idx = chunk.at_position(3).unwrap();
        assert_eq!(p2_idx, 1);

        let (ptr, rem) = chunk.at_memory(2, 0).unwrap();
        assert_eq!(rem, 1);
        assert_eq!(unsafe { *ptr }, 30);

        // Test find_address
        assert_eq!(chunk.find_address(ptr), Some(0));

        // Test ChunkIterator across range [1, 5)
        let mut itr = ChunkIterator::new(&chunk, 1, 5).unwrap();
        assert!(!itr.is_empty());
        assert_eq!(itr.current_slice(), &[20, 30]); // part 0 from pos 1 to 3

        assert!(itr.next());
        assert_eq!(itr.current_slice(), &[40, 50]); // part 1 from pos 3 to 5

        assert!(!itr.next());
    }

    #[test]
    fn test_chunk_list_basic() {
        let chunk_manager = Arc::new(ChunkManager::new());
        let mut chunk_list = ChunkList::new();

        assert_eq!(chunk_list.flags(), 0);
        assert_eq!(chunk_list.chunk_size(), 0);

        chunk_list.set_chunk_size(1 << 16);
        chunk_list.set_manager(chunk_manager);
        chunk_list.resize(32);

        assert_eq!(chunk_list.size(), 32);
        assert_eq!(chunk_list.chunk_size(), 1 << 16);

        for i in 0..32 {
            assert_eq!(chunk_list.node(i).index(), i as u32);
        }
    }

    fn setup_test_chunk_list() -> ChunkList {
        let chunk_manager = Arc::new(ChunkManager::new());
        let mut chunk_list = ChunkList::new();
        chunk_list.set_chunk_size(10);
        chunk_list.set_manager(chunk_manager);
        chunk_list.resize(32);

        chunk_list.set_slot_create_chunk(|index, prot| {
            let mut chunk = Chunk::new();
            let mem = MemoryChunk::from_vec(vec![index as u8; 10], prot);
            chunk.push_back(MappedType::Mmap, mem);
            Ok(chunk)
        });

        chunk_list
    }

    #[test]
    fn test_chunk_list_get_release() {
        let mut chunk_list = setup_test_chunk_list();

        assert!(!chunk_list.node(0).is_valid());

        // Get read-only handle for chunk 0
        let mut handle_0 = chunk_list.get(0, GetFlags::GET_NOT_HASHING);
        assert!(handle_0.is_valid());
        assert_eq!(handle_0.index(), 0);
        assert!(!handle_0.is_writable());
        assert!(!handle_0.is_blocking());

        assert!(chunk_list.node(0).is_valid());
        assert_eq!(chunk_list.node(0).references(), 1);
        assert_eq!(chunk_list.node(0).writable(), 0);
        assert_eq!(chunk_list.node(0).blocking(), 0);

        chunk_list.release(&mut handle_0, ReleaseFlags::DEFAULT);

        // Get writable handle for chunk 1
        let mut handle_1 = chunk_list.get(1, GetFlags::GET_NOT_HASHING | GetFlags::GET_WRITABLE);
        assert!(handle_1.is_valid());
        assert_eq!(handle_1.index(), 1);
        assert!(handle_1.is_writable());
        assert!(!handle_1.is_blocking());

        assert!(chunk_list.node(1).is_valid());
        assert_eq!(chunk_list.node(1).references(), 1);
        assert_eq!(chunk_list.node(1).writable(), 1);
        assert_eq!(chunk_list.node(1).blocking(), 0);

        chunk_list.release(&mut handle_1, ReleaseFlags::DEFAULT);
        assert_eq!(chunk_list.queue_size(), 1); // was modified, queued for sync

        // Get blocking handle for chunk 2
        let mut handle_2 = chunk_list.get(2, GetFlags::GET_NOT_HASHING | GetFlags::GET_BLOCKING);
        assert!(handle_2.is_valid());
        assert_eq!(handle_2.index(), 2);
        assert!(!handle_2.is_writable());
        assert!(handle_2.is_blocking());

        assert!(chunk_list.node(2).is_valid());
        assert_eq!(chunk_list.node(2).references(), 1);
        assert_eq!(chunk_list.node(2).writable(), 0);
        assert_eq!(chunk_list.node(2).blocking(), 1);

        chunk_list.release(&mut handle_2, ReleaseFlags::DEFAULT);
    }

    #[test]
    fn test_chunk_list_blocking() {
        let mut chunk_list = setup_test_chunk_list();

        let mut handle_0_ro = chunk_list.get(0, GetFlags::GET_NOT_HASHING | GetFlags::GET_BLOCKING);
        assert!(handle_0_ro.is_valid());

        // Requesting writable while blocked with GET_NONBLOCK returns EAGAIN
        let mut handle_0_rw = chunk_list.get(
            0,
            GetFlags::GET_NOT_HASHING | GetFlags::GET_WRITABLE | GetFlags::GET_NONBLOCK,
        );
        assert!(!handle_0_rw.is_valid());
        assert_eq!(handle_0_rw.error_number(), libc::EAGAIN);

        chunk_list.release(&mut handle_0_ro, ReleaseFlags::DEFAULT);

        // After releasing blocking handle, writable succeeds
        handle_0_rw = chunk_list.get(0, GetFlags::GET_NOT_HASHING | GetFlags::GET_WRITABLE);
        assert!(handle_0_rw.is_valid());

        chunk_list.release(&mut handle_0_rw, ReleaseFlags::DEFAULT);
    }

    #[test]
    fn test_chunk_list_find_address() {
        let mut chunk_list = setup_test_chunk_list();

        let mut handle_0 = chunk_list.get(0, GetFlags::GET_NOT_HASHING);
        let mut handle_1 = chunk_list.get(1, GetFlags::GET_NOT_HASHING);
        assert!(handle_0.is_valid());
        assert!(handle_1.is_valid());

        let address = chunk_list.node(1).chunk().unwrap().parts()[0]
            .chunk()
            .begin();
        let result = chunk_list.find_address(address);

        assert_eq!(result, Some((1, 0)));

        chunk_list.release(&mut handle_0, ReleaseFlags::DEFAULT);
        chunk_list.release(&mut handle_1, ReleaseFlags::DEFAULT);
    }

    #[test]
    fn test_chunk_list_sync_and_queue() {
        let mut chunk_list = setup_test_chunk_list();

        let mut handle = chunk_list.get(3, GetFlags::GET_NOT_HASHING | GetFlags::GET_WRITABLE);
        assert!(handle.is_valid());

        chunk_list.release(&mut handle, ReleaseFlags::DEFAULT);
        assert_eq!(chunk_list.queue_size(), 1);

        let failed = chunk_list.sync_chunks(ChunkSyncFlags::FORCE | ChunkSyncFlags::SAFE);
        assert_eq!(failed, 0);
        assert_eq!(chunk_list.queue_size(), 0);
    }

    #[test]
    fn test_file_list_initialize_overflow() {
        // Port of test_file_list.cc
        let mut file_list = FileList::new();
        let chunk_size = 1u32 << 20; // 1 MB
        let max_u64 = u64::MAX;

        // test_initialize_rejects_max_u64
        assert!(file_list.initialize(max_u64, chunk_size).is_err());

        // test_initialize_rejects_wrap_boundary
        assert!(file_list
            .initialize(max_u64 - (chunk_size as u64) + 2, chunk_size)
            .is_err());

        // test_initialize_rejects_just_below_wrap_boundary
        assert!(file_list
            .initialize(max_u64 - (chunk_size as u64) + 1, chunk_size)
            .is_err());
    }

    #[test]
    fn test_file_list_multi_file_assembly() {
        let mut file_list = FileList::new();

        let mut f1 = File::new();
        f1.set_offset(0);
        f1.set_size_bytes(1000);

        let mut f2 = File::new();
        f2.set_offset(1000);
        f2.set_size_bytes(2500);

        file_list.push_file(f1);
        file_list.push_file(f2);

        // Total 3500 bytes, chunk size 1024 -> 4 chunks (0, 1, 2, 3)
        file_list.initialize(3500, 1024).unwrap();

        assert_eq!(file_list.size_files(), 2);
        assert_eq!(file_list.size_bytes(), 3500);
        assert_eq!(file_list.size_chunks(), 4);
        assert_eq!(file_list.completed_chunks(), 0);
        assert!(!file_list.is_done());

        // Chunk 0 (0..1024) spans File 1 (0..1000) and File 2 (1000..1024)
        let chunk0 = file_list
            .create_chunk_index(0, Prot::READ | Prot::WRITE)
            .unwrap();
        assert_eq!(chunk0.chunk_size(), 1024);
        assert_eq!(chunk0.parts().len(), 2);
        assert_eq!(chunk0.parts()[0].size(), 1000);
        assert_eq!(chunk0.parts()[1].size(), 24);

        // Chunk 3 (3072..3500 = 428 bytes) is entirely within File 2
        let chunk3 = file_list
            .create_chunk_index(3, Prot::READ | Prot::WRITE)
            .unwrap();
        assert_eq!(chunk3.chunk_size(), 428);
        assert_eq!(chunk3.parts().len(), 1);

        // Mark completed
        file_list.mark_completed(0);
        assert_eq!(file_list.completed_chunks(), 1);
        assert_eq!(file_list.file(0).unwrap().completed_chunks(), 1);
    }

    #[test]
    fn test_file_manager_eviction() {
        // Port of test_file_manager.cc
        let temp_dir = std::env::temp_dir().join(format!("lt_test_fm_{}", std::process::id()));
        fs::create_dir_all(&temp_dir).unwrap();

        let mut manager = FileManager::new();
        manager.set_max_open_files(64);

        let count = 48;
        let mut files: Vec<Box<File>> = Vec::new();

        for i in 0..count {
            let path = temp_dir.join(format!("file_{}", i));
            let mut file = Box::new(File::new());
            file.set_path(&path);

            assert!(manager.open(
                &mut file,
                false,
                Prot::READ | Prot::WRITE,
                OpenFlags::CREATE | OpenFlags::TRUNCATE
            ));

            file.set_last_touched(100 + (i as u64) * 10);
            files.push(file);
        }

        assert_eq!(manager.open_files(), 48);

        // Lower max open files to 47 -> exactly one file must be evicted (file 0)
        manager.set_max_open_files(47);

        assert_eq!(manager.files_closed_counter(), 1);
        assert_eq!(manager.open_files(), 47);

        assert!(!files[0].is_open());
        assert!(files[1].is_open());
        assert!(files[2].is_open());

        // Close remaining files and cleanup
        for file in &mut files {
            manager.close(file);
        }
        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_hash_queue_flow() {
        use std::collections::HashMap;
        use std::sync::Mutex;

        let mut hash_queue = HashQueue::new();
        let done_chunks = Arc::new(Mutex::new(HashMap::new()));

        for i in 0..5 {
            let handle = ChunkHandle::new(i, false, true);
            let data = vec![i as u8; 100];
            let done_map = done_chunks.clone();

            hash_queue.push_back(handle, 1, data.clone(), move |h, hash_opt| {
                if let Some(hash) = hash_opt {
                    done_map.lock().unwrap().insert(h.index(), hash);
                }
            });
        }

        assert_eq!(hash_queue.size(), 5);
        assert!(hash_queue.has(1));
        assert!(hash_queue.has_index(1, 2));

        // Process queue
        hash_queue.work();

        assert_eq!(hash_queue.size(), 0);
        let results = done_chunks.lock().unwrap();
        assert_eq!(results.len(), 5);

        for i in 0..5 {
            let expected_hash = HashString::new(lt_crypto::sha1::sha1_hash(&vec![i as u8; 100]));
            assert_eq!(results.get(&i), Some(&expected_hash));
        }
    }

    #[test]
    fn test_hash_queue_remove() {
        let mut hash_queue = HashQueue::new();
        let cancelled = Arc::new(Mutex::new(0));

        for i in 0..10 {
            let handle = ChunkHandle::new(i, false, true);
            let data = vec![i as u8; 50];
            let canc = cancelled.clone();

            hash_queue.push_back(handle, 42, data, move |_h, hash_opt| {
                if hash_opt.is_none() {
                    *canc.lock().unwrap() += 1;
                }
            });
        }

        assert_eq!(hash_queue.size(), 10);
        hash_queue.remove(42);

        assert_eq!(hash_queue.size(), 0);
        assert_eq!(*cancelled.lock().unwrap(), 10);
    }

    #[test]
    fn test_block_and_block_list_accounting() {
        // Port of test_block::test_completed_skips_erased_not_stalled_accounting
        let mut block_list = BlockList::new(Piece::new(0, 0, 16), 16);
        assert_eq!(block_list.len(), 1);

        let block = &mut block_list[0];

        let leader_idx = block.insert(1001); // peer 1
        let trailing_idx = block.insert(1002); // peer 2

        assert_eq!(block.size_not_stalled(), 2);
        assert!(block.transfering(leader_idx));
        assert!(!block.transfering(trailing_idx));
        assert!(block.get_transfer(trailing_idx).unwrap().is_not_leader());

        // Dissimilar transfer marks trailing as erased
        block.transfer_dissimilar(trailing_idx);
        assert!(block.get_transfer(trailing_idx).unwrap().is_erased());
        assert!(!block.get_transfer(trailing_idx).unwrap().is_valid());
        assert_eq!(block.size_not_stalled(), 1);

        // Adjust position and complete block with leader
        block
            .get_transfer_mut(leader_idx)
            .unwrap()
            .adjust_position(16);
        assert!(block.completed(leader_idx));
        assert!(block.is_finished());

        block_list.inc_finished();
        assert!(block_list.is_all_finished());
    }

    #[test]
    fn test_resume_data_serialization() {
        let mut resume = ResumeData::new();
        let mut bf = Bitfield::new(16);
        bf.set(0);
        bf.set(2);
        bf.set(5);
        resume.bitfield = Some(bf);

        resume.file_priorities = vec![Priority::Normal, Priority::Off, Priority::High];
        resume.file_mtimes = vec![1680000000, MTIME_SKIPPED, MTIME_RECHECK];
        resume.completed_bytes = 40960;

        let obj = resume.to_object();
        let loaded = ResumeData::from_object(&obj).unwrap();

        assert_eq!(loaded.completed_bytes, 40960);
        assert_eq!(
            loaded.file_priorities,
            vec![Priority::Normal, Priority::Off, Priority::High]
        );
        assert_eq!(
            loaded.file_mtimes,
            vec![1680000000, MTIME_SKIPPED, MTIME_RECHECK]
        );

        let loaded_bf = loaded.bitfield.unwrap();
        assert!(loaded_bf.get(0));
        assert!(!loaded_bf.get(1));
        assert!(loaded_bf.get(2));
        assert!(!loaded_bf.get(3));
        assert!(loaded_bf.get(5));
    }
}
