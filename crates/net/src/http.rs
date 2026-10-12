//! URLs and HTTP/1.1 messages.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Url<'a> {
    pub secure: bool,
    pub host: &'a str,
    pub port: u16,
    /// Path and query, starting with '/'; never empty.
    pub path: &'a str,
}

/// Parses an absolute http(s) URL. A bare "host/path" counts as http.
pub fn parse(url: &str) -> Option<Url<'_>> {
    let url = url.trim();
    let (secure, rest) = if let Some(rest) = strip_prefix_ignore_case(url, "http://") {
        (false, rest)
    } else if let Some(rest) = strip_prefix_ignore_case(url, "https://") {
        (true, rest)
    } else if url.contains("://") || url.is_empty() {
        return None;
    } else {
        (false, url)
    };
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..end];
    let mut path = &rest[end..];
    if let Some(hash) = path.find('#') {
        path = &path[..hash];
    }
    let authority = authority.rsplit('@').next().unwrap_or("");
    let (host, port) = match authority.rfind(':') {
        Some(colon) => (
            &authority[..colon],
            authority[colon + 1..].parse::<u16>().ok()?,
        ),
        None => (authority, if secure { 443 } else { 80 }),
    };
    if host.is_empty()
        || !host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-' || b == b'_')
    {
        return None;
    }
    let path = if path.is_empty() || path.starts_with('?') {
        // "host?x" means "host/?x"; the caller rebuilds the request line.
        if path.is_empty() { "/" } else { path }
    } else {
        path
    };
    Some(Url {
        secure,
        host,
        port,
        path,
    })
}
fn strip_prefix_ignore_case<'a>(text: &'a str, prefix: &str) -> Option<&'a str> {
    (text.len() >= prefix.len()
        && text.as_bytes()[..prefix.len()].eq_ignore_ascii_case(prefix.as_bytes()))
    .then(|| &text[prefix.len()..])
}

struct Writer<'a> {
    out: &'a mut [u8],
    len: usize,
    overflow: bool,
}
impl Writer<'_> {
    fn put(&mut self, text: &[u8]) {
        if self.len + text.len() > self.out.len() {
            self.overflow = true;
            return;
        }
        self.out[self.len..self.len + text.len()].copy_from_slice(text);
        self.len += text.len();
    }
    fn number(&mut self, mut value: u32) {
        let mut digits = [0u8; 10];
        let mut n = 0;
        loop {
            digits[n] = b'0' + (value % 10) as u8;
            value /= 10;
            n += 1;
            if value == 0 {
                break;
            }
        }
        digits[..n].reverse();
        self.put(&digits[..n]);
    }
    fn done(self) -> Option<usize> {
        (!self.overflow).then_some(self.len)
    }
}

/// A GET request that asks the server to close the connection afterwards.
pub fn request(url: &Url, out: &mut [u8]) -> Option<usize> {
    let mut w = Writer {
        out,
        len: 0,
        overflow: false,
    };
    w.put(b"GET ");
    if url.path.starts_with('?') {
        w.put(b"/");
    }
    for &b in url.path.as_bytes() {
        // Spaces and non-ASCII bytes are not allowed on the request line.
        if b <= b' ' || b >= 0x7f {
            const HEX: &[u8; 16] = b"0123456789ABCDEF";
            w.put(&[b'%', HEX[(b >> 4) as usize], HEX[(b & 15) as usize]]);
        } else {
            w.put(&[b]);
        }
    }
    w.put(b" HTTP/1.1\r\nHost: ");
    w.put(url.host.as_bytes());
    if url.port != 80 {
        w.put(b":");
        w.number(url.port as u32);
    }
    w.put(
        b"\r\nUser-Agent: hpOS/0.10 (Huynya Polneyshaya OS)\r\nAccept: text/html, text/plain;q=0.9, */*;q=0.1\r\n\
Accept-Language: ru, en;q=0.8, uk;q=0.6\r\nAccept-Encoding: identity\r\nConnection: close\r\n\r\n",
    );
    w.done()
}

