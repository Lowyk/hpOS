//! Izzop: a deliberately dumb offline chatter. It is a word-level n-gram
//! generator over a token stream: the next word is drawn from what followed
//! the last one to three words somewhere in the training messages, falling
//! back to shorter contexts. No neural network, no understanding.
//!
//! Model file: "IZZ1", u32 vocabulary size, u32 token count, u32 word-byte
//! count, (vocabulary + 1) u32 word offsets, word bytes, u16 tokens.
//! Token 0 is the end of a message.
#![no_std]

pub const MAGIC: [u8; 4] = *b"IZZ1";
pub const END: u16 = 0;
const MAX_WORDS: usize = 32;

/// Characters the hpOS fonts can draw; anything else separates words.
pub fn drawable(c: char) -> bool {
    matches!(c as u32, 0x21..=0x7e | 0x400..=0x45f)
}
fn punctuation(c: char) -> bool {
    matches!(c, '.' | ',' | '!' | '?' | ';' | ':' | '(' | ')' | '"')
}

/// Splits text into words and runs of punctuation.
pub fn tokenize(text: &str, mut emit: impl FnMut(&str)) {
    let mut start = None;
    let mut kind = false;
    for (i, c) in text.char_indices() {
        let part = drawable(c);
        let punct = part && punctuation(c);
        match start {
            Some(s) if !part || punct != kind || (punct && matches!(c, '(' | ')' | '"')) => {
                emit(&text[s..i]);
                start = part.then_some(i);
            }
            None if part => start = Some(i),
            _ => {}
        }
        kind = punct;
    }
    if let Some(s) = start {
        emit(&text[s..]);
    }
}

pub struct Rng(u64);
impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed ^ 0x9e37_79b9_7f4a_7c15 | 1)
    }
    pub fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }
    pub fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

pub struct Model<'a> {
    offsets: &'a [u8],
    words: &'a [u8],
    tokens: &'a [u8],
    vocabulary: usize,
}

