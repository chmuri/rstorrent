use std::net::IpAddr;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::addr::SocketAddress;
use crate::error::NetError;

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
                return Err(NetError::Input("HTTP proxy closed connection prematurely".into()));
            }
            response_buf.push(byte[0]);

            if response_buf.ends_with(b"\r\n\r\n") {
                break;
            }
        }

        if !response_buf.ends_with(b"\r\n\r\n") {
            return Err(NetError::Input("HTTP proxy response header too large or invalid".into()));
        }

        let response_str = String::from_utf8_lossy(&response_buf);
        let first_line = response_str.lines().next().unwrap_or("");
        if !first_line.starts_with("HTTP/1.1 200") && !first_line.starts_with("HTTP/1.0 200") {
            return Err(NetError::Input(format!("HTTP proxy CONNECT rejected: {}", first_line)));
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
            return Err(NetError::Input(format!("Invalid SOCKS5 version in greeting: {}", resp[0])));
        }

        let selected_method = resp[1];
        if selected_method == 0xFF {
            return Err(NetError::Input("SOCKS5 proxy rejected authentication methods".into()));
        }

        // 2. Authentication if required
        if selected_method == 0x02 {
            let user = username.unwrap_or("");
            let pass = password.unwrap_or("");
            if user.len() > 255 || pass.len() > 255 {
                return Err(NetError::Input("SOCKS5 username or password exceeds 255 bytes".into()));
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
            return Err(NetError::Input(format!("Invalid SOCKS5 version in reply: {}", header[0])));
        }
        if header[1] != 0x00 {
            return Err(NetError::Input(format!("SOCKS5 connection rejected with code: {}", header[1])));
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
                return Err(NetError::Input(format!("Invalid SOCKS5 bound address type: {}", other)));
            }
        }

        Ok(())
    }
}
