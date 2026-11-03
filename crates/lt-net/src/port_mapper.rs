//! Automatic Port Forwarding (UPnP IGD and NAT-PMP / PCP).
//!
//! Provides automatic port mapping on home routers so incoming BitTorrent TCP
//! connections and UDP DHT traffic can reach the client without manual router
//! configuration.

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::time::Duration;
use tokio::net::UdpSocket;
use tokio::time::timeout;

use crate::error::NetError;

/// Protocol for the port mapping request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortMappingProtocol {
    Tcp,
    Udp,
}

impl PortMappingProtocol {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Tcp => "TCP",
            Self::Udp => "UDP",
        }
    }
}

/// NAT-PMP request constants.
pub const NAT_PMP_PORT: u16 = 5351;
const NAT_PMP_VERSION: u8 = 0;
const NAT_PMP_OPCODE_MAP_UDP: u8 = 1;
const NAT_PMP_OPCODE_MAP_TCP: u8 = 2;

/// Builds a 12-byte NAT-PMP port mapping request.
pub fn format_nat_pmp_request(
    protocol: PortMappingProtocol,
    internal_port: u16,
    external_port: u16,
    lifetime_secs: u32,
) -> [u8; 12] {
    let mut buf = [0u8; 12];
    buf[0] = NAT_PMP_VERSION;
    buf[1] = match protocol {
        PortMappingProtocol::Udp => NAT_PMP_OPCODE_MAP_UDP,
        PortMappingProtocol::Tcp => NAT_PMP_OPCODE_MAP_TCP,
    };
    buf[2] = 0; // reserved
    buf[3] = 0; // reserved
    buf[4..6].copy_from_slice(&internal_port.to_be_bytes());
    buf[6..8].copy_from_slice(&external_port.to_be_bytes());
    buf[8..12].copy_from_slice(&lifetime_secs.to_be_bytes());
    buf
}

/// Parses a 16-byte NAT-PMP port mapping response.
pub fn parse_nat_pmp_response(
    buf: &[u8],
    expected_protocol: PortMappingProtocol,
) -> Result<u16, NetError> {
    if buf.len() < 16 {
        return Err(NetError::Input("NAT-PMP response too short".into()));
    }

    let version = buf[0];
    let opcode = buf[1];
    let result_code = u16::from_be_bytes([buf[2], buf[3]]);

    if version != NAT_PMP_VERSION {
        return Err(NetError::Input(format!(
            "unsupported NAT-PMP version: {version}"
        )));
    }

    let expected_opcode = match expected_protocol {
        PortMappingProtocol::Udp => NAT_PMP_OPCODE_MAP_UDP + 128,
        PortMappingProtocol::Tcp => NAT_PMP_OPCODE_MAP_TCP + 128,
    };

    if opcode != expected_opcode {
        return Err(NetError::Input(format!(
            "unexpected NAT-PMP opcode: {opcode}"
        )));
    }

    if result_code != 0 {
        return Err(NetError::Input(format!(
            "NAT-PMP error code: {result_code}"
        )));
    }

    let mapped_external_port = u16::from_be_bytes([buf[10], buf[11]]);
    Ok(mapped_external_port)
}

/// Attempts to map a port using NAT-PMP on the default gateway router.
pub async fn try_map_nat_pmp(
    gateway: Ipv4Addr,
    internal_port: u16,
    external_port: u16,
    protocol: PortMappingProtocol,
    lifetime_secs: u32,
) -> Result<u16, NetError> {
    let socket = UdpSocket::bind("0.0.0.0:0")
        .await
        .map_err(|e| NetError::Io(std::io::Error::other(e)))?;

    let target = SocketAddr::V4(SocketAddrV4::new(gateway, NAT_PMP_PORT));
    let req = format_nat_pmp_request(protocol, internal_port, external_port, lifetime_secs);

    socket
        .send_to(&req, target)
        .await
        .map_err(|e| NetError::Io(std::io::Error::other(e)))?;

    let mut buf = [0u8; 64];
    let (len, _src) = timeout(Duration::from_secs(2), socket.recv_from(&mut buf))
        .await
        .map_err(|_| {
            NetError::Io(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "NAT-PMP timeout",
            ))
        })?
        .map_err(|e| NetError::Io(std::io::Error::other(e)))?;

    parse_nat_pmp_response(&buf[..len], protocol)
}

