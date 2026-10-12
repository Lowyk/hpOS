//! Web pages: HTML to styled text runs, line layout and link lookup.
//! A [`Page`] is large and lives in the desktop heap; all-zero bytes are a
//! valid empty page, so it never has to be built on the stack.
use crate::{canvas::Canvas, font::Font};

pub const TEXT_BYTES: usize = 512 * 1024;
pub const RUNS: usize = 32768;
pub const FRAGMENTS: usize = 32768;
pub const LINKS: usize = 4096;
pub const HREF_BYTES: usize = 128 * 1024;
pub const TITLE_BYTES: usize = 256;

/// Run kinds. `TEXT` carries characters; the others start a block.
pub mod kind {
    pub const TEXT: u8 = 0;
    pub const PARAGRAPH: u8 = 1;
    /// `level` 1..=6.
    pub const HEADING: u8 = 2;
    /// `level` is the list depth.
    pub const ITEM: u8 = 3;
    pub const PRE: u8 = 4;
    pub const QUOTE: u8 = 5;
    pub const RULE: u8 = 6;
    pub const BREAK: u8 = 7;
}
/// Fragment styles.
pub mod style {
    pub const BODY: u8 = 0;
    pub const TITLE: u8 = 1;
    pub const SUBTITLE: u8 = 2;
    pub const PRE: u8 = 3;
    pub const BULLET: u8 = 4;
    pub const RULE: u8 = 5;
    pub const QUOTE: u8 = 6;
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Run {
    pub start: u32,
    pub len: u32,
    pub kind: u8,
    pub level: u8,
    /// Link number plus one; 0 for plain text.
    pub link: u16,
}
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fragment {
    pub x: i32,
    pub y: i32,
    pub start: u32,
    pub len: u32,
    pub style: u8,
    pub link: u16,
}

#[repr(C)]
pub struct Page {
    pub text: [u8; TEXT_BYTES],
    pub text_len: usize,
    pub runs: [Run; RUNS],
    pub run_count: usize,
    pub hrefs: [u8; HREF_BYTES],
    pub href_len: usize,
    pub link_start: [u32; LINKS],
    pub link_len: [u16; LINKS],
    pub link_count: usize,
    pub title: [u8; TITLE_BYTES],
    pub title_len: usize,
    pub fragments: [Fragment; FRAGMENTS],
    pub fragment_count: usize,
    /// Width the fragments were laid out for, and the resulting height.
    pub width: i32,
    pub height: i32,
    /// Text or layout space ran out; the rest of the page is missing.
    pub truncated: bool,
}

pub const MARGIN: i32 = 16;
const BODY_LINE: i32 = 24;
const TITLE_LINE: i32 = 32;
const INDENT: i32 = 26;

impl Page {
    pub fn clear(&mut self) {
        self.text_len = 0;
        self.run_count = 0;
        self.href_len = 0;
        self.link_count = 0;
        self.title_len = 0;
        self.fragment_count = 0;
        self.width = 0;
        self.height = 0;
        self.truncated = false;
    }
    pub fn runs(&self) -> &[Run] {
        &self.runs[..self.run_count]
    }
    pub fn fragments(&self) -> &[Fragment] {
        &self.fragments[..self.fragment_count]
    }
    pub fn text_of(&self, start: u32, len: u32) -> &str {
        let end = (start + len) as usize;
        core::str::from_utf8(&self.text[start as usize..end.min(self.text_len)]).unwrap_or("")
    }
    pub fn title(&self) -> &str {
        core::str::from_utf8(&self.title[..self.title_len]).unwrap_or("")
    }
    /// The href of link `n` (zero-based).
    pub fn href(&self, n: usize) -> &str {
        if n >= self.link_count {
            return "";
        }
        let start = self.link_start[n] as usize;
        core::str::from_utf8(&self.hrefs[start..start + self.link_len[n] as usize]).unwrap_or("")
    }
    /// Plain text of the whole page, for tests and the terminal.
    pub fn plain(&self) -> &str {
        core::str::from_utf8(&self.text[..self.text_len]).unwrap_or("")
    }
}

// ---- Character sets ----

pub mod charset {
    pub const UTF8: u32 = 0;
    pub const WINDOWS_1251: u32 = 1;
    pub const LATIN1: u32 = 2;
    pub const KOI8R: u32 = 3;
    pub const UNKNOWN: u32 = 0xff;
}
const CP1251_HIGH: [u16; 64] = [
    0x0402, 0x0403, 0x201a, 0x0453, 0x201e, 0x2026, 0x2020, 0x2021, 0x20ac, 0x2030, 0x0409, 0x2039,
    0x040a, 0x040c, 0x040b, 0x040f, 0x0452, 0x2018, 0x2019, 0x201c, 0x201d, 0x2022, 0x2013, 0x2014,
    0xfffd, 0x2122, 0x0459, 0x203a, 0x045a, 0x045c, 0x045b, 0x045f, 0x00a0, 0x040e, 0x045e, 0x0408,
    0x00a4, 0x0490, 0x00a6, 0x00a7, 0x0401, 0x00a9, 0x0404, 0x00ab, 0x00ac, 0x00ad, 0x00ae, 0x0407,
    0x00b0, 0x00b1, 0x0406, 0x0456, 0x0491, 0x00b5, 0x00b6, 0x00b7, 0x0451, 0x2116, 0x0454, 0x00bb,
    0x0458, 0x0405, 0x0455, 0x0457,
];
const KOI8_LETTERS: &str = "юабцдефгхийклмнопярстужвьызшэщчъЮАБЦДЕФГХИЙКЛМНОПЯРСТУЖВЬЫЗШЭЩЧЪ";

/// Decodes one character at `bytes[*at]` and advances `at`.
fn decode(bytes: &[u8], at: &mut usize, set: u32) -> char {
    let b = bytes[*at];
    *at += 1;
    if b < 0x80 {
        return b as char;
    }
    match set {
        charset::WINDOWS_1251 => {
            if b >= 0xc0 {
                char::from_u32(0x410 + (b - 0xc0) as u32).unwrap_or('?')
            } else {
                char::from_u32(CP1251_HIGH[(b - 0x80) as usize] as u32).unwrap_or('?')
            }
        }
        charset::KOI8R => match b {
            0xc0..=0xff => KOI8_LETTERS.chars().nth((b - 0xc0) as usize).unwrap_or('?'),
            0xa3 => 'ё',
            0xb3 => 'Ё',
            0xa4 => 'є',
            0xa6 => 'і',
            0xa7 => 'ї',
            0xad => 'ґ',
            0xb4 => 'Є',
            0xb6 => 'І',
            0xb7 => 'Ї',
            0xbd => 'Ґ',
            0x9a => '\u{a0}',
            _ => ' ',
        },
        charset::LATIN1 => b as char,
        _ => {
            let (len, init) = match b {
                0xc2..=0xdf => (2, (b & 0x1f) as u32),
                0xe0..=0xef => (3, (b & 0x0f) as u32),
                0xf0..=0xf4 => (4, (b & 0x07) as u32),
                _ => return '\u{fffd}',
            };
            let mut value = init;
            for _ in 1..len {
                match bytes.get(*at) {
                    Some(&c) if c & 0xc0 == 0x80 => {
                        value = value << 6 | (c & 0x3f) as u32;
                        *at += 1;
                    }
                    _ => return '\u{fffd}',
                }
            }
            char::from_u32(value).unwrap_or('\u{fffd}')
        }
    }
}
/// Picks the charset of a page whose server did not name one: a meta tag
/// near the top, else UTF-8 if the bytes are valid UTF-8, else windows-1251.
pub fn sniff(bytes: &[u8]) -> u32 {
    let head = &bytes[..bytes.len().min(4096)];
    let mut at = 0;
    while let Some(pos) = find_ci(&head[at..], b"charset=") {
        let start = at + pos + 8;
        let rest = &head[start..];
        let rest = rest.strip_prefix(b"\"").or_else(|| rest.strip_prefix(b"'")).unwrap_or(rest);
        let end = rest
            .iter()
            .position(|c| matches!(c, b'"' | b'\'' | b';' | b'>' | b' ' | b'/'))
            .unwrap_or(rest.len());
        if let Some(set) = hpos_net::http::charset(&rest[..end]) {
            return set;
        }
        at = start;
    }
    match core::str::from_utf8(bytes) {
        Ok(_) => charset::UTF8,
        // A sequence cut off at the very end is still UTF-8.
        Err(e) if e.error_len().is_none() && e.valid_up_to() > 0 => charset::UTF8,
        Err(_) => charset::WINDOWS_1251,
    }
}
fn find_ci(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|w| w.eq_ignore_ascii_case(needle))
}

