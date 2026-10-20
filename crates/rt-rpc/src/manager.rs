use crate::command_map::CommandMap;
use crate::error::RpcError;
use crate::object_storage::ObjectStorage;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

pub const MIN_SIZE_LIMIT: u64 = 1024;
pub const MAX_CONTENT_SIZE: u64 = 1 << 26; // 64 MiB

pub struct RpcManager {
    commands: CommandMap,
    storage: ObjectStorage,
    size_limit: u64,
    dialect: i32,
    use_xmlrpc: bool,
    use_jsonrpc: bool,
    trusted: bool,
    handlers_initialized: bool,
    scgi_allow_compression: AtomicBool,
    scgi_min_compress_size: AtomicU32,
}

impl Default for RpcManager {
    fn default() -> Self {
        Self::new()
    }
}

impl RpcManager {
    pub fn new() -> Self {
        Self {
            commands: CommandMap::new(),
            storage: ObjectStorage::new(),
            size_limit: MAX_CONTENT_SIZE,
            dialect: 0,
            use_xmlrpc: true,
            use_jsonrpc: true,
            trusted: true,
            handlers_initialized: false,
            scgi_allow_compression: AtomicBool::new(true),
            scgi_min_compress_size: AtomicU32::new(1000),
        }
    }

    pub fn is_handlers_initialized(&self) -> bool {
        self.handlers_initialized
    }

    pub fn initialize_handlers(&mut self) {
        self.handlers_initialized = true;
    }

    pub fn cleanup(&mut self) {
        self.handlers_initialized = false;
    }

    pub fn size_limit(&self) -> u64 {
        self.size_limit
    }

    pub fn set_size_limit(&mut self, size: u64) -> Result<(), RpcError> {
        if size > MAX_CONTENT_SIZE {
            return Err(RpcError::InputError(
                "XMLRPC size limit cannot exceed the SCGI content size limit.".to_string(),
            ));
        }
        if size < MIN_SIZE_LIMIT {
            return Err(RpcError::InputError(
                "XMLRPC size limit is too small to hold a request.".to_string(),
            ));
        }
        self.size_limit = size;
        Ok(())
    }

    pub fn dialect(&self) -> i32 {
        self.dialect
    }

    pub fn set_dialect(&mut self, dialect: i32) {
        self.dialect = dialect;
    }

    pub fn use_xmlrpc(&self) -> bool {
        self.use_xmlrpc
    }

    pub fn set_use_xmlrpc(&mut self, v: bool) {
        self.use_xmlrpc = v;
    }

    pub fn use_jsonrpc(&self) -> bool {
        self.use_jsonrpc
    }

    pub fn set_use_jsonrpc(&mut self, v: bool) {
        self.use_jsonrpc = v;
    }

    pub fn is_trusted(&self) -> bool {
        self.trusted
    }

    pub fn set_trusted(&mut self, trusted: bool) {
        self.trusted = trusted;
    }

    pub fn scgi_allow_compression(&self) -> bool {
        self.scgi_allow_compression.load(Ordering::Relaxed)
    }

    pub fn set_scgi_allow_compression(&self, allow: bool) {
        self.scgi_allow_compression.store(allow, Ordering::Relaxed);
    }

    pub fn scgi_min_compress_size(&self) -> u32 {
        self.scgi_min_compress_size.load(Ordering::Relaxed)
    }

    pub fn set_scgi_min_compress_size(&self, size: u32) {
        self.scgi_min_compress_size.store(size, Ordering::Relaxed);
    }

    pub fn commands(&self) -> &CommandMap {
        &self.commands
    }

    pub fn commands_mut(&mut self) -> &mut CommandMap {
        &mut self.commands
    }

    pub fn storage(&self) -> &ObjectStorage {
        &self.storage
    }

    pub fn storage_mut(&mut self) -> &mut ObjectStorage {
        &mut self.storage
    }
}
