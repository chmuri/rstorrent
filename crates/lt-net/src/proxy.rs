use std::net::{IpAddr, SocketAddr};
use std::sync::{OnceLock, RwLock};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::timeout;

use crate::addr::{AddressFamily, SocketAddress};
use crate::error::NetError;
use crate::resolver::shared_dns_resolver;

pub struct HttpProxy;

impl HttpProxy {
    pub async fn handshake<S>(stream: &mut S, host: &str, port: u16) -> Result<(), NetError>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        let request = format!(
            "CONNECT {}:{} HTTP/1.1\r\nHost: {}:{}\r\n\r\n",
            host, port, host, port
        );
        stream.write_all(request.as_bytes()).await?;

        let mut response_buf = Vec::new();
        let mut byte = [0u8; 1];

        // Read until \r\n\r\n
        while response_buf.len() < 4096 {
            let n = stream.read(&mut byte).await?;
            if n == 0 {
                return Err(NetError::Input(
                    "HTTP proxy closed connection prematurely".into(),
                ));
            }
            response_buf.push(byte[0]);

            if response_buf.ends_with(b"\r\n\r\n") {
                break;
            }
        }

        if !response_buf.ends_with(b"\r\n\r\n") {
            return Err(NetError::Input(
                "HTTP proxy response header too large or invalid".into(),
            ));
        }

        let response_str = String::from_utf8_lossy(&response_buf);
        let first_line = response_str.lines().next().unwrap_or("");
        if !first_line.starts_with("HTTP/1.1 200") && !first_line.starts_with("HTTP/1.0 200") {
            return Err(NetError::Input(format!(
                "HTTP proxy CONNECT rejected: {}",
                first_line
            )));
        }

        Ok(())
    }
}

pub struct Socks5Proxy;

impl Socks5Proxy {
    pub async fn handshake<S>(
        stream: &mut S,
        target: &SocketAddress,
        username: Option<&str>,
        password: Option<&str>,
    ) -> Result<(), NetError>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        // 1. Send Greeting
        let has_auth = username.is_some() && !username.unwrap().is_empty();
        if has_auth {
            stream.write_all(&[0x05, 0x02, 0x00, 0x02]).await?;
        } else {
            stream.write_all(&[0x05, 0x01, 0x00]).await?;
        }

        // Receive Greeting Response
        let mut resp = [0u8; 2];
        stream.read_exact(&mut resp).await?;
        if resp[0] != 0x05 {
            return Err(NetError::Input(format!(
                "Invalid SOCKS5 version in greeting: {}",
                resp[0]
            )));
        }

        let selected_method = resp[1];
        if selected_method == 0xFF {
            return Err(NetError::Input(
                "SOCKS5 proxy rejected authentication methods".into(),
            ));
        }

        // 2. Authentication if required
        if selected_method == 0x02 {
            let user = username.unwrap_or("");
            let pass = password.unwrap_or("");
            if user.len() > 255 || pass.len() > 255 {
                return Err(NetError::Input(
                    "SOCKS5 username or password exceeds 255 bytes".into(),
                ));
            }

            let mut auth_req = Vec::new();
            auth_req.push(0x01); // Auth subnegotiation version
            auth_req.push(user.len() as u8);
            auth_req.extend_from_slice(user.as_bytes());
            auth_req.push(pass.len() as u8);
            auth_req.extend_from_slice(pass.as_bytes());

            stream.write_all(&auth_req).await?;

            let mut auth_resp = [0u8; 2];
            stream.read_exact(&mut auth_resp).await?;
            if auth_resp[0] != 0x01 || auth_resp[1] != 0x00 {
                return Err(NetError::Input(format!(
                    "SOCKS5 authentication failed with status: {}",
                    auth_resp[1]
                )));
            }
        } else if selected_method != 0x00 {
            return Err(NetError::Input(format!(
                "Unsupported SOCKS5 auth method selected: {}",
                selected_method
            )));
        }

        // 3. Send CONNECT Request
        let mut req = vec![0x05, 0x01, 0x00]; // VER, CMD=CONNECT, RSV=0x00
        let target_ip = target.ip().ok_or_else(|| {
            NetError::Input("SOCKS5 connect requires an IP socket address".into())
        })?;
        let target_port = target.port();

        match target_ip {
            IpAddr::V4(v4) => {
                req.push(0x01); // ATYP IPv4
                req.extend_from_slice(&v4.octets());
            }
            IpAddr::V6(v6) => {
                req.push(0x04); // ATYP IPv6
                req.extend_from_slice(&v6.octets());
            }
        }

        req.push((target_port >> 8) as u8);
        req.push((target_port & 0xFF) as u8);

        stream.write_all(&req).await?;

        // 4. Receive CONNECT Response
        let mut header = [0u8; 4];
        stream.read_exact(&mut header).await?;
        if header[0] != 0x05 {
            return Err(NetError::Input(format!(
                "Invalid SOCKS5 version in reply: {}",
                header[0]
            )));
        }
        if header[1] != 0x00 {
            return Err(NetError::Input(format!(
                "SOCKS5 connection rejected with code: {}",
                header[1]
            )));
        }