/// Characters the interface font cannot draw, replaced by close ones.
fn substitute(c: char) -> Option<&'static str> {
    Some(match c {
        '«' | '»' | '“' | '”' | '„' | '″' => "\"",
        '‘' | '’' | '‚' | '′' => "'",
        '–' | '—' | '−' | '‐' | '‑' => "-",
        '…' => "...",
        '•' | '·' | '●' | '▪' => "*",
        '©' => "(c)",
        '®' => "(R)",
        '™' => "TM",
        '№' => "No",
        '€' => "EUR",
        '₽' => "руб.",
        '×' => "x",
        '°' => "o",
        '→' => "->",
        '←' => "<-",
        'ґ' => "г",
        'Ґ' => "Г",
        '\u{ad}' | '\u{200b}' | '\u{200c}' | '\u{200d}' | '\u{feff}' => "",
        _ => return None,
    })
}
fn entity(name: &[u8]) -> Option<char> {
    if let Some(number) = name.strip_prefix(b"#") {
        let (digits, radix) = match number.first() {
            Some(b'x' | b'X') => (&number[1..], 16),
            _ => (number, 10),
        };
        if digits.is_empty() || digits.len() > 7 {
            return None;
        }
        let mut value = 0u32;
        for &d in digits {
            value = value * radix + (d as char).to_digit(radix)?;
        }
        return char::from_u32(value);
    }
    Some(match name {
        b"amp" => '&',
        b"lt" => '<',
        b"gt" => '>',
        b"quot" => '"',
        b"apos" => '\'',
        b"nbsp" => '\u{a0}',
        b"laquo" => '«',
        b"raquo" => '»',
        b"mdash" => '—',
        b"ndash" => '–',
        b"hellip" => '…',
        b"copy" => '©',
        b"reg" => '®',
        b"trade" => '™',
        b"middot" => '·',
        b"bull" => '•',
        b"lsquo" => '‘',
        b"rsquo" => '’',
        b"ldquo" => '“',
        b"rdquo" => '”',
        b"bdquo" => '„',
        b"times" => '×',
        b"deg" => '°',
        b"euro" => '€',
        b"shy" => '\u{ad}',
        b"rarr" => '→',
        b"larr" => '←',
        b"numero" => '№',
        _ => return None,
    })
}

