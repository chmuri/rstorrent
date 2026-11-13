//! Transport authentication helpers shared by the gRPC and SCGI servers.
//!
//! Token comparison is constant-time so a remote caller cannot recover the
//! shared secret byte-by-byte from response timing.

use subtle::ConstantTimeEq;

/// Constant-time equality of two byte strings.
pub fn tokens_equal(a: &[u8], b: &[u8]) -> bool {
    a.ct_eq(b).into()
}

/// Validate an `Authorization` header against an expected bearer token.
///
/// Returns `true` when no token is configured (authentication disabled). When a
/// token is configured the header must be `Bearer <token>` and match exactly.
pub fn bearer_authorized(header: Option<&str>, expected: Option<&str>) -> bool {
    let expected = match expected {
        Some(token) if !token.is_empty() => token,
        _ => return true,
    };
    let presented = header.and_then(|value| {
        value
            .strip_prefix("Bearer ")
            .or_else(|| value.strip_prefix("bearer "))
    });
    match presented {
        Some(token) => tokens_equal(token.as_bytes(), expected.as_bytes()),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_when_no_token() {
        assert!(bearer_authorized(None, None));
        assert!(bearer_authorized(None, Some("")));
        assert!(bearer_authorized(Some("Bearer anything"), None));
    }

    #[test]
    fn matches_bearer_token() {
        assert!(bearer_authorized(Some("Bearer s3cret"), Some("s3cret")));
        assert!(bearer_authorized(Some("bearer s3cret"), Some("s3cret")));
    }

    #[test]
    fn rejects_wrong_or_missing_token() {
        assert!(!bearer_authorized(None, Some("s3cret")));
        assert!(!bearer_authorized(Some("Bearer wrong"), Some("s3cret")));
        assert!(!bearer_authorized(Some("s3cret"), Some("s3cret")));
        assert!(!bearer_authorized(Some("Bearer s3cret "), Some("s3cret")));
    }

    #[test]
    fn constant_time_equal() {
        assert!(tokens_equal(b"abc", b"abc"));
        assert!(!tokens_equal(b"abc", b"abd"));
        assert!(!tokens_equal(b"abc", b"ab"));
        assert!(!tokens_equal(b"", b"a"));
    }
}
