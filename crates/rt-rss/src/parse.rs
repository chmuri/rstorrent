//! Feed XML parsing: RSS 2.0, Atom and RSS 1.0 (RDF) documents are turned
//! into [`ParsedFeed`] values using the quick-xml events API. Also provides
//! magnet info-hash normalization and best-effort RFC 2822/822 + ISO 8601
//! date parsing. Parsing never panics: malformed input yields an error or a
//! partial result, and unparseable field values are simply left out.

use quick_xml::encoding::Decoder;
use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;

use crate::error::ParseError;

/// Hard cap on the number of items accepted from a single feed document.
const MAX_ITEMS: usize = 1000;

/// A parsed feed document: the channel/feed title (if present) and its items.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedFeed {
    pub title: Option<String>,
    pub items: Vec<ParsedItem>,
}

/// A single feed item. Missing XML fields stay `None`; items without a title
/// are dropped by [`parse_feed`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedItem {
    pub guid: Option<String>,
    pub title: String,
    pub link: Option<String>,
    pub torrent_url: Option<String>,
    pub magnet: Option<String>,
    pub size_bytes: Option<u64>,
    pub published_at: Option<u64>,
}

/// Parses an RSS 2.0, Atom or RSS 1.0 (RDF) document. The format is detected
/// from the root element (`rss`, `feed`, `rdf:RDF`); anything else is an
/// error. More than [`MAX_ITEMS`] item elements yields
/// [`ParseError::TooManyItems`].
pub fn parse_feed(bytes: &[u8]) -> Result<ParsedFeed, ParseError> {
    let mut reader = Reader::from_reader(bytes);
    {
        let config = reader.config_mut();
        config.trim_text(true);
        config.expand_empty_elements = true;
    }
    let format = detect_format(&mut reader)?;
    let mut feed = ParsedFeed {
        title: None,
        items: Vec::new(),
    };
    let mut seen = 0usize;
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                let name = e.local_name();
                match name.as_ref() {
                    b"channel" if format != Format::Atom => {
                        parse_channel(&mut reader, format, &mut feed, &mut seen)?;
                    }
                    b"item" | b"entry" => {
                        parse_item(&mut reader, format, &e, &mut feed, &mut seen)?;
                    }
                    b"title" if format == Format::Atom && feed.title.is_none() => {
                        feed.title = non_empty(consume_text(&mut reader)?);
                    }
                    _ => {
                        consume_text(&mut reader)?;
                    }
                }
            }
            Ok(Event::End(_) | Event::Eof) => break,
            Err(e) => return Err(ParseError::Xml(e.to_string())),
            _ => {}
        }
    }
    Ok(feed)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Format {
    Rss2,
    Atom,
    Rss1,
}

/// Skips prologue events (declaration, comments, doctype, whitespace) and
/// identifies the format from the first element.
fn detect_format(reader: &mut Reader<&[u8]>) -> Result<Format, ParseError> {
    loop {
        match reader.read_event() {
            Ok(Event::Start(e) | Event::Empty(e)) => {
                return match e.local_name().as_ref() {
                    b"rss" => Ok(Format::Rss2),
                    b"feed" => Ok(Format::Atom),
                    b"RDF" => Ok(Format::Rss1),
                    _ => Err(ParseError::UnsupportedFormat),
                };
            }
            Ok(Event::Eof) => return Err(ParseError::UnsupportedFormat),
            Err(e) => return Err(ParseError::Xml(e.to_string())),
            _ => {}
        }
    }
}

/// Reads a `channel` element (RSS 2.0 / RSS 1.0): feed title plus any items
/// found inside. RSS 1.0 items normally sit next to the channel, but feeds
/// that nest them are tolerated.
fn parse_channel(
    reader: &mut Reader<&[u8]>,
    format: Format,
    feed: &mut ParsedFeed,
    seen: &mut usize,
) -> Result<(), ParseError> {
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => match e.local_name().as_ref() {
                b"title" if feed.title.is_none() => {
                    feed.title = non_empty(consume_text(reader)?);
                }
                b"item" | b"entry" => parse_item(reader, format, &e, feed, seen)?,
                _ => {
                    consume_text(reader)?;
                }
            },
            Ok(Event::End(_) | Event::Eof) => break,
            Err(e) => return Err(ParseError::Xml(e.to_string())),
            _ => {}
        }
    }
    Ok(())
}

/// Reads one `item`/`entry` element through its end tag and appends the
/// finished item unless it has no title.
fn parse_item(
    reader: &mut Reader<&[u8]>,
    format: Format,
    start: &BytesStart<'_>,
    feed: &mut ParsedFeed,
    seen: &mut usize,
) -> Result<(), ParseError> {
    if *seen >= MAX_ITEMS {
        return Err(ParseError::TooManyItems);
    }
    *seen += 1;

    let mut item = ItemBuilder::default();
    if format == Format::Rss1 {
        // RSS 1.0 identifies items by the rdf:about attribute.
        item.guid = attr(start, reader.decoder(), b"about");
    }
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => process_child(reader, format, &e, &mut item, true)?,
            Ok(Event::Empty(e)) => process_child(reader, format, &e, &mut item, false)?,
            Ok(Event::End(_) | Event::Eof) => break,
            Err(e) => return Err(ParseError::Xml(e.to_string())),
            _ => {}
        }
    }
    if let Some(parsed) = item.finish() {
        feed.items.push(parsed);
    }
    Ok(())
}

