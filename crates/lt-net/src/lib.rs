//! Crate `lt-net`
//!
//! Networking types, socket address abstraction, socket management,
//! network configuration, DNS resolver, HTTP client, and rate throttling.

pub mod addr;
pub mod config;
pub mod error;
pub mod fd;
pub mod http;
pub mod lsd;
pub mod port_mapper;
pub mod proxy;
pub mod resolver;
pub mod socket_manager;
pub mod throttle;

pub use addr::{AddressFamily, AddressList, SocketAddress, SocketAddressKey};
pub use config::{AddressDevice, EncryptionMode, ListenAddresses, NetworkConfig};
pub use error::NetError;
pub use fd::{fd_valid_flags, fd_valid_flags_raw, FdFlags};
pub use http::{HttpClient, HttpConfig, HttpGet, DEFAULT_MAX_FILE_SIZE};
pub use lsd::{
    create_lsd_socket, format_lsd_packet, parse_lsd_packet, LsdPacket, LSD_MULTICAST_IPV4, LSD_PORT,
};
pub use port_mapper::{
    format_nat_pmp_request, format_upnp_soap_add_port_mapping,
    format_upnp_soap_delete_port_mapping, format_upnp_ssdp_search, parse_nat_pmp_response,
    try_map_nat_pmp, PortMappingProtocol, NAT_PMP_PORT,
};
pub use proxy::{HttpProxy, Socks5Proxy};
pub use resolver::{DnsCache, DnsResolver};
pub use socket_manager::{SocketCategory, SocketInfo, SocketManager};
pub use throttle::{Throttle, ThrottleNode};
