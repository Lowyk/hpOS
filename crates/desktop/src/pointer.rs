#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pointer {
    pub dx: i16,
    pub dy: i16,
    pub wheel: i8,
    pub buttons: u8,
}
impl Pointer {
    pub fn encode(self) -> u64 {
        (1 << 63)
            | self.dx as u16 as u64
            | ((self.dy as u16 as u64) << 16)
            | ((self.wheel as u8 as u64) << 32)
            | ((self.buttons as u64) << 40)
    }
    pub fn decode(word: u64) -> Option<Self> {
        if word >> 48 != 0x8000 || ((word >> 40) & 255) > 7 {
            return None;
        }
        Some(Self {
            dx: word as i16,
            dy: (word >> 16) as i16,
            wheel: (word >> 32) as i8,
            buttons: (word >> 40) as u8,
        })
    }
}
pub struct MouseDecoder {
    packet: [u8; 4],
    count: usize,
    id: u8,
}
impl MouseDecoder {
    pub fn new(id: u8) -> Self {
        Self {
            packet: [0; 4],
            count: 0,
            id,
        }
    }
    pub fn reset(&mut self) {
        self.count = 0;
    }
    pub fn feed(&mut self, byte: u8) -> Option<Pointer> {
        if self.count == 0 && byte & 8 == 0 {
            return None;
        }
        self.packet[self.count] = byte;
        self.count += 1;
        if self.count < if self.id == 3 || self.id == 4 { 4 } else { 3 } {
            return None;
        }
        self.count = 0;
        let flags = self.packet[0];
        let x = self.packet[1] as i16 - if flags & 0x10 != 0 { 256 } else { 0 };
        let y = self.packet[2] as i16 - if flags & 0x20 != 0 { 256 } else { 0 };
        Some(Pointer {
            dx: if flags & 0x40 != 0 { 0 } else { x },
            dy: if flags & 0x80 != 0 { 0 } else { -y },
            wheel: if self.id == 3 {
                self.packet[3] as i8
            } else if self.id == 4 {
                ((self.packet[3] << 4) as i8) >> 4
            } else {
                0
            },
            buttons: flags & 7,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn nine_bit_sign_and_screen_y_are_decoded() {
        let mut d = MouseDecoder::new(0);
        assert_eq!(d.feed(0), None);
        assert_eq!(d.feed(0x39), None);
        assert_eq!(d.feed(0xff), None);
        let p = d.feed(0xfe).unwrap();
        assert_eq!(
            p,
            Pointer {
                dx: -1,
                dy: 2,
                wheel: 0,
                buttons: 1
            }
        );
        assert_eq!(Pointer::decode(p.encode()), Some(p));
        assert_eq!(Pointer::decode(1), None);
    }
    #[test]
    fn overflow_preserves_buttons_and_wheel_packets_have_four_bytes() {
        let mut d = MouseDecoder::new(3);
        for b in [0xcb, 0x7f, 0x80] {
            assert_eq!(d.feed(b), None);
        }
        assert_eq!(
            d.feed(0xff),
            Some(Pointer {
                dx: 0,
                dy: 0,
                wheel: -1,
                buttons: 3
            })
        );
        let mut d = MouseDecoder::new(4);
        for b in [8, 0, 0] {
            d.feed(b);
        }
        assert_eq!(d.feed(0x3e).unwrap().wheel, -2);
    }
}
