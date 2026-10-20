use bitflags::bitflags;
use std::os::unix::io::RawFd;

use crate::error::NetError;

bitflags! {
    #[derive(Copy, Clone, Debug, PartialEq, Eq)]
    pub struct FdFlags: u32 {
        const STREAM        = 0x1;
        const DATAGRAM      = 0x10;
        const NONBLOCK      = 0x20;
        const REUSE_ADDRESS = 0x40;
        const V4_ONLY       = 0x80;
        const V6_ONLY       = 0x100;
        const ALL           = 0x1ff;
    }
}

pub fn fd_valid_flags(flags: FdFlags) -> bool {
    let has_stream = flags.contains(FdFlags::STREAM);
    let has_datagram = flags.contains(FdFlags::DATAGRAM);
    let has_v4only = flags.contains(FdFlags::V4_ONLY);
    let has_v6only = flags.contains(FdFlags::V6_ONLY);

    // Exactly one of STREAM or DATAGRAM must be set
    let valid_type = (has_stream || has_datagram) && !(has_stream && has_datagram);
    // Cannot have both V4_ONLY and V6_ONLY
    let valid_ip_version = !(has_v4only && has_v6only);
    // Cannot contain bits outside ALL
    let valid_bits = FdFlags::ALL.contains(flags);

    valid_type && valid_ip_version && valid_bits
}

pub fn fd_valid_flags_raw(raw: u32) -> bool {
    // If unknown bits are present, it's invalid
    if raw & !FdFlags::ALL.bits() != 0 {
        return false;
    }
    match FdFlags::from_bits(raw) {
        Some(flags) => fd_valid_flags(flags),
        None => false,
    }
}

pub fn set_nonblocking(fd: RawFd, nonblock: bool) -> Result<(), NetError> {
    unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFL, 0);
        if flags < 0 {
            return Err(NetError::Io(std::io::Error::last_os_error()));
        }
        let new_flags = if nonblock {
            flags | libc::O_NONBLOCK
        } else {
            flags & !libc::O_NONBLOCK
        };
        if libc::fcntl(fd, libc::F_SETFL, new_flags) < 0 {
            return Err(NetError::Io(std::io::Error::last_os_error()));
        }
        Ok(())
    }
}

pub fn set_reuse_address(fd: RawFd, reuse: bool) -> Result<(), NetError> {
    let val: libc::c_int = if reuse { 1 } else { 0 };
    unsafe {
        if libc::setsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_REUSEADDR,
            &val as *const _ as *const libc::c_void,
            std::mem::size_of_val(&val) as libc::socklen_t,
        ) < 0
        {
            return Err(NetError::Io(std::io::Error::last_os_error()));
        }
        Ok(())
    }
}

pub fn set_tcp_nodelay(fd: RawFd, nodelay: bool) -> Result<(), NetError> {
    let val: libc::c_int = if nodelay { 1 } else { 0 };
    unsafe {
        if libc::setsockopt(
            fd,
            libc::IPPROTO_TCP,
            libc::TCP_NODELAY,
            &val as *const _ as *const libc::c_void,
            std::mem::size_of_val(&val) as libc::socklen_t,
        ) < 0
        {
            return Err(NetError::Io(std::io::Error::last_os_error()));
        }
        Ok(())
    }
}

pub fn set_send_buffer_size(fd: RawFd, size: u32) -> Result<(), NetError> {
    let val = size as libc::c_int;
    unsafe {
        if libc::setsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_SNDBUF,
            &val as *const _ as *const libc::c_void,
            std::mem::size_of_val(&val) as libc::socklen_t,
        ) < 0
        {
            return Err(NetError::Io(std::io::Error::last_os_error()));
        }
        Ok(())
    }
}

pub fn set_receive_buffer_size(fd: RawFd, size: u32) -> Result<(), NetError> {
    let val = size as libc::c_int;
    unsafe {
        if libc::setsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_RCVBUF,
            &val as *const _ as *const libc::c_void,
            std::mem::size_of_val(&val) as libc::socklen_t,
        ) < 0
        {
            return Err(NetError::Io(std::io::Error::last_os_error()));
        }
        Ok(())
    }
}

pub fn set_priority(fd: RawFd, family: i32, priority: i32) -> Result<(), NetError> {
    unsafe {
        let (level, optname) = match family {
            libc::AF_INET => (libc::IPPROTO_IP, libc::IP_TOS),
            libc::AF_INET6 => (libc::IPPROTO_IPV6, libc::IPV6_TCLASS),
            _ => {
                return Err(NetError::Input(format!(
                    "Invalid address family: {}",
                    family
                )))
            }
        };
        let val = priority as libc::c_int;
        if libc::setsockopt(
            fd,
            level,
            optname,
            &val as *const _ as *const libc::c_void,
            std::mem::size_of_val(&val) as libc::socklen_t,
        ) < 0
        {
            return Err(NetError::Io(std::io::Error::last_os_error()));
        }
        Ok(())
    }
}

pub fn set_v6only(fd: RawFd, v6only: bool) -> Result<(), NetError> {
    let val: libc::c_int = if v6only { 1 } else { 0 };
    unsafe {
        if libc::setsockopt(
            fd,
            libc::IPPROTO_IPV6,
            libc::IPV6_V6ONLY,
            &val as *const _ as *const libc::c_void,
            std::mem::size_of_val(&val) as libc::socklen_t,
        ) < 0
        {
            return Err(NetError::Io(std::io::Error::last_os_error()));
        }
        Ok(())
    }
}
