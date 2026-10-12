//! Terminal model: scrollback, one input line and history. Commands are run by
//! the desktop service, which prints their output back here.

pub const LINES: usize = 400;
pub const WIDTH: usize = 96;
const LINE_BYTES: usize = WIDTH * 2;
pub const INPUT_BYTES: usize = 192;
const HISTORY: usize = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Shell,
    /// Lines go to Izzop instead of the command interpreter.
    Izzop,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Normal,
    Prompt,
    Error,
    Izzop,
}

#[derive(Clone, Copy)]
struct Line {
    bytes: [u8; LINE_BYTES],
    len: u8,
    tone: Tone,
}
const EMPTY: Line = Line {
    bytes: [0; LINE_BYTES],
    len: 0,
    tone: Tone::Normal,
};

pub struct Terminal {
    lines: [Line; LINES],
    first: usize,
    count: usize,
    input: [u8; INPUT_BYTES],
    input_len: usize,
    history: [[u8; INPUT_BYTES]; HISTORY],
    history_len: [usize; HISTORY],
    history_count: usize,
    browsing: Option<usize>,
    /// Lines scrolled back from the bottom.
    pub scroll: usize,
    pub mode: Mode,
    pub prompt: [u8; 128],
    pub prompt_len: usize,
    pending: Option<(Mode, usize)>,
    pending_line: [u8; INPUT_BYTES],
    pub started: bool,
}

