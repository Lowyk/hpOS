use super::*;
use std::{boxed::Box, vec::Vec};

fn page() -> Box<Page> {
    // All-zero bytes are an empty page, as in the desktop heap.
    unsafe { Box::<Page>::new_zeroed().assume_init() }
}
fn kinds(p: &Page) -> Vec<(u8, &str)> {
    p.runs().iter().map(|r| (r.kind, p.text_of(r.start, r.len))).collect()
}

#[test]
fn html_becomes_blocks_text_and_links() {
    let mut p = page();
    let html = b"<!DOCTYPE html><html><head><title>Test &amp; page</title>\
<style>body{color:red}</style><script>var a = '<p>no</p>';</script></head>\
<body><h1>Hello,   world</h1><p>First <b>bold</b> line.<br>Second &laquo;line&raquo; \
<a href=\"/next?a=1&amp;b=2\">next  page</a>.</p><!-- comment --><ul><li>one<li>two</ul>\
<p>&#1055;&#x440;&#1080;&#1074;&#1077;&#1090; &mdash; 5&nbsp;&lt; 6</p><img alt=\"cat\" src=x></body></html>";
    load(&mut p, html, charset::UTF8, true);
    assert_eq!(p.title(), "Test & page");
    assert_eq!(
        kinds(&p),
        [
            (kind::HEADING, ""),
            (kind::TEXT, "Hello, world"),
            (kind::PARAGRAPH, ""),
            (kind::TEXT, "First bold line."),
            (kind::BREAK, ""),
            (kind::TEXT, "Second \"line\""),
            (kind::TEXT, " next page"),
            (kind::TEXT, "."),
            (kind::ITEM, ""),
            (kind::TEXT, "one"),
            (kind::ITEM, ""),
            (kind::TEXT, "two"),
            (kind::PARAGRAPH, ""),
            (kind::TEXT, "Привет - 5 < 6"),
            (kind::PARAGRAPH, ""),
            (kind::TEXT, "[cat]"),
        ]
    );
    assert_eq!(p.link_count, 1);
    assert_eq!(p.href(0), "/next?a=1&b=2");
    assert_eq!(p.runs()[6].link, 1);
}

#[test]
fn legacy_cyrillic_charsets_decode() {
    let mut p = page();
    // "Привет, мир" in windows-1251, declared in a meta tag only.
    let mut html = b"<meta http-equiv=\"Content-Type\" content=\"text/html; charset=windows-1251\"><p>".to_vec();
    html.extend([0xcf, 0xf0, 0xe8, 0xe2, 0xe5, 0xf2, 0x2c, 0x20, 0xec, 0xe8, 0xf0, 0xb8]);
    assert_eq!(sniff(&html), charset::WINDOWS_1251);
    load(&mut p, &html, charset::UNKNOWN, true);
    assert_eq!(p.plain(), "Привет, мирё");
    // KOI8-R "Мир" as plain text.
    load(&mut p, &[0xed, 0xc9, 0xd2, b'\n', b'x'], charset::KOI8R, false);
    assert_eq!(p.plain(), "Мир\nx");
    // Undeclared bytes that are not UTF-8 are taken as windows-1251.
    assert_eq!(sniff(&[0xcf, 0xf0, 0xe8]), charset::WINDOWS_1251);
    assert_eq!(sniff("Привет".as_bytes()), charset::UTF8);
}

#[test]
fn layout_wraps_words_and_finds_links() {
    let mut p = page();
    let mut html = Vec::new();
    html.extend_from_slice(b"<h2>Title</h2><p>");
    for _ in 0..40 {
        html.extend_from_slice(b"word ");
    }
    html.extend_from_slice(b"<a href=\"http://x.org/\">link</a></p><hr><pre>a  b\n\nc</pre>");
    load(&mut p, &html, charset::UTF8, true);
    layout(&mut p, 400);
    let f = p.fragments();
    assert_eq!(f[0].style, style::TITLE);
    assert_eq!(p.text_of(f[0].start, f[0].len), "Title");
    // Body lines never pass the right margin, and words merge per line.
    let body: Vec<&Fragment> = f.iter().filter(|f| f.style == style::BODY).collect();
    assert!(body.len() >= 3);
    for fragment in &body {
        assert!(fragment.x + fragment_width(&p, fragment) <= 400 - MARGIN, "{fragment:?}");
    }
    let link = f.iter().find(|f| f.link == 1).unwrap();
    assert_eq!(link_at(&p, link.x + 3, link.y + 10), Some(0));
    assert_eq!(link_at(&p, link.x + 3, link.y - 30), None);
    assert!(f.iter().any(|f| f.style == style::RULE));
    let pre: Vec<&str> = f.iter().filter(|f| f.style == style::PRE).map(|f| p.text_of(f.start, f.len)).collect();
    assert_eq!(pre, ["a  b", "c"]);
    let a = f.iter().find(|f| p.text_of(f.start, f.len) == "a  b").unwrap();
    let c = f.iter().find(|f| p.text_of(f.start, f.len) == "c").unwrap();
    assert_eq!(c.y - a.y, 2 * 24, "the empty line keeps its height");
    assert!(p.height > c.y);
    // Fragments come in reading order, so the visible range is a slice.
    assert!(f.windows(2).all(|w| w[0].y <= w[1].y));
    assert!(visible(&p, c.y, c.y).iter().any(|f| f.start == c.start));
    // Narrow widths still lay out a long unbroken word.
    load(&mut p, &[b'x'; 300], charset::UTF8, true);
    layout(&mut p, 200);
    assert!(p.fragments().len() > 3);
}