/// Handles one direct child of an item. The body is always consumed so the
/// item loop only ever sees direct children (nested `<title>` inside e.g.
/// `<source>` cannot leak into the item).
fn process_child(
    reader: &mut Reader<&[u8]>,
    format: Format,
    e: &BytesStart<'_>,
    item: &mut ItemBuilder,
    has_body: bool,
) -> Result<(), ParseError> {
    let text = if has_body {
        consume_text(reader)?
    } else {
        String::new()
    };
    let decoder = reader.decoder();
    match e.local_name().as_ref() {
        b"title" => {
            if item.title.is_none() {
                item.title = non_empty(text);
            }
        }
        b"guid" if format != Format::Atom => {
            if item.guid.is_none() {
                item.guid = non_empty(text);
            }
        }
        b"id" if format == Format::Atom => {
            if item.guid.is_none() {
                item.guid = non_empty(text);
            }
        }
        b"link" if format == Format::Atom => apply_atom_link(e, decoder, item),
        b"link" => {
            if item.link.is_none() {
                item.link = non_empty(text.clone());
            }
            apply_url(&text, item);
        }
        b"enclosure" => apply_enclosure(e, decoder, item),
        b"pubDate" | b"date" => set_date(item, &text),
        b"published" | b"updated" if format == Format::Atom => set_date(item, &text),
        _ => {}
    }
    Ok(())
}

#[derive(Debug, Default)]
struct ItemBuilder {
    guid: Option<String>,
    title: Option<String>,
    link: Option<String>,
    torrent_url: Option<String>,
    magnet: Option<String>,
    size_bytes: Option<u64>,
    published_at: Option<u64>,
}

impl ItemBuilder {
    fn finish(self) -> Option<ParsedItem> {
        let title = self.title?;
        let guid = self
            .guid
            .or_else(|| self.link.clone())
            .or_else(|| self.torrent_url.clone());
        Some(ParsedItem {
            guid,
            title,
            link: self.link,
            torrent_url: self.torrent_url,
            magnet: self.magnet,
            size_bytes: self.size_bytes,
            published_at: self.published_at,
        })
    }
}

fn set_date(item: &mut ItemBuilder, text: &str) {
    if item.published_at.is_none() {
        item.published_at = parse_date(text);
    }
}

/// Classifies a plain RSS link: magnet URIs and `.torrent` URLs are also
/// recorded as download sources.
fn apply_url(url: &str, item: &mut ItemBuilder) {
    if is_magnet(url) {
        if item.magnet.is_none() {
            item.magnet = Some(url.to_string());
        }
    } else if ends_with_torrent(url) && item.torrent_url.is_none() {
        item.torrent_url = Some(url.to_string());
    }
}

/// RSS/RDF `enclosure` element: torrent by MIME type or `.torrent` suffix,
/// magnet by URI scheme; `length` feeds `size_bytes` for torrent enclosures.
fn apply_enclosure(e: &BytesStart<'_>, decoder: Decoder, item: &mut ItemBuilder) {
    let Some(url) = attr(e, decoder, b"url").or_else(|| attr(e, decoder, b"href")) else {
        return;
    };
    if is_magnet(&url) {
        if item.magnet.is_none() {
            item.magnet = Some(url);
        }
        return;
    }
    let torrent_type = attr(e, decoder, b"type")
        .is_some_and(|t| t.trim().eq_ignore_ascii_case("application/x-bittorrent"));
    if torrent_type || ends_with_torrent(&url) {
        if item.torrent_url.is_none() {
            item.torrent_url = Some(url);
        }
        if item.size_bytes.is_none() {
            item.size_bytes = attr(e, decoder, b"length").and_then(|l| l.trim().parse().ok());
        }
    }
}

/// Atom `link` element: `rel="enclosure"` with the BitTorrent MIME type (or a
/// `.torrent`/magnet href) is a download source; `rel="alternate"` (the
/// default) is the item link.
fn apply_atom_link(e: &BytesStart<'_>, decoder: Decoder, item: &mut ItemBuilder) {
    let Some(href) = attr(e, decoder, b"href") else {
        return;
    };
    let rel = attr(e, decoder, b"rel").unwrap_or_default();
    if is_magnet(&href) {
        if item.magnet.is_none() {
            item.magnet = Some(href.clone());
        }
    } else {
        let torrent_type = attr(e, decoder, b"type")
            .is_some_and(|t| t.trim().eq_ignore_ascii_case("application/x-bittorrent"));
        let is_enclosure = rel.eq_ignore_ascii_case("enclosure") && torrent_type;
        if is_enclosure || ends_with_torrent(&href) {
            if item.torrent_url.is_none() {
                item.torrent_url = Some(href.clone());
            }
            if item.size_bytes.is_none() {
                item.size_bytes = attr(e, decoder, b"length").and_then(|l| l.trim().parse().ok());
            }
        }
    }
    if (rel.is_empty() || rel.eq_ignore_ascii_case("alternate")) && item.link.is_none() {
        item.link = Some(href);
    }
}

