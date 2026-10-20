use std::collections::HashMap;
use std::os::unix::io::RawFd;
use std::sync::{Arc, Mutex};

use crate::error::NetError;

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum SocketCategory {
    Generic = 0,
    Http = 1,
    Internal = 2,
    Rpc = 3,
    Files = 4,
}

pub const CATEGORY_COUNT: usize = 5;
pub const CATEGORY_MAX_ALLOC: u32 = 1_000_000;
pub const HTTP_MAX_ALLOC: u32 = 4096;
pub const FILES_MAX_ALLOC: u32 = 1 << 16;
pub const FILES_MIN_ALLOC: u32 = 4;
pub const ALLOCATION_HEADROOM: u32 = 8;

pub fn calculate_alloc_limit(category: SocketCategory) -> u32 {
    match category {
        SocketCategory::Http => HTTP_MAX_ALLOC,
        SocketCategory::Files => FILES_MAX_ALLOC,
        _ => CATEGORY_MAX_ALLOC,
    }
}

pub fn calculate_alloc_minimum(category: SocketCategory) -> u32 {
    match category {
        SocketCategory::Files => FILES_MIN_ALLOC,
        _ => 0,
    }
}

pub fn calculate_min_generic(open_max: u32) -> u32 {
    if open_max >= 16384 {
        12288
    } else if open_max >= 8096 {
        6144
    } else if open_max >= 1024 {
        512
    } else {
        256
    }
}

pub fn calculate_reserved(open_max: u32) -> u32 {
    if open_max >= 16384 {
        512
    } else if open_max >= 8096 {
        256
    } else if open_max >= 1024 {
        128
    } else {
        64
    }
}

pub fn calculate_http(open_max: u32) -> u32 {
    if open_max >= 16384 {
        128
    } else if open_max >= 8096 {
        64
    } else if open_max >= 1024 {
        32
    } else {
        16
    }
}

pub fn calculate_internal(open_max: u32) -> u32 {
    if open_max >= 16384 {
        64
    } else if open_max >= 1024 {
        32
    } else {
        16
    }
}

pub fn calculate_rpc(open_max: u32) -> u32 {
    if open_max >= 16384 {
        64
    } else if open_max >= 8096 {
        48
    } else if open_max >= 1024 {
        32
    } else {
        16
    }
}

pub fn calculate_files(open_max: u32) -> u32 {
    if open_max >= 32768 {
        4096
    } else if open_max >= 16384 {
        2048
    } else if open_max >= 8096 {
        1024
    } else if open_max >= 1024 {
        128
    } else {
        64
    }
}

#[derive(Clone, Debug)]
pub struct SocketInfo {
    pub fd: RawFd,
    pub category: SocketCategory,
}

struct Inner {
    managed_size: u32,
    unmanaged_size: u32,
    max_size: u32,

    category_managed_size: [u32; CATEGORY_COUNT],
    category_max_size: [u32; CATEGORY_COUNT],

    category_min_alloc: [u32; CATEGORY_COUNT],
    category_max_alloc: [u32; CATEGORY_COUNT],

    sockets: HashMap<RawFd, SocketInfo>,
    subscribers: Vec<Box<dyn Fn() + Send + Sync + 'static>>,
}

#[derive(Clone)]
pub struct SocketManager {
    inner: Arc<Mutex<Inner>>,
}

impl Default for SocketManager {
    fn default() -> Self {
        Self::new()
    }
}

impl SocketManager {
    pub fn new() -> Self {
        let mut inner = Inner {
            managed_size: 0,
            unmanaged_size: 0,
            max_size: 1024,
            category_managed_size: [0; CATEGORY_COUNT],
            category_max_size: [0; CATEGORY_COUNT],
            category_min_alloc: [0; CATEGORY_COUNT],
            category_max_alloc: [CATEGORY_MAX_ALLOC; CATEGORY_COUNT],
            sockets: HashMap::new(),
            subscribers: Vec::new(),
        };

        // Initialize with default allocation for 1024
        let _ = Self::adjust_allocation_inner(&mut inner, 1024);

        Self {
            inner: Arc::new(Mutex::new(inner)),
        }
    }

