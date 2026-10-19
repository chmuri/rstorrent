//! `execute.*` command family.
//!
//! The command names and flag mapping mirror `old/src/command_local.cc`
//! (lines 143-153):
//!
//! | command                       | expand tilde | throw | capture | background |
//! |-------------------------------|--------------|-------|---------|------------|
//! | `execute`, `execute.throw`    | yes          | yes   | no      | no         |
//! | `execute.throw.bg`            | yes          | yes   | no      | yes        |
//! | `execute.nothrow`             | yes          | no    | no      | no         |
//! | `execute.nothrow.bg`          | yes          | no    | no      | yes        |
//! | `execute.raw`                 | no           | yes   | no      | no         |
//! | `execute.raw.bg`              | no           | yes   | no      | yes        |
//! | `execute.raw_nothrow`         | no           | no    | no      | no         |
//! | `execute.raw_nothrow.bg`      | no           | no    | no      | yes        |
//! | `execute.capture`             | yes          | yes   | yes     | no         |
//! | `execute.capture_nothrow`     | yes          | no    | yes     | no         |
//!
//! `execute2` is the legacy redirect to `execute` from `old/src/main.cc`
//! (`CMD_REDIRECT("execute2", "execute")`). It is kept as an alias.
//!
//! These commands never invoke a shell. Each argument is converted to bytes
//! and passed to `execve(2)` verbatim, so globs, pipes, redirections and
//! environment-variable substitutions are not interpreted.
//!
//! # Reaping background children
//!
//! The daemon installs `SIGCHLD` with `SA_NOCLDWAIT` (`signals.rs`), so the
//! kernel reaps background children without the RPC thread waiting. In the
//! standalone library case that setup is not guaranteed; each background
//! spawn therefore starts a short-lived waiter thread which reaps the child
//! (`waitpid`) without ever delaying the caller. When `SA_NOCLDWAIT` is
//! active the waiter observes `ECHILD` and exits immediately.

use std::ffi::OsString;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::process::ExitStatusExt;
use std::process::{Command, ExitStatus, Stdio};

use lt_common::{Object, ObjectVal};
use rt_rpc::command_flags::*;
use rt_rpc::exec_flags;
use rt_rpc::{CommandMap, RpcError};

/// Maximum number of arguments after the executable path.
///
/// The legacy buffer is `char* argsBuffer[128]` with one slot reserved for the
/// terminating NULL, i.e. at most 127 argv entries. The task contract asks for
/// a command path plus up to 128 arguments; keeping the extra slack is
/// harmless because the kernel enforces its own `ARG_MAX`.
const MAX_ARGS: usize = 128;

/// Expands a leading `~` exactly like the legacy `expand_path` helper.
///
/// Only `~` and `~/...` are supported; `~user` is rejected instead of being
/// passed through literally, matching `old/src/globals.cc`.
fn expand_tilde(bytes: &[u8]) -> Result<Vec<u8>, RpcError> {
    if bytes.first() != Some(&b'~') {
        return Ok(bytes.to_vec());
    }

    if bytes.len() >= 2 && bytes[1] != b'/' {
        return Err(RpcError::InputError(
            "Could not expand ~ in path, only '~' or '~/...' is supported.".to_string(),
        ));
    }

    let home = std::env::var_os("HOME").ok_or_else(|| {
        RpcError::InputError(
            "Could not expand ~ in path, HOME environment variable not set.".to_string(),
        )
    })?;

    let home = home.as_bytes();
    if home.is_empty() {
        return Err(RpcError::InputError(
            "Could not expand ~ in path, HOME environment variable not set.".to_string(),
        ));
    }

    let mut expanded = Vec::with_capacity(home.len() + bytes.len() - 1);
    expanded.extend_from_slice(home);
    expanded.extend_from_slice(&bytes[1..]);
    Ok(expanded)
}

/// Converts an `Object` argument to the bytes passed to the child.
///
/// Strings and values follow legacy `print_object`; lists are concatenated.
/// For nested lists only the first element participates in tilde expansion,
/// which is also how the C++ implementation recurses.
fn render_argument(object: &Object, expand: bool, allow_expand: bool) -> Result<Vec<u8>, RpcError> {
    match &object.val {
        ObjectVal::String(bytes) => {
            if expand && allow_expand {
                expand_tilde(bytes)
            } else {
                Ok(bytes.clone())
            }
        }
        ObjectVal::Value(value) => Ok(value.to_string().into_bytes()),
        ObjectVal::None => Ok(Vec::new()),
        ObjectVal::List(items) => {
            let mut rendered = Vec::new();
            for (index, item) in items.iter().enumerate() {
                // As in `print_object`, expansion is disabled after the first
                // list element.
                rendered.extend(render_argument(item, expand, allow_expand && index == 0)?);
            }
            Ok(rendered)
        }
        _ => Err(RpcError::InputError("Invalid type.".to_string())),
    }
}

/// Extracts argv from the RPC object, validating the legacy limits.
fn command_arguments(raw_args: &Object, expand: bool) -> Result<Vec<OsString>, RpcError> {
    let values: Vec<&Object> = match raw_args.as_list() {
        Ok(list) => {
            if list.is_empty() {
                return Err(RpcError::InputError("Too few arguments.".to_string()));
            }
            list.iter().collect()
        }
        Err(_) => {
            if raw_args.is_empty() {
                return Err(RpcError::InputError("Too few arguments.".to_string()));
            }
            vec![raw_args]
        }
    };

    if values.len() > MAX_ARGS + 1 {
        return Err(RpcError::InputError("Too many arguments.".to_string()));
    }

    let mut args = Vec::with_capacity(values.len());
    for value in values {
        args.push(OsString::from_vec(render_argument(value, expand, true)?));
    }

    if args[0].is_empty() {
        return Err(RpcError::InputError(
            "Command path cannot be empty.".to_string(),
        ));
    }

    Ok(args)
}

