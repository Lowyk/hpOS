#![no_std]
#[cfg(test)]
extern crate std;
pub mod browser;
pub mod canvas;
pub mod editor;
pub mod files;
pub mod font;
pub mod lang;
pub mod pointer;
pub mod terminal;
pub mod view;
pub mod web;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    System,
    Help,
    Settings,
    Files,
    Menu,
    Up,
    Down,
    Left,
    Right,
    Enter,
    Escape,
    Tab,
    Backspace,
    Delete,
    Home,
    End,
    PageUp,
    PageDown,
    Terminal,
    Editor,
    Browser,
    Network,
    /// Ctrl with a letter or digit key, identified by scan code.
    Ctrl {
        scan: u8,
    },
    /// A printable character in the active layout; `scan` identifies the physical key.
    Text {
        ch: char,
        scan: u8,
        repeat: bool,
    },
    Layout {
        russian: bool,
    },
}
const SIMPLE: [Key; 22] = [
    Key::System,
    Key::Help,
    Key::Settings,
    Key::Files,
    Key::Menu,
    Key::Up,
    Key::Down,
    Key::Left,
    Key::Right,
    Key::Enter,
    Key::Escape,
    Key::Tab,
    Key::Backspace,
    Key::Delete,
    Key::Home,
    Key::End,
    Key::PageUp,
    Key::PageDown,
    Key::Terminal,
    Key::Editor,
    Key::Browser,
    Key::Network,
];
impl Key {
    /// IPC word; bit 63 stays clear because it marks pointer packets.
    pub fn encode(self) -> u64 {
        match self {
            Key::Text { ch, scan, repeat } => {
                (1 << 40) | ((repeat as u64) << 39) | ((scan as u64) << 24) | ch as u64
            }
            Key::Layout { russian } => (2 << 40) | russian as u64,
            Key::Ctrl { scan } => (3 << 40) | scan as u64,
            simple => SIMPLE.iter().position(|k| *k == simple).unwrap_or(0) as u64 + 1,
        }
    }
    pub fn decode(value: u64) -> Option<Self> {
        match value >> 40 {
            0 => SIMPLE.get((value as usize).checked_sub(1)?).copied(),
            1 if value & 0x7f_0000_0000 == 0 && value & 0xff_ffff > 0 => Some(Key::Text {
                ch: char::from_u32((value & 0xff_ffff) as u32)?,
                scan: (value >> 24) as u8,
                repeat: value >> 39 & 1 != 0,
            }),
            2 if value & !(2 << 40) <= 1 => Some(Key::Layout {
                russian: value & 1 != 0,
            }),
            3 if value & !(3 << 40) <= 0x7f => Some(Key::Ctrl { scan: value as u8 }),
            _ => None,
        }
    }
}

/// Scan code set 1 keys 0x02..=0x35 (and space) for the US and Russian layouts.
const FIRST: u8 = 0x02;
const US: [&str; 2] = [
    "1234567890-=\0\0qwertyuiop[]\0\0asdfghjkl;'`\0\\zxcvbnm,./",
    "!@#$%^&*()_+\0\0QWERTYUIOP{}\0\0ASDFGHJKL:\"~\0|ZXCVBNM<>?",
];
const RU: [&str; 2] = [
    "1234567890-=\0\0йцукенгшщзхъ\0\0фывапролджэё\0\\ячсмитьбю.",
    "!\"#;%:?*()_+\0\0ЙЦУКЕНГШЩЗХЪ\0\0ФЫВАПРОЛДЖЭЁ\0/ЯЧСМИТЬБЮ,",
];
fn layout_char(table: &str, scan: u8) -> Option<char> {
    let ch = table.chars().nth(scan.checked_sub(FIRST)? as usize)?;
    (ch != '\0').then_some(ch)
}

