use std::sync::{Arc, Mutex};

use crate::addr::{AddressFamily, SocketAddress};
use crate::error::NetError;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum EncryptionMode {
    Deny = 0,
    Allow = 1,
    Prefer = 2,
    Require = 3,
}

#[derive(Clone, Debug)]
pub struct AddressDevice {
    pub address: Option<SocketAddress>,
    pub device: String,
}

#[derive(Clone, Debug)]
pub struct ListenAddresses {
    pub inet_address: Option<SocketAddress>,
    pub inet_device: String,
    pub inet6_address: Option<SocketAddress>,
    pub inet6_device: String,
    pub block_ipv4in6: bool,
}

struct Inner {
    block_ipv4: bool,
    block_ipv6: bool,
    block_ipv4in6: bool,
    block_outgoing: bool,
    prefer_ipv6: bool,
    block_udp: bool,
    block_incoming: bool,

    priority: i32,

    bind_inet_address: SocketAddress,
    bind_inet_device_name: String,
    bind_inet6_address: SocketAddress,
    bind_inet6_device_name: String,

    local_inet_address: SocketAddress,
    local_inet6_address: SocketAddress,

    local_inet_port: u16,
    local_inet6_port: u16,

    listen_backlog: i32,
    override_dht_port: u16,

    send_buffer_size: u32,
    receive_buffer_size: u32,

    handshake_encryption_mode: EncryptionMode,
    stream_encryption_mode: EncryptionMode,

    subscribers: Vec<Box<dyn Fn() + Send + Sync + 'static>>,
}

#[derive(Clone)]
pub struct NetworkConfig {
    inner: Arc<Mutex<Inner>>,
}

impl Default for NetworkConfig {
    fn default() -> Self {
        Self::new()
    }
}