/// Discovers a UPnP IGD device and adds a port mapping.
pub async fn try_map_upnp(
    internal_port: u16,
    external_port: u16,
    protocol: PortMappingProtocol,
    description: &str,
    lease_duration: u32,
) -> Result<u16, NetError> {
    let socket = UdpSocket::bind("0.0.0.0:0")
        .await
        .map_err(|e| NetError::Io(std::io::Error::other(e)))?;

    let search_packet = format_upnp_ssdp_search();
    let ssdp_target: SocketAddr = "239.255.255.250:1900".parse().unwrap();
    let _ = socket.send_to(search_packet, ssdp_target).await;

    let mut buf = [0u8; 2048];
    let (len, _sender) = timeout(Duration::from_secs(2), socket.recv_from(&mut buf))
        .await
        .map_err(|_| {
            NetError::Io(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "SSDP timeout",
            ))
        })?
        .map_err(|e| NetError::Io(std::io::Error::other(e)))?;

    let response_text = String::from_utf8_lossy(&buf[..len]);
    let location_url = response_text
        .lines()
        .find(|line| line.to_ascii_lowercase().starts_with("location:"))
        .and_then(|line| line.split_once(':').map(|(_, url)| url.trim()))
        .ok_or_else(|| NetError::Input("UPnP location header missing".into()))?;

    let local_ip = socket
        .local_addr()
        .map(|a| a.ip().to_string())
        .unwrap_or_else(|_| "127.0.0.1".to_string());

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
        .map_err(|e| NetError::Input(e.to_string()))?;

    let desc_xml = client
        .get(location_url)
        .send()
        .await
        .map_err(|e| NetError::Input(e.to_string()))?
        .text()
        .await
        .map_err(|e| NetError::Input(e.to_string()))?;

    let service_type = if desc_xml.contains("urn:schemas-upnp-org:service:WANIPConnection:1") {
        "urn:schemas-upnp-org:service:WANIPConnection:1"
    } else if desc_xml.contains("urn:schemas-upnp-org:service:WANPPPConnection:1") {
        "urn:schemas-upnp-org:service:WANPPPConnection:1"
    } else {
        "urn:schemas-upnp-org:service:WANIPConnection:1"
    };

    let control_url = if let Some(idx) = desc_xml.find("<controlURL>") {
        let rest = &desc_xml[idx + "<controlURL>".len()..];
        if let Some(end_idx) = rest.find("</controlURL>") {
            let u = rest[..end_idx].trim();
            if u.starts_with("http://") || u.starts_with("https://") {
                u.to_string()
            } else {
                let base = if let Some((prefix, _)) = location_url.split_once("://") {
                    if let Some((host_port, _)) = location_url[prefix.len() + 3..].split_once('/') {
                        format!("{}://{}", prefix, host_port)
                    } else {
                        location_url.to_string()
                    }
                } else {
                    location_url.to_string()
                };
                let slash = if u.starts_with('/') { "" } else { "/" };
                format!("{}{}{}", base, slash, u)
            }
        } else {
            location_url.to_string()
        }
    } else {
        location_url.to_string()
    };

    let soap_body = format_upnp_soap_add_port_mapping(
        service_type,
        external_port,
        internal_port,
        &local_ip,
        protocol,
        description,
        lease_duration,
    );

    let soap_action = format!("\"{}#AddPortMapping\"", service_type);
    let resp = client
        .post(&control_url)
        .header("SOAPAction", soap_action)
        .header("Content-Type", "text/xml; charset=\"utf-8\"")
        .body(soap_body)
        .send()
        .await
        .map_err(|e| NetError::Input(e.to_string()))?;

    if resp.status().is_success() {
        Ok(external_port)
    } else {
        Err(NetError::Input(format!(
            "UPnP SOAP error: status {}",
            resp.status()
        )))
    }
}

/// Formats an SSDP M-SEARCH discovery packet for UPnP IGD.
pub fn format_upnp_ssdp_search() -> &'static [u8] {
    b"M-SEARCH * HTTP/1.1\r\n\
Host: 239.255.255.250:1900\r\n\
Man: \"ssdp:discover\"\r\n\
MX: 2\r\n\
ST: urn:schemas-upnp-org:device:InternetGatewayDevice:1\r\n\
\r\n"
}

