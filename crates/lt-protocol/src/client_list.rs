use lt_common::hash_string::HashString;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClientType {
    Unknown,
    Azureus,
    Compact,
    Mainline,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClientInfo {
    pub name: String,
    pub version: String,
    pub client_type: ClientType,
}

impl Default for ClientInfo {
    fn default() -> Self {
        Self {
            name: "Unknown".to_string(),
            version: String::new(),
            client_type: ClientType::Unknown,
        }
    }
}

pub struct ClientList;

impl ClientList {
    pub fn identify(peer_id: &HashString) -> ClientInfo {
        let bytes = peer_id.as_bytes();

        // 1. Azureus-style: -XXYYYY-
        if bytes.len() >= 8
            && bytes[0] == b'-'
            && bytes[7] == b'-'
            && bytes[1].is_ascii_alphanumeric()
            && bytes[2].is_ascii_alphanumeric()
        {
            let code = &bytes[1..3];
            let name = match code {
                b"RS" | b"rs" => "rstorrent",
                b"lt" => "rTorrent",
                b"qB" => "qBittorrent",
                b"UT" => "uTorrent",
                b"TR" => "Transmission",
                b"DE" => "DelugeTorrent",
                b"AZ" => "Vuze",
                b"UM" => "uTorrent Mac",
                b"LT" => "libtorrent",
                b"BT" => "Mainline",
                b"A2" => "aria2",
                b"BC" => "BitComet",
                b"XL" | b"SD" => "Xunlei",
                b"FL" => "Flud",
                b"KT" => "KTorrent",
                b"BI" => "BiglyBT",
                _ => "Unknown Azureus-style",
            };

            let version = Self::parse_azureus_version(&bytes[3..7]);
            return ClientInfo {
                name: name.to_string(),
                version,
                client_type: ClientType::Azureus,
            };
        }

        // 2. Compact-style: XYYY-
        if bytes.len() >= 5
            && bytes[0].is_ascii_alphabetic()
            && bytes[4] == b'-'
            && bytes[1].is_ascii_hexdigit()
            && bytes[2].is_ascii_hexdigit()
            && bytes[3].is_ascii_hexdigit()
        {
            let name = match bytes[0] {
                b'A' => "ABC",
                b'O' => "Osprey Permaseed",
                _ => "Unknown Compact-style",
            };

            let version = format!(
                "{}.{}.{}",
                Self::hex_val(bytes[1]),
                Self::hex_val(bytes[2]),
                Self::hex_val(bytes[3])
            );

            return ClientInfo {
                name: name.to_string(),
                version,
                client_type: ClientType::Compact,
            };
        }

        // 3. Mainline-style: MX-Y-Z--
        if bytes.len() >= 8 && bytes[0] == b'M' && bytes[1].is_ascii_digit() && bytes[2] == b'-' {
            let v1 = bytes[1] - b'0';
            let version = if bytes[4] == b'-' && bytes[5].is_ascii_digit() && bytes[6] == b'-' {
                let v2 = bytes[3] - b'0';
                let v3 = bytes[5] - b'0';
                format!("{}.{}.{}", v1, v2, v3)
            } else {
                format!("{}.{}", v1, bytes[3] - b'0')
            };

            return ClientInfo {
                name: "Mainline".to_string(),
                version,
                client_type: ClientType::Mainline,
            };
        }

        ClientInfo::default()
    }

    fn hex_val(b: u8) -> u8 {
        match b {
            b'0'..=b'9' => b - b'0',
            b'a'..=b'f' => b - b'a' + 10,
            b'A'..=b'F' => b - b'A' + 10,
            _ => 0,
        }
    }

    fn parse_azureus_version(ver_bytes: &[u8]) -> String {
        let digits: Vec<u8> = ver_bytes.iter().map(|&b| Self::hex_val(b)).collect();
        if digits.len() == 4 {
            format!("{}.{}.{}.{}", digits[0], digits[1], digits[2], digits[3])
        } else {
            String::new()
        }
    }
}