/// Looks up an attribute by local name, decoding and unescaping its value;
/// falls back to lossy raw bytes so a bad value never fails the feed.
fn attr(e: &BytesStart<'_>, decoder: Decoder, local_key: &[u8]) -> Option<String> {
    for a in e.attributes().flatten() {
        if a.key.local_name().as_ref() == local_key {
            return Some(
                a.decode_and_unescape_value(decoder)
                    .map(|v| v.into_owned())
                    .unwrap_or_else(|_| String::from_utf8_lossy(&a.value).into_owned()),
            );
        }
    }
    None
}

/// Consumes events through the end of the element whose start tag was just
/// read, accumulating text (plain, CDATA and text inside nested markup).
fn consume_text(reader: &mut Reader<&[u8]>) -> Result<String, ParseError> {
    let mut depth: usize = 1;
    let mut out = String::new();
    loop {
        match reader.read_event() {
            Ok(Event::Start(_)) => depth += 1,
            Ok(Event::End(_)) => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    break;
                }
            }
            Ok(Event::Text(e)) => {
                // Unknown entities (e.g. &nbsp;) must not fail the whole feed.
                match e.unescape() {
                    Ok(text) => out.push_str(&text),
                    Err(_) => out.push_str(&String::from_utf8_lossy(&e)),
                }
            }
            Ok(Event::CData(e)) => match e.decode() {
                Ok(text) => out.push_str(&text),
                Err(_) => out.push_str(&String::from_utf8_lossy(&e)),
            },
            Ok(Event::Eof) => break,
            Err(e) => return Err(ParseError::Xml(e.to_string())),
            _ => {}
        }
    }
    Ok(out.trim().to_string())
}

fn non_empty(text: String) -> Option<String> {
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

fn is_magnet(url: &str) -> bool {
    url.get(..8)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("magnet:?"))
}

/// True when the URL path (before any query string or fragment) ends in
/// `.torrent`, case-insensitively.
fn ends_with_torrent(url: &str) -> bool {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    path.get(path.len().saturating_sub(8)..)
        .is_some_and(|suffix| suffix.eq_ignore_ascii_case(".torrent"))
}

/// Extracts the BitTorrent info hash from a magnet URI's `xt=urn:btih:`
/// parameter and normalizes it to 40 lowercase hex characters. Accepts both
/// 40-char hex and 32-char RFC 4648 base32 (no padding) encodings; returns
/// `None` when no valid btih hash is present.
pub fn normalize_info_hash(magnet_uri: &str) -> Option<String> {
    let mut query = magnet_uri;
    if let Some(prefix) = query.get(..8) {
        if prefix.eq_ignore_ascii_case("magnet:?") {
            query = query.get(8..)?;
        }
    }
    for param in query.split('&') {
        let Some((key, value)) = param.split_once('=') else {
            continue;
        };
        if !key.eq_ignore_ascii_case("xt") {
            continue;
        }
        let Some(prefix) = value.get(..9) else {
            continue;
        };
        if !prefix.eq_ignore_ascii_case("urn:btih:") {
            continue;
        }
        let Some(hash) = value.get(9..) else {
            continue;
        };
        if let Some(normalized) = normalize_hash(hash) {
            return Some(normalized);
        }
    }
    None
}

fn normalize_hash(hash: &str) -> Option<String> {
    if hash.len() == 40 && hash.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Some(hash.to_ascii_lowercase());
    }
    if hash.len() == 32 {
        let bytes = base32_decode(hash)?;
        if bytes.len() != 20 {
            return None;
        }
        return Some(bytes.iter().map(|b| format!("{b:02x}")).collect());
    }
    None
}