fn utf8_prefix(text: &str, max: usize) -> &str {
    let mut end = text.len().min(max);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

impl Terminal {
    pub const fn new() -> Self {
        Self {
            lines: [EMPTY; LINES],
            first: 0,
            count: 0,
            input: [0; INPUT_BYTES],
            input_len: 0,
            history: [[0; INPUT_BYTES]; HISTORY],
            history_len: [0; HISTORY],
            history_count: 0,
            browsing: None,
            scroll: 0,
            mode: Mode::Shell,
            prompt: [0; 128],
            prompt_len: 0,
            pending: None,
            pending_line: [0; INPUT_BYTES],
            started: false,
        }
    }
    pub fn count(&self) -> usize {
        self.count
    }
    /// Line `n` counted from the oldest kept line.
    pub fn line(&self, n: usize) -> Option<(&str, Tone)> {
        (n < self.count).then(|| {
            let line = &self.lines[(self.first + n) % LINES];
            (
                core::str::from_utf8(&line.bytes[..line.len as usize]).unwrap_or(""),
                line.tone,
            )
        })
    }
    fn push_line(&mut self, text: &str, tone: Tone) {
        let index = if self.count < LINES {
            self.count += 1;
            (self.first + self.count - 1) % LINES
        } else {
            let index = self.first;
            self.first = (self.first + 1) % LINES;
            index
        };
        let line = &mut self.lines[index];
        line.bytes[..text.len()].copy_from_slice(text.as_bytes());
        line.len = text.len() as u8;
        line.tone = tone;
    }
    /// Appends text, splitting on newlines and wrapping long lines by characters.
    pub fn print(&mut self, text: &str, tone: Tone) {
        for raw in text.split('\n') {
            let mut rest = raw;
            loop {
                let mut chars = 0;
                let mut end = 0;
                for (i, ch) in rest.char_indices() {
                    if chars == WIDTH {
                        break;
                    }
                    chars += 1;
                    end = i + ch.len_utf8();
                }
                let piece = utf8_prefix(&rest[..end], LINE_BYTES);
                self.push_line(piece, tone);
                rest = &rest[piece.len()..];
                if rest.is_empty() {
                    break;
                }
            }
        }
        self.scroll = 0;
    }
    pub fn clear(&mut self) {
        self.count = 0;
        self.first = 0;
        self.scroll = 0;
    }
    pub fn input(&self) -> &str {
        core::str::from_utf8(&self.input[..self.input_len]).unwrap_or("")
    }
    pub fn prompt(&self) -> &str {
        core::str::from_utf8(&self.prompt[..self.prompt_len]).unwrap_or("")
    }
    pub fn set_prompt(&mut self, text: &str) {
        let text = utf8_prefix(text, self.prompt.len());
        self.prompt[..text.len()].copy_from_slice(text.as_bytes());
        self.prompt_len = text.len();
    }
    fn set_input(&mut self, text: &str) {
        let text = utf8_prefix(text, INPUT_BYTES);
        self.input[..text.len()].copy_from_slice(text.as_bytes());
        self.input_len = text.len();
    }
    pub fn type_char(&mut self, ch: char) {
        let mut buffer = [0; 4];
        let encoded = ch.encode_utf8(&mut buffer).as_bytes();
        if (ch as u32) >= 0x20 && self.input_len + encoded.len() <= INPUT_BYTES {
            self.input[self.input_len..self.input_len + encoded.len()].copy_from_slice(encoded);
            self.input_len += encoded.len();
            self.scroll = 0;
        }
    }
    pub fn backspace(&mut self) {
        if let Some(ch) = self.input().chars().last() {
            self.input_len -= ch.len_utf8();
        }
    }
    /// Echoes the input line and queues it for the service.
    pub fn enter(&mut self) {
        let line = self.input_len;
        let mut echo = [0u8; 128 + INPUT_BYTES];
        let prompt = self.prompt_len;
        echo[..prompt].copy_from_slice(&self.prompt[..prompt]);
        echo[prompt..prompt + line].copy_from_slice(&self.input[..line]);
        let echo_text = core::str::from_utf8(&echo[..prompt + line]).unwrap_or("");
        let mut copy = [0u8; 128 + INPUT_BYTES];
        copy[..echo_text.len()].copy_from_slice(echo_text.as_bytes());
        let copy = core::str::from_utf8(&copy[..echo_text.len()]).unwrap_or("");
        self.print(copy, Tone::Prompt);
        if line > 0 {
            let slot = self.history_count % HISTORY;
            self.history[slot][..line].copy_from_slice(&self.input[..line]);
            self.history_len[slot] = line;
            self.history_count += 1;
        }
        self.pending_line[..line].copy_from_slice(&self.input[..line]);
        self.pending = Some((self.mode, line));
        self.input_len = 0;
        self.browsing = None;
    }
    pub fn take_command(&mut self) -> Option<(Mode, &str)> {
        let (mode, len) = self.pending.take()?;
        Some((
            mode,
            core::str::from_utf8(&self.pending_line[..len]).unwrap_or(""),
        ))
    }
    pub fn history(&mut self, older: bool) {
        let available = self.history_count.min(HISTORY);
        if available == 0 {
            return;
        }
        let next = match (self.browsing, older) {
            (None, true) => Some(0),
            (None, false) => None,
            (Some(n), true) => Some((n + 1).min(available - 1)),
            (Some(0), false) => None,
            (Some(n), false) => Some(n - 1),
        };
        self.browsing = next;
        match next {
            Some(n) => {
                let slot = (self.history_count - 1 - n) % HISTORY;
                let mut copy = [0u8; INPUT_BYTES];
                let len = self.history_len[slot];
                copy[..len].copy_from_slice(&self.history[slot][..len]);
                self.set_input(core::str::from_utf8(&copy[..len]).unwrap_or(""));
            }
            None => self.input_len = 0,
        }
    }
    pub fn scroll_by(&mut self, lines: i32, visible: usize) {
        let max = self.count.saturating_sub(visible);
        self.scroll = (self.scroll as i32 + lines).clamp(0, max as i32) as usize;
    }
}
impl Default for Terminal {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    extern crate std;
    use std::{boxed::Box, string::String};
    #[test]
    fn printing_wraps_and_keeps_the_newest_lines() {
        let mut t = Box::new(Terminal::new());
        t.print(&"ж".repeat(WIDTH + 3), Tone::Normal);
        assert_eq!(t.count(), 2);
        assert_eq!(t.line(1).unwrap().0, "жжж");
        for n in 0..LINES + 5 {
            t.print(&std::format!("{n}"), Tone::Normal);
        }
        assert_eq!(t.count(), LINES);
        assert_eq!(t.line(LINES - 1).unwrap().0, std::format!("{}", LINES + 4));
    }
    #[test]
    fn enter_echoes_queues_and_remembers_history() {
        let mut t = Box::new(Terminal::new());
        t.set_prompt("hpOS:/$ ");
        for ch in "ls Документы".chars() {
            t.type_char(ch);
        }
        t.backspace();
        t.enter();
        assert_eq!(t.line(0).unwrap(), ("hpOS:/$ ls Документ", Tone::Prompt));
        assert_eq!(t.take_command(), Some((Mode::Shell, "ls Документ")));
        assert_eq!(t.take_command(), None);
        t.type_char('x');
        t.enter();
        t.history(true);
        assert_eq!(t.input(), "x");
        t.history(true);
        assert_eq!(t.input(), "ls Документ");
        t.history(false);
        t.history(false);
        assert_eq!(t.input(), "");
        let long: String = "я".repeat(200);
        for ch in long.chars() {
            t.type_char(ch);
        }
        assert_eq!(t.input().len(), INPUT_BYTES);
    }
}