        // Skip bound address & port
        match header[3] {
            0x01 => {
                let mut bnd = [0u8; 4 + 2]; // IPv4 + port
                stream.read_exact(&mut bnd).await?;
            }
            0x03 => {
                let mut len = [0u8; 1];
                stream.read_exact(&mut len).await?;
                let mut bnd = vec![0u8; len[0] as usize + 2];
                stream.read_exact(&mut bnd).await?;
            }
            0x04 => {
                let mut bnd = [0u8; 16 + 2]; // IPv6 + port
                stream.read_exact(&mut bnd).await?;
            }
            other => {
                return Err(NetError::Input(format!(
                    "Invalid SOCKS5 bound address type: {}",
                    other
                )));
            }
        }

        Ok(())
    }
}

/// Transport used to reach a proxy server.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProxyKind {
    Socks5,
    Http,
}

/// A parsed proxy endpoint used to tunnel outbound peer TCP connections.
#[derive(Clone, Debug)]
pub struct ProxyConfig {
    pub kind: ProxyKind,
    pub host: String,
    pub port: u16,
    pub username: Option<String>,
    pub password: Option<String>,
}

impl ProxyConfig {
    /// Parse a proxy URL (`socks5://`, `socks5h://`, `http://`, `https://`).
    /// An empty string yields `None` (no proxy). `socks5h` is treated as
    /// `socks5` because peer targets are always numeric IPs.
    pub fn parse(url: &str) -> Result<Option<Self>, NetError> {
        let trimmed = url.trim();
        if trimmed.is_empty() {
            return Ok(None);
        }
        if trimmed.chars().any(char::is_whitespace) {
            return Err(NetError::Input(
                "Proxy address must not contain whitespace.".into(),
            ));
        }
        let (scheme, rest) = trimmed
            .split_once("://")
            .ok_or_else(|| NetError::Input("Proxy address must include a scheme.".into()))?;
        let kind = match scheme {
            "socks5" | "socks5h" => ProxyKind::Socks5,
            "http" | "https" => ProxyKind::Http,
            other => {
                return Err(NetError::Input(format!(
                    "Unsupported proxy scheme: {other}"
                )))
            }
        };
        if rest.is_empty() || rest.starts_with('/') {
            return Err(NetError::Input("Proxy address must include a host.".into()));
        }
        if rest.contains('/') || rest.contains('?') || rest.contains('#') {
            return Err(NetError::Input(
                "Proxy address must not include a path, query, or fragment.".into(),
            ));
        }

        let (userinfo, host_port) = match rest.rsplit_once('@') {
            Some((u, hp)) => (Some(u), hp),
            None => (None, rest),
        };
        let (username, password) = match userinfo {
            Some(u) => match u.split_once(':') {
                Some((user, pass)) => (Some(user.to_string()), Some(pass.to_string())),
                None => (Some(u.to_string()), None),
            },
            None => (None, None),
        };

        let (host, port_str) = host_port
            .rsplit_once(':')
            .ok_or_else(|| NetError::Input("Proxy address must include a port.".into()))?;
        if host.is_empty() {
            return Err(NetError::Input("Proxy address must include a host.".into()));
        }
        let port: u16 = port_str
            .parse()
            .map_err(|_| NetError::Input("Proxy address must include a valid port.".into()))?;
        if port == 0 {
            return Err(NetError::Input("Proxy address must include a port.".into()));
        }

        Ok(Some(Self {
            kind,
            host: host.to_string(),
            port,
            username,
            password,
        }))
    }

    /// Open a TCP connection to `target` through this proxy, bounded by
    /// `connect_timeout` (covers both the proxy connect and the handshake).
    pub async fn connect(
        &self,
        target: SocketAddr,
        connect_timeout: Duration,
    ) -> Result<TcpStream, NetError> {
        timeout(connect_timeout, self.connect_inner(target))
            .await
            .map_err(|_| {
                NetError::Input(format!(
                    "Proxy connection timed out: {}:{}",
                    self.host, self.port
                ))
            })?
    }

    async fn connect_inner(&self, target: SocketAddr) -> Result<TcpStream, NetError> {
        let proxy_addr = resolve_proxy_host(&self.host, self.port).await?;
        let mut stream = TcpStream::connect(proxy_addr)
            .await
            .map_err(|e| NetError::Input(format!("Proxy connect to {proxy_addr} failed: {e}")))?;
        match self.kind {
            ProxyKind::Socks5 => {
                let sa = SocketAddress::from(target);
                Socks5Proxy::handshake(
                    &mut stream,
                    &sa,
                    self.username.as_deref(),
                    self.password.as_deref(),
                )
                .await?;
            }
            ProxyKind::Http => {
                HttpProxy::handshake(&mut stream, &target.ip().to_string(), target.port()).await?;
            }
        }
        Ok(stream)
    }
}