/// Manual RFC 4648 base32 decoder (alphabet A-Z2-7, case-insensitive, no
/// padding). Returns `None` on any invalid character.
fn base32_decode(input: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(input.len() * 5 / 8);
    let mut acc: u32 = 0;
    let mut bits: u32 = 0;
    for byte in input.bytes() {
        let value: u32 = match byte {
            b'A'..=b'Z' => u32::from(byte - b'A'),
            b'a'..=b'z' => u32::from(byte - b'a'),
            b'2'..=b'7' => u32::from(byte - b'2') + 26,
            _ => return None,
        };
        acc = (acc << 5) | value;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Some(out)
}

/// Best-effort date parsing: RFC 2822/822 (English month names, numeric and
/// named zones) and ISO 8601 (`T`/space separator, `Z` or numeric offset,
/// optional fractional seconds, date-only). Returns Unix seconds, or `None`
/// for anything unparseable or before the epoch. Never panics.
pub fn parse_date(s: &str) -> Option<u64> {
    let s = s.trim();
    let iso = s.len() >= 10
        && s.as_bytes()[4] == b'-'
        && s.as_bytes()[..4].iter().all(u8::is_ascii_digit);
    if iso {
        parse_iso8601(s)
    } else {
        parse_rfc2822(s)
    }
}

fn parse_rfc2822(s: &str) -> Option<u64> {
    let mut tokens = s.split_whitespace();
    let mut tok = tokens.next()?;
    if let Some(pos) = tok.find(',') {
        // Skip the optional weekday prefix ("Wed," or "Wed,15").
        let after = tok.get(pos + 1..)?;
        tok = if after.is_empty() {
            tokens.next()?
        } else {
            after
        };
    }
    let day: u32 = tok.parse().ok()?;
    let month = month_number(tokens.next()?)?;
    let year = parse_year(tokens.next()?)?;
    let (hour, minute, second) = parse_clock(tokens.next()?)?;
    let offset = match tokens.next() {
        Some(zone) => zone_offset(zone)?,
        None => 0,
    };
    timestamp(year, month, day, hour, minute, second, offset)
}

fn parse_iso8601(s: &str) -> Option<u64> {
    let year_str = s.get(..4)?;
    if !year_str.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let year: i64 = year_str.parse().ok()?;
    if s.as_bytes().get(7) != Some(&b'-') {
        return None;
    }
    let month: u32 = s.get(5..7)?.parse().ok()?;
    let day: u32 = s.get(8..10)?.parse().ok()?;
    let rest = s.get(10..)?;
    if rest.is_empty() {
        return timestamp(year, month, day, 0, 0, 0, 0);
    }
    if !matches!(rest.as_bytes().first(), Some(b'T' | b't' | b' ')) {
        return None;
    }
    let rest = rest.get(1..)?;
    let (clock, offset) = split_iso_zone(rest)?;
    let (hour, minute, second) = parse_clock(clock)?;
    timestamp(year, month, day, hour, minute, second, offset)
}

/// Splits an ISO time part into clock and zone offset. A missing zone is
/// treated as UTC.
fn split_iso_zone(rest: &str) -> Option<(&str, i64)> {
    if let Some(pos) = rest.find(['Z', 'z']) {
        return Some((rest.get(..pos)?, 0));
    }
    if let Some(pos) = rest.find(['+', '-']) {
        return Some((rest.get(..pos)?, signed_offset(rest.get(pos..)?)?));
    }
    Some((rest, 0))
}

fn parse_year(tok: &str) -> Option<i64> {
    if !tok.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    match tok.len() {
        4 => tok.parse().ok(),
        // RFC 2822 two-digit year window.
        2 => {
            let y: i64 = tok.parse().ok()?;
            Some(if y <= 49 { 2000 + y } else { 1900 + y })
        }
        _ => None,
    }
}

fn month_number(name: &str) -> Option<u32> {
    let month = match name.to_ascii_lowercase().as_str() {
        "jan" => 1,
        "feb" => 2,
        "mar" => 3,
        "apr" => 4,
        "may" => 5,
        "jun" => 6,
        "jul" => 7,
        "aug" => 8,
        "sep" => 9,
        "oct" => 10,
        "nov" => 11,
        "dec" => 12,
        _ => return None,
    };
    Some(month)
}

/// Parses `HH:MM[:SS[.frac]]`; a leap second (60) is clamped to 59.
fn parse_clock(clock: &str) -> Option<(u32, u32, u32)> {
    let clock = clock.split(['.', ',']).next()?;
    let mut parts = clock.split(':');
    let hour: u32 = parts.next()?.parse().ok()?;
    let minute: u32 = parts.next()?.parse().ok()?;
    let second: u32 = match parts.next() {
        Some(s) => s.parse().ok()?,
        None => 0,
    };
    if parts.next().is_some() || hour > 23 || minute > 59 || second > 60 {
        return None;
    }
    Some((hour, minute, second.min(59)))
}

fn zone_offset(zone: &str) -> Option<i64> {
    match zone.to_ascii_uppercase().as_str() {
        "GMT" | "UT" | "UTC" | "Z" => Some(0),
        "EST" => Some(-5 * 3600),
        "EDT" => Some(-4 * 3600),
        "CST" => Some(-6 * 3600),
        "CDT" => Some(-5 * 3600),
        "MST" => Some(-7 * 3600),
        "MDT" => Some(-6 * 3600),
        "PST" => Some(-8 * 3600),
        "PDT" => Some(-7 * 3600),
        _ => signed_offset(zone),
    }
}

/// Numeric zone offset: `±HHMM`, `±HH:MM` or `±HH`, in seconds.
fn signed_offset(zone: &str) -> Option<i64> {
    let (sign, rest) = match zone.as_bytes().first()? {
        b'+' => (1i64, zone.get(1..)?),
        b'-' => (-1i64, zone.get(1..)?),
        _ => return None,
    };
    let digits: String = rest.chars().filter(|c| *c != ':').collect();
    if (digits.len() != 2 && digits.len() != 4) || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let hours: i64 = digits.get(..2)?.parse().ok()?;
    let minutes: i64 = if digits.len() == 4 {
        digits.get(2..)?.parse().ok()?
    } else {
        0
    };
    if hours > 23 || minutes > 59 {
        return None;
    }
    Some(sign * (hours * 3600 + minutes * 60))
}

fn timestamp(
    year: i64,
    month: u32,
    day: u32,
    hour: u32,
    minute: u32,
    second: u32,
    offset_secs: i64,
) -> Option<u64> {
    if month == 0 || month > 12 || day == 0 || day > days_in_month(year, month) {
        return None;
    }
    let days = days_from_civil(year, i64::from(month), i64::from(day));
    let secs = days * 86_400 + i64::from(hour) * 3600 + i64::from(minute) * 60 + i64::from(second)
        - offset_secs;
    u64::try_from(secs).ok()
}

fn days_in_month(year: i64, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if is_leap_year(year) {
                29
            } else {
                28
            }
        }
        _ => 0,
    }
}

