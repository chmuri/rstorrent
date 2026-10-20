use std::cmp::Ordering;
use std::fmt;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6};
use std::path::{Path, PathBuf};
use std::str::FromStr;

use crate::error::NetError;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum AddressFamily {
    Unspec,
    Inet,
    Inet6,
    Unix,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SocketAddress {
    Unspec,
    Inet(SocketAddrV4),
    Inet6(SocketAddrV6),
    Unix(PathBuf),
}

impl Default for SocketAddress {
    fn default() -> Self {
        SocketAddress::Unspec
    }
}

impl SocketAddress {
    pub fn new_unspec() -> Self {
        SocketAddress::Unspec
    }

    pub fn new_inet(ip: Ipv4Addr, port: u16) -> Self {
        SocketAddress::Inet(SocketAddrV4::new(ip, port))
    }

    pub fn new_inet_any() -> Self {
        SocketAddress::Inet(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0))
    }

    pub fn new_inet6(ip: Ipv6Addr, port: u16) -> Self {
        SocketAddress::Inet6(SocketAddrV6::new(ip, port, 0, 0))
    }

    pub fn new_inet6_any() -> Self {
        SocketAddress::Inet6(SocketAddrV6::new(Ipv6Addr::UNSPECIFIED, 0, 0, 0))
    }

    pub fn new_unix(path: impl AsRef<Path>) -> Self {
        SocketAddress::Unix(path.as_ref().to_path_buf())
    }

    pub fn family(&self) -> AddressFamily {
        match self {
            SocketAddress::Unspec => AddressFamily::Unspec,
            SocketAddress::Inet(_) => AddressFamily::Inet,
            SocketAddress::Inet6(_) => AddressFamily::Inet6,
            SocketAddress::Unix(_) => AddressFamily::Unix,
        }
    }

    pub fn is_unspec(&self) -> bool {
        matches!(self, SocketAddress::Unspec)
    }

    pub fn is_inet(&self) -> bool {
        matches!(self, SocketAddress::Inet(_))
    }

    pub fn is_inet6(&self) -> bool {
        matches!(self, SocketAddress::Inet6(_))
    }

    pub fn is_inet_inet6(&self) -> bool {
        matches!(self, SocketAddress::Inet(_) | SocketAddress::Inet6(_))
    }

    pub fn is_unix(&self) -> bool {
        matches!(self, SocketAddress::Unix(_))
    }

    pub fn is_any(&self) -> bool {
        match self {
            SocketAddress::Inet(v4) => v4.ip().is_unspecified(),
            SocketAddress::Inet6(v6) => {
                if let Some(v4) = self.v4_mapped_ip() {
                    v4.is_unspecified()
                } else {
                    v6.ip().is_unspecified()
                }
            }
            _ => true,
        }
    }

    pub fn is_broadcast(&self) -> bool {
        match self {
            SocketAddress::Inet(v4) => v4.ip().is_broadcast(),
            SocketAddress::Inet6(_) => {
                if let Some(v4) = self.v4_mapped_ip() {
                    v4.is_broadcast()
                } else {
                    false
                }
            }
            _ => false,
        }
    }

    pub fn is_loopback(&self) -> bool {
        match self {
            SocketAddress::Inet(v4) => v4.ip().is_loopback(),
            SocketAddress::Inet6(v6) => {
                if let Some(v4) = self.v4_mapped_ip() {
                    v4.is_loopback()
                } else {
                    v6.ip().is_loopback()
                }
            }
            _ => false,
        }
    }

    pub fn is_link_local(&self) -> bool {
        match self {
            SocketAddress::Inet(v4) => {
                let oct = v4.ip().octets();
                oct[0] == 169 && oct[1] == 254
            }
            SocketAddress::Inet6(v6) => {
                if let Some(v4) = self.v4_mapped_ip() {
                    let oct = v4.octets();
                    oct[0] == 169 && oct[1] == 254
                } else {
                    let oct = v6.ip().octets();
                    // Unicast link-local: fe80::/10
                    let is_unicast_ll = oct[0] == 0xfe && (oct[1] & 0xc0) == 0x80;
                    // Multicast link-local: ff02::/16
                    let is_multicast_ll = oct[0] == 0xff && (oct[1] & 0x0f) == 0x02;
                    is_unicast_ll || is_multicast_ll
                }
            }
            _ => false,
        }
    }

    pub fn is_v4mapped(&self) -> bool {
        self.v4_mapped_ip().is_some()
    }

    pub fn v4_mapped_ip(&self) -> Option<Ipv4Addr> {
        match self {
            SocketAddress::Inet6(v6) => {
                let oct = v6.ip().octets();
                if oct[0..10] == [0; 10] && oct[10] == 0xff && oct[11] == 0xff {
                    Some(Ipv4Addr::new(oct[12], oct[13], oct[14], oct[15]))
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    pub fn is_port_any(&self) -> bool {
        self.port() == 0
    }

    pub fn port(&self) -> u16 {
        match self {
            SocketAddress::Inet(v4) => v4.port(),
            SocketAddress::Inet6(v6) => v6.port(),
            _ => 0,
        }
    }

    pub fn set_port(&mut self, port: u16) -> Result<(), NetError> {
        match self {
            SocketAddress::Inet(v4) => {
                v4.set_port(port);
                Ok(())
            }
            SocketAddress::Inet6(v6) => {
                v6.set_port(port);
                Ok(())
            }
            _ => Err(NetError::Internal(
                "Cannot set port on non-inet address".into(),
            )),
        }
    }

    pub fn copy_addr(&self, port: u16) -> Result<Self, NetError> {
        match self {
            SocketAddress::Unspec => Ok(SocketAddress::Unspec),
            SocketAddress::Inet(v4) => Ok(SocketAddress::Inet(SocketAddrV4::new(*v4.ip(), port))),
            SocketAddress::Inet6(v6) => Ok(SocketAddress::Inet6(SocketAddrV6::new(
                *v6.ip(),
                port,
                0,
                0,
            ))),
            SocketAddress::Unix(p) => Ok(SocketAddress::Unix(p.clone())),
        }
    }

    pub fn to_v4mapped(&self) -> Result<Self, NetError> {
        match self {
            SocketAddress::Inet(v4) => {
                let v6_ip = v4.ip().to_ipv6_mapped();
                Ok(SocketAddress::Inet6(SocketAddrV6::new(
                    v6_ip,
                    v4.port(),
                    0,
                    0,
                )))
            }
            _ => Err(NetError::Internal(
                "sa_to_v4mapped: address is not inet".into(),
            )),
        }
    }

    pub fn from_v4mapped(&self) -> Result<Self, NetError> {
        match self {
            SocketAddress::Inet6(v6) => {
                if let Some(v4) = self.v4_mapped_ip() {
                    Ok(SocketAddress::Inet(SocketAddrV4::new(v4, v6.port())))
                } else {
                    Err(NetError::Internal(
                        "sa_from_v4mapped: address is not v4mapped".into(),
                    ))
                }
            }
            _ => Err(NetError::Internal(
                "sa_from_v4mapped: address is not inet6".into(),
            )),
        }
    }

    pub fn addr_str(&self) -> String {
        match self {
            SocketAddress::Unspec => String::new(),
            SocketAddress::Inet(v4) => v4.ip().to_string(),
            SocketAddress::Inet6(v6) => v6.ip().to_string(),
            SocketAddress::Unix(p) => p.to_string_lossy().into_owned(),
        }
    }

    pub fn pretty_str(&self) -> String {
        match self {
            SocketAddress::Unspec => "unspec".to_string(),
            SocketAddress::Inet(v4) => {
                if v4.port() != 0 {
                    format!("{}:{}", v4.ip(), v4.port())
                } else {
                    v4.ip().to_string()
                }
            }
            SocketAddress::Inet6(v6) => {
                if v6.port() != 0 {
                    format!("[{}]:{}", v6.ip(), v6.port())
                } else {
                    format!("[{}]", v6.ip())
                }
            }
            SocketAddress::Unix(p) => p.to_string_lossy().into_owned(),
        }
    }

    pub fn equal(&self, other: &Self) -> Result<bool, NetError> {
        if self.is_unix() || other.is_unix() {
            return Err(NetError::Internal(
                "sa_equal: unix sockaddr is not supported".into(),
            ));
        }

        match (self, other) {
            (SocketAddress::Unspec, SocketAddress::Unspec) => Ok(true),
            (SocketAddress::Inet(l), SocketAddress::Inet(r)) => {
                Ok(l.port() == r.port() && l.ip() == r.ip())
            }
            (SocketAddress::Inet6(l), SocketAddress::Inet6(r)) => {
                // In C++ libtorrent sin6_equal compares only port and sin6_addr bytes,
                // ignoring flowinfo and scope_id!
                Ok(l.port() == r.port() && l.ip() == r.ip())
            }
            _ => Ok(false),
        }
    }

    pub fn equal_addr(&self, other: &Self) -> Result<bool, NetError> {
        if self.is_unix() || other.is_unix() {
            return Err(NetError::Internal(
                "sa_equal_addr: unix sockaddr is not supported".into(),
            ));
        }

        match (self, other) {
            (SocketAddress::Unspec, SocketAddress::Unspec) => Ok(true),
            (SocketAddress::Inet(l), SocketAddress::Inet(r)) => Ok(l.ip() == r.ip()),
            (SocketAddress::Inet6(l), SocketAddress::Inet6(r)) => Ok(l.ip() == r.ip()),
            _ => Ok(false),
        }
    }

    pub fn less(&self, other: &Self) -> Result<bool, NetError> {
        if !self.is_inet_inet6() || !other.is_inet_inet6() {
            return Err(NetError::Internal(
                "sa_less: address is not inet or inet6".into(),
            ));
        }

        match (self, other) {
            (SocketAddress::Inet(l), SocketAddress::Inet(r)) => {
                let lip = u32::from(*l.ip());
                let rip = u32::from(*r.ip());
                if lip != rip {
                    Ok(lip < rip)
                } else {
                    Ok(l.port() < r.port())
                }
            }
            (SocketAddress::Inet6(l), SocketAddress::Inet6(r)) => {
                let loct = l.ip().octets();
                let roct = r.ip().octets();
                match loct.cmp(&roct) {
                    Ordering::Less => Ok(true),
                    Ordering::Greater => Ok(false),
                    Ordering::Equal => Ok(l.port() < r.port()),
                }
            }
            (SocketAddress::Inet(_), SocketAddress::Inet6(_)) => Ok(true),
            (SocketAddress::Inet6(_), SocketAddress::Inet(_)) => Ok(false),
            _ => unreachable!(),
        }
    }

    pub fn less_addr(&self, other: &Self) -> Result<bool, NetError> {
        if !self.is_inet_inet6() || !other.is_inet_inet6() {
            return Err(NetError::Internal(
                "sa_less_addr: address is not inet or inet6".into(),
            ));
        }

        match (self, other) {
            (SocketAddress::Inet(l), SocketAddress::Inet(r)) => {
                Ok(u32::from(*l.ip()) < u32::from(*r.ip()))
            }
            (SocketAddress::Inet6(l), SocketAddress::Inet6(r)) => {
                Ok(l.ip().octets() < r.ip().octets())
            }
            (SocketAddress::Inet(_), SocketAddress::Inet6(_)) => Ok(true),
            (SocketAddress::Inet6(_), SocketAddress::Inet(_)) => Ok(false),
            _ => unreachable!(),
        }
    }

    pub fn as_socket_addr(&self) -> Option<SocketAddr> {
        match self {
            SocketAddress::Inet(v4) => Some(SocketAddr::V4(*v4)),
            SocketAddress::Inet6(v6) => Some(SocketAddr::V6(*v6)),
            _ => None,
        }
    }

    pub fn ip(&self) -> Option<IpAddr> {
        match self {
            SocketAddress::Inet(v4) => Some(IpAddr::V4(*v4.ip())),
            SocketAddress::Inet6(v6) => Some(IpAddr::V6(*v6.ip())),
            _ => None,
        }
    }

    pub fn lookup_numeric(
        address_str: &str,
        family: AddressFamily,
    ) -> Result<(Option<Self>, bool), NetError> {
        let (v4, v6) = Self::try_lookup_numeric(address_str)?;
        match family {
            AddressFamily::Unspec => {
                if let Some(sa) = v4 {
                    Ok((Some(sa), true))
                } else if let Some(sa) = v6 {
                    Ok((Some(sa), true))
                } else {
                    Ok((None, true))
                }
            }
            AddressFamily::Inet => {
                if let Some(sa) = v4 {
                    Ok((Some(sa), true))
                } else if v6.is_some() {
                    Ok((None, false))
                } else {
                    Ok((None, true))
                }
            }
            AddressFamily::Inet6 => {
                if let Some(sa) = v6 {
                    Ok((Some(sa), true))
                } else if v4.is_some() {
                    Ok((None, false))
                } else {
                    Ok((None, true))
                }
            }
            AddressFamily::Unix => Err(NetError::Internal(
                "lookup_numeric: unix invalid family".into(),
            )),
        }
    }

    pub fn try_lookup_numeric(hostname: &str) -> Result<(Option<Self>, Option<Self>), NetError> {
        if let Ok(v4) = Ipv4Addr::from_str(hostname) {
            return Ok((Some(SocketAddress::new_inet(v4, 0)), None));
        }
        if let Ok(v6) = Ipv6Addr::from_str(hostname) {
            return Ok((None, Some(SocketAddress::new_inet6(v6, 0))));
        }
        Ok((None, None))
    }

    pub fn lookup_address(address_str: &str, family: AddressFamily) -> Result<Self, NetError> {
        if address_str.is_empty() {
            return Ok(SocketAddress::Unspec);
        }

        // First attempt direct numeric parsing
        if let Ok((opt, _)) = Self::lookup_numeric(address_str, family) {
            if let Some(sa) = opt {
                return Ok(sa);
            }
        }

        // Try parsing full socket addr (with port)
        if let Ok(sa) = SocketAddr::from_str(address_str) {
            match (sa, family) {
                (SocketAddr::V4(v4), AddressFamily::Unspec | AddressFamily::Inet) => {
                    return Ok(SocketAddress::Inet(v4));
                }
                (SocketAddr::V6(v6), AddressFamily::Unspec | AddressFamily::Inet6) => {
                    return Ok(SocketAddress::Inet6(v6));
                }
                _ => {}
            }
        }

        // Fallback to std::net::ToSocketAddrs for DNS / system resolution
        use std::net::ToSocketAddrs;
        let host_port = if address_str.contains(':')
            && !address_str.contains("::")
            && !address_str.starts_with('[')
        {
            address_str.to_string()
        } else if address_str.starts_with('[') && address_str.contains("]:") {
            address_str.to_string()
        } else {
            format!("{}:0", address_str)
        };

        match host_port.to_socket_addrs() {
            Ok(addrs) => {
                for addr in addrs {
                    match (addr, family) {
                        (SocketAddr::V4(v4), AddressFamily::Unspec | AddressFamily::Inet) => {
                            return Ok(SocketAddress::Inet(v4));
                        }
                        (SocketAddr::V6(v6), AddressFamily::Unspec | AddressFamily::Inet6) => {
                            return Ok(SocketAddress::Inet6(v6));
                        }
                        _ => continue,
                    }
                }
                Err(NetError::Input(format!(
                    "Could not get address info for family: {}",
                    address_str
                )))
            }
            Err(e) => Err(NetError::Input(format!(
                "Could not get address info: {}: {}",
                address_str, e
            ))),
        }
    }
}

impl From<SocketAddr> for SocketAddress {
    fn from(sa: SocketAddr) -> Self {
        match sa {
            SocketAddr::V4(v4) => SocketAddress::Inet(v4),
            SocketAddr::V6(v6) => SocketAddress::Inet6(v6),
        }
    }
}

impl fmt::Display for SocketAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.pretty_str())
    }
}

impl FromStr for SocketAddress {
    type Err = NetError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let trimmed = s.trim();
        if trimmed.is_empty() || trimmed == "unspec" {
            return Ok(SocketAddress::Unspec);
        }

        if let Ok(v4) = Ipv4Addr::from_str(trimmed) {
            return Ok(SocketAddress::new_inet(v4, 0));
        }
        if let Ok(v6) = Ipv6Addr::from_str(trimmed) {
            return Ok(SocketAddress::new_inet6(v6, 0));
        }

        if let Ok(sa) = SocketAddr::from_str(trimmed) {
            return match sa {
                SocketAddr::V4(v4) => Ok(SocketAddress::Inet(v4)),
                SocketAddr::V6(v6) => Ok(SocketAddress::Inet6(v6)),
            };
        }

        Err(NetError::Input(format!(
            "Invalid socket address format: {}",
            s
        )))
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SocketAddressKey {
    Unspec,
    V4(Ipv4Addr),
    V6(Ipv6Addr),
}

impl SocketAddressKey {
    pub fn is_valid(&self) -> bool {
        !matches!(self, SocketAddressKey::Unspec)
    }

    pub fn from_socket_address(sa: &SocketAddress) -> Self {
        match sa {
            SocketAddress::Unspec => SocketAddressKey::Unspec,
            SocketAddress::Inet(v4) => SocketAddressKey::V4(*v4.ip()),
            SocketAddress::Inet6(v6) => SocketAddressKey::V6(*v6.ip()),
            SocketAddress::Unix(_) => SocketAddressKey::Unspec,
        }
    }

    pub fn from_ip(ip: IpAddr) -> Self {
        match ip {
            IpAddr::V4(v4) => SocketAddressKey::V4(v4),
            IpAddr::V6(v6) => SocketAddressKey::V6(v6),
        }
    }
}

/// A collection of peer socket addresses.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AddressList(pub Vec<SocketAddress>);

impl AddressList {
    pub fn new() -> Self {
        Self(Vec::new())
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self(Vec::with_capacity(capacity))
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn push(&mut self, sa: SocketAddress) {
        self.0.push(sa);
    }

    pub fn clear(&mut self) {
        self.0.clear();
    }

    pub fn sort(&mut self) {
        self.0.sort();
    }

    pub fn sort_and_unique(&mut self) {
        self.sort();
        self.0.dedup();
    }

    pub fn parse_address_compact(&mut self, data: &[u8]) {
        for chunk in data.chunks_exact(6) {
            let ip = Ipv4Addr::new(chunk[0], chunk[1], chunk[2], chunk[3]);
            let port = u16::from_be_bytes([chunk[4], chunk[5]]);
            self.0.push(SocketAddress::new_inet(ip, port));
        }
    }

    pub fn parse_address_compact_ipv6(&mut self, data: &[u8]) {
        for chunk in data.chunks_exact(18) {
            let mut ip_bytes = [0u8; 16];
            ip_bytes.copy_from_slice(&chunk[0..16]);
            let ip = Ipv6Addr::from(ip_bytes);
            let port = u16::from_be_bytes([chunk[16], chunk[17]]);
            self.0.push(SocketAddress::new_inet6(ip, port));
        }
    }

    pub fn parse_address_normal(&mut self, list: &[lt_common::object::Object]) {
        for obj in list {
            if let Ok(ip_obj) = obj.get_key("ip") {
                if let Ok(port_obj) = obj.get_key("port") {
                    let port_val = port_obj.as_value().unwrap_or(0);
                    if port_val <= 0 || port_val >= (1 << 16) {
                        continue;
                    }
                    let port = port_val as u16;

                    if let Ok(ip_bytes) = ip_obj.as_string() {
                        if let Ok(ip_str) = std::str::from_utf8(ip_bytes) {
                            if let Ok(ip) = ip_str.parse::<IpAddr>() {
                                match ip {
                                    IpAddr::V4(v4) => {
                                        if !v4.is_unspecified() {
                                            self.0.push(SocketAddress::new_inet(v4, port));
                                        }
                                    }
                                    IpAddr::V6(v6) => {
                                        if !v6.is_unspecified() {
                                            self.0.push(SocketAddress::new_inet6(v6, port));
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

impl std::ops::Deref for AddressList {
    type Target = Vec<SocketAddress>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl std::ops::DerefMut for AddressList {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}
