use crate::command::{flags::*, CommandEntry, CommandFn};
use crate::error::RpcError;
use crate::target::Target;
use lt_common::Object;
use std::collections::BTreeMap;
use std::sync::Arc;

#[derive(Default, Clone)]
pub struct CommandMap {
    commands: BTreeMap<String, CommandEntry>,
}

impl CommandMap {
    pub fn new() -> Self {
        Self {
            commands: BTreeMap::new(),
        }
    }

    pub fn has(&self, key: &str) -> bool {
        self.commands.contains_key(key)
    }

    pub fn is_modifiable(&self, key: &str) -> bool {
        self.commands
            .get(key)
            .map(|entry| entry.flags & FLAG_MODIFIABLE != 0)
            .unwrap_or(false)
    }

    pub fn insert(
        &mut self,
        key: &str,
        func: CommandFn,
        flags: u32,
        parm: &str,
        doc: &str,
    ) -> Result<(), RpcError> {
        if self.commands.contains_key(key) {
            return Err(RpcError::InternalError(
                "CommandMap::insert(...) tried to insert an already existing key.".to_string(),
            ));
        }

        self.commands.insert(
            key.to_string(),
            CommandEntry {
                func,
                flags,
                parm: parm.to_string(),
                doc: doc.to_string(),
            },
        );
        Ok(())
    }

    pub fn insert_func<F>(
        &mut self,
        key: &str,
        f: F,
        flags: u32,
        parm: &str,
        doc: &str,
    ) -> Result<(), RpcError>
    where
        F: Fn(&Target, &Object) -> Result<Object, RpcError> + Send + Sync + 'static,
    {
        self.insert(key, Arc::new(f), flags, parm, doc)
    }

    pub fn erase(&mut self, key: &str) -> Result<(), RpcError> {
        if let Some(entry) = self.commands.get(key) {
            if entry.flags & FLAG_HAS_REDIRECTS != 0 {
                return Err(RpcError::InputError(
                    "Can't erase a command that has redirects.".to_string(),
                ));
            }
            self.commands.remove(key);
        }
        Ok(())
    }

    pub fn create_redirect(
        &mut self,
        key_new: &str,
        key_dest: &str,
        mut flags: u32,
    ) -> Result<(), RpcError> {
        if self.commands.contains_key(key_new) {
            return Err(RpcError::InputError(format!(
                "Tried to create a redirect key that already exists: '{}'.",
                key_new
            )));
        }

        let (func, parm, doc) = {
            let dest_entry = self.commands.get_mut(key_dest).ok_or_else(|| {
                RpcError::InputError(format!(
                    "Tried to redirect to a key that doesn't exist: '{}'.",
                    key_dest
                ))
            })?;

            if dest_entry.flags & FLAG_IS_REDIRECT == 0 {
                return Err(RpcError::InputError(format!(
                    "Tried to redirect to a key that is not marked 'flag_is_redirect': '{}'.",
                    key_dest
                )));
            }

            dest_entry.flags |= FLAG_HAS_REDIRECTS;
            flags |= dest_entry.flags & !(FLAG_HAS_REDIRECTS | FLAG_PUBLIC_RPC);

            (
                dest_entry.func.clone(),
                dest_entry.parm.clone(),
                dest_entry.doc.clone(),
            )
        };

        self.commands.insert(
            key_new.to_string(),
            CommandEntry {
                func,
                flags,
                parm,
                doc,
            },
        );

        Ok(())
    }

    pub fn call_command(
        &self,
        key: &str,
        arg: &Object,
        target: &Target,
        trusted: bool,
    ) -> Result<Object, RpcError> {
        let entry = self
            .commands
            .get(key)
            .ok_or_else(|| RpcError::InputError(format!("Command \"{}\" does not exist.", key)))?;

        if !trusted && (entry.flags & FLAG_UNTRUSTED_SAFE == 0) {
            return Err(RpcError::UntrustedError(format!(
                "Command \"{}\" is not allowed for untrusted connections.",
                key
            )));
        }

        (entry.func)(target, arg)
    }

    pub fn call_catch(&self, key: &str, arg: &Object, target: &Target, trusted: bool) -> Object {
        match self.call_command(key, arg, target, trusted) {
            Ok(res) => res,
            Err(_) => Object::create_empty(),
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = (&String, &CommandEntry)> {
        self.commands.iter()
    }

    pub fn len(&self) -> usize {
        self.commands.len()
    }

    pub fn is_empty(&self) -> bool {
        self.commands.is_empty()
    }
}