pub struct Decoder {
    pressed: [bool; 256],
    extended: bool,
    pause: u8,
    shift: u8,
    ctrl: bool,
    alt: bool,
    caps: bool,
    russian: bool,
}
impl Decoder {
    pub const fn new() -> Self {
        Self {
            pressed: [false; 256],
            extended: false,
            pause: 0,
            shift: 0,
            ctrl: false,
            alt: false,
            caps: false,
            russian: false,
        }
    }
    pub fn feed(&mut self, byte: u8) -> Option<Key> {
        if self.pause != 0 {
            self.pause -= 1;
            return None;
        }
        if byte == 0xe1 {
            self.pause = 5;
            self.extended = false;
            return None;
        }
        if byte == 0xe0 {
            self.extended = true;
            return None;
        }
        let extended = self.extended;
        self.extended = false;
        let scan = byte & 0x7f;
        let index = scan as usize + if extended { 128 } else { 0 };
        let released = byte & 0x80 != 0;
        let repeat = !released && self.pressed[index];
        self.pressed[index] = !released;
        match (extended, scan) {
            (false, 0x2a) | (false, 0x36) => {
                let bit = if scan == 0x2a { 1 } else { 2 };
                if released {
                    self.shift &= !bit;
                    return None;
                }
                let was = self.shift;
                self.shift |= bit;
                return (self.alt && was == 0 && !repeat).then(|| self.toggle_layout());
            }
            (_, 0x1d) => {
                self.ctrl = !released;
                return None;
            }
            (_, 0x38) => {
                let was = self.alt;
                self.alt = !released;
                return (!released && !was && self.shift != 0).then(|| self.toggle_layout());
            }
            (false, 0x3a) => {
                if !released && !repeat {
                    self.caps = !self.caps;
                }
                return None;
            }
            _ => {}
        }
        if released {
            return None;
        }
        // Editing keys auto-repeat; commands fire once per press.
        let once = |key| (!repeat).then_some(key);
        match (extended, scan) {
            (_, 0x48) => Some(Key::Up),
            (_, 0x50) => Some(Key::Down),
            (_, 0x4b) => Some(Key::Left),
            (_, 0x4d) => Some(Key::Right),
            (false, 0x0e) => Some(Key::Backspace),
            (true, 0x53) => Some(Key::Delete),
            (_, 0x47) => Some(Key::Home),
            (_, 0x4f) => Some(Key::End),
            (_, 0x49) => Some(Key::PageUp),
            (_, 0x51) => Some(Key::PageDown),
            (false, 0x3f) => once(Key::Terminal),
            (false, 0x40) => once(Key::Editor),
            (false, 0x41) => once(Key::Browser),
            (false, 0x42) => once(Key::Network),
            (false, 0x3b) => once(Key::System),
            (false, 0x3c) => once(Key::Help),
            (false, 0x3d) => once(Key::Settings),
            (false, 0x3e) => once(Key::Files),
            (true, 0x5b) | (true, 0x5c) => once(Key::Menu),
            (_, 0x1c) => once(Key::Enter),
            (false, 0x01) => once(Key::Escape),
            (false, 0x0f) => once(Key::Tab),
            _ => self.text(extended, scan, repeat),
        }
    }
    /// After lost bytes: forget held keys but keep the layout and Caps Lock.
    pub fn reset(&mut self) {
        *self = Self {
            russian: self.russian,
            caps: self.caps,
            ..Self::new()
        };
    }
    fn toggle_layout(&mut self) -> Key {
        self.russian = !self.russian;
        Key::Layout {
            russian: self.russian,
        }
    }
    fn text(&self, extended: bool, scan: u8, repeat: bool) -> Option<Key> {
        if self.ctrl && !self.alt && !extended && !repeat && layout_char(US[0], scan).is_some() {
            return Some(Key::Ctrl { scan });
        }
        if extended || self.ctrl || self.alt {
            return None;
        }
        let ch = if scan == 0x39 {
            ' '
        } else {
            let tables = if self.russian { RU } else { US };
            let shifted = self.shift != 0;
            let base = layout_char(tables[0], scan)?;
            if base.is_alphabetic() {
                let ch = layout_char(tables[usize::from(shifted)], scan)?;
                if self.caps { flip_case(ch) } else { ch }
            } else {
                layout_char(tables[usize::from(shifted)], scan)?
            }
        };
        Some(Key::Text { ch, scan, repeat })
    }
}
fn flip_case(ch: char) -> char {
    if ch.is_uppercase() {
        ch.to_lowercase().next().unwrap_or(ch)
    } else {
        ch.to_uppercase().next().unwrap_or(ch)
    }
}
impl Default for Decoder {
    fn default() -> Self {
        Self::new()
    }
}