// ---- HTML to runs ----

struct Builder<'a> {
    page: &'a mut Page,
    /// Current link plus one.
    link: u16,
    pre: bool,
    quote: u8,
    list: u8,
    /// A space is owed before the next word.
    space: bool,
    /// Inside <title>.
    title: bool,
}
impl Builder<'_> {
    fn last_kind(&self) -> Option<u8> {
        self.page.runs().last().map(|r| r.kind)
    }
    fn push_run(&mut self, run: Run) -> bool {
        if self.page.run_count == RUNS {
            self.page.truncated = true;
            return false;
        }
        self.page.runs[self.page.run_count] = run;
        self.page.run_count += 1;
        true
    }
    /// Starts a block. Consecutive empty blocks collapse into the last one.
    fn block(&mut self, kind: u8, level: u8) {
        self.space = false;
        if self.title {
            return;
        }
        let run = Run {
            start: self.page.text_len as u32,
            len: 0,
            kind,
            level,
            link: 0,
        };
        match self.last_kind() {
            None if kind == kind::PARAGRAPH || kind == kind::BREAK => {}
            Some(last) if last != kind::TEXT && last != kind::RULE && kind != kind::RULE => {
                let index = self.page.run_count - 1;
                // A break never replaces a stronger block.
                if kind != kind::BREAK || last == kind::BREAK {
                    self.page.runs[index] = run;
                }
            }
            _ => {
                self.push_run(run);
            }
        }
    }
    /// The block that text after a closing tag continues in.
    fn resume(&mut self) {
        if self.quote > 0 {
            self.block(kind::QUOTE, self.quote);
        } else {
            self.block(kind::PARAGRAPH, 0);
        }
    }
    fn push_str(&mut self, s: &str) {
        if self.title {
            let room = TITLE_BYTES - self.page.title_len;
            let s = clip(s, room);
            self.page.title[self.page.title_len..self.page.title_len + s.len()].copy_from_slice(s.as_bytes());
            self.page.title_len += s.len();
            return;
        }
        if s.is_empty() {
            return;
        }
        if self.page.text_len + s.len() > TEXT_BYTES {
            self.page.truncated = true;
            return;
        }
        let start = self.page.text_len;
        self.page.text[start..start + s.len()].copy_from_slice(s.as_bytes());
        self.page.text_len += s.len();
        let link = self.link;
        if let Some(last) = self.page.runs[..self.page.run_count].last_mut() {
            if last.kind == kind::TEXT && last.link == link && (last.start + last.len) as usize == start {
                last.len += s.len() as u32;
                return;
            }
        }
        if !self.push_run(Run {
            start: start as u32,
            len: s.len() as u32,
            kind: kind::TEXT,
            level: 0,
            link,
        }) {
            self.page.text_len = start;
        }
    }
    fn char(&mut self, c: char) {
        if !self.pre && (c.is_whitespace() || c == '\u{a0}') {
            self.space = !self.at_block_start();
            return;
        }
        if self.pre && c == '\r' {
            return;
        }
        if self.space {
            self.space = false;
            self.push_str(" ");
        }
        if let Some(s) = substitute(c) {
            self.push_str(s);
        } else if self.pre && c == '\t' {
            self.push_str("    ");
        } else {
            let mut buffer = [0u8; 4];
            self.push_str(c.encode_utf8(&mut buffer));
        }
    }
    fn at_block_start(&self) -> bool {
        if self.title {
            return self.page.title_len == 0
                || self.page.title[self.page.title_len - 1] == b' ';
        }
        match self.page.runs().last() {
            None => true,
            Some(r) if r.kind != kind::TEXT => true,
            Some(_) => self.page.text_len == 0 || self.page.text[self.page.text_len - 1] == b' ',
        }
    }
    fn add_link(&mut self, href: &[u8]) -> u16 {
        let n = self.page.link_count;
        if n == LINKS || self.page.href_len + href.len() > HREF_BYTES || href.len() > u16::MAX as usize {
            return 0;
        }
        let start = self.page.href_len;
        self.page.hrefs[start..start + href.len()].copy_from_slice(href);
        self.page.href_len += href.len();
        self.page.link_start[n] = start as u32;
        self.page.link_len[n] = href.len() as u16;
        self.page.link_count += 1;
        n as u16 + 1
    }
}
fn clip(s: &str, max: usize) -> &str {
    let mut end = s.len().min(max);
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// One tag: lowercase name (without '/'), whether it closes, and its raw
/// attribute text.
struct Tag<'a> {
    name: [u8; 16],
    name_len: usize,
    closing: bool,
    attributes: &'a [u8],
}
impl Tag<'_> {
    fn is(&self, name: &str) -> bool {
        &self.name[..self.name_len] == name.as_bytes()
    }
    fn heading(&self) -> Option<u8> {
        match &self.name[..self.name_len] {
            [b'h', d @ b'1'..=b'6'] => Some(d - b'0'),
            _ => None,
        }
    }
    /// Value of attribute `wanted`, entity-decoded into `out`.
    fn attribute<'o>(&self, wanted: &[u8], out: &'o mut [u8], set: u32) -> Option<&'o [u8]> {
        let a = self.attributes;
        let mut at = 0;
        while at < a.len() {
            while at < a.len() && (a[at].is_ascii_whitespace() || a[at] == b'/') {
                at += 1;
            }
            let name_start = at;
            while at < a.len() && !a[at].is_ascii_whitespace() && a[at] != b'=' && a[at] != b'/' {
                at += 1;
            }
            let name = &a[name_start..at];
            while at < a.len() && a[at].is_ascii_whitespace() {
                at += 1;
            }
            let mut value: &[u8] = &[];
            if at < a.len() && a[at] == b'=' {
                at += 1;
                while at < a.len() && a[at].is_ascii_whitespace() {
                    at += 1;
                }
                if at < a.len() && (a[at] == b'"' || a[at] == b'\'') {
                    let quote = a[at];
                    let start = at + 1;
                    let end = start + a[start..].iter().position(|c| *c == quote).unwrap_or(a.len() - start);
                    value = &a[start..end];
                    at = (end + 1).min(a.len());
                } else {
                    let start = at;
                    while at < a.len() && !a[at].is_ascii_whitespace() {
                        at += 1;
                    }
                    value = &a[start..at];
                }
            }
            if name.is_empty() {
                at += 1;
                continue;
            }
            if name.eq_ignore_ascii_case(wanted) {
                let n = decode_text(value, set, out);
                return Some(&out[..n]);
            }
        }
        None
    }
}
/// Decodes `bytes` in `set` with entities into UTF-8.
fn decode_text(bytes: &[u8], set: u32, out: &mut [u8]) -> usize {
    let mut len = 0;
    let mut at = 0;
    while at < bytes.len() {
        let c = if bytes[at] == b'&' {
            match entity_at(bytes, at) {
                Some((c, next)) => {
                    at = next;
                    c
                }
                None => {
                    at += 1;
                    '&'
                }
            }
        } else {
            decode(bytes, &mut at, set)
        };
        let mut buffer = [0u8; 4];
        let s = c.encode_utf8(&mut buffer);
        if len + s.len() > out.len() {
            break;
        }
        out[len..len + s.len()].copy_from_slice(s.as_bytes());
        len += s.len();
    }
    len
}
/// An entity starting at `bytes[at]` ('&'): the character and the index after it.
fn entity_at(bytes: &[u8], at: usize) -> Option<(char, usize)> {
    let rest = &bytes[at + 1..bytes.len().min(at + 12)];
    let end = rest.iter().position(|c| *c == b';')?;
    entity(&rest[..end]).map(|c| (c, at + 2 + end))
}
fn read_tag(bytes: &[u8], at: usize) -> Option<(Tag<'_>, usize)> {
    let mut i = at + 1;
    let closing = bytes.get(i) == Some(&b'/');
    if closing {
        i += 1;
    }
    if !bytes.get(i)?.is_ascii_alphabetic() {
        return None;
    }
    let mut name = [0u8; 16];
    let mut name_len = 0;
    while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'-' || bytes[i] == b':') {
        if name_len < 16 {
            name[name_len] = bytes[i].to_ascii_lowercase();
            name_len += 1;
        }
        i += 1;
    }
    let attributes_start = i;
    let mut quote = 0u8;
    while i < bytes.len() {
        let c = bytes[i];
        if quote != 0 {
            if c == quote {
                quote = 0;
            }
        } else if c == b'"' || c == b'\'' {
            quote = c;
        } else if c == b'>' {
            return Some((
                Tag {
                    name,
                    name_len,
                    closing,
                    attributes: &bytes[attributes_start..i],
                },
                i + 1,
            ));
        }
        i += 1;
    }
    None
}
/// Skips raw text up to the matching closing tag, e.g. `</script`.
fn skip_raw(bytes: &[u8], at: usize, name: &[u8]) -> usize {
    let mut i = at;
    while let Some(pos) = bytes[i..].iter().position(|c| *c == b'<') {
        i += pos;
        if bytes.get(i + 1) == Some(&b'/')
            && bytes.len() >= i + 2 + name.len()
            && bytes[i + 2..i + 2 + name.len()].eq_ignore_ascii_case(name)
        {
            return bytes[i..].iter().position(|c| *c == b'>').map_or(bytes.len(), |p| i + p + 1);
        }
        i += 1;
    }
    bytes.len()
}