/// Formats a SOAP AddPortMapping request body for UPnP IGD.
pub fn format_upnp_soap_add_port_mapping(
    control_service: &str,
    external_port: u16,
    internal_port: u16,
    internal_ip: &str,
    protocol: PortMappingProtocol,
    description: &str,
    lease_duration: u32,
) -> String {
    format!(
        "<?xml version=\"1.0\"?>\r\n\
<s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\" s:encodingStyle=\"http://schemas.xmlsoap.org/soap/encoding/\">\r\n\
  <s:Body>\r\n\
    <u:AddPortMapping xmlns:u=\"{}\">\r\n\
      <NewRemoteHost></NewRemoteHost>\r\n\
      <NewExternalPort>{}</NewExternalPort>\r\n\
      <NewProtocol>{}</NewProtocol>\r\n\
      <NewInternalPort>{}</NewInternalPort>\r\n\
      <NewInternalClient>{}</NewInternalClient>\r\n\
      <NewEnabled>1</NewEnabled>\r\n\
      <NewPortMappingDescription>{}</NewPortMappingDescription>\r\n\
      <NewLeaseDuration>{}</NewLeaseDuration>\r\n\
    </u:AddPortMapping>\r\n\
  </s:Body>\r\n\
</s:Envelope>",
        control_service,
        external_port,
        protocol.as_str(),
        internal_port,
        internal_ip,
        description,
        lease_duration
    )
}

/// Formats a SOAP DeletePortMapping request body for UPnP IGD.
pub fn format_upnp_soap_delete_port_mapping(
    control_service: &str,
    external_port: u16,
    protocol: PortMappingProtocol,
) -> String {
    format!(
        "<?xml version=\"1.0\"?>\r\n\
<s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\" s:encodingStyle=\"http://schemas.xmlsoap.org/soap/encoding/\">\r\n\
  <s:Body>\r\n\
    <u:DeletePortMapping xmlns:u=\"{}\">\r\n\
      <NewRemoteHost></NewRemoteHost>\r\n\
      <NewExternalPort>{}</NewExternalPort>\r\n\
      <NewProtocol>{}</NewProtocol>\r\n\
    </u:DeletePortMapping>\r\n\
  </s:Body>\r\n\
</s:Envelope>",
        control_service,
        external_port,
        protocol.as_str()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nat_pmp_request_formatting() {
        let req = format_nat_pmp_request(PortMappingProtocol::Tcp, 51413, 51413, 3600);
        assert_eq!(req[0], 0); // version
        assert_eq!(req[1], 2); // TCP opcode
        assert_eq!(u16::from_be_bytes([req[4], req[5]]), 51413);
        assert_eq!(u16::from_be_bytes([req[6], req[7]]), 51413);
        assert_eq!(u32::from_be_bytes([req[8], req[9], req[10], req[11]]), 3600);
    }

    #[test]
    fn nat_pmp_response_parsing_success() {
        let mut resp = [0u8; 16];
        resp[0] = 0; // version 0
        resp[1] = 130; // TCP response opcode (2 + 128)
        resp[2] = 0; // result code 0 (success)
        resp[3] = 0;
        resp[4..8].copy_from_slice(&123456u32.to_be_bytes()); // epoch
        resp[8..10].copy_from_slice(&51413u16.to_be_bytes()); // internal port
        resp[10..12].copy_from_slice(&51413u16.to_be_bytes()); // external port
        resp[12..16].copy_from_slice(&3600u32.to_be_bytes()); // lifetime

        let mapped = parse_nat_pmp_response(&resp, PortMappingProtocol::Tcp).unwrap();
        assert_eq!(mapped, 51413);
    }

    #[test]
    fn nat_pmp_response_error_code() {
        let mut resp = [0u8; 16];
        resp[0] = 0;
        resp[1] = 130;
        resp[2] = 0;
        resp[3] = 2; // Not Authorized / Refused

        assert!(parse_nat_pmp_response(&resp, PortMappingProtocol::Tcp).is_err());
    }

    #[test]
    fn upnp_soap_add_port_mapping_contains_elements() {
        let xml = format_upnp_soap_add_port_mapping(
            "urn:schemas-upnp-org:service:WANIPConnection:1",
            6881,
            6881,
            "192.168.1.100",
            PortMappingProtocol::Tcp,
            "rstorrent",
            7200,
        );
        assert!(xml.contains("<NewExternalPort>6881</NewExternalPort>"));
        assert!(xml.contains("<NewProtocol>TCP</NewProtocol>"));
        assert!(xml.contains("<NewInternalClient>192.168.1.100</NewInternalClient>"));
        assert!(xml.contains("<NewPortMappingDescription>rstorrent</NewPortMappingDescription>"));
    }
}
