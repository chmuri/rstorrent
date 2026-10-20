use lt_common::HashString;
use lt_net::SocketAddress;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Target {
    None,
    Download(HashString),
    File(HashString, u32),
    FileItr(HashString, u32),
    Peer(HashString, SocketAddress),
    Tracker(HashString, u32),
    Pair(Box<Target>, Box<Target>),
}

impl Default for Target {
    fn default() -> Self {
        Target::None
    }
}

impl Target {
    pub fn none() -> Self {
        Target::None
    }

    pub fn is_none(&self) -> bool {
        matches!(self, Target::None)
    }

    pub fn is_pair(&self) -> bool {
        matches!(self, Target::Pair(_, _))
    }

    pub fn download(&self) -> Option<&HashString> {
        match self {
            Target::Download(h) => Some(h),
            Target::File(h, _) | Target::FileItr(h, _) => Some(h),
            Target::Peer(h, _) => Some(h),
            Target::Tracker(h, _) => Some(h),
            _ => None,
        }
    }

    pub fn file_index(&self) -> Option<u32> {
        match self {
            Target::File(_, idx) | Target::FileItr(_, idx) => Some(*idx),
            _ => None,
        }
    }

    pub fn peer_addr(&self) -> Option<&SocketAddress> {
        match self {
            Target::Peer(_, addr) => Some(addr),
            _ => None,
        }
    }

    pub fn tracker_index(&self) -> Option<u32> {
        match self {
            Target::Tracker(_, idx) => Some(*idx),
            _ => None,
        }
    }

    pub fn left(&self) -> Option<&Target> {
        match self {
            Target::Pair(left, _) => Some(left),
            _ => None,
        }
    }

    pub fn right(&self) -> Option<&Target> {
        match self {
            Target::Pair(_, right) => Some(right),
            _ => None,
        }
    }

    pub fn pair(left: Target, right: Target) -> Self {
        Target::Pair(Box::new(left), Box::new(right))
    }

    pub fn parse_target_str(s: &str) -> Result<Self, crate::error::RpcError> {
        if s.is_empty() {
            return Ok(Target::None);
        }
        if s.len() < 40 {
            return Err(crate::error::RpcError::Custom {
                code: -500,
                message: "invalid parameters: invalid target".to_string(),
            });
        }
        let hash_part = &s[..40];
        let hash =
            HashString::from_hex(hash_part).ok_or_else(|| crate::error::RpcError::Custom {
                code: -500,
                message: "invalid parameters: invalid target hash".to_string(),
            })?;

        if s.len() == 40 {
            return Ok(Target::Download(hash));
        }

        if s.as_bytes()[40] == b':' && s.len() >= 42 {
            let target_type = s.as_bytes()[41] as char;
            if target_type == 'd' {
                return Ok(Target::Download(hash));
            }
            if s.len() < 43 {
                return Err(crate::error::RpcError::Custom {
                    code: -500,
                    message: "invalid parameters: invalid index".to_string(),
                });
            }
            let index_str = &s[42..];
            let index = index_str
                .parse::<u32>()
                .map_err(|_| crate::error::RpcError::Custom {
                    code: -500,
                    message: "invalid parameters: invalid index".to_string(),
                })?;
            match target_type {
                'f' => Ok(Target::File(hash, index)),
                't' => Ok(Target::Tracker(hash, index)),
                _ => Err(crate::error::RpcError::Custom {
                    code: -500,
                    message: "invalid parameters: unexpected target type".to_string(),
                }),
            }
        } else {
            Ok(Target::Download(hash))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_target_str() {
        assert_eq!(Target::parse_target_str("").unwrap(), Target::None);

        let hash_hex = "1111222233334444555566667777888899990000";
        let hash = HashString::from_hex(hash_hex).unwrap();

        assert_eq!(
            Target::parse_target_str(hash_hex).unwrap(),
            Target::Download(hash)
        );
        assert_eq!(
            Target::parse_target_str(&format!("{hash_hex}:d")).unwrap(),
            Target::Download(hash)
        );
        assert_eq!(
            Target::parse_target_str(&format!("{hash_hex}:f0")).unwrap(),
            Target::File(hash, 0)
        );
        assert_eq!(
            Target::parse_target_str(&format!("{hash_hex}:f12")).unwrap(),
            Target::File(hash, 12)
        );
        assert_eq!(
            Target::parse_target_str(&format!("{hash_hex}:t3")).unwrap(),
            Target::Tracker(hash, 3)
        );

        assert!(Target::parse_target_str("short").is_err());
        assert!(Target::parse_target_str(&format!("{hash_hex}:f")).is_err());
        assert!(Target::parse_target_str(&format!("{hash_hex}:fx")).is_err());
        assert!(Target::parse_target_str(&format!("{hash_hex}:x0")).is_err());
    }
}
