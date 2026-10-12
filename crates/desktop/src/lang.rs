//! Interface languages. Strings are written inline at the call site as
//! `[русский, English, українська]` and picked with [`pick`].

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lang {
    Ru = 0,
    En = 1,
    Uk = 2,
}
pub const LANGS: [Lang; 3] = [Lang::Ru, Lang::En, Lang::Uk];

impl Lang {
    pub fn code(self) -> &'static str {
        ["ru", "en", "uk"][self as usize]
    }
    pub fn from_code(code: &str) -> Option<Self> {
        LANGS.iter().copied().find(|l| l.code() == code)
    }
    /// Short button label in the language itself.
    pub fn short(self) -> &'static str {
        ["Рус", "Eng", "Укр"][self as usize]
    }
}

pub fn pick(lang: Lang, text: [&'static str; 3]) -> &'static str {
    text[lang as usize]
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn codes_round_trip() {
        for lang in LANGS {
            assert_eq!(Lang::from_code(lang.code()), Some(lang));
        }
        assert_eq!(Lang::from_code("de"), None);
        assert_eq!(pick(Lang::Uk, ["а", "b", "в"]), "в");
    }
}
