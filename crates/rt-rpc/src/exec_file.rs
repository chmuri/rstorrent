use crate::error::RpcError;
use lt_common::Object;
use std::process::{Command, Stdio};

pub mod flags {
    pub const FLAG_EXPAND_TILDE: u32 = 0x1;
    pub const FLAG_THROW: u32 = 0x2;
    pub const FLAG_CAPTURE: u32 = 0x4;
    pub const FLAG_BACKGROUND: u32 = 0x8;
}

pub struct ExecFile {
    log_fd: Option<i32>,
}

impl Default for ExecFile {
    fn default() -> Self {
        Self::new()
    }
}

impl ExecFile {
    pub const MAX_ARGS: usize = 128;

    pub fn new() -> Self {
        Self { log_fd: None }
    }

    pub fn log_fd(&self) -> Option<i32> {
        self.log_fd
    }

    pub fn set_log_fd(&mut self, fd: Option<i32>) {
        self.log_fd = fd;
    }

    fn expand_tilde(path: &str) -> String {
        if let Some(stripped) = path.strip_prefix("~/") {
            if let Ok(home) = std::env::var("HOME") {
                return format!("{}/{}", home, stripped);
            }
        }
        path.to_string()
    }

    pub fn execute(
        &self,
        file: &str,
        args: &[&str],
        exec_flags: u32,
    ) -> Result<(i32, String), RpcError> {
        let file_path = if exec_flags & flags::FLAG_EXPAND_TILDE != 0 {
            Self::expand_tilde(file)
        } else {
            file.to_string()
        };

        let mut cmd = Command::new(&file_path);

        for arg in args {
            let processed_arg = if exec_flags & flags::FLAG_EXPAND_TILDE != 0 {
                Self::expand_tilde(arg)
            } else {
                arg.to_string()
            };
            cmd.arg(processed_arg);
        }

        if exec_flags & flags::FLAG_BACKGROUND != 0 {
            cmd.stdin(Stdio::null());
            cmd.stdout(Stdio::null());
            cmd.stderr(Stdio::null());
            let mut child = cmd.spawn().map_err(|e| {
                RpcError::InputError(format!("Failed to spawn background process: {}", e))
            })?;

            // Reap the child without blocking the caller. SIGCHLD is also
            // ignored by the daemon (see TUI-07), but this keeps the library
            // safe when used standalone.
            std::thread::spawn(move || {
                let _ = child.wait();
            });

            return Ok((0, String::new()));
        }

        if exec_flags & flags::FLAG_CAPTURE != 0 {
            cmd.stdout(Stdio::piped());
        }

        let output = cmd.output().map_err(|e| {
            RpcError::InputError(format!("Failed to execute process '{}': {}", file_path, e))
        })?;

        let status_code = output.status.code().unwrap_or(-1);

        if exec_flags & flags::FLAG_THROW != 0 && !output.status.success() {
            return Err(RpcError::InputError(format!(
                "Command '{}' returned non-zero exit status: {}",
                file_path, status_code
            )));
        }

        let capture = if exec_flags & flags::FLAG_CAPTURE != 0 {
            String::from_utf8_lossy(&output.stdout).to_string()
        } else {
            String::new()
        };

        Ok((status_code, capture))
    }

    pub fn execute_object(&self, raw_args: &Object, exec_flags: u32) -> Result<Object, RpcError> {
        let mut arg_strings = Vec::new();

        if let Ok(list) = raw_args.as_list() {
            if list.is_empty() {
                return Err(RpcError::InputError("Too few arguments.".to_string()));
            }
            if list.len() > Self::MAX_ARGS {
                return Err(RpcError::InputError("Too many arguments.".to_string()));
            }

            for item in list {
                if let Ok(s) = item.as_str() {
                    arg_strings.push(s.to_string());
                } else if let Ok(v) = item.as_value() {
                    arg_strings.push(v.to_string());
                } else {
                    return Err(RpcError::InputError(
                        "Invalid argument type for exec.".to_string(),
                    ));
                }
            }
        } else if let Ok(s) = raw_args.as_str() {
            arg_strings.push(s.to_string());
        } else {
            return Err(RpcError::InputError("Too few arguments.".to_string()));
        }

        let file = &arg_strings[0];
        let args: Vec<&str> = arg_strings[1..].iter().map(|s| s.as_str()).collect();

        let (code, captured) = self.execute(file, &args, exec_flags)?;

        if exec_flags & flags::FLAG_CAPTURE != 0 {
            Ok(Object::from(captured.as_str()))
        } else {
            Ok(Object::from(code as i64))
        }
    }
}
