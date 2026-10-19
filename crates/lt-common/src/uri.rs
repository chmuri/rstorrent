#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UriStateCode {
    Empty,
    Valid,
    Invalid,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UriState {
    pub state: UriStateCode,
    pub uri: String,
    pub scheme: String,
    pub resource: String,
    pub query: String,
    pub fragment: String,
}

impl Default for UriState {
    fn default() -> Self {
        Self {
            state: UriStateCode::Empty,
            uri: String::new(),
            scheme: String::new(),
            resource: String::new(),
            query: String::new(),
            fragment: String::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UriQueryState {
    pub state: UriStateCode,
    pub query: String,
    pub elements: Vec<String>,
}

impl Default for UriQueryState {
    fn default() -> Self {
        Self {
            state: UriStateCode::Empty,
            query: String::new(),
            elements: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum UriError {
    #[error("URI error: {0}")]
    ParseError(String),
}

fn is_unreserved_uri_char(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'-' || c == b'_' || c == b'.' || c == b'~'
}

fn is_valid_uri_query_char(c: u8) -> bool {
    is_unreserved_uri_char(c) || c == b':' || c == b'&' || c == b'=' || c == b'/' || c == b'%'
}

fn is_unreserved_uri_query_char(c: u8) -> bool {
    is_unreserved_uri_char(c) || c == b':' || c == b'=' || c == b'/' || c == b'%'
}

pub fn uri_parse_str(uri: &str, state: &mut UriState) -> Result<(), UriError> {
    if state.state != UriStateCode::Empty {
        return Err(UriError::ParseError(
            "uri_state.state is not uri_state::state_empty".into(),
        ));
    }

    state.uri = uri.to_string();
    state.state = UriStateCode::Invalid;

    let bytes = uri.as_bytes();
    let mut pos = 0;

    // Parse scheme:
    let scheme_start = pos;
    while pos < bytes.len() && is_unreserved_uri_char(bytes[pos]) {
        pos += 1;
    }
    state.scheme = uri[scheme_start..pos].to_string();

    if pos == bytes.len() {
        state.state = UriStateCode::Valid;
        return Ok(());
    }

    if bytes[pos] != b':' {
        return Err(UriError::ParseError(format!(
            "could not find ':' after scheme, found character 0x{:02X}",
            bytes[pos]
        )));
    }
    pos += 1;

    // Parse resource:
    let resource_start = pos;
    while pos < bytes.len() && is_unreserved_uri_char(bytes[pos]) {
        pos += 1;
    }
    state.resource = uri[resource_start..pos].to_string();

    if pos == bytes.len() {
        state.state = UriStateCode::Valid;
        return Ok(());
    }

    if bytes[pos] != b'?' {
        return Err(UriError::ParseError(format!(
            "could not find '?' after resource, found character 0x{:02X}",
            bytes[pos]
        )));
    }
    pos += 1;

    // Parse query:
    let query_start = pos;
    while pos < bytes.len() && is_valid_uri_query_char(bytes[pos]) {
        pos += 1;
    }
    state.query = uri[query_start..pos].to_string();

    if pos == bytes.len() {
        state.state = UriStateCode::Valid;
        return Ok(());
    }

    if bytes[pos] != b'#' {
        return Err(UriError::ParseError(format!(
            "could not find '#' after query, found character 0x{:02X}",
            bytes[pos]
        )));
    }
    pos += 1;

    state.fragment = uri[pos..].to_string();
    state.state = UriStateCode::Valid;
    Ok(())
}

pub fn uri_parse_query_str(query: &str, state: &mut UriQueryState) -> Result<(), UriError> {
    if state.state != UriStateCode::Empty {
        return Err(UriError::ParseError(
            "uri_query_state.state is not uri_query_state::state_empty".into(),
        ));
    }

    state.query = query.to_string();
    state.state = UriStateCode::Invalid;

    let bytes = query.as_bytes();
    let mut pos = 0;

    while pos < bytes.len() {
        let elem_start = pos;
        while pos < bytes.len() && is_unreserved_uri_query_char(bytes[pos]) {
            pos += 1;
        }

        let elem = &query[elem_start..pos];
        if pos < bytes.len() {
            if bytes[pos] != b'&' {
                return Err(UriError::ParseError(format!(
                    "query element contains invalid character 0x{:02X}",
                    bytes[pos]
                )));
            }
            pos += 1;
        }

        state.elements.push(elem.to_string());
    }

    state.state = UriStateCode::Valid;
    Ok(())
}

pub fn uri_generate_scrape_url(uri: &str) -> Result<String, UriError> {
    let delim_slash = uri
        .rfind('/')
        .ok_or_else(|| UriError::ParseError("Tried to make scrape url from invalid uri.".into()))?;

    if !uri[delim_slash..].starts_with("/announce") {
        return Err(UriError::ParseError(
            "Tried to make scrape url from invalid uri.".into(),
        ));
    }

    let mut res = uri.to_string();
    res.replace_range(delim_slash..delim_slash + "/announce".len(), "/scrape");
    Ok(res)
}

pub fn uri_can_scrape(uri: &str) -> bool {
    if let Some(delim_slash) = uri.rfind('/') {
        uri[delim_slash..].starts_with("/announce")
    } else {
        false
    }
}

pub fn uri_has_query(uri: &str) -> bool {
    if let Some(delim_options) = uri.rfind('?') {
        !uri[delim_options..].contains('/')
    } else {
        false
    }
}

pub fn uri_escape_html(input: &[u8]) -> String {
    let mut result = String::with_capacity(input.len() * 3);
    for &b in input {
        if b.is_ascii_alphanumeric() || b == b'-' {
            result.push(b as char);
        } else {
            result.push('%');
            result.push_str(&format!("{:02X}", b));
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAGNET_BASIC: &str = "magnet:?xt=urn:sha1:YNCKHTQCWBTRNJIV4WNAE52SJUQCZO5C";
    const QUERY_MAGNET_QUERY: &str = "xt=urn:ed2k:31D6CFE0D16AE931B73C59D7E0C089C0\
        &xl=0&dn=zero_len.fil\
        &xt=urn:bitprint:3I42H3S6NNFQ2MSVX7XZKYAYSCX5QBYJ.LWPNACQDBZRYXW3VHJVCJ64QBZNGHOHHHZWCLNQ\
        &xt=urn:md5:D41D8CD98F00B204E9800998ECF8427E";

    #[test]
    fn test_basic() {
        let state = UriState::default();
        assert_eq!(state.state, UriStateCode::Empty);
        assert_ne!(state.state, UriStateCode::Valid);
        assert_ne!(state.state, UriStateCode::Invalid);
    }

    #[test]
    fn test_basic_magnet() {
        let mut state = UriState::default();
        uri_parse_str(MAGNET_BASIC, &mut state).unwrap();

        assert_eq!(state.state, UriStateCode::Valid);
        assert_eq!(state.uri, MAGNET_BASIC);
        assert_eq!(state.scheme, "magnet");
        assert_eq!(state.resource, "");
        assert_eq!(state.query, "xt=urn:sha1:YNCKHTQCWBTRNJIV4WNAE52SJUQCZO5C");
        assert_eq!(state.fragment, "");
    }

    #[test]
    fn test_query_magnet() {
        let query_magnet = format!("magnet:?{}", QUERY_MAGNET_QUERY);
        let mut state = UriState::default();
        let mut query_state = UriQueryState::default();

        uri_parse_str(&query_magnet, &mut state).unwrap();

        assert_eq!(state.state, UriStateCode::Valid);
        assert_eq!(state.uri, query_magnet);
        assert_eq!(state.scheme, "magnet");
        assert_eq!(state.resource, "");
        assert_eq!(state.query, QUERY_MAGNET_QUERY);
        assert_eq!(state.fragment, "");

        uri_parse_query_str(&state.query, &mut query_state).unwrap();

        assert_eq!(query_state.state, UriStateCode::Valid);
        assert_eq!(query_state.elements.len(), 5);
        assert_eq!(
            query_state.elements[0],
            "xt=urn:ed2k:31D6CFE0D16AE931B73C59D7E0C089C0"
        );
        assert_eq!(query_state.elements[1], "xl=0");
        assert_eq!(query_state.elements[2], "dn=zero_len.fil");
        assert_eq!(
            query_state.elements[3],
            "xt=urn:bitprint:3I42H3S6NNFQ2MSVX7XZKYAYSCX5QBYJ.LWPNACQDBZRYXW3VHJVCJ64QBZNGHOHHHZWCLNQ"
        );
        assert_eq!(
            query_state.elements[4],
            "xt=urn:md5:D41D8CD98F00B204E9800998ECF8427E"
        );
    }

    #[test]
    fn test_scrape_urls() {
        assert!(uri_can_scrape("http://tracker.example.com/announce"));
        assert!(!uri_can_scrape("http://tracker.example.com/other"));
        assert_eq!(
            uri_generate_scrape_url("http://tracker.example.com/announce").unwrap(),
            "http://tracker.example.com/scrape"
        );
    }
}
