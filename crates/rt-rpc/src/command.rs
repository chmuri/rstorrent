use crate::error::RpcError;
use crate::target::Target;
use lt_common::Object;
use std::cell::RefCell;
use std::sync::Arc;

pub mod flags {
    pub const FLAG_DONT_DELETE: u32 = 0x1;
    pub const FLAG_PUBLIC_RPC: u32 = 0x4;
    pub const FLAG_MODIFIABLE: u32 = 0x10;
    pub const FLAG_IS_REDIRECT: u32 = 0x20;
    pub const FLAG_HAS_REDIRECTS: u32 = 0x40;
    pub const FLAG_FILE_TARGET: u32 = 0x100;
    pub const FLAG_TRACKER_TARGET: u32 = 0x200;
    pub const FLAG_UNTRUSTED_SAFE: u32 = 0x400;
}

pub type CommandFn = Arc<dyn Fn(&Target, &Object) -> Result<Object, RpcError> + Send + Sync>;

#[derive(Clone)]
pub struct CommandEntry {
    pub func: CommandFn,
    pub flags: u32,
    pub parm: String,
    pub doc: String,
}

pub const MAX_ARGUMENTS: usize = 10;

thread_local! {
    static CURRENT_STACK: RefCell<Vec<Vec<Object>>> = const { RefCell::new(Vec::new()) };
}

pub struct CommandStack;

pub struct CommandStackGuard {
    _private: (),
}

impl Drop for CommandStackGuard {
    fn drop(&mut self) {
        CURRENT_STACK.with(|stack| {
            stack.borrow_mut().pop();
        });
    }
}

impl CommandStack {
    pub fn push_stack(args: &[Object]) -> CommandStackGuard {
        CURRENT_STACK.with(|stack| {
            let mut frame = Vec::with_capacity(MAX_ARGUMENTS);
            for arg in args.iter().take(MAX_ARGUMENTS) {
                frame.push(arg.clone());
            }
            stack.borrow_mut().push(frame);
        });
        CommandStackGuard { _private: () }
    }

    pub fn pop_stack() {
        CURRENT_STACK.with(|stack| {
            stack.borrow_mut().pop();
        });
    }

    pub fn is_empty() -> bool {
        CURRENT_STACK.with(|stack| {
            let b = stack.borrow();
            if let Some(top) = b.last() {
                top.iter().all(|obj| obj.is_empty())
            } else {
                true
            }
        })
    }

    pub fn argument(idx: usize) -> Object {
        CURRENT_STACK.with(|stack| {
            let b = stack.borrow();
            if let Some(top) = b.last() {
                if idx < top.len() {
                    return top[idx].clone();
                }
            }
            Object::create_empty()
        })
    }
}

type PrepareCallback = Box<dyn FnOnce(&mut PreparsedCommands)>;

pub struct PreparsedCommands {
    prepare: Option<PrepareCallback>,
    items: Vec<Object>,
}

impl PreparsedCommands {
    pub fn new<F>(prepare: F) -> Self
    where
        F: FnOnce(&mut PreparsedCommands) + 'static,
    {
        Self {
            prepare: Some(Box::new(prepare)),
            items: Vec::new(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn push_back(&mut self, obj: Object) {
        self.items.push(obj);
    }

    pub fn prepare_if_needed(&mut self) {
        if let Some(prepare) = self.prepare.take() {
            prepare(self);
        }
    }

    pub fn iter(&self) -> std::slice::Iter<'_, Object> {
        self.items.iter()
    }
}

impl IntoIterator for PreparsedCommands {
    type Item = Object;
    type IntoIter = std::vec::IntoIter<Object>;

    fn into_iter(mut self) -> Self::IntoIter {
        self.prepare_if_needed();
        self.items.into_iter()
    }
}

impl<'a> IntoIterator for &'a mut PreparsedCommands {
    type Item = &'a Object;
    type IntoIter = std::slice::Iter<'a, Object>;

    fn into_iter(self) -> Self::IntoIter {
        self.prepare_if_needed();
        self.items.iter()
    }
}

pub fn parse_command_object(input: &str) -> Result<Object, RpcError> {
    let trimmed = input.trim_start();
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return Ok(Object::create_empty());
    }

    let eq_pos = trimmed.find('=').ok_or_else(|| {
        RpcError::InputError(format!("Could not find '=' in command '{}'.", trimmed))
    })?;

    let key = trimmed[..eq_pos].trim();
    if key.is_empty() {
        return Err(RpcError::InputError("Empty command key.".to_string()));
    }

    let rest = &trimmed[eq_pos + 1..];

    // Delimiters for command object: ';', '\n', '\r'
    if let Some(pos) = rest.find([';', '\n', '\r']) {
        let suffix = &rest[pos..];
        if !suffix.trim().is_empty()
            || suffix.contains(';')
            || suffix.contains('\n')
            || suffix.contains('\r')
        {
            return Err(RpcError::InputError("Junk at end of input.".to_string()));
        }
    }

    let val_str = rest.trim();
    // Parse value into an Object
    let val_obj = if let Ok(val) = val_str.parse::<i64>() {
        Object::from(val)
    } else {
        Object::from(val_str)
    };

    Ok(Object::create_dict_key(key, val_obj))
}
