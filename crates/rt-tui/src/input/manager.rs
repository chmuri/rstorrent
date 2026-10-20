use crate::input::bindings::Bindings;
use crate::input::keymap::{Keymap, KeymapStyle, NavigationKey};
use crate::input::text_input::InputTarget;
use std::sync::{Arc, Mutex};

pub struct InputManager {
    bindings_stack: Vec<Arc<Mutex<Bindings>>>,
    text_input: Option<Arc<Mutex<dyn InputTarget>>>,
    keymap: Keymap,
}

impl Default for InputManager {
    fn default() -> Self {
        Self::new()
    }
}

impl InputManager {
    pub fn new() -> Self {
        Self {
            bindings_stack: Vec::new(),
            text_input: None,
            keymap: Keymap::default(),
        }
    }

    pub fn push_bindings(&mut self, b: Arc<Mutex<Bindings>>) {
        self.bindings_stack.push(b);
    }

    pub fn pop_bindings(&mut self) -> Option<Arc<Mutex<Bindings>>> {
        self.bindings_stack.pop()
    }

    pub fn set_text_input(&mut self, input: Option<Arc<Mutex<dyn InputTarget>>>) {
        self.text_input = input;
    }

    pub fn has_active_text_input(&self) -> bool {
        self.text_input.is_some()
    }

    /// Returns the active keymap style used for navigation keys.
    pub fn keymap_style(&self) -> KeymapStyle {
        self.keymap.style()
    }

    /// Canonical name of the active keymap style.
    pub fn keymap_style_name(&self) -> &'static str {
        self.keymap.style().as_str()
    }

    /// Selects the keymap style.
    pub fn set_keymap_style(&mut self, style: KeymapStyle) {
        self.keymap.set_style(style);
    }

    /// Selects the keymap style by name (`default`, `emacs` or `vi`).
    pub fn set_keymap_style_name(&mut self, style: &str) -> Result<(), String> {
        self.keymap.set_style_name(style)
    }

    /// Whether `key` triggers `navigation` under the active style.
    pub fn matches_navigation(&self, navigation: NavigationKey, key: i32) -> bool {
        self.keymap.matches(navigation, key)
    }

    /// Primary key code for `navigation` under the active style.
    pub fn navigation_key(&self, navigation: NavigationKey) -> i32 {
        self.keymap.key(navigation)
    }

    pub fn paste(&mut self, text: &str) -> bool {
        if let Some(target) = &self.text_input {
            let mut input = target.lock().unwrap();
            input.insert_str(text);
            true
        } else {
            false
        }
    }

    pub fn pressed(&mut self, key: i32) -> bool {
        if let Some(target) = &self.text_input {
            let mut input = target.lock().unwrap();
            input.pressed(key)
        } else {
            // Search bindings in reverse order (top of stack first)
            for b in self.bindings_stack.iter().rev() {
                let bind = b.lock().unwrap();
                if bind.pressed(key) {
                    return true;
                }
            }
            false
        }
    }
}