/// Percent-encodes `text` as a query value.
pub fn encode_query(text: &str, out: &mut [u8]) -> Option<usize> {
    let mut w = Writer {
        out,
        len: 0,
        overflow: false,
    };
    for &b in text.as_bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            w.put(&[b]);
        } else if b == b' ' {
            w.put(b"+");
        } else {
            const HEX: &[u8; 16] = b"0123456789ABCDEF";
            w.put(&[b'%', HEX[(b >> 4) as usize], HEX[(b & 15) as usize]]);
        }
    }
    w.done()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Head {
    pub status: u16,
    /// Bytes up to and including the blank line.
    pub length: usize,
    pub content_length: Option<usize>,
    pub chunked: bool,
    /// One of `service_protocol::net::content`: 0 other, 1 HTML, 2 text.
    pub content: u32,
    /// One of `service_protocol::net::charset`, or `None` if not given.
    pub charset: Option<u32>,
    /// Byte range of the Location header value.
    pub location: Option<(usize, usize)>,
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}
fn trim(b: &[u8]) -> &[u8] {
    let start = b
        .iter()
        .position(|c| !c.is_ascii_whitespace())
        .unwrap_or(b.len());
    let end = b
        .iter()
        .rposition(|c| !c.is_ascii_whitespace())
        .map_or(start, |e| e + 1);
    &b[start..end]
}
fn parse_number(b: &[u8], radix: u32) -> Option<usize> {
    let b = trim(b);
    if b.is_empty() || b.len() > 15 {
        return None;
    }
    let mut value = 0usize;
    for &c in b {
        value = value * radix as usize + (c as char).to_digit(radix)? as usize;
    }
    Some(value)
}
/// Maps a charset label to `service_protocol::net::charset` values.
pub fn charset(label: &[u8]) -> Option<u32> {
    let label = trim(label);
    let label = label.strip_prefix(b"\"").unwrap_or(label);
    let label = label.strip_suffix(b"\"").unwrap_or(label);
    let is = |name: &[u8]| label.eq_ignore_ascii_case(name);
    if is(b"utf-8") || is(b"utf8") || is(b"us-ascii") {
        Some(0)
    } else if is(b"windows-1251") || is(b"cp1251") || is(b"x-cp1251") {
        Some(1)
    } else if is(b"iso-8859-1") || is(b"windows-1252") || is(b"latin1") {
        Some(2)
    } else if is(b"koi8-r") || is(b"koi8-u") {
        Some(3)
    } else {
        None
    }
}
/// Content kind and charset from a Content-Type value.
pub fn content_type(value: &[u8]) -> (u32, Option<u32>) {
    let mut parts = value.split(|b| *b == b';');
    let kind = trim(parts.next().unwrap_or(b""));
    let content = if find(&kind.to_ascii_lowercase_array(), b"html").is_some() {
        1
    } else if kind.len() >= 5 && kind[..5].eq_ignore_ascii_case(b"text/") {
        2
    } else {
        0
    };
    let mut set = None;
    for part in parts {
        let part = trim(part);
        if part.len() > 8 && part[..8].eq_ignore_ascii_case(b"charset=") {
            set = charset(&part[8..]);
        }
    }
    (content, set)
}
trait Lowercase {
    fn to_ascii_lowercase_array(&self) -> [u8; 64];
}
impl Lowercase for [u8] {
    fn to_ascii_lowercase_array(&self) -> [u8; 64] {
        let mut out = [0u8; 64];
        for (o, b) in out.iter_mut().zip(self) {
            *o = b.to_ascii_lowercase();
        }
        out
    }
}

/// Parses the status line and headers once they are complete.
pub fn head(b: &[u8]) -> Option<Head> {
    let end = find(b, b"\r\n\r\n")? + 4;
    let text = &b[..end];
    let first = find(text, b"\r\n")?;
    let line = &text[..first];
    if line.len() < 12 || !line.starts_with(b"HTTP/1.") {
        return None;
    }
    let status = parse_number(&line[9..12], 10)? as u16;
    let mut h = Head {
        status,
        length: end,
        content_length: None,
        chunked: false,
        content: 0,
        charset: None,
        location: None,
    };
    let mut at = first + 2;
    while at < end - 2 {
        let line_end = at + find(&text[at..], b"\r\n")?;
        let line = &text[at..line_end];
        if let Some(colon) = line.iter().position(|c| *c == b':') {
            let name = trim(&line[..colon]);
            let value = trim(&line[colon + 1..]);
            let value_start = at
                + colon
                + 1
                + line[colon + 1..]
                    .iter()
                    .position(|c| !c.is_ascii_whitespace())
                    .unwrap_or(0);
            let is = |n: &[u8]| name.eq_ignore_ascii_case(n);
            if is(b"content-length") {
                h.content_length = parse_number(value, 10);
            } else if is(b"transfer-encoding") {
                h.chunked = find(&value.to_ascii_lowercase_array(), b"chunked").is_some();
            } else if is(b"content-type") {
                (h.content, h.charset) = content_type(value);
            } else if is(b"location") {
                h.location = Some((value_start, value_start + value.len()));
            }
        }
        at = line_end + 2;
    }
    Some(h)
}

