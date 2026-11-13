//! Loom model of the rt-core download lock ordering (plan.md :261).
//!
//! Loom cannot intercept `std::sync`, so exercising the real types would mean
//! migrating every sync primitive in rt-core/lt-engine behind `cfg(loom)`. This
//! is instead a faithful *model* of the documented acquisition order, matched
//! against the concrete types in the tree:
//!
//! ```text
//! RwLock<DownloadList> -> RwLock<Download> -> Mutex<EngineDownload> -> Mutex<PeerPool>
//! ```
//!
//! (`DownloadList { downloads: Vec<Arc<RwLock<Download>>> }`,
//!  `Download { download: Arc<Mutex<EngineDownload>> }`,
//!  `PeerPoolHandle = Arc<Mutex<PeerPool>>`.)
//!
//! Loom exhaustively explores thread interleavings to prove the fixed order is
//! deadlock-free. Reversing the order does deadlock, but loom reacts by
//! aborting the process (SIGABRT during guard/Arc drop unwind), which
//! `#[should_panic]` cannot catch — so no negative-control test is included.

use loom::sync::{Arc, Mutex, RwLock};
use loom::thread;

/// The four lock levels, outermost first, mirroring rt-core's nesting.
struct Locks {
    download_list: RwLock<Vec<usize>>,
    download: RwLock<usize>,
    engine: Mutex<usize>,
    peer_pool: Mutex<usize>,
}

impl Locks {
    fn new() -> Self {
        Self {
            download_list: RwLock::new(vec![0]),
            download: RwLock::new(0),
            engine: Mutex::new(0),
            peer_pool: Mutex::new(0),
        }
    }
}

#[test]
fn fixed_order_is_deadlock_free() {
    loom::model(|| {
        let locks = Arc::new(Locks::new());
        let mut handles = Vec::new();

        for _ in 0..2 {
            let locks = Arc::clone(&locks);
            handles.push(thread::spawn(move || {
                let _list = locks.download_list.read().unwrap();
                let mut download = locks.download.write().unwrap();
                let mut engine = locks.engine.lock().unwrap();
                let mut pool = locks.peer_pool.lock().unwrap();
                *download += 1;
                *engine += 1;
                *pool += 1;
            }));
        }

        for handle in handles {
            handle.join().unwrap();
        }

        assert_eq!(*locks.download.read().unwrap(), 2);
        assert_eq!(*locks.engine.lock().unwrap(), 2);
        assert_eq!(*locks.peer_pool.lock().unwrap(), 2);
    });
}