/// Replaces the page with `bytes`. HTML is parsed; anything else is shown
/// as preformatted text.
pub fn load(page: &mut Page, bytes: &[u8], set: u32, html: bool) {
    page.clear();
    let set = if set == charset::UNKNOWN { sniff(bytes) } else { set };
    let mut b = Builder {
        page,
        link: 0,
        pre: !html,
        quote: 0,
        list: 0,
        space: false,
        title: false,
    };
    if !html {
        b.block(kind::PRE, 0);
        let mut at = 0;
        while at < bytes.len() && !b.page.truncated {
            let c = decode(bytes, &mut at, set);
            b.char(c);
        }
        return;
    }
    parse(&mut b, bytes, set);
    // No empty block at the end.
    while matches!(b.last_kind(), Some(k) if k != kind::TEXT && k != kind::RULE) {
        b.page.run_count -= 1;
    }
}
fn parse(b: &mut Builder, bytes: &[u8], set: u32) {
    let mut at = 0;
    while at < bytes.len() && !b.page.truncated {
        let c = bytes[at];
        if c == b'<' {
            if bytes[at..].starts_with(b"<!--") {
                at = find_ci(&bytes[at + 4..], b"-->").map_or(bytes.len(), |p| at + 4 + p + 3);
                continue;
            }
            if bytes.get(at + 1) == Some(&b'!') || bytes.get(at + 1) == Some(&b'?') {
                at = bytes[at..].iter().position(|c| *c == b'>').map_or(bytes.len(), |p| at + p + 1);
                continue;
            }
            if let Some((tag, next)) = read_tag(bytes, at) {
                at = next;
                at = element(b, &tag, bytes, at, set);
                continue;
            }
            b.char('<');
            at += 1;
        } else if c == b'&' {
            match entity_at(bytes, at) {
                Some((c, next)) => {
                    b.char(c);
                    at = next;
                }
                None => {
                    b.char('&');
                    at += 1;
                }
            }
        } else {
            let c = decode(bytes, &mut at, set);
            b.char(c);
        }
    }
}
/// Applies one tag; returns where parsing continues.
fn element(b: &mut Builder, tag: &Tag, bytes: &[u8], at: usize, set: u32) -> usize {
    let name = &tag.name[..tag.name_len];
    if !tag.closing {
        match name {
            b"script" | b"style" | b"template" | b"svg" | b"select" | b"iframe" | b"object" | b"textarea" => {
                return skip_raw(bytes, at, name);
            }
            _ => {}
        }
    }
    if tag.is("title") {
        b.title = !tag.closing && b.page.title_len == 0;
        b.space = false;
        return at;
    }
    if let Some(level) = tag.heading() {
        if tag.closing {
            b.resume();
        } else {
            b.block(kind::HEADING, level);
        }
        return at;
    }
    match name {
        b"p" | b"div" | b"section" | b"article" | b"header" | b"footer" | b"nav" | b"main" | b"aside"
        | b"table" | b"tr" | b"form" | b"center" | b"figure" | b"figcaption" | b"address" | b"details"
        | b"summary" | b"fieldset" | b"dl" | b"dt" | b"caption" | b"body" | b"html" => {
            b.resume();
        }
        b"ul" | b"ol" | b"menu" => {
            if tag.closing {
                b.list = b.list.saturating_sub(1);
            } else {
                b.list = (b.list + 1).min(6);
            }
            b.resume();
        }
        b"li" | b"dd" => {
            if tag.closing {
                b.resume();
            } else {
                b.block(kind::ITEM, b.list.max(1));
            }
        }
        b"blockquote" => {
            if tag.closing {
                b.quote = b.quote.saturating_sub(1);
            } else {
                b.quote = (b.quote + 1).min(4);
            }
            b.resume();
        }
        b"pre" => {
            if tag.closing {
                b.pre = false;
                b.resume();
            } else {
                b.block(kind::PRE, 0);
                b.pre = true;
            }
        }
        b"br" => {
            if b.pre {
                b.char('\n');
            } else {
                b.block(kind::BREAK, 0);
            }
        }
        b"hr" => b.block(kind::RULE, 0),
        b"td" | b"th" => {
            if !tag.closing {
                b.space = !b.at_block_start();
            }
        }
        b"a" => {
            if tag.closing {
                b.link = 0;
            } else {
                let mut href = [0u8; 2048];
                b.link = match tag.attribute(b"href", &mut href, set) {
                    Some(h) if !h.is_empty() => {
                        let h: &[u8] = h;
                        let mut copy = [0u8; 2048];
                        copy[..h.len()].copy_from_slice(h);
                        b.add_link(&copy[..h.len()])
                    }
                    _ => 0,
                };
            }
        }
        b"img" => {
            let mut alt = [0u8; 256];
            if let Some(text) = tag.attribute(b"alt", &mut alt, set) {
                if let Ok(text) = core::str::from_utf8(text) {
                    let text = text.trim();
                    if !text.is_empty() {
                        let mut copy = [0u8; 256];
                        copy[..text.len()].copy_from_slice(text.as_bytes());
                        let text = core::str::from_utf8(&copy[..text.len()]).unwrap_or("");
                        b.char('[');
                        for c in text.chars() {
                            b.char(c);
                        }
                        b.char(']');
                    }
                }
            }
        }
        b"input" => {
            let mut value = [0u8; 64];
            if let Some(kind) = tag.attribute(b"type", &mut value, set) {
                if kind.eq_ignore_ascii_case(b"submit") || kind.eq_ignore_ascii_case(b"button") {
                    let mut label = [0u8; 128];
                    if let Some(text) = tag.attribute(b"value", &mut label, set) {
                        let mut copy = [0u8; 128];
                        copy[..text.len()].copy_from_slice(text);
                        let text = core::str::from_utf8(&copy[..text.len()]).unwrap_or("");
                        b.char('[');
                        for c in text.chars() {
                            b.char(c);
                        }
                        b.char(']');
                    }
                }
            }
        }
        _ => {}
    }
    at
}