fn spawn_error(path: &OsString, error: std::io::Error) -> RpcError {
    RpcError::InputError(format!(
        "Failed to spawn command '{}': {}",
        path.to_string_lossy(),
        error
    ))
}

fn status_error(path: &OsString, status: ExitStatus) -> RpcError {
    if let Some(code) = status.code() {
        RpcError::InputError(format!(
            "Command '{}' returned non-zero exit status: {}",
            path.to_string_lossy(),
            code
        ))
    } else if let Some(signal) = status.signal() {
        RpcError::InputError(format!(
            "Command '{}' was terminated by signal: {}",
            path.to_string_lossy(),
            signal
        ))
    } else {
        RpcError::InputError(format!(
            "Command '{}' failed with an unknown exit status.",
            path.to_string_lossy()
        ))
    }
}

/// Numeric result for non-throwing commands.
///
/// Normal exits use the exit code (so `/bin/false` yields `1`). A process
/// killed by a signal yields `128 + signal`, the conventional shell value.
fn status_value(status: ExitStatus) -> i64 {
    match status.code() {
        Some(code) => i64::from(code),
        None => i64::from(status.signal().unwrap_or(0)) + 128,
    }
}

fn execute_raw(args: &[OsString], flags: u32) -> Result<Object, RpcError> {
    let path = &args[0];
    let mut command = Command::new(path);
    command.args(&args[1..]);

    if flags & exec_flags::FLAG_BACKGROUND != 0 {
        command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());

        let mut child = command.spawn().map_err(|e| spawn_error(path, e))?;

        // Reap without blocking the RPC thread. `SA_NOCLDWAIT` may already
        // have reaped the child, in which case `wait()` returns `ECHILD`.
        std::thread::spawn(move || {
            let _ = child.wait();
        });

        return Ok(Object::create_value(0));
    }

    if flags & exec_flags::FLAG_CAPTURE != 0 {
        let output = command
            .stdin(Stdio::inherit())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .output()
            .map_err(|e| spawn_error(path, e))?;

        if flags & exec_flags::FLAG_THROW != 0 && !output.status.success() {
            return Err(status_error(path, output.status));
        }

        // Captured stdout is binary and returned verbatim; no newline is
        // appended or removed.
        return Ok(Object::from(output.stdout));
    }

    let status = command.status().map_err(|e| spawn_error(path, e))?;

    if flags & exec_flags::FLAG_THROW != 0 && !status.success() {
        return Err(status_error(path, status));
    }

    Ok(Object::create_value(status_value(status)))
}

fn execute_object(raw_args: &Object, flags: u32) -> Result<Object, RpcError> {
    let expand = flags & exec_flags::FLAG_EXPAND_TILDE != 0;
    let args = command_arguments(raw_args, expand)?;
    execute_raw(&args, flags)
}

/// Registers the `execute.*` command family plus the legacy `execute2` alias.
pub fn register_exec_commands(map: &mut CommandMap) -> Result<(), RpcError> {
    let specifications: &[(&str, u32, &str)] = &[
        (
            "execute",
            exec_flags::FLAG_EXPAND_TILDE | exec_flags::FLAG_THROW,
            "Execute a command, throwing on a non-zero exit status.",
        ),
        (
            "execute.throw",
            exec_flags::FLAG_EXPAND_TILDE | exec_flags::FLAG_THROW,
            "Execute a command, throwing on a non-zero exit status.",
        ),
        (
            "execute.throw.bg",
            exec_flags::FLAG_EXPAND_TILDE | exec_flags::FLAG_THROW | exec_flags::FLAG_BACKGROUND,
            "Execute a command in the background, throwing only on spawn failure.",
        ),
        (
            "execute.nothrow",
            exec_flags::FLAG_EXPAND_TILDE,
            "Execute a command without throwing on a non-zero exit status.",
        ),
        (
            "execute.nothrow.bg",
            exec_flags::FLAG_EXPAND_TILDE | exec_flags::FLAG_BACKGROUND,
            "Execute a command in the background without throwing.",
        ),
        (
            "execute.raw",
            exec_flags::FLAG_THROW,
            "Execute a command without tilde expansion, throwing on failure.",
        ),
        (
            "execute.raw.bg",
            exec_flags::FLAG_THROW | exec_flags::FLAG_BACKGROUND,
            "Execute a command in the background without tilde expansion.",
        ),
        (
            "execute.raw_nothrow",
            0,
            "Execute a command without tilde expansion or throwing.",
        ),
        (
            "execute.raw_nothrow.bg",
            exec_flags::FLAG_BACKGROUND,
            "Execute a command in the background without expansion or throwing.",
        ),
        (
            "execute.capture",
            exec_flags::FLAG_EXPAND_TILDE | exec_flags::FLAG_THROW | exec_flags::FLAG_CAPTURE,
            "Execute a command and return its captured standard output.",
        ),
        (
            "execute.capture_nothrow",
            exec_flags::FLAG_EXPAND_TILDE | exec_flags::FLAG_CAPTURE,
            "Execute a command and return its captured standard output without throwing.",
        ),
        // Legacy `CMD_REDIRECT("execute2", "execute")` from main.cc.
        (
            "execute2",
            exec_flags::FLAG_EXPAND_TILDE | exec_flags::FLAG_THROW,
            "Execute a command (legacy alias for execute).",
        ),
    ];

    for (name, flags, description) in specifications {
        let flags = *flags;
        map.insert_func(
            name,
            move |_target, obj: &Object| execute_object(obj, flags),
            FLAG_PUBLIC_RPC,
            "<command>, <args>...",
            description,
        )?;
    }

    Ok(())
}
