//! Crate `lt-net`
//!
//! Networking types, socket address abstraction, socket management,
//! network configuration, DNS resolver, HTTP client, and rate throttling.

pub mod addr;
pub mod config;
pub mod error;
pub mod fd;
pub mod http;
pub mod proxy;
pub mod resolver;
pub mod socket_manager;
pub mod throttle;

pub use addr::{AddressFamily, AddressList, SocketAddress, SocketAddressKey};
pub use config::{AddressDevice, EncryptionMode, ListenAddresses, NetworkConfig};
pub use error::NetError;
pub use fd::{fd_valid_flags, fd_valid_flags_raw, FdFlags};
pub use http::{HttpClient, HttpConfig, HttpGet, DEFAULT_MAX_FILE_SIZE};
pub use proxy::{HttpProxy, Socks5Proxy};
pub use resolver::{DnsCache, DnsResolver};
pub use socket_manager::{SocketCategory, SocketInfo, SocketManager};
pub use throttle::{Throttle, ThrottleNode};