// ---- Layout ----

fn font_of(style: u8) -> Font {
    if style == style::TITLE { Font::Title } else { Font::Interface }
}
pub fn line_height(style: u8) -> i32 {
    if style == style::TITLE { TITLE_LINE } else { BODY_LINE }
}
struct Layout<'a> {
    page: &'a mut Page,
    left: i32,
    right: i32,
    x: i32,
    y: i32,
    line: i32,
    /// The line holds something.
    filled: bool,
}
impl Layout<'_> {
    fn newline(&mut self) {
        if self.filled {
            self.y += self.line;
        }
        self.x = self.left;
        self.filled = false;
        self.line = BODY_LINE;
    }
    fn gap(&mut self, pixels: i32) {
        self.newline();
        self.y += pixels;
    }
    fn push(&mut self, fragment: Fragment) {
        let n = self.page.fragment_count;
        if let Some(last) = self.page.fragments[..n].last_mut() {
            let joined_text = last.start + last.len;
            if last.y == fragment.y
                && last.style == fragment.style
                && last.link == fragment.link
                && (joined_text == fragment.start || joined_text + 1 == fragment.start)
                && fragment.style != style::BULLET
            {
                last.len = fragment.start + fragment.len - last.start;
                return;
            }
        }
        if n == FRAGMENTS {
            self.page.truncated = true;
            return;
        }
        self.page.fragments[n] = fragment;
        self.page.fragment_count += 1;
    }
    /// Places one word (no spaces inside) of `style`, wrapping as needed.
    fn word(&mut self, start: u32, len: u32, style: u8, link: u16, space_before: bool) {
        let font = font_of(style);
        let width = Canvas::measure_with(font, self.page.text_of(start, len));
        let space = if space_before && self.filled { Canvas::measure_with(font, " ") } else { 0 };
        self.line = self.line.max(line_height(style));
        if self.filled && self.x + space + width > self.right {
            self.newline();
            self.line = line_height(style);
        } else {
            self.x += space;
        }
        if width <= self.right - self.x || self.filled {
            let (x, y) = (self.x, self.y);
            self.push(Fragment { x, y, start, len, style, link });
            self.x += width;
            self.filled = true;
            return;
        }
        // Longer than a whole line: break between characters.
        let mut pieces = [(0u32, 0u32, 0i32); 64];
        let mut count = 0;
        {
            let text = self.page.text_of(start, len);
            let room = self.right - self.left;
            let (mut piece_start, mut piece_width, mut at) = (0u32, 0, 0u32);
            let mut buffer = [0u8; 4];
            for c in text.chars() {
                let w = Canvas::measure_with(font, c.encode_utf8(&mut buffer));
                if piece_width + w > room && at > piece_start && count < 63 {
                    pieces[count] = (piece_start, at - piece_start, piece_width);
                    count += 1;
                    piece_start = at;
                    piece_width = 0;
                }
                piece_width += w;
                at += c.len_utf8() as u32;
            }
            pieces[count] = (piece_start, at - piece_start, piece_width);
            count += 1;
        }
        for (i, &(offset, n, w)) in pieces[..count].iter().enumerate() {
            if i > 0 {
                self.newline();
                self.line = line_height(style);
            }
            let (x, y) = (self.x, self.y);
            self.push(Fragment { x, y, start: start + offset, len: n, style, link });
            self.x += w;
            self.filled = true;
        }
    }
}

