use std::collections::HashMap;
use std::sync::Arc;

pub type KeyHandler = Arc<dyn Fn() + Send + Sync>;

#[derive(Clone, Default)]
pub struct Bindings {
    handlers: HashMap<i32, KeyHandler>,
    enabled: bool,
}

impl Bindings {
    pub fn new() -> Self {
        Self {
            handlers: HashMap::new(),
            enabled: true,
        }
    }

    pub fn enable(&mut self) {
        self.enabled = true;
    }

    pub fn disable(&mut self) {
        self.enabled = false;
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    pub fn bind<F>(&mut self, key: i32, handler: F)
    where
        F: Fn() + Send + Sync + 'static,
    {
        self.handlers.insert(key, Arc::new(handler));
    }

    pub fn unbind(&mut self, key: i32) {
        self.handlers.remove(&key);
    }

    pub fn pressed(&self, key: i32) -> bool {
        if !self.enabled {
            return false;
        }
        if let Some(handler) = self.handlers.get(&key) {
            handler();
            true
        } else {
            false
        }
    }
}