/// For a chunked body starting at `b[0]`: the decoded length once the final
/// chunk has arrived, or `None` while more is needed or on malformed input.
pub fn chunked_complete(b: &[u8]) -> Option<usize> {
    let mut at = 0;
    let mut total = 0;
    loop {
        let line_end = at + find(&b[at..], b"\r\n")?;
        let size_text = &b[at..line_end];
        let size_text = &size_text[..size_text
            .iter()
            .position(|c| *c == b';')
            .unwrap_or(size_text.len())];
        let size = parse_number(size_text, 16)?;
        at = line_end + 2;
        if size == 0 {
            return Some(total);
        }
        if at + size + 2 > b.len() {
            return None;
        }
        at += size + 2;
        total += size;
    }
}
/// Decodes a chunked body in place; returns its length. Trailing garbage
/// or a missing final chunk keeps what was decoded.
pub fn dechunk(b: &mut [u8]) -> usize {
    let mut read = 0;
    let mut write = 0;
    while let Some(line) = find(&b[read..], b"\r\n") {
        let size_text = &b[read..read + line];
        let size_text = &size_text[..size_text
            .iter()
            .position(|c| *c == b';')
            .unwrap_or(size_text.len())];
        let Some(size) = parse_number(size_text, 16) else {
            break;
        };
        read += line + 2;
        if size == 0 {
            break;
        }
        let size = size.min(b.len() - read);
        b.copy_within(read..read + size, write);
        write += size;
        read += size + 2;
        if read > b.len() {
            break;
        }
    }
    write
}

/// Resolves `href` against the absolute URL `base`. Returns `None` for
/// links that are not http(s), such as mailto: or javascript:, and for
/// fragment-only links.
pub fn resolve(base: &str, href: &str, out: &mut [u8]) -> Option<usize> {
    let href = href.trim();
    let href = &href[..href.find('#').unwrap_or(href.len())];
    if href.is_empty() {
        return None;
    }
    let mut w = Writer {
        out,
        len: 0,
        overflow: false,
    };
    if let Some(colon) = href.find(':') {
        let scheme = &href[..colon];
        if scheme
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'-' || b == b'.')
            && !scheme.is_empty()
        {
            if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
                return None;
            }
            w.put(href.as_bytes());
            return w.done();
        }
    }
    let url = parse(base)?;
    let scheme: &[u8] = if url.secure { b"https:" } else { b"http:" };
    if let Some(rest) = href.strip_prefix("//") {
        w.put(scheme);
        w.put(b"//");
        w.put(rest.as_bytes());
        return w.done();
    }
    w.put(scheme);
    w.put(b"//");
    w.put(url.host.as_bytes());
    if url.port != if url.secure { 443 } else { 80 } {
        w.put(b":");
        w.number(url.port as u32);
    }
    let base_path = url.path;
    let base_path = &base_path[..base_path.find('?').unwrap_or(base_path.len())];
    let start = w.len;
    if href.starts_with('/') {
        w.put(href.as_bytes());
    } else if href.starts_with('?') {
        w.put(base_path.as_bytes());
        w.put(href.as_bytes());
    } else {
        let dir = &base_path[..base_path.rfind('/').map_or(0, |i| i + 1)];
        if !dir.starts_with('/') {
            w.put(b"/");
        }
        w.put(dir.as_bytes());
        w.put(href.as_bytes());
    }
    let len = w.done()?;
    Some(start + normalize(&mut out[start..len]))
}
/// Removes "." and ".." segments from a path (query left untouched).
fn normalize(path: &mut [u8]) -> usize {
    let query = path.iter().position(|b| *b == b'?').unwrap_or(path.len());
    let mut segments = [(0usize, 0usize); 64];
    let mut count = 0;
    let mut at = 1;
    while at <= query {
        let end = at
            + path[at..query]
                .iter()
                .position(|b| *b == b'/')
                .unwrap_or(query - at);
        let segment = &path[at..end];
        let last = end == query;
        match segment {
            b"." => {
                if last {
                    segments[count.min(63)] = (end, end);
                    count = (count + 1).min(64);
                }
            }
            b".." => {
                count = count.saturating_sub(1);
                if last {
                    segments[count.min(63)] = (end, end);
                    count = (count + 1).min(64);
                }
            }
            _ => {
                if count < 64 {
                    segments[count] = (at, end);
                    count += 1;
                }
            }
        }
        at = end + 1;
    }
    let mut out = [0u8; 2048];
    let mut len = 0;
    for &(start, end) in &segments[..count] {
        if len + 1 + (end - start) > out.len() {
            break;
        }
        out[len] = b'/';
        out[len + 1..len + 1 + end - start].copy_from_slice(&path[start..end]);
        len += 1 + end - start;
    }
    if len == 0 {
        out[0] = b'/';
        len = 1;
    }
    let rest = path.len() - query;
    if len + rest > path.len() {
        return path.len();
    }
    let mut tail = [0u8; 2048];
    let rest = rest.min(tail.len());
    tail[..rest].copy_from_slice(&path[query..query + rest]);
    path[..len].copy_from_slice(&out[..len]);
    path[len..len + rest].copy_from_slice(&tail[..rest]);
    len + rest
}