/// Lays the page out for a content area `width` pixels wide.
pub fn layout(page: &mut Page, width: i32) {
    page.fragment_count = 0;
    page.width = width;
    let mut l = Layout {
        left: MARGIN,
        right: (width - MARGIN).max(MARGIN + 40),
        x: MARGIN,
        y: 12,
        line: BODY_LINE,
        filled: false,
        page,
    };
    let mut current = style::BODY;
    let mut pre = false;
    for index in 0..l.page.run_count {
        let run = l.page.runs[index];
        match run.kind {
            kind::TEXT => {
                let style = current;
                if pre {
                    let mut line_start = run.start;
                    let end = run.start + run.len;
                    let mut at = run.start;
                    while at <= end {
                        if at == end || l.page.text[at as usize] == b'\n' {
                            if at > line_start {
                                l.word(line_start, at - line_start, style, run.link, false);
                            }
                            if at < end {
                                // Empty lines keep their height too.
                                l.filled = true;
                                l.newline();
                            }
                            line_start = at + 1;
                        }
                        at += 1;
                    }
                    continue;
                }
                let mut at = run.start;
                let end = run.start + run.len;
                while at < end {
                    let space = l.page.text[at as usize] == b' ';
                    if space {
                        at += 1;
                        if at == end {
                            // The space belongs before the next run's first word.
                            l.x += Canvas::measure_with(font_of(style), " ");
                            break;
                        }
                    }
                    let word_end = (at..end).find(|i| l.page.text[*i as usize] == b' ').unwrap_or(end);
                    l.word(at, word_end - at, style, run.link, space);
                    at = word_end;
                }
            }
            kind::HEADING => {
                l.gap(if l.page.fragment_count == 0 { 0 } else { 14 });
                current = if run.level <= 2 { style::TITLE } else { style::SUBTITLE };
                l.left = MARGIN;
                l.x = l.left;
                pre = false;
            }
            kind::ITEM => {
                l.newline();
                l.y += 4;
                l.left = MARGIN + INDENT * run.level as i32;
                l.x = l.left;
                let (x, y) = (l.left - 14, l.y + 9);
                l.push(Fragment { x, y, start: 0, len: 0, style: style::BULLET, link: 0 });
                current = style::BODY;
                pre = false;
            }
            kind::QUOTE => {
                l.gap(8);
                l.left = MARGIN + INDENT * run.level as i32;
                l.x = l.left;
                current = style::QUOTE;
                pre = false;
            }
            kind::PRE => {
                l.gap(8);
                l.left = MARGIN + 8;
                l.x = l.left;
                current = style::PRE;
                pre = true;
            }
            kind::RULE => {
                l.gap(8);
                let y = l.y;
                l.push(Fragment { x: MARGIN, y, start: 0, len: 0, style: style::RULE, link: 0 });
                l.y += 10;
                l.left = MARGIN;
                l.x = l.left;
                current = style::BODY;
                pre = false;
            }
            kind::BREAK => {
                l.filled = true;
                l.newline();
            }
            _ => {
                l.gap(10);
                l.left = MARGIN;
                l.x = l.left;
                current = style::BODY;
                pre = false;
            }
        }
    }
    l.newline();
    let height = l.y + 16;
    l.page.height = height;
}