/// Resolve a proxy host (numeric IP or hostname) to a socket address, using
/// the shared caching DNS resolver when available.
async fn resolve_proxy_host(host: &str, port: u16) -> Result<SocketAddr, NetError> {
    let bare = host
        .strip_prefix('[')
        .and_then(|h| h.strip_suffix(']'))
        .unwrap_or(host);
    if let Ok(ip) = bare.parse::<IpAddr>() {
        return Ok(SocketAddr::new(ip, port));
    }
    if let Some(resolver) = shared_dns_resolver() {
        let sa = resolver
            .resolve_preferred(host, AddressFamily::Unspec, AddressFamily::Inet)
            .await?;
        if let Some(addr) = sa.and_then(|s| s.as_socket_addr()) {
            return Ok(addr);
        }
        return Err(NetError::Input(format!(
            "Proxy host resolved to no address: {host}"
        )));
    }
    tokio::net::lookup_host((host, port))
        .await
        .map_err(|e| NetError::Input(format!("Proxy host lookup failed for {host}: {e}")))?
        .next()
        .ok_or_else(|| NetError::Input(format!("Proxy host resolved to no address: {host}")))
}

static PROXY_CONFIG: OnceLock<RwLock<Option<ProxyConfig>>> = OnceLock::new();

/// The process-wide proxy configuration handle, created on first use.
pub fn shared_proxy_config() -> &'static RwLock<Option<ProxyConfig>> {
    PROXY_CONFIG.get_or_init(|| RwLock::new(None))
}

/// Install (or clear, with `None`) the process-wide outbound peer proxy.
pub fn set_shared_proxy(config: Option<ProxyConfig>) {
    *shared_proxy_config()
        .write()
        .unwrap_or_else(|e| e.into_inner()) = config;
}

/// Snapshot the currently configured outbound peer proxy, if any.
pub fn current_proxy() -> Option<ProxyConfig> {
    shared_proxy_config()
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_empty_yields_none() {
        assert!(ProxyConfig::parse("").unwrap().is_none());
        assert!(ProxyConfig::parse("   ").unwrap().is_none());
    }

    #[test]
    fn parse_socks5_with_auth() {
        let cfg = ProxyConfig::parse("socks5://user:pass@127.0.0.1:1080")
            .unwrap()
            .unwrap();
        assert_eq!(cfg.kind, ProxyKind::Socks5);
        assert_eq!(cfg.host, "127.0.0.1");
        assert_eq!(cfg.port, 1080);
        assert_eq!(cfg.username.as_deref(), Some("user"));
        assert_eq!(cfg.password.as_deref(), Some("pass"));
    }

    #[test]
    fn parse_socks5h_is_socks5() {
        let cfg = ProxyConfig::parse("socks5h://proxy.example:1080")
            .unwrap()
            .unwrap();
        assert_eq!(cfg.kind, ProxyKind::Socks5);
        assert_eq!(cfg.host, "proxy.example");
        assert!(cfg.username.is_none());
    }

    #[test]
    fn parse_http() {
        let cfg = ProxyConfig::parse("http://10.0.0.1:8080").unwrap().unwrap();
        assert_eq!(cfg.kind, ProxyKind::Http);
        assert_eq!(cfg.port, 8080);
    }

    #[test]
    fn parse_ipv6_host_keeps_brackets() {
        let cfg = ProxyConfig::parse("socks5://[::1]:1080").unwrap().unwrap();
        assert_eq!(cfg.host, "[::1]");
        assert_eq!(cfg.port, 1080);
    }

    #[test]
    fn parse_rejects_bad_urls() {
        assert!(ProxyConfig::parse("127.0.0.1:1080").is_err());
        assert!(ProxyConfig::parse("ftp://host:21").is_err());
        assert!(ProxyConfig::parse("socks5://host").is_err());
        assert!(ProxyConfig::parse("socks5://host:0").is_err());
        assert!(ProxyConfig::parse("socks5://host:1080/path").is_err());
        assert!(ProxyConfig::parse("socks5://host:abc").is_err());
        assert!(ProxyConfig::parse("socks5://ho st:1080").is_err());
    }

    #[tokio::test]
    async fn resolve_numeric_proxy_host() {
        let addr = resolve_proxy_host("127.0.0.1", 1080).await.unwrap();
        assert_eq!(addr, SocketAddr::from(([127, 0, 0, 1], 1080)));
        let v6 = resolve_proxy_host("[::1]", 1080).await.unwrap();
        assert_eq!(v6, SocketAddr::from(([0, 0, 0, 0, 0, 0, 0, 1], 1080)));
    }

    #[test]
    fn shared_proxy_roundtrip() {
        set_shared_proxy(None);
        assert!(current_proxy().is_none());
        let cfg = ProxyConfig::parse("socks5://127.0.0.1:1080").unwrap();
        set_shared_proxy(cfg);
        assert!(current_proxy().is_some());
        set_shared_proxy(None);
        assert!(current_proxy().is_none());
    }
}
