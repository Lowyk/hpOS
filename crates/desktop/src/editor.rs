//! Plain-text editor model: a UTF-8 buffer with a byte cursor that always sits
//! on a character boundary. The desktop service loads and saves the bytes.
use crate::files::Name;

/// The largest file one file-system request can carry.
pub const EDIT_BYTES: usize = hpos_service_protocol::fs::DATA_BYTES;

pub struct Editor {
    text: [u8; EDIT_BYTES],
    len: usize,
    cursor: usize,
    /// Preferred column (in characters) for vertical movement.
    column: usize,
    pub top: usize,
    pub file: Option<(u16, Name)>,
    pub dirty: bool,
    /// The user tried to close with unsaved changes once already.
    pub confirm_close: bool,
    pub message: &'static str,
}

impl Editor {
    pub const fn new() -> Self {
        Self {
            text: [0; EDIT_BYTES],
            len: 0,
            cursor: 0,
            column: 0,
            top: 0,
            file: None,
            dirty: false,
            confirm_close: false,
            message: "",
        }
    }
    pub fn text(&self) -> &str {
        core::str::from_utf8(&self.text[..self.len]).unwrap_or("")
    }
    pub fn bytes(&self) -> &[u8] {
        &self.text[..self.len]
    }
    pub fn cursor(&self) -> usize {
        self.cursor
    }
    /// Replaces the document; fails for non-UTF-8 or oversized data.
    pub fn load(&mut self, file: Option<(u16, Name)>, bytes: &[u8]) -> bool {
        if bytes.len() > EDIT_BYTES || core::str::from_utf8(bytes).is_err() {
            return false;
        }
        self.text[..bytes.len()].copy_from_slice(bytes);
        self.len = bytes.len();
        self.cursor = 0;
        self.column = 0;
        self.top = 0;
        self.file = file;
        self.dirty = false;
        self.confirm_close = false;
        self.message = "";
        true
    }
    pub fn insert(&mut self, ch: char) -> bool {
        let mut buffer = [0; 4];
        let encoded = ch.encode_utf8(&mut buffer).as_bytes();
        if self.len + encoded.len() > EDIT_BYTES || ((ch as u32) < 0x20 && ch != '\n') {
            return false;
        }
        self.text
            .copy_within(self.cursor..self.len, self.cursor + encoded.len());
        self.text[self.cursor..self.cursor + encoded.len()].copy_from_slice(encoded);
        self.len += encoded.len();
        self.cursor += encoded.len();
        self.edited();
        true
    }
    fn edited(&mut self) {
        self.dirty = true;
        self.confirm_close = false;
        self.column = self.position().1;
    }
    fn previous(&self, at: usize) -> usize {
        self.text()[..at]
            .chars()
            .last()
            .map_or(at, |c| at - c.len_utf8())
    }
    fn next(&self, at: usize) -> usize {
        self.text()[at..]
            .chars()
            .next()
            .map_or(at, |c| at + c.len_utf8())
    }
    pub fn backspace(&mut self) {
        if self.cursor > 0 {
            let start = self.previous(self.cursor);
            self.remove(start, self.cursor);
            self.cursor = start;
            self.edited();
        }
    }
    pub fn delete(&mut self) {
        if self.cursor < self.len {
            let end = self.next(self.cursor);
            self.remove(self.cursor, end);
            self.edited();
        }
    }
    fn remove(&mut self, start: usize, end: usize) {
        self.text.copy_within(end..self.len, start);
        self.len -= end - start;
    }
    pub fn left(&mut self) {
        self.cursor = self.previous(self.cursor);
        self.column = self.position().1;
    }
    pub fn right(&mut self) {
        self.cursor = self.next(self.cursor);
        self.column = self.position().1;
    }
    fn line_start(&self, at: usize) -> usize {
        self.text[..at]
            .iter()
            .rposition(|b| *b == b'\n')
            .map_or(0, |i| i + 1)
    }
    fn line_end(&self, at: usize) -> usize {
        self.text[at..self.len]
            .iter()
            .position(|b| *b == b'\n')
            .map_or(self.len, |i| at + i)
    }
    pub fn home(&mut self) {
        self.cursor = self.line_start(self.cursor);
        self.column = 0;
    }
    pub fn end(&mut self) {
        self.cursor = self.line_end(self.cursor);
        self.column = self.position().1;
    }
    /// Moves to `column` characters into the line starting at `start`.
    fn to_column(&mut self, start: usize) {
        let end = self.line_end(start);
        let mut at = start;
        for _ in 0..self.column {
            if at >= end {
                break;
            }
            at = self.next(at);
        }
        self.cursor = at;
    }
    pub fn up(&mut self) {
        let start = self.line_start(self.cursor);
        if start > 0 {
            let previous = self.line_start(start - 1);
            self.to_column(previous);
        }
    }
    pub fn down(&mut self) {
        let end = self.line_end(self.cursor);
        if end < self.len {
            self.to_column(end + 1);
        }
    }
    /// Moves to `column` characters into line `line` (both clamped).
    pub fn go_to(&mut self, line: usize, column: usize) {
        let mut start = 0;
        for _ in 0..line {
            let end = self.line_end(start);
            if end >= self.len {
                break;
            }
            start = end + 1;
        }
        self.column = column;
        self.to_column(start);
    }
    pub fn page(&mut self, rows: usize, down: bool) {
        for _ in 0..rows.max(1) {
            if down { self.down() } else { self.up() }
        }
    }
    /// (line, column) of the cursor, both from zero, column in characters.
    pub fn position(&self) -> (usize, usize) {
        let line = self.text[..self.cursor]
            .iter()
            .filter(|b| **b == b'\n')
            .count();
        let start = self.line_start(self.cursor);
        (line, self.text()[start..self.cursor].chars().count())
    }
    pub fn lines(&self) -> usize {
        self.text[..self.len]
            .iter()
            .filter(|b| **b == b'\n')
            .count()
            + 1
    }
    /// Text of line `n` (without the newline), if it exists.
    pub fn line(&self, n: usize) -> Option<&str> {
        let text = self.text();
        text.split('\n').nth(n)
    }
    /// Keeps the cursor line inside a window of `rows` lines.
    pub fn follow(&mut self, rows: usize) {
        let (line, _) = self.position();
        let rows = rows.max(1);
        if line < self.top {
            self.top = line;
        } else if line >= self.top + rows {
            self.top = line + 1 - rows;
        }
    }
    pub fn saved(&mut self) {
        self.dirty = false;
        self.confirm_close = false;
    }
}
impl Default for Editor {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    extern crate std;
    use std::boxed::Box;
    fn editor(text: &str) -> Box<Editor> {
        let mut e = Box::new(Editor::new());
        assert!(e.load(None, text.as_bytes()));
        e
    }
    #[test]
    fn typing_and_deleting_keeps_utf8_boundaries() {
        let mut e = editor("");
        for ch in "Привет, мир".chars() {
            e.insert(ch);
        }
        e.backspace();
        e.left();
        e.left();
        e.insert('Ж');
        assert_eq!(e.text(), "Привет, Жми");
        e.home();
        e.delete();
        assert_eq!(e.text(), "ривет, Жми");
        assert!(e.dirty);
        assert!(!e.load(None, &[0xff]));
    }
    #[test]
    fn vertical_movement_keeps_the_preferred_column() {
        let mut e = editor("длинная строка\nок\nещё одна строка");
        for _ in 0..9 {
            e.right();
        }
        e.down();
        assert_eq!(e.position(), (1, 2));
        e.down();
        assert_eq!(e.position(), (2, 9));
        e.up();
        e.up();
        assert_eq!(e.position(), (0, 9));
        e.end();
        assert_eq!(e.position(), (0, 14));
        assert_eq!(e.lines(), 3);
        assert_eq!(e.line(1), Some("ок"));
        e.page(10, true);
        assert_eq!(e.position().0, 2);
        e.follow(2);
        assert_eq!(e.top, 1);
    }
    #[test]
    fn the_buffer_never_overflows() {
        let mut e = editor("");
        let mut typed = 0;
        while e.insert('ж') {
            typed += 1;
        }
        assert_eq!(typed, EDIT_BYTES / 2);
        assert!(!e.insert('a'));
        assert!(!e.insert('\u{7}'));
    }
}