/// Width of a fragment as drawn.
pub fn fragment_width(page: &Page, f: &Fragment) -> i32 {
    match f.style {
        style::RULE => page.width - 2 * MARGIN,
        style::BULLET => 6,
        _ => Canvas::measure_with(font_of(f.style), page.text_of(f.start, f.len)),
    }
}
/// The link under page coordinates (x, y), zero-based.
pub fn link_at(page: &Page, x: i32, y: i32) -> Option<usize> {
    let fragments = page.fragments();
    let first = fragments.partition_point(|f| f.y + TITLE_LINE < y);
    for f in &fragments[first..] {
        if f.y > y {
            break;
        }
        if f.link != 0
            && y < f.y + line_height(f.style)
            && x >= f.x - 2
            && x < f.x + fragment_width(page, f) + 2
        {
            return Some(f.link as usize - 1);
        }
    }
    None
}
/// Fragments that may be visible between page rows `top` and `bottom`.
pub fn visible(page: &Page, top: i32, bottom: i32) -> &[Fragment] {
    let fragments = page.fragments();
    let first = fragments.partition_point(|f| f.y + TITLE_LINE < top);
    let last = first + fragments[first..].partition_point(|f| f.y <= bottom);
    &fragments[first..last]
}

#[cfg(test)]
mod tests;