pub mod model;
pub use model::{COUNT, Cursor, FILES, Rect, Shell, WINDOWS, Window, WindowState};
#[cfg(test)]
mod tests {
    use super::*;
    fn text(d: &mut Decoder, scan: u8) -> Option<char> {
        match d.feed(scan) {
            Some(Key::Text { ch, .. }) => Some(ch),
            _ => None,
        }
    }
    #[test]
    fn make_repeat_break_and_prefixes() {
        let mut d = Decoder::new();
        assert_eq!(
            d.feed(0x14),
            Some(Key::Text {
                ch: 't',
                scan: 0x14,
                repeat: false
            })
        );
        assert_eq!(
            d.feed(0x14),
            Some(Key::Text {
                ch: 't',
                scan: 0x14,
                repeat: true
            })
        );
        d.feed(0x94);
        assert_eq!(d.feed(0x3b), Some(Key::System));
        assert_eq!(d.feed(0x3b), None);
        for b in [0xe1, 0x1d, 0x45, 0xe1, 0x9d, 0xc5] {
            assert_eq!(d.feed(b), None);
        }
        d.feed(0xe0);
        assert_eq!(d.feed(0x48), Some(Key::Up));
        assert_eq!(Key::decode(u64::MAX), None);
    }
    #[test]
    fn layouts_shift_caps_and_alt_shift_switching() {
        let mut d = Decoder::new();
        assert_eq!(text(&mut d, 0x1e), Some('a'));
        d.feed(0x2a);
        assert_eq!(text(&mut d, 0x1e), Some('A'));
        assert_eq!(text(&mut d, 0x02), Some('!'));
        d.feed(0xaa);
        d.feed(0x38);
        assert_eq!(d.feed(0x2a), Some(Key::Layout { russian: true }));
        d.feed(0xaa);
        d.feed(0xb8);
        assert_eq!(text(&mut d, 0x1e), Some('ф'));
        assert_eq!(text(&mut d, 0x29), Some('ё'));
        assert_eq!(text(&mut d, 0x35), Some('.'));
        d.feed(0x3a);
        assert_eq!(text(&mut d, 0x10), Some('Й'));
        assert_eq!(text(&mut d, 0x02), Some('1'));
        d.feed(0x2a);
        assert_eq!(text(&mut d, 0x10), Some('й'));
        assert_eq!(text(&mut d, 0x35), Some(','));
        d.feed(0xaa);
        assert_eq!(text(&mut d, 0x39), Some(' '));
        d.feed(0x1d);
        assert_eq!(d.feed(0x1f), Some(Key::Ctrl { scan: 0x1f }));
        assert_eq!(d.feed(0x1f), None);
        d.feed(0x9d);
        d.feed(0xe0);
        assert_eq!(d.feed(0x47), Some(Key::Home));
    }
    #[test]
    fn every_key_round_trips_through_ipc_words() {
        let mut keys = SIMPLE.to_vec();
        keys.push(Key::Text {
            ch: 'Ж',
            scan: 0x27,
            repeat: true,
        });
        keys.push(Key::Text {
            ch: '~',
            scan: 0x29,
            repeat: false,
        });
        keys.push(Key::Layout { russian: true });
        keys.push(Key::Layout { russian: false });
        keys.push(Key::Ctrl { scan: 0x1f });
        for key in keys {
            let word = key.encode();
            assert_eq!(word >> 63, 0);
            assert_eq!(Key::decode(word), Some(key));
        }
        assert_eq!(Key::decode(0), None);
        assert_eq!(Key::decode(SIMPLE.len() as u64 + 1), None);
        assert_eq!(Key::decode(4 << 40), None);
    }
}
