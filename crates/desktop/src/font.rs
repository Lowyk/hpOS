//! Pre-rasterized Noto Sans atlases: ASCII and the Cyrillic block U+0400..U+045F,
//! 4-bit alpha, two pixels per byte, preceded by one advance byte per glyph.
const COUNT: usize = 191;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Font {
    /// 16 px regular: body text.
    Interface,
    /// 20 px semibold: window titles and headings.
    Title,
    /// 34 px bold: the system name and large numbers.
    Display,
}
static INTERFACE: &[u8; COUNT + COUNT * 16 * 22 / 2] = include_bytes!("../assets/interface.bin");
static TITLE: &[u8; COUNT + COUNT * 22 * 28 / 2] = include_bytes!("../assets/title.bin");
#[cfg(feature = "display-font")]
static DISPLAY: &[u8; COUNT + COUNT * 40 * 46 / 2] = include_bytes!("../assets/display.bin");

impl Font {
    fn atlas(self) -> (&'static [u8], usize, usize) {
        match self {
            Font::Interface => (INTERFACE, 16, 22),
            Font::Title => (TITLE, 22, 28),
            #[cfg(feature = "display-font")]
            Font::Display => (DISPLAY, 40, 46),
            #[cfg(not(feature = "display-font"))]
            Font::Display => (TITLE, 22, 28),
        }
    }
    pub fn cell(self) -> (usize, usize) {
        let (_, w, h) = self.atlas();
        (w, h)
    }
    pub fn advance(self, index: usize) -> usize {
        self.atlas().0[index] as usize
    }
    pub fn glyph(self, index: usize) -> &'static [u8] {
        let (data, w, h) = self.atlas();
        let size = w * h / 2;
        &data[COUNT + index * size..COUNT + (index + 1) * size]
    }
}

pub fn index(c: char) -> usize {
    // Typographic marks the atlas lacks are drawn as their plain forms.
    let c = match c {
        '«' | '»' | '“' | '”' | '„' => '"',
        '‘' | '’' => '\'',
        '–' | '—' | '−' => '-',
        '№' => 'N',
        '\u{a0}' => ' ',
        c => c,
    };
    let cp = c as usize;
    if (32..=126).contains(&cp) {
        cp - 32
    } else if (0x400..=0x45f).contains(&cp) {
        95 + cp - 0x400
    } else {
        '?' as usize - 32
    }
}
pub fn advance(index: usize) -> usize {
    Font::Interface.advance(index)
}
pub fn glyph(index: usize) -> &'static [u8] {
    Font::Interface.glyph(index)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn atlas_covers_cyrillic_and_keeps_capital_top_bar_above_stem() {
        for font in [Font::Interface, Font::Title, Font::Display] {
            let (w, h) = font.cell();
            assert_ne!(font.glyph(index('ё')), font.glyph(index('?')));
            assert_ne!(font.glyph(index('Я')), font.glyph(index('?')));
            let glyph = font.glyph(index('T'));
            let row = w / 2;
            let ink = |rows: &[u8]| -> u32 {
                rows.iter().map(|v| (v >> 4) as u32 + (v & 15) as u32).sum()
            };
            assert!(
                ink(&glyph[..h / 2 * row]) > ink(&glyph[h / 2 * row..]),
                "{font:?} atlas is vertically flipped"
            );
            assert!(font.advance(index(' ')) >= 3);
            assert!(font.advance(index('Ж')) <= w);
        }
        #[cfg(feature = "display-font")]
        assert!(Font::Display.advance(index('Ж')) > Font::Interface.advance(index('Ж')));
    }
}