    pub fn size(&self) -> u32 {
        let inner = self.inner.lock().unwrap();
        inner.managed_size + inner.unmanaged_size
    }

    pub fn max_size(&self) -> u32 {
        let inner = self.inner.lock().unwrap();
        inner.max_size
    }

    pub fn category_managed_size(&self, category: SocketCategory) -> u32 {
        let inner = self.inner.lock().unwrap();
        inner.category_managed_size[category as usize]
    }

    pub fn category_max_size(&self, category: SocketCategory) -> u32 {
        let inner = self.inner.lock().unwrap();
        inner.category_max_size[category as usize]
    }

    pub fn category_min_allocation(&self, category: SocketCategory) -> Result<u32, NetError> {
        if category == SocketCategory::Generic {
            return Err(NetError::Internal(
                "SocketManager::category_min_allocation(): generic has no min alloc".into(),
            ));
        }
        let inner = self.inner.lock().unwrap();
        Ok(inner.category_min_alloc[category as usize])
    }

    pub fn category_max_allocation(&self, category: SocketCategory) -> Result<u32, NetError> {
        if category == SocketCategory::Generic {
            return Err(NetError::Internal(
                "SocketManager::category_max_allocation(): generic has no max alloc".into(),
            ));
        }
        let inner = self.inner.lock().unwrap();
        Ok(inner.category_max_alloc[category as usize])
    }

    pub fn set_category_min_allocation(
        &self,
        category: SocketCategory,
        min_alloc: u32,
    ) -> Result<(), NetError> {
        if category == SocketCategory::Generic {
            return Err(NetError::Internal(
                "SocketManager::set_category_min_allocation(): generic invalid".into(),
            ));
        }
        let mut inner = self.inner.lock().unwrap();
        inner.category_min_alloc[category as usize] = min_alloc;
        Ok(())
    }

    pub fn set_category_max_allocation(
        &self,
        category: SocketCategory,
        max_alloc: u32,
    ) -> Result<(), NetError> {
        if category == SocketCategory::Generic {
            return Err(NetError::Internal(
                "SocketManager::set_category_max_allocation(): generic invalid".into(),
            ));
        }
        let mut inner = self.inner.lock().unwrap();
        inner.category_max_alloc[category as usize] = max_alloc;
        Ok(())
    }

    pub fn set_max_size_and_adjust(&self, max_open: u32) -> Result<(), NetError> {
        if max_open < 512 {
            return Err(NetError::Input(
                "set_max_size_and_adjust: max_open too low, minimum is 512".into(),
            ));
        }
        let mut inner = self.inner.lock().unwrap();
        Self::adjust_allocation_inner(&mut inner, max_open)
    }

    pub fn adjust_allocation(&self) -> Result<(), NetError> {
        let mut inner = self.inner.lock().unwrap();
        let max_open = inner.max_size;
        Self::adjust_allocation_inner(&mut inner, max_open)
    }

    fn adjust_allocation_inner(inner: &mut Inner, max_open: u32) -> Result<(), NetError> {
        let mut total_allocated: u32 = 0;
        let mut new_max_size = [0u32; CATEGORY_COUNT];

        let categories_to_calc = [
            (SocketCategory::Internal, calculate_internal(max_open)),
            (SocketCategory::Http, calculate_http(max_open)),
            (SocketCategory::Rpc, calculate_rpc(max_open)),
            (SocketCategory::Files, calculate_files(max_open)),
        ];

        for (category, mut alloc) in categories_to_calc {
            let cat_idx = category as usize;
            alloc = alloc.max(inner.category_min_alloc[cat_idx]);
            alloc = alloc.min(inner.category_max_alloc[cat_idx]);

            let limit = calculate_alloc_limit(category);
            if alloc > limit {
                return Err(NetError::Input(format!(
                    "adjust_allocation: allocation exceeds category limit: {} > {}",
                    alloc, limit
                )));
            }

            let min_limit = calculate_alloc_minimum(category);
            if alloc < min_limit {
                return Err(NetError::Input(format!(
                    "adjust_allocation: allocation below category minimum: {} < {}",
                    alloc, min_limit
                )));
            }

            total_allocated += alloc;
            new_max_size[cat_idx] = alloc;
        }

        total_allocated += calculate_reserved(max_open);
        let min_generic = calculate_min_generic(max_open);

        if total_allocated + min_generic + ALLOCATION_HEADROOM > max_open {
            return Err(NetError::Input(format!(
                "adjust_allocation: total + min_generic + 8 reserve exceeds max_open : {} + {} + 8 > {}",
                total_allocated, min_generic, max_open
            )));
        }

        new_max_size[SocketCategory::Generic as usize] = max_open - total_allocated;
        inner.max_size = max_open;
        inner.category_max_size = new_max_size;

        for sub in &inner.subscribers {
            sub();
        }

        Ok(())
    }