fn word32(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

impl<'a> Model<'a> {
    pub fn parse(bytes: &'a [u8]) -> Option<Self> {
        if bytes.get(..4)? != MAGIC {
            return None;
        }
        let vocabulary = word32(bytes, 4)? as usize;
        let count = word32(bytes, 8)? as usize;
        let word_bytes = word32(bytes, 12)? as usize;
        if vocabulary == 0 || vocabulary > 65536 {
            return None;
        }
        let offsets_end = 16usize.checked_add((vocabulary + 1).checked_mul(4)?)?;
        let words_end = offsets_end.checked_add(word_bytes)?;
        let tokens_end = words_end.checked_add(count.checked_mul(2)?)?;
        let model = Self {
            offsets: bytes.get(16..offsets_end)?,
            words: bytes.get(offsets_end..words_end)?,
            tokens: bytes.get(words_end..tokens_end)?,
            vocabulary,
        };
        // Validate once so lookups never fail later.
        let mut previous = 0;
        for i in 0..=vocabulary {
            let offset = word32(model.offsets, i * 4)? as usize;
            if offset < previous || offset > word_bytes {
                return None;
            }
            previous = offset;
        }
        core::str::from_utf8(model.words).ok()?;
        for i in 0..vocabulary {
            core::str::from_utf8(model.word_bytes(i as u16)).ok()?;
        }
        (0..count)
            .all(|i| (model.token(i) as usize) < vocabulary)
            .then_some(model)
    }
    pub fn len(&self) -> usize {
        self.tokens.len() / 2
    }
    pub fn is_empty(&self) -> bool {
        self.tokens.is_empty()
    }
    fn token(&self, i: usize) -> u16 {
        u16::from_le_bytes([self.tokens[i * 2], self.tokens[i * 2 + 1]])
    }
    fn word_bytes(&self, token: u16) -> &'a [u8] {
        let i = token as usize * 4;
        let start = u32::from_le_bytes(self.offsets[i..i + 4].try_into().unwrap()) as usize;
        let end = u32::from_le_bytes(self.offsets[i + 4..i + 8].try_into().unwrap()) as usize;
        &self.words[start..end]
    }
    pub fn word(&self, token: u16) -> &'a str {
        core::str::from_utf8(self.word_bytes(token)).unwrap_or("")
    }
    fn find_word(&self, word: &str) -> Option<u16> {
        (1..self.vocabulary as u16).find(|t| {
            let mut a = self.word(*t).chars().flat_map(char::to_lowercase);
            let mut b = word.chars().flat_map(char::to_lowercase);
            loop {
                match (a.next(), b.next()) {
                    (None, None) => return true,
                    (x, y) if x == y => continue,
                    _ => return false,
                }
            }
        })
    }
    /// A random position holding `token`, or a random message start.
    fn position(&self, token: Option<u16>, rng: &mut Rng) -> Option<usize> {
        let mut seen = 0;
        let mut chosen = None;
        for i in 0..self.len() {
            let hit = match token {
                Some(t) => self.token(i) == t,
                None => self.token(i) != END && (i == 0 || self.token(i - 1) == END),
            };
            if hit {
                seen += 1;
                if rng.below(seen) == 0 {
                    chosen = Some(i);
                }
            }
        }
        chosen
    }
    /// Draws a continuation of the last `k` words of `context`.
    fn next(&self, context: &[u16], k: usize, rng: &mut Rng) -> (usize, u16) {
        let tail = &context[context.len() - k..];
        let mut seen = 0;
        let mut chosen = END;
        for i in k..self.len() {
            if (0..k).all(|j| self.token(i - k + j) == tail[j]) {
                seen += 1;
                if rng.below(seen) == 0 {
                    chosen = self.token(i);
                }
            }
        }
        (seen, chosen)
    }

    /// Known words of the prompt (letters only, two characters or more), deduplicated.
    fn prompt_words(&self, prompt: &str) -> ([u16; 8], usize) {
        let mut known = [0u16; 8];
        let mut found = 0;
        tokenize(prompt, |w| {
            if found < known.len() && w.chars().filter(|c| c.is_alphabetic()).count() >= 2 {
                if let Some(t) = self.find_word(w) {
                    if !known[..found].contains(&t) {
                        known[found] = t;
                        found += 1;
                    }
                }
            }
        });
        (known, found)
    }
    /// Calls `visit(start, end)` for every message in the token stream.
    fn messages(&self, mut visit: impl FnMut(usize, usize)) {
        let mut start = 0;
        for i in 0..self.len() {
            if self.token(i) == END {
                if i > start {
                    visit(start, i);
                }
                start = i + 1;
            }
        }
        if start < self.len() {
            visit(start, self.len());
        }
    }

    /// Writes a reply to `prompt` into `out` and returns its length in bytes.
    ///
    /// First Izzop looks for the message most like the prompt (rare shared
    /// words count more) and answers with the message that followed it in the
    /// chat. Sometimes he cuts that answer short and babbles on from there.
    /// With nothing to go on he babbles from the start.
    pub fn reply(&self, prompt: &str, rng: &mut Rng, out: &mut [u8]) -> usize {
        let (known, found) = self.prompt_words(prompt);
        let mut words = [END; MAX_WORDS];
        let mut n = 0;
        if let Some((start, end)) = self.recall(&known[..found], rng) {
            for i in start..end.min(start + MAX_WORDS) {
                words[n] = self.token(i);
                n += 1;
            }
            // One answer in four goes off the rails after a few real words.
            if n >= 5 && rng.below(4) == 0 {
                n = 2 + rng.below(n - 3);
                n = self.babble(&mut words, n, rng);
            }
            return render(self, &words[..n], out);
        }
        let seed = (found > 0).then(|| known[rng.below(found)]);
        let Some(start) = self
            .position(seed, rng)
            .or_else(|| self.position(None, rng))
        else {
            return 0;
        };
        words[0] = self.token(start);
        n = self.babble(&mut words, 1, rng);
        render(self, &words[..n], out)
    }

    /// The reply that followed the message best matching `known`, as a token range.
    fn recall(&self, known: &[u16], rng: &mut Rng) -> Option<(usize, usize)> {
        if known.is_empty() {
            return None;
        }
        let present = |start: usize, end: usize| -> u32 {
            let mut mask = 0;
            for i in start..end {
                if let Some(k) = known.iter().position(|t| *t == self.token(i)) {
                    mask |= 1 << k;
                }
            }
            mask
        };
        // How many messages contain each prompt word: rare words weigh more.
        let mut frequency = [0u32; 8];
        let mut total = 0u32;
        self.messages(|start, end| {
            total += 1;
            let mask = present(start, end);
            for (k, f) in frequency.iter_mut().enumerate().take(known.len()) {
                if mask & 1 << k != 0 {
                    *f += 1;
                }
            }
        });
        let score = |mask: u32| -> u32 {
            (0..known.len())
                .filter(|k| mask & 1 << k != 0)
                .map(|k| 1000 * total / frequency[k].max(1))
                .sum()
        };
        let mut best = 0;
        self.messages(|start, end| best = best.max(score(present(start, end))));
        if best == 0 {
            return None;
        }
        // Pick among close matches whose following message exists.
        let mut previous: Option<u32> = None;
        let mut seen = 0;
        let mut chosen = None;
        self.messages(|start, end| {
            if let Some(matched) = previous {
                if matched * 4 >= best * 3 {
                    seen += 1;
                    if rng.below(seen) == 0 {
                        chosen = Some((start, end));
                    }
                }
            }
            let s = score(present(start, end));
            previous = (s > 0).then_some(s);
        });
        chosen
    }

    /// Extends `words[..n]` with n-gram continuations; returns the new length.
    fn babble(&self, words: &mut [u16; MAX_WORDS], mut n: usize, rng: &mut Rng) -> usize {
        while n < MAX_WORDS {
            let longest = n.min(if rng.below(3) == 0 { 3 } else { 2 });
            let mut chosen = None;
            for k in (1..=longest).rev() {
                let (seen, token) = self.next(&words[..n], k, rng);
                // A single continuation of a long context would just quote a message.
                if seen > 1 || (seen == 1 && (k == 1 || rng.below(2) == 0)) {
                    chosen = Some(token);
                    break;
                }
            }
            match chosen {
                Some(END) | None if n >= 4 => break,
                Some(END) | None => {
                    let Some(restart) = self.position(None, rng) else {
                        break;
                    };
                    words[n] = self.token(restart);
                }
                Some(token) => words[n] = token,
            }
            n += 1;
        }
        n
    }
}

