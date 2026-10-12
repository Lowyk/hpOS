use crate::{Rect, font::Font};
use core::ptr::{read_volatile, write_volatile};

#[derive(Clone, Copy)]
pub struct Canvas {
    pixels: *mut u32,
    pub width: usize,
    pub height: usize,
    stride: usize,
    clip: Rect,
}
impl Canvas {
    /// Caller owns writable backing for stride * height pixels for this canvas lifetime.
    pub unsafe fn new(
        pixels: *mut u32,
        width: usize,
        height: usize,
        stride: usize,
    ) -> Option<Self> {
        if pixels.is_null()
            || width == 0
            || height == 0
            || stride < width
            || width > i32::MAX as usize
            || height > i32::MAX as usize
            || stride.checked_mul(height)?.checked_mul(4)? > isize::MAX as usize
        {
            return None;
        }
        Some(Self {
            pixels,
            width,
            height,
            stride,
            clip: Rect::new(0, 0, width as i32, height as i32),
        })
    }
    pub fn clipped(&self, rect: Rect) -> Self {
        Self {
            clip: self.clip.intersect(rect),
            ..*self
        }
    }
    pub fn fill(&self, rect: Rect, color: u32) {
        let r = self.clip.intersect(rect);
        for y in r.y..r.bottom() {
            for x in r.x..r.right() {
                unsafe {
                    write_volatile(
                        self.pixels.add(y as usize * self.stride + x as usize),
                        color,
                    );
                }
            }
        }
    }
    pub fn rect(&self, x: usize, y: usize, w: usize, h: usize, color: u32) {
        if x >= self.width || y >= self.height {
            return;
        }
        self.fill(
            Rect::new(
                x as i32,
                y as i32,
                w.min(self.width - x) as i32,
                h.min(self.height - y) as i32,
            ),
            color,
        );
    }
    pub fn blend(&self, x: i32, y: i32, color: u32, alpha: u8) {
        if alpha == 0 || !self.clip.contains(x, y) {
            return;
        }
        let ptr = unsafe { self.pixels.add(y as usize * self.stride + x as usize) };
        let old = unsafe { read_volatile(ptr) };
        let a = alpha.min(15) as u32;
        let mut out = 0;
        for shift in [0, 8, 16] {
            out |= ((((old >> shift) & 255) * (15 - a) + ((color >> shift) & 255) * a + 7) / 15)
                << shift;
        }
        unsafe {
            write_volatile(ptr, out);
        }
    }
    pub fn copy_from(&self, source: &Self, rect: Rect) {
        let r = self.clip.intersect(rect).intersect(Rect::new(
            0,
            0,
            source.width as i32,
            source.height as i32,
        ));
        for y in r.y..r.bottom() {
            for x in r.x..r.right() {
                unsafe {
                    write_volatile(
                        self.pixels.add(y as usize * self.stride + x as usize),
                        read_volatile(source.pixels.add(y as usize * source.stride + x as usize)),
                    );
                }
            }
        }
    }
    pub fn text(&self, x: usize, y: usize, text: &str, color: u32, scale: usize) {
        if scale == 0 {
            return;
        }
        let mut x = x as i32;
        for character in text.chars() {
            let index = crate::font::index(character);
            let glyph = crate::font::glyph(index);
            for dy in 0..22 * scale / 2 {
                for dx in 0..16 * scale / 2 {
                    let sx = dx * 2 / scale;
                    let sy = dy * 2 / scale;
                    let byte = glyph[sy * 8 + sx / 2];
                    let alpha = if sx % 2 == 0 { byte >> 4 } else { byte & 15 };
                    self.blend(x + dx as i32, y as i32 + dy as i32, color, alpha);
                }
            }
            x += (crate::font::advance(index) * scale / 2) as i32;
        }
    }
    /// Width in pixels of `text` drawn at scale 2.
    pub fn measure(text: &str) -> i32 {
        Self::measure_with(Font::Interface, text)
    }
    pub fn measure_with(font: Font, text: &str) -> i32 {
        text.chars()
            .map(|c| font.advance(crate::font::index(c)) as i32)
            .sum()
    }
    pub fn clip(&self) -> Rect {
        self.clip
    }
    /// Draws `text` at its native size; glyph cells outside the clip are skipped.
    pub fn label(&self, font: Font, x: i32, y: i32, text: &str, color: u32) {
        let (w, h) = font.cell();
        let mut x = x;
        for character in text.chars() {
            let index = crate::font::index(character);
            let cell = Rect::new(x, y, w as i32, h as i32);
            if self.clip.intersect(cell).w > 0 && self.clip.intersect(cell).h > 0 {
                let glyph = font.glyph(index);
                for dy in 0..h {
                    for dx in 0..w {
                        let byte = glyph[dy * w / 2 + dx / 2];
                        let alpha = if dx % 2 == 0 { byte >> 4 } else { byte & 15 };
                        self.blend(x + dx as i32, y + dy as i32, color, alpha);
                    }
                }
            }
            x += font.advance(index) as i32;
            if x >= self.clip.right() {
                break;
            }
        }
    }
    /// Solid colour mixed toward `color` by `alpha`/255, without reading pixels.
    pub fn mix(base: u32, color: u32, alpha: u32) -> u32 {
        let mut out = 0;
        for shift in [0, 8, 16] {
            let a = (base >> shift) & 255;
            let b = (color >> shift) & 255;
            out |= ((a * (255 - alpha) + b * alpha + 127) / 255) << shift;
        }
        out
    }
    /// Alpha-blends a rectangle (alpha 0..=15) over the existing pixels.
    pub fn shade(&self, rect: Rect, color: u32, alpha: u8) {
        let r = self.clip.intersect(rect);
        for y in r.y..r.bottom() {
            for x in r.x..r.right() {
                self.blend(x, y, color, alpha);
            }
        }
    }
    pub fn number(&self, x: usize, y: usize, mut value: u64, color: u32) {
        let mut digits = [b'0'; 20];
        let mut start = 20;
        loop {
            start -= 1;
            digits[start] = b'0' + (value % 10) as u8;
            value /= 10;
            if value == 0 {
                break;
            }
        }
        self.text(
            x,
            y,
            core::str::from_utf8(&digits[start..]).unwrap_or("?"),
            color,
            2,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn clipping_preserves_pitch_padding_and_surrounding_pixels() {
        let mut pixels = [0xabcdefu32; 24];
        let c = unsafe { Canvas::new(pixels.as_mut_ptr().add(4), 3, 3, 4).unwrap() };
        c.rect(2, 1, usize::MAX, usize::MAX, 7);
        assert_eq!(pixels[10], 7);
        assert_eq!(pixels[14], 7);
        for (i, value) in pixels.iter().enumerate() {
            if i != 10 && i != 14 {
                assert_eq!(*value, 0xabcdef);
            }
        }
        c.rect(100, 100, 4, 4, 8);
        assert!(unsafe { Canvas::new(pixels.as_mut_ptr(), 5, 3, 4) }.is_none());
    }
    #[test]
    fn blit_and_clip_leave_pixels_outside_damage_unchanged() {
        let mut from = [7u32; 12];
        let mut to = [0u32; 16];
        let source = unsafe { Canvas::new(from.as_mut_ptr(), 3, 3, 4).unwrap() };
        let target = unsafe { Canvas::new(to.as_mut_ptr(), 3, 3, 5).unwrap() };
        target
            .clipped(Rect::new(1, 1, 2, 1))
            .copy_from(&source, Rect::new(0, 0, 3, 3));
        assert_eq!(to[6], 7);
        assert_eq!(to[7], 7);
        for (i, v) in to.iter().enumerate() {
            if i != 6 && i != 7 {
                assert_eq!(*v, 0);
            }
        }
    }
}