fn is_leap_year(year: i64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

/// Days since 1970-01-01 from a civil date (Howard Hinnant's algorithm).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = (if y >= 0 { y } else { y - 399 }) / 400;
    let yoe = y - era * 400;
    let mp = if m > 2 { m - 3 } else { m + 9 };
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEX40: &str = "0123456789abcdef0123456789abcdef01234567";
    // base32 of twenty 0x61 bytes ("MFQWCYLB" is the encoding of 5x 0x61).
    const BASE32_A: &str = "MFQWCYLBMFQWCYLBMFQWCYLBMFQWCYLB";
    const HEX_A: &str = "6161616161616161616161616161616161616161";
    // 2023-11-15 06:00:00 UTC.
    const TS: u64 = 1_700_028_000;

    fn rss2(items: &str) -> String {
        format!("<rss version=\"2.0\"><channel><title>Feed</title>{items}</channel></rss>")
    }

    #[test]
    fn rss2_enclosure_torrent_and_magnet() {
        let doc = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
            <rss version="2.0"><channel>
              <title>Example Torrents</title>
              <link>https://example.com</link>
              <item>
                <title>Show S01E01 1080p</title>
                <link>https://example.com/items/1</link>
                <guid>item-1</guid>
                <pubDate>Wed, 15 Nov 2023 06:00:00 GMT</pubDate>
                <enclosure url="https://example.com/t/1.torrent" length="1500000000" type="application/x-bittorrent"/>
              </item>
              <item>
                <title>Show S01E02 1080p</title>
                <guid>item-2</guid>
                <enclosure url="magnet:?xt=urn:btih:{HEX40}&amp;dn=show" type="application/x-bittorrent"/>
              </item>
            </channel></rss>"#
        );
        let feed = parse_feed(doc.as_bytes()).unwrap();
        assert_eq!(feed.title.as_deref(), Some("Example Torrents"));
        assert_eq!(feed.items.len(), 2);
        let first = &feed.items[0];
        assert_eq!(first.guid.as_deref(), Some("item-1"));
        assert_eq!(first.title, "Show S01E01 1080p");
        assert_eq!(first.link.as_deref(), Some("https://example.com/items/1"));
        assert_eq!(
            first.torrent_url.as_deref(),
            Some("https://example.com/t/1.torrent")
        );
        assert_eq!(first.magnet, None);
        assert_eq!(first.size_bytes, Some(1_500_000_000));
        assert_eq!(first.published_at, Some(TS));
        let second = &feed.items[1];
        assert_eq!(
            second.magnet.as_deref(),
            Some(format!("magnet:?xt=urn:btih:{HEX40}&dn=show").as_str())
        );
        assert_eq!(second.torrent_url, None);
        assert_eq!(second.size_bytes, None);
    }

    #[test]
    fn atom_enclosure_link_and_id() {
        let doc = r#"<?xml version="1.0" encoding="utf-8"?>
            <feed xmlns="http://www.w3.org/2005/Atom">
              <title>Atom Feed</title>
              <entry>
                <id>urn:uuid:1234</id>
                <title>Atom Item</title>
                <link rel="alternate" href="https://example.com/items/1"/>
                <link rel="enclosure" type="application/x-bittorrent"
                      href="https://example.com/t/1.torrent" length="42"/>
                <published>2023-11-15T06:00:00Z</published>
              </entry>
            </feed>"#;
        let feed = parse_feed(doc.as_bytes()).unwrap();
        assert_eq!(feed.title.as_deref(), Some("Atom Feed"));
        assert_eq!(feed.items.len(), 1);
        let item = &feed.items[0];
        assert_eq!(item.guid.as_deref(), Some("urn:uuid:1234"));
        assert_eq!(item.title, "Atom Item");
        assert_eq!(item.link.as_deref(), Some("https://example.com/items/1"));
        assert_eq!(
            item.torrent_url.as_deref(),
            Some("https://example.com/t/1.torrent")
        );
        assert_eq!(item.size_bytes, Some(42));
        assert_eq!(item.published_at, Some(TS));
    }

    #[test]
    fn rdf_rss10_item() {
        let doc = r#"<?xml version="1.0"?>
            <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"
                     xmlns="http://purl.org/rss/1.0/"
                     xmlns:dc="http://purl.org/dc/elements/1.1/">
              <channel><title>RDF Feed</title></channel>
              <item rdf:about="https://example.com/items/1">
                <title>RDF Item</title>
                <link>https://example.com/items/1</link>
                <dc:date>2023-11-15T06:00:00Z</dc:date>
              </item>
            </rdf:RDF>"#;
        let feed = parse_feed(doc.as_bytes()).unwrap();
        assert_eq!(feed.title.as_deref(), Some("RDF Feed"));
        assert_eq!(feed.items.len(), 1);
        let item = &feed.items[0];
        assert_eq!(item.guid.as_deref(), Some("https://example.com/items/1"));
        assert_eq!(item.title, "RDF Item");
        assert_eq!(item.link.as_deref(), Some("https://example.com/items/1"));
        assert_eq!(item.published_at, Some(TS));
    }

    #[test]
    fn cdata_and_entities_in_title() {
        let doc = rss2(
            r#"<item><title><![CDATA[Show & <Special> "1080p"]]></title></item>
               <item><title>A &amp; B &lt;C&gt;</title></item>"#,
        );
        let feed = parse_feed(doc.as_bytes()).unwrap();
        assert_eq!(feed.items[0].title, r#"Show & <Special> "1080p""#);
        assert_eq!(feed.items[1].title, "A & B <C>");
    }

    #[test]
    fn guid_falls_back_to_link() {
        let doc = rss2("<item><title>t</title><link>https://example.com/x</link></item>");
        let feed = parse_feed(doc.as_bytes()).unwrap();
        assert_eq!(feed.items[0].guid.as_deref(), Some("https://example.com/x"));
    }

    #[test]
    fn guid_falls_back_to_torrent_url() {
        let doc = rss2(
            r#"<item><title>t</title>
               <enclosure url="https://example.com/x.torrent" type="application/x-bittorrent"/>
               </item>"#,
        );
        let feed = parse_feed(doc.as_bytes()).unwrap();
        assert_eq!(
            feed.items[0].guid.as_deref(),
            Some("https://example.com/x.torrent")
        );
    }

    #[test]
    fn torrent_link_with_query_and_fragment() {
        let doc = rss2(
            r#"<item><title>t</title>
               <link>https://example.com/f.TORRENT?token=abc&amp;x=1#frag</link>
               </item>"#,
        );
        let feed = parse_feed(doc.as_bytes()).unwrap();
        assert_eq!(
            feed.items[0].torrent_url.as_deref(),
            Some("https://example.com/f.TORRENT?token=abc&x=1#frag")
        );
    }

    #[test]
    fn magnet_in_rss_link() {
        let doc = rss2(&format!(
            "<item><title>t</title><link>magnet:?xt=urn:btih:{HEX40}&amp;dn=x</link></item>"
        ));
        let feed = parse_feed(doc.as_bytes()).unwrap();
        let item = &feed.items[0];
        assert!(item.magnet.as_deref().unwrap().starts_with("magnet:?"));
        assert_eq!(item.torrent_url, None);
        assert_eq!(item.link, item.magnet);
    }

    #[test]
    fn atom_magnet_link() {
        let doc = format!(
            r#"<feed xmlns="http://www.w3.org/2005/Atom"><entry>
               <title>t</title>
               <link rel="enclosure" href="magnet:?xt=urn:btih:{HEX40}"/>
               </entry></feed>"#
        );
        let feed = parse_feed(doc.as_bytes()).unwrap();
        assert_eq!(
            feed.items[0].magnet.as_deref(),
            Some(format!("magnet:?xt=urn:btih:{HEX40}").as_str())
        );
        assert_eq!(feed.items[0].torrent_url, None);
    }

    #[test]
    fn atom_updated_fallback_date() {
        let doc = r#"<feed xmlns="http://www.w3.org/2005/Atom"><entry>
            <title>t</title><updated>2023-11-15T06:00:00Z</updated>
            </entry></feed>"#;
        let feed = parse_feed(doc.as_bytes()).unwrap();
        assert_eq!(feed.items[0].published_at, Some(TS));
    }

    #[test]
    fn enclosure_torrent_suffix_without_type() {
        let doc = rss2(
            r#"<item><title>t</title>
               <enclosure url="https://example.com/f.torrent" length="99"/>
               </item>"#,
        );
        let feed = parse_feed(doc.as_bytes()).unwrap();
        assert_eq!(
            feed.items[0].torrent_url.as_deref(),
            Some("https://example.com/f.torrent")
        );
        assert_eq!(feed.items[0].size_bytes, Some(99));
    }

    #[test]
    fn nested_markup_does_not_leak() {
        let doc = r#"<rss><channel><title>Real</title>
            <image><title>ImageTitle</title></image>
            <item><title>Item</title><source url="x"><title>SourceTitle</title></source></item>
            </channel></rss>"#;
        let feed = parse_feed(doc.as_bytes()).unwrap();
        assert_eq!(feed.title.as_deref(), Some("Real"));
        assert_eq!(feed.items[0].title, "Item");
    }

    #[test]
    fn whitespace_and_missing_fields_tolerated() {
        let doc = " \n <rss version=\"2.0\"> \n <channel> \n <title>  Feed  </title> \n \
                   <item> \n <title>  spaced title  </title> \n </item> \n </channel> \n </rss> \n ";
        let feed = parse_feed(doc.as_bytes()).unwrap();
        assert_eq!(feed.title.as_deref(), Some("Feed"));
        assert_eq!(feed.items.len(), 1);
        let item = &feed.items[0];
        assert_eq!(item.title, "spaced title");
        assert_eq!(item.guid, None);
        assert_eq!(item.link, None);
        assert_eq!(item.torrent_url, None);
        assert_eq!(item.magnet, None);
        assert_eq!(item.size_bytes, None);
        assert_eq!(item.published_at, None);
    }

    #[test]
    fn base32_vector_repeating() {
        let magnet = format!("magnet:?xt=urn:btih:{BASE32_A}&dn=x");
        assert_eq!(normalize_info_hash(&magnet).as_deref(), Some(HEX_A));
        // Lowercase base32 is accepted too.
        let lower = magnet.to_ascii_lowercase();
        assert_eq!(normalize_info_hash(&lower).as_deref(), Some(HEX_A));
    }

    #[test]
    fn base32_vector_all_ones() {
        let magnet = "magnet:?xt=urn:btih:77777777777777777777777777777777";
        assert_eq!(
            normalize_info_hash(magnet).as_deref(),
            Some("ffffffffffffffffffffffffffffffffffffffff")
        );
        let zeros = "magnet:?xt=urn:btih:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
        assert_eq!(
            normalize_info_hash(zeros).as_deref(),
            Some("0000000000000000000000000000000000000000")
        );
    }

    #[test]
    fn hex_uppercase_normalized() {
        let magnet = "magnet:?xt=urn:btih:0123456789ABCDEF0123456789ABCDEF01234567&dn=x";
        assert_eq!(normalize_info_hash(magnet).as_deref(), Some(HEX40));
        // Case-insensitive scheme/param names and a non-btih xt before the valid one.
        let magnet = "MAGNET:?xt=urn:sha1:zzz&XT=URN:BTIH:0123456789ABCDEF0123456789ABCDEF01234567";
        assert_eq!(normalize_info_hash(magnet).as_deref(), Some(HEX40));
    }

    #[test]
    fn info_hash_invalid_inputs() {
        let bad = [
            "",
            "magnet:?",
            "https://example.com/f.torrent",
            "magnet:?dn=noxt",
            "magnet:?xt=urn:sha1:0123456789abcdef0123456789abcdef01234567",
            "magnet:?xt=urn:btih:",
            // 39 / 41 hex chars.
            "magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef0123456",
            "magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef012345678",
            // Non-hex character in a 40-char value.
            "magnet:?xt=urn:btih:g123456789abcdef0123456789abcdef01234567",
            // 31 / 33 base32 chars.
            "magnet:?xt=urn:btih:MFQWCYLBMFQWCYLBMFQWCYLBMFQWCYL",
            "magnet:?xt=urn:btih:MFQWCYLBMFQWCYLBMFQWCYLBMFQWCYLBB",
            // Characters outside the base32 alphabet (0, 1, 8, 9, padding).
            "magnet:?xt=urn:btih:1FQWCYLBMFQWCYLBMFQWCYLBMFQWCYLB",
            "magnet:?xt=urn:btih:8FQWCYLBMFQWCYLBMFQWCYLBMFQWCYLB",
            "magnet:?xt=urn:btih:MFQWCYLBMFQWCYLBMFQWCYLBMFQWCYLB=",
        ];
        for input in bad {
            assert_eq!(normalize_info_hash(input), None, "input: {input}");
        }
    }

    #[test]
    fn date_rfc2822_zone_variants() {
        let cases = [
            "Wed, 15 Nov 2023 06:00:00 GMT",
            "15 Nov 2023 06:00:00 +0000",
            "Wed, 15 Nov 2023 07:00:00 +0100",
            "Wed, 15 Nov 2023 01:00:00 -0500",
            "15 Nov 2023 06:00:00 UT",
            "15 Nov 2023 06:00:00 UTC",
            "Wed, 15 Nov 2023 06:00:00",
            "15 Nov 2023 06:00:00 +0000 (UTC)",
            "Wed,15 Nov 2023 06:00:00 GMT",
        ];
        for case in cases {
            assert_eq!(parse_date(case), Some(TS), "input: {case}");
        }
    }

    #[test]
    fn date_rfc822_two_digit_year() {
        assert_eq!(parse_date("Wed, 15 Nov 23 06:00:00 GMT"), Some(TS));
        assert_eq!(
            parse_date("15 Nov 99 06:00:00 GMT"),
            parse_date("15 Nov 1999 06:00:00 GMT")
        );
    }

    #[test]
    fn date_iso8601_variants() {
        let cases = [
            "2023-11-15T06:00:00Z",
            "2023-11-15T07:00:00+01:00",
            "2023-11-15T01:00:00-05:00",
            "2023-11-15T06:00:00.123Z",
            "2023-11-15T06:00:00+0000",
            "2023-11-15T06:00Z",
            "2023-11-15 06:00:00",
            "2023-11-15t06:00:00z",
        ];
        for case in cases {
            assert_eq!(parse_date(case), Some(TS), "input: {case}");
        }
        assert_eq!(parse_date("2023-11-15"), Some(TS - 6 * 3600));
    }

    #[test]
    fn date_epoch_and_leap_day() {
        assert_eq!(parse_date("Thu, 01 Jan 1970 00:00:00 GMT"), Some(0));
        assert_eq!(parse_date("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_date("2024-02-29T00:00:00Z"), Some(1_709_164_800));
        assert_eq!(parse_date("2024-02-29"), Some(1_709_164_800));
    }

    #[test]
    fn date_invalid_inputs() {
        let bad = [
            "",
            "   ",
            "not a date",
            "Wed, 32 Nov 2023 06:00:00 GMT",
            "15 Foo 2023 06:00:00 GMT",
            "2023-02-30T00:00:00Z",
            "2023-13-01T00:00:00Z",
            "2023-02-29T00:00:00Z",
            "2023-11-15T25:00:00Z",
            "2023-11-15T06:60:00Z",
            "15 Nov 2023 06:00:00 XYZ",
            "2023-11-15T06:00:00+25:00",
            // Before the epoch cannot be represented as u64.
            "Wed, 31 Dec 1969 23:59:59 GMT",
            "1969-12-31T23:59:59Z",
        ];
        for case in bad {
            assert_eq!(parse_date(case), None, "input: {case}");
        }
    }

    #[test]
    fn garbage_input_is_error() {
        let bad: [&[u8]; 6] = [
            b"this is not xml at all",
            b"",
            b"   ",
            b"\x00\x01\x02",
            b"<<<>>>",
            b"<rss><channel></wrong>",
        ];
        for input in bad {
            assert!(parse_feed(input).is_err(), "input: {input:?}");
        }
    }

    #[test]
    fn unsupported_root_is_error() {
        assert!(matches!(
            parse_feed(b"<html><body>hi</body></html>"),
            Err(ParseError::UnsupportedFormat)
        ));
        assert!(matches!(
            parse_feed(b"<?xml version='1.0'?><unknown/>"),
            Err(ParseError::UnsupportedFormat)
        ));
    }

    #[test]
    fn empty_feeds_have_zero_items() {
        for doc in [
            "<rss><channel><title>t</title></channel></rss>",
            "<rss/>",
            "<feed xmlns=\"http://www.w3.org/2005/Atom\"/>",
            "<feed></feed>",
            "<rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\"/>",
        ] {
            let feed = parse_feed(doc.as_bytes()).unwrap();
            assert!(feed.items.is_empty(), "doc: {doc}");
        }
    }

    #[test]
    fn item_without_title_is_skipped() {
        let doc = rss2(
            r#"<item><link>https://example.com/no-title</link></item>
               <item><title></title></item>
               <item><title>   </title></item>
               <item><title>valid</title></item>"#,
        );
        let feed = parse_feed(doc.as_bytes()).unwrap();
        assert_eq!(feed.items.len(), 1);
        assert_eq!(feed.items[0].title, "valid");
    }

    #[test]
    fn exactly_1000_items_ok() {
        let items: String = (0..1000)
            .map(|i| format!("<item><title>t{i}</title></item>"))
            .collect();
        let doc = format!("<rss><channel>{items}</channel></rss>");
        let feed = parse_feed(doc.as_bytes()).unwrap();
        assert_eq!(feed.items.len(), 1000);
    }

    #[test]
    fn over_1000_items_error() {
        let items: String = (0..1001)
            .map(|i| format!("<item><title>t{i}</title></item>"))
            .collect();
        let doc = format!("<rss><channel>{items}</channel></rss>");
        assert!(matches!(
            parse_feed(doc.as_bytes()),
            Err(ParseError::TooManyItems)
        ));
    }

    #[test]
    fn truncated_xml_never_panics() {
        let docs = [
            rss2(&format!(
                r#"<item><title>t</title><guid>g</guid>
                   <enclosure url="magnet:?xt=urn:btih:{HEX40}" type="application/x-bittorrent"/>
                   </item>"#
            )),
            r#"<feed xmlns="http://www.w3.org/2005/Atom"><title>T</title>
               <entry><id>i</id><title>t</title>
               <link rel="enclosure" type="application/x-bittorrent" href="https://x/f.torrent"/>
               </entry></feed>"#
                .to_string(),
        ];
        for doc in docs {
            for cut in 0..=doc.len() {
                let _ = parse_feed(&doc.as_bytes()[..cut]);
            }
        }
    }

    #[test]
    fn random_bytes_never_panic() {
        // Deterministic LCG; no rand dependency needed.
        let mut state: u64 = 0xdead_beef;
        let mut next = || {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (state >> 33) as u8
        };
        let charset = b"<>/=?&;:\"'abcXYZ0123 \t\n[]!-";
        for len in [0usize, 1, 2, 7, 64, 255, 1024] {
            for _ in 0..50 {
                let bytes: Vec<u8> = (0..len).map(|_| next()).collect();
                let _ = parse_feed(&bytes);
                let text = String::from_utf8_lossy(&bytes);
                let _ = parse_date(&text);
                let _ = normalize_info_hash(&text);
                // XML-ish strings exercise more parser paths than pure noise.
                let markup: String = (0..len)
                    .map(|_| charset[usize::from(next()) % charset.len()] as char)
                    .collect();
                let _ = parse_feed(markup.as_bytes());
                let _ = parse_date(&markup);
                let _ = normalize_info_hash(&markup);
            }
        }
    }
}