impl NetworkConfig {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner {
                block_ipv4: false,
                block_ipv6: false,
                block_ipv4in6: false,
                block_outgoing: false,
                prefer_ipv6: false,
                block_udp: false,
                block_incoming: false,
                priority: 0x08, // IPTOS_THROUGHPUT
                bind_inet_address: SocketAddress::Unspec,
                bind_inet_device_name: String::new(),
                bind_inet6_address: SocketAddress::Unspec,
                bind_inet6_device_name: String::new(),
                local_inet_address: SocketAddress::Unspec,
                local_inet6_address: SocketAddress::Unspec,
                local_inet_port: 0,
                local_inet6_port: 0,
                listen_backlog: 128,
                override_dht_port: 0,
                send_buffer_size: 0,
                receive_buffer_size: 0,
                handshake_encryption_mode: EncryptionMode::Allow,
                stream_encryption_mode: EncryptionMode::Allow,
                subscribers: Vec::new(),
            })),
        }
    }

    pub fn is_block_ipv4(&self) -> bool {
        self.inner.lock().unwrap().block_ipv4
    }

    pub fn set_block_ipv4(&self, v: bool) {
        let mut inner = self.inner.lock().unwrap();
        inner.block_ipv4 = v;
        Self::notify(&inner);
    }

    pub fn is_block_ipv6(&self) -> bool {
        self.inner.lock().unwrap().block_ipv6
    }

    pub fn set_block_ipv6(&self, v: bool) {
        let mut inner = self.inner.lock().unwrap();
        inner.block_ipv6 = v;
        Self::notify(&inner);
    }

    pub fn is_block_ipv4in6(&self) -> bool {
        self.inner.lock().unwrap().block_ipv4in6
    }

    pub fn set_block_ipv4in6(&self, v: bool) {
        let mut inner = self.inner.lock().unwrap();
        inner.block_ipv4in6 = v;
        Self::notify(&inner);
    }

    pub fn is_block_outgoing(&self) -> bool {
        self.inner.lock().unwrap().block_outgoing
    }

    pub fn set_block_outgoing(&self, v: bool) {
        let mut inner = self.inner.lock().unwrap();
        inner.block_outgoing = v;
        Self::notify(&inner);
    }

    pub fn is_prefer_ipv6(&self) -> bool {
        self.inner.lock().unwrap().prefer_ipv6
    }

    pub fn set_prefer_ipv6(&self, v: bool) {
        self.inner.lock().unwrap().prefer_ipv6 = v;
    }

    pub fn is_block_udp(&self) -> bool {
        self.inner.lock().unwrap().block_udp
    }

    pub fn set_block_udp(&self, v: bool) {
        self.inner.lock().unwrap().block_udp = v;
    }

    pub fn is_block_incoming(&self) -> bool {
        self.inner.lock().unwrap().block_incoming
    }

    pub fn set_block_incoming(&self, v: bool) {
        self.inner.lock().unwrap().block_incoming = v;
    }

    pub fn priority(&self) -> i32 {
        self.inner.lock().unwrap().priority
    }

    pub fn set_priority(&self, p: i32) {
        self.inner.lock().unwrap().priority = p;
    }

    pub fn bind_inet_address(&self) -> SocketAddress {
        self.inner.lock().unwrap().bind_inet_address.clone()
    }

    pub fn bind_inet6_address(&self) -> SocketAddress {
        self.inner.lock().unwrap().bind_inet6_address.clone()
    }

    pub fn set_bind_address(&self, addr_str: &str) -> Result<(), NetError> {
        let sa = SocketAddress::lookup_address(addr_str, AddressFamily::Unspec)?;
        if sa.family() != AddressFamily::Inet
            && sa.family() != AddressFamily::Inet6
            && sa.family() != AddressFamily::Unspec
        {
            return Err(NetError::Input(
                "Tried to set a bind address that is not unspec/inet/inet6".into(),
            ));
        }
        if sa.port() != 0 {
            return Err(NetError::Input(
                "Tried to set a bind address with a non-zero port".into(),
            ));
        }

        let mut inner = self.inner.lock().unwrap();
        match sa {
            SocketAddress::Unspec => {
                inner.bind_inet_address = SocketAddress::Unspec;
                inner.bind_inet6_address = SocketAddress::Unspec;
            }
            SocketAddress::Inet(_) => {
                inner.bind_inet_address = sa;
                inner.bind_inet6_address = SocketAddress::Unspec;
            }
            SocketAddress::Inet6(_) => {
                inner.bind_inet_address = SocketAddress::Unspec;
                inner.bind_inet6_address = sa;
            }
            _ => unreachable!(),
        }
        Self::notify(&inner);
        Ok(())
    }

    pub fn set_bind_inet_address(&self, addr_str: &str) -> Result<(), NetError> {
        let sa = SocketAddress::lookup_address(addr_str, AddressFamily::Inet)?;
        if sa.family() != AddressFamily::Inet && sa.family() != AddressFamily::Unspec {
            return Err(NetError::Input(
                "Tried to set a bind inet address that is not unspec/inet".into(),
            ));
        }
        if sa.port() != 0 {
            return Err(NetError::Input(
                "Tried to set a bind inet address with non-zero port".into(),
            ));
        }

        let mut inner = self.inner.lock().unwrap();
        inner.bind_inet_address = sa;
        Self::notify(&inner);
        Ok(())
    }

    pub fn set_bind_inet6_address(&self, addr_str: &str) -> Result<(), NetError> {
        let sa = SocketAddress::lookup_address(addr_str, AddressFamily::Inet6)?;
        if sa.family() != AddressFamily::Inet6 && sa.family() != AddressFamily::Unspec {
            return Err(NetError::Input(
                "Tried to set a bind inet6 address that is not unspec/inet6".into(),
            ));
        }
        if sa.port() != 0 {
            return Err(NetError::Input(
                "Tried to set a bind inet6 address with non-zero port".into(),
            ));
        }

        let mut inner = self.inner.lock().unwrap();
        inner.bind_inet6_address = sa;
        Self::notify(&inner);
        Ok(())
    }

    pub fn bind_inet_device_name(&self) -> String {
        self.inner.lock().unwrap().bind_inet_device_name.clone()
    }

    pub fn set_bind_inet_device_name(&self, name: String) {
        let mut inner = self.inner.lock().unwrap();
        inner.bind_inet_device_name = name;
        Self::notify(&inner);
    }

    pub fn bind_inet6_device_name(&self) -> String {
        self.inner.lock().unwrap().bind_inet6_device_name.clone()
    }

    pub fn set_bind_inet6_device_name(&self, name: String) {
        let mut inner = self.inner.lock().unwrap();
        inner.bind_inet6_device_name = name;
        Self::notify(&inner);
    }

    pub fn set_bind_device_name(&self, name: String) {
        let mut inner = self.inner.lock().unwrap();
        inner.bind_inet_device_name = name.clone();
        inner.bind_inet6_device_name = name;
        Self::notify(&inner);
    }

    pub fn local_inet_address(&self) -> SocketAddress {
        self.inner.lock().unwrap().local_inet_address.clone()
    }

    pub fn local_inet6_address(&self) -> SocketAddress {
        self.inner.lock().unwrap().local_inet6_address.clone()
    }

    pub fn set_local_address(&self, addr_str: &str) -> Result<(), NetError> {
        let sa = SocketAddress::lookup_address(addr_str, AddressFamily::Unspec)?;
        if sa.is_any() && !sa.is_unspec() {
            return Err(NetError::Input(
                "Tried to set local address to an any address".into(),
            ));
        }
        if sa.port() != 0 {
            return Err(NetError::Input(
                "Tried to set a local address with a non-zero port".into(),
            ));
        }

        let mut inner = self.inner.lock().unwrap();
        match sa {
            SocketAddress::Unspec => {
                inner.local_inet_address = SocketAddress::Unspec;
                inner.local_inet6_address = SocketAddress::Unspec;
            }
            SocketAddress::Inet(_) => {
                inner.local_inet_address = sa;
                inner.local_inet6_address = SocketAddress::Unspec;
            }
            SocketAddress::Inet6(_) => {
                inner.local_inet_address = SocketAddress::Unspec;
                inner.local_inet6_address = sa;
            }
            _ => unreachable!(),
        }
        Ok(())
    }

    pub fn set_local_inet_address(&self, addr_str: &str) -> Result<(), NetError> {
        let sa = SocketAddress::lookup_address(addr_str, AddressFamily::Inet)?;
        if sa.is_any() && !sa.is_unspec() {
            return Err(NetError::Input(
                "Tried to set local inet address to an any address".into(),
            ));
        }
        if sa.port() != 0 {
            return Err(NetError::Input(
                "Tried to set a local inet address with a non-zero port".into(),
            ));
        }

        let mut inner = self.inner.lock().unwrap();
        inner.local_inet_address = sa;
        Ok(())
    }

    pub fn set_local_inet6_address(&self, addr_str: &str) -> Result<(), NetError> {
        let sa = SocketAddress::lookup_address(addr_str, AddressFamily::Inet6)?;
        if sa.is_any() && !sa.is_unspec() {
            return Err(NetError::Input(
                "Tried to set local inet6 address to an any address".into(),
            ));
        }
        if sa.port() != 0 {
            return Err(NetError::Input(
                "Tried to set a local inet6 address with a non-zero port".into(),
            ));
        }

        let mut inner = self.inner.lock().unwrap();
        inner.local_inet6_address = sa;
        Ok(())
    }

    pub fn local_inet_port(&self) -> u16 {
        self.inner.lock().unwrap().local_inet_port
    }

    pub fn set_local_inet_port(&self, port: u16) {
        self.inner.lock().unwrap().local_inet_port = port;
    }

    pub fn local_inet6_port(&self) -> u16 {
        self.inner.lock().unwrap().local_inet6_port
    }

    pub fn set_local_inet6_port(&self, port: u16) {
        self.inner.lock().unwrap().local_inet6_port = port;
    }

    pub fn set_local_port(&self, port: u16) {
        let mut inner = self.inner.lock().unwrap();
        inner.local_inet_port = port;
        inner.local_inet6_port = port;
    }

    pub fn local_port_best_match(&self) -> u16 {
        let inner = self.inner.lock().unwrap();
        if inner.local_inet_port == 0 {
            return inner.local_inet6_port;
        }
        if inner.local_inet6_port == 0 {
            return inner.local_inet_port;
        }

        if inner.prefer_ipv6 {
            if inner.block_ipv6 && !inner.block_ipv4 {
                return inner.local_inet_port;
            }
            return inner.local_inet6_port;
        }

        if inner.block_ipv4 && !inner.block_ipv6 {
            return inner.local_inet6_port;
        }

        inner.local_inet_port
    }

    pub fn listen_backlog(&self) -> i32 {
        self.inner.lock().unwrap().listen_backlog
    }

    pub fn set_listen_backlog(&self, backlog: i32) -> Result<(), NetError> {
        if backlog < 1 {
            return Err(NetError::Input(
                "Tried to set a listen backlog less than 1".into(),
            ));
        }
        if backlog > 65536 {
            return Err(NetError::Input(
                "Tried to set a listen backlog greater than 65536".into(),
            ));
        }
        let mut inner = self.inner.lock().unwrap();
        inner.listen_backlog = backlog;
        Self::notify(&inner);
        Ok(())
    }

    pub fn override_dht_port(&self) -> u16 {
        self.inner.lock().unwrap().override_dht_port
    }

    pub fn set_override_dht_port(&self, port: u16) {
        self.inner.lock().unwrap().override_dht_port = port;
    }

    pub fn send_buffer_size(&self) -> u32 {
        self.inner.lock().unwrap().send_buffer_size
    }

    pub fn set_send_buffer_size(&self, s: u32) {
        self.inner.lock().unwrap().send_buffer_size = s;
    }

    pub fn receive_buffer_size(&self) -> u32 {
        self.inner.lock().unwrap().receive_buffer_size
    }

    pub fn set_receive_buffer_size(&self, s: u32) {
        self.inner.lock().unwrap().receive_buffer_size = s;
    }

    pub fn encryption_modes(&self) -> (EncryptionMode, EncryptionMode) {
        let inner = self.inner.lock().unwrap();
        (
            inner.handshake_encryption_mode,
            inner.stream_encryption_mode,
        )
    }

    pub fn set_encryption_modes(
        &self,
        handshake: EncryptionMode,
        stream: EncryptionMode,
    ) -> Result<(), NetError> {
        if handshake == EncryptionMode::Deny && stream == EncryptionMode::Require {
            return Err(NetError::Internal(
                "Invalid encryption modes: handshake_deny/stream_require".into(),
            ));
        }
        let mut inner = self.inner.lock().unwrap();
        inner.handshake_encryption_mode = handshake;
        inner.stream_encryption_mode = stream;
        Ok(())
    }

    pub fn bind_address_best_match(&self) -> SocketAddress {
        let inner = self.inner.lock().unwrap();
        Self::generic_address_best_match_inner(
            &inner,
            &inner.bind_inet_address,
            &inner.bind_inet6_address,
        )
    }

    pub fn local_address_best_match(&self) -> SocketAddress {
        let inner = self.inner.lock().unwrap();
        Self::generic_address_best_match_inner(
            &inner,
            &inner.local_inet_address,
            &inner.local_inet6_address,
        )
    }

    fn generic_address_best_match_inner(
        inner: &Inner,
        inet: &SocketAddress,
        inet6: &SocketAddress,
    ) -> SocketAddress {
        if inet.is_unspec() {
            return inet6.clone();
        }
        if inet6.is_unspec() {
            return inet.clone();
        }

        if inner.prefer_ipv6 {
            if inner.block_ipv6 && !inner.block_ipv4 {
                return inet.clone();
            }
            return inet6.clone();
        }

        if inner.block_ipv4 && !inner.block_ipv6 {
            return inet6.clone();
        }

        inet.clone()
    }

    pub fn bind_address_or_unspec_and_null(&self) -> Option<SocketAddress> {
        let inner = self.inner.lock().unwrap();
        Self::generic_address_or_unspec_and_null_inner(
            &inner,
            &inner.bind_inet_address,
            &inner.bind_inet6_address,
        )
    }

    pub fn local_address_or_unspec_and_null(&self) -> Option<SocketAddress> {
        let inner = self.inner.lock().unwrap();
        Self::generic_address_or_unspec_and_null_inner(
            &inner,
            &inner.local_inet_address,
            &inner.local_inet6_address,
        )
    }

    fn generic_address_or_unspec_and_null_inner(
        inner: &Inner,
        inet: &SocketAddress,
        inet6: &SocketAddress,
    ) -> Option<SocketAddress> {
        if inner.block_ipv4 && inner.block_ipv6 {
            return None;
        }

        if inet.is_unspec() && inet6.is_unspec() {
            return Some(inet.clone());
        }

        if inet.is_unspec() {
            if inner.block_ipv6 {
                return None;
            }
            return Some(inet6.clone());
        }

        if inet6.is_unspec() {
            if inner.block_ipv4 {
                return None;
            }
            return Some(inet.clone());
        }

        if inner.prefer_ipv6 && !inner.block_ipv6 {
            return Some(inet6.clone());
        }

        if inner.block_ipv4 {
            return Some(inet6.clone());
        }

        Some(inet.clone())
    }

    pub fn bind_address_for_tcp_connect(&self, family: AddressFamily) -> AddressDevice {
        let inner = self.inner.lock().unwrap();
        match family {
            AddressFamily::Inet => {
                if inner.block_ipv4 || inner.bind_inet_address.is_unspec() {
                    AddressDevice {
                        address: None,
                        device: String::new(),
                    }
                } else {
                    AddressDevice {
                        address: Some(inner.bind_inet_address.clone()),
                        device: inner.bind_inet_device_name.clone(),
                    }
                }
            }
            AddressFamily::Inet6 => {
                if inner.block_ipv6 || inner.bind_inet6_address.is_unspec() {
                    AddressDevice {
                        address: None,
                        device: String::new(),
                    }
                } else {
                    AddressDevice {
                        address: Some(inner.bind_inet6_address.clone()),
                        device: inner.bind_inet6_device_name.clone(),
                    }
                }
            }
            _ => AddressDevice {
                address: None,
                device: String::new(),
            },
        }
    }

    pub fn bind_address_for_udp_connect(&self, family: AddressFamily) -> AddressDevice {
        let inner = self.inner.lock().unwrap();
        if inner.block_udp {
            return AddressDevice {
                address: None,
                device: String::new(),
            };
        }
        drop(inner);
        self.bind_address_for_tcp_connect(family)
    }

    pub fn listen_addresses(&self) -> ListenAddresses {
        let inner = self.inner.lock().unwrap();

        let inet_address = if inner.block_ipv4 {
            None
        } else {
            Some(inner.bind_inet_address.clone())
        };

        let inet6_address = if inner.block_ipv6 {
            None
        } else {
            Some(inner.bind_inet6_address.clone())
        };

        if inet_address.is_none() && inet6_address.is_none() {
            return ListenAddresses {
                inet_address: None,
                inet_device: String::new(),
                inet6_address: None,
                inet6_device: String::new(),
                block_ipv4in6: false,
            };
        }

        if inet_address.is_none() {
            let inet6 = inet6_address.unwrap();
            let final_inet6 = if inet6.is_unspec() {
                SocketAddress::new_inet6_any()
            } else {
                inet6
            };
            return ListenAddresses {
                inet_address: None,
                inet_device: String::new(),
                inet6_address: Some(final_inet6),
                inet6_device: inner.bind_inet6_device_name.clone(),
                block_ipv4in6: inner.block_ipv4in6,
            };
        }

        if inet6_address.is_none() {
            let inet = inet_address.unwrap();
            let final_inet = if inet.is_unspec() {
                SocketAddress::new_inet_any()
            } else {
                inet
            };
            return ListenAddresses {
                inet_address: Some(final_inet),
                inet_device: inner.bind_inet_device_name.clone(),
                inet6_address: None,
                inet6_device: String::new(),
                block_ipv4in6: false,
            };
        }

        let inet = inet_address.unwrap();
        let inet6 = inet6_address.unwrap();

        if inet.is_unspec() && inet6.is_unspec() {
            return ListenAddresses {
                inet_address: Some(SocketAddress::new_inet_any()),
                inet_device: inner.bind_inet_device_name.clone(),
                inet6_address: Some(SocketAddress::new_inet6_any()),
                inet6_device: inner.bind_inet6_device_name.clone(),
                block_ipv4in6: true,
            };
        }

        if !inet.is_unspec() && !inet6.is_unspec() {
            return ListenAddresses {
                inet_address: Some(inet),
                inet_device: inner.bind_inet_device_name.clone(),
                inet6_address: Some(inet6),
                inet6_device: inner.bind_inet6_device_name.clone(),
                block_ipv4in6: inner.block_ipv4in6,
            };
        }

        if !inet.is_unspec() {
            return ListenAddresses {
                inet_address: Some(inet),
                inet_device: inner.bind_inet_device_name.clone(),
                inet6_address: None,
                inet6_device: String::new(),
                block_ipv4in6: false,
            };
        }

        ListenAddresses {
            inet_address: None,
            inet_device: String::new(),
            inet6_address: Some(inet6),
            inet6_device: inner.bind_inet6_device_name.clone(),
            block_ipv4in6: inner.block_ipv4in6,
        }
    }

    fn notify(inner: &Inner) {
        for sub in &inner.subscribers {
            sub();
        }
    }

    pub fn subscribe_to_changes<F>(&self, callback: F)
    where
        F: Fn() + Send + Sync + 'static,
    {
        let mut inner = self.inner.lock().unwrap();
        inner.subscribers.push(Box::new(callback));
    }
}