    pub fn generic_min_allocation(&self) -> u32 {
        let inner = self.inner.lock().unwrap();
        calculate_min_generic(inner.max_size)
    }

    pub fn reserved_allocation(&self) -> u32 {
        let inner = self.inner.lock().unwrap();
        calculate_reserved(inner.max_size)
    }

    pub fn available_allocation(&self) -> u32 {
        let inner = self.inner.lock().unwrap();
        let max_open = inner.max_size;
        let allocated =
            calculate_min_generic(max_open) + calculate_reserved(max_open) + ALLOCATION_HEADROOM;
        if allocated >= max_open {
            0
        } else {
            max_open - allocated
        }
    }

    pub fn can_open_socket(&self, category: SocketCategory) -> bool {
        let inner = self.inner.lock().unwrap();
        let total = inner.managed_size + inner.unmanaged_size;
        if total >= inner.max_size {
            return false;
        }
        let cat_max = inner.category_max_size[category as usize];
        if cat_max != 0 {
            inner.category_managed_size[category as usize] < cat_max
        } else {
            true
        }
    }

    pub fn add_unmanaged_socket(&self) {
        let mut inner = self.inner.lock().unwrap();
        inner.unmanaged_size += 1;
    }

    pub fn remove_unmanaged_socket(&self) -> Result<(), NetError> {
        let mut inner = self.inner.lock().unwrap();
        if inner.unmanaged_size == 0 {
            return Err(NetError::Internal(
                "remove_unmanaged_socket(): no unmanaged sockets to remove".into(),
            ));
        }
        inner.unmanaged_size -= 1;
        Ok(())
    }

    pub fn record_open_socket(&self, fd: RawFd, category: SocketCategory) -> Result<(), NetError> {
        let mut inner = self.inner.lock().unwrap();
        if inner.sockets.contains_key(&fd) {
            return Err(NetError::Internal(format!(
                "record_open_socket(): fd {} already open",
                fd
            )));
        }
        inner.sockets.insert(fd, SocketInfo { fd, category });
        inner.managed_size += 1;
        inner.category_managed_size[category as usize] += 1;
        Ok(())
    }

    pub fn record_close_socket(&self, fd: RawFd) -> Result<(), NetError> {
        let mut inner = self.inner.lock().unwrap();
        let info = inner.sockets.remove(&fd).ok_or_else(|| {
            NetError::Internal(format!(
                "record_close_socket(): fd {} not found in socket map",
                fd
            ))
        })?;
        if inner.managed_size == 0 {
            return Err(NetError::Internal(
                "record_close_socket(): managed_size underflow".into(),
            ));
        }
        let cat_idx = info.category as usize;
        if inner.category_managed_size[cat_idx] == 0 {
            return Err(NetError::Internal(
                "record_close_socket(): category_managed_size underflow".into(),
            ));
        }
        inner.managed_size -= 1;
        inner.category_managed_size[cat_idx] -= 1;
        Ok(())
    }

    pub fn subscribe_to_changes<F>(&self, callback: F)
    where
        F: Fn() + Send + Sync + 'static,
    {
        let mut inner = self.inner.lock().unwrap();
        inner.subscribers.push(Box::new(callback));
    }
}