fn render(model: &Model, words: &[u16], out: &mut [u8]) -> usize {
    let mut len = 0;
    let mut previous = "";
    for token in words {
        let word = model.word(*token);
        if word.is_empty() {
            continue;
        }
        let first = word.chars().next().unwrap_or(' ');
        let glue =
            len == 0 || matches!(first, '.' | ',' | '!' | '?' | ';' | ':' | ')') || previous == "(";
        let needed = word.len() + usize::from(!glue);
        if len + needed > out.len() {
            break;
        }
        if !glue {
            out[len] = b' ';
            len += 1;
        }
        out[len..len + word.len()].copy_from_slice(word.as_bytes());
        len += word.len();
        previous = word;
    }
    len
}

/// Host-side model builder shared by the trainer and the tests.
#[cfg(any(test, feature = "std"))]
pub mod build {
    extern crate std;
    use super::*;
    use std::{collections::HashMap, string::String, vec, vec::Vec};

    pub fn train(corpus: &str) -> Vec<u8> {
        let mut vocabulary: Vec<String> = vec![String::new()];
        let mut index: HashMap<String, u16> = HashMap::new();
        let mut tokens: Vec<u16> = Vec::new();
        for line in corpus.lines() {
            let before = tokens.len();
            tokenize(line, |w| {
                if vocabulary.len() >= 65535 && !index.contains_key(w) {
                    return;
                }
                let id = *index.entry(w.into()).or_insert_with(|| {
                    vocabulary.push(w.into());
                    (vocabulary.len() - 1) as u16
                });
                tokens.push(id);
            });
            if tokens.len() > before {
                tokens.push(END);
            }
        }
        let mut out = Vec::new();
        out.extend_from_slice(&MAGIC);
        let words: String = vocabulary.concat();
        out.extend_from_slice(&(vocabulary.len() as u32).to_le_bytes());
        out.extend_from_slice(&(tokens.len() as u32).to_le_bytes());
        out.extend_from_slice(&(words.len() as u32).to_le_bytes());
        let mut offset = 0u32;
        for word in &vocabulary {
            out.extend_from_slice(&offset.to_le_bytes());
            offset += word.len() as u32;
        }
        out.extend_from_slice(&offset.to_le_bytes());
        out.extend_from_slice(words.as_bytes());
        for t in tokens {
            out.extend_from_slice(&t.to_le_bytes());
        }
        out
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use std::{string::String, vec::Vec};

    const CORPUS: &str = "пельмень был назначен директором дождя\n\
        директор дождя ушёл пить чай\n\
        чай был холодный, как вторник!\n\
        вторник подозрительно изучил пельмень 🥟\n";

    #[test]
    fn tokenizer_separates_punctuation_and_drops_undrawable_characters() {
        let mut parts = Vec::new();
        tokenize("Привет,мир!!! (ну) да🥟нет", |w| {
            parts.push(String::from(w))
        });
        assert_eq!(
            parts,
            ["Привет", ",", "мир", "!!!", "(", "ну", ")", "да", "нет"]
        );
    }
    #[test]
    fn replies_answer_with_what_followed_the_closest_message() {
        let bytes = build::train(CORPUS);
        let model = Model::parse(&bytes).unwrap();
        let mut rng = Rng::new(7);
        let mut out = [0u8; 256];
        let follow = "вторник подозрительно изучил пельмень";
        let mut exact = 0;
        for _ in 0..100 {
            // «холодный» appears only in message 3, so message 4 answers it.
            let n = model.reply("а почему ХОЛОДНЫЙ?", &mut rng, &mut out);
            let text = core::str::from_utf8(&out[..n]).unwrap();
            assert!(n > 0);
            if text == follow {
                exact += 1;
            } else {
                // A remixed answer still starts with the real reply.
                assert!(text.starts_with("вторник"), "{text}");
            }
            for word in text.split(' ') {
                assert!(
                    CORPUS.contains(word.trim_matches(|c: char| ",.!?".contains(c))),
                    "{word}"
                );
            }
        }
        assert!(exact > 50, "most answers are the real reply: {exact}");
        let n = model.reply("ничего знакомого", &mut rng, &mut out);
        assert!(n > 0, "unknown words fall back to babbling");
    }
    #[test]
    fn rare_words_outweigh_common_ones() {
        let corpus = "я люблю чай\nответ про чай\nя люблю пельмени\nответ про пельмени\nя люблю дождь\nответ про дождь\n";
        let bytes = build::train(corpus);
        let model = Model::parse(&bytes).unwrap();
        let mut out = [0u8; 128];
        let mut rng = Rng::new(3);
        for _ in 0..30 {
            let n = model.reply("я люблю пельмени", &mut rng, &mut out);
            assert_eq!(
                core::str::from_utf8(&out[..n]).unwrap(),
                "ответ про пельмени"
            );
        }
    }
    #[test]
    fn damaged_models_are_rejected_and_output_never_overflows() {
        let bytes = build::train(CORPUS);
        assert!(Model::parse(&bytes[..bytes.len() - 1]).is_none());
        let mut bad = bytes.clone();
        bad[0] = b'X';
        assert!(Model::parse(&bad).is_none());
        let model = Model::parse(&bytes).unwrap();
        let mut tiny = [0u8; 9];
        let n = model.reply("чай", &mut Rng::new(1), &mut tiny);
        assert!(core::str::from_utf8(&tiny[..n]).is_ok());
    }
}
