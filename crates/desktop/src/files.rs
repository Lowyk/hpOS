//! State of the «Файлы» window. The desktop service performs the actual
//! file-system requests and feeds results back through these methods.

pub const NAME_MAX: usize = 63;
pub const MAX_ENTRIES: usize = 128;
pub const MAX_DEPTH: usize = 12;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Name {
    bytes: [u8; NAME_MAX],
    len: u8,
}
impl Name {
    pub const EMPTY: Self = Self {
        bytes: [0; NAME_MAX],
        len: 0,
    };
    pub fn new(text: &str) -> Self {
        let mut name = Self::EMPTY;
        for ch in text.chars() {
            if !name.push(ch) {
                break;
            }
        }
        name
    }
    /// Raw UTF-8 from the file system; invalid input yields an empty name.
    pub fn from_bytes(bytes: &[u8]) -> Self {
        core::str::from_utf8(&bytes[..bytes.len().min(NAME_MAX)]).map_or(Self::EMPTY, Self::new)
    }
    pub fn as_str(&self) -> &str {
        core::str::from_utf8(&self.bytes[..self.len as usize]).unwrap_or("")
    }
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
    pub fn push(&mut self, ch: char) -> bool {
        let mut buffer = [0; 4];
        let encoded = ch.encode_utf8(&mut buffer).as_bytes();
        let len = self.len as usize;
        if ch == '/' || (ch as u32) < 0x20 || len + encoded.len() > NAME_MAX {
            return false;
        }
        self.bytes[len..len + encoded.len()].copy_from_slice(encoded);
        self.len += encoded.len() as u8;
        true
    }
    pub fn pop(&mut self) {
        if let Some(ch) = self.as_str().chars().last() {
            self.len -= ch.len_utf8() as u8;
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Entry {
    pub id: u16,
    pub directory: bool,
    pub size: u32,
    pub name: Name,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditMode {
    NewFolder,
    NewFile,
    Rename(u16),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Edit {
    pub mode: EditMode,
    pub text: Name,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileOp {
    Refresh,
    Open(u16, Name),
    /// Load a file into the editor.
    Edit(u16, Name),
    Up,
    CreateFolder(Name),
    CreateFile(Name),
    Rename(u16, Name),
    Delete(u16),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Storage {
    Unknown,
    Unavailable,
    Ready { free_kib: u64, total_kib: u64 },
}

pub struct Files {
    path: [(u16, Name); MAX_DEPTH],
    depth: usize,
    pub entries: [Entry; MAX_ENTRIES],
    pub count: usize,
    pub truncated: bool,
    pub selected: Option<usize>,
    pub scroll: usize,
    pub message: &'static str,
    pub storage: Storage,
    pub edit: Option<Edit>,
    pub confirm_delete: Option<u16>,
}
impl Files {
    pub const fn new() -> Self {
        Self {
            path: [(0, Name::EMPTY); MAX_DEPTH],
            depth: 1,
            entries: [Entry {
                id: 0,
                directory: false,
                size: 0,
                name: Name::EMPTY,
            }; MAX_ENTRIES],
            count: 0,
            truncated: false,
            selected: None,
            scroll: 0,
            message: "",
            storage: Storage::Unknown,
            edit: None,
            confirm_delete: None,
        }
    }
    pub fn directory(&self) -> u16 {
        self.path[self.depth - 1].0
    }
    pub fn parent(&self) -> Option<u16> {
        (self.depth > 1).then(|| self.path[self.depth - 2].0)
    }
    pub fn depth(&self) -> usize {
        self.depth
    }
    /// Path components below the root, outermost first.
    pub fn components(&self) -> impl Iterator<Item = &str> {
        self.path[1..self.depth]
            .iter()
            .map(|(_, name)| name.as_str())
    }
    pub fn can_enter(&self) -> bool {
        self.depth < MAX_DEPTH
    }
    pub fn enter(&mut self, id: u16, name: Name) {
        if self.can_enter() {
            self.path[self.depth] = (id, name);
            self.depth += 1;
        }
    }
    pub fn leave(&mut self) {
        if self.depth > 1 {
            self.depth -= 1;
        }
    }
    pub fn reset(&mut self) {
        self.depth = 1;
        self.count = 0;
        self.selected = None;
        self.scroll = 0;
    }
    /// Directories first, then files, each by name.
    pub fn set_entries(&mut self, entries: &[Entry], truncated: bool, keep: Option<u16>) {
        let previous = keep.or_else(|| self.selected_entry().map(|e| e.id));
        self.count = entries.len().min(MAX_ENTRIES);
        self.entries[..self.count].copy_from_slice(&entries[..self.count]);
        let list = &mut self.entries[..self.count];
        for i in 1..list.len() {
            let mut j = i;
            while j > 0 && Self::before(&list[j], &list[j - 1]) {
                list.swap(j, j - 1);
                j -= 1;
            }
        }
        self.truncated = truncated || entries.len() > MAX_ENTRIES;
        self.selected =
            previous.and_then(|id| self.entries[..self.count].iter().position(|e| e.id == id));
        self.scroll = self.scroll.min(self.count.saturating_sub(1));
        self.confirm_delete = None;
    }
    fn before(a: &Entry, b: &Entry) -> bool {
        if a.directory != b.directory {
            return a.directory;
        }
        let mut x = a.name.as_str().chars().flat_map(char::to_lowercase);
        let mut y = b.name.as_str().chars().flat_map(char::to_lowercase);
        loop {
            match (x.next(), y.next()) {
                (Some(p), Some(q)) if p == q => continue,
                (Some(p), Some(q)) => return p < q,
                (None, Some(_)) => return true,
                _ => return false,
            }
        }
    }
    pub fn selected_entry(&self) -> Option<&Entry> {
        self.selected
            .filter(|i| *i < self.count)
            .map(|i| &self.entries[i])
    }
    pub fn contains(&self, name: &str) -> bool {
        self.entries[..self.count]
            .iter()
            .any(|e| e.name.as_str() == name)
    }
    /// First free «base», «base 2», … name in the current listing.
    pub fn unique_name(&self, base: &str, extension: &str) -> Name {
        for n in 1..1000u32 {
            let mut name = Name::new(base);
            if n > 1 {
                name.push(' ');
                let mut digits = [0u8; 4];
                let mut len = 0;
                let mut v = n;
                while v > 0 {
                    digits[len] = b'0' + (v % 10) as u8;
                    v /= 10;
                    len += 1;
                }
                for d in digits[..len].iter().rev() {
                    name.push(*d as char);
                }
            }
            for ch in extension.chars() {
                name.push(ch);
            }
            if !self.contains(name.as_str()) {
                return name;
            }
        }
        Name::new(base)
    }
}
impl Default for Files {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn entry(id: u16, name: &str, directory: bool) -> Entry {
        Entry {
            id,
            directory,
            size: 0,
            name: Name::new(name),
        }
    }
    #[test]
    fn names_reject_separators_and_stop_at_the_byte_limit() {
        let mut name = Name::new("Документы");
        assert_eq!(name.as_str(), "Документы");
        assert!(!name.push('/'));
        name.pop();
        assert_eq!(name.as_str(), "Документ");
        let long = Name::new(&"ж".repeat(40));
        assert_eq!(long.as_str().chars().count(), 31);
        assert_eq!(Name::from_bytes(&[0xff, 0xfe]), Name::EMPTY);
    }
    #[test]
    fn listing_sorts_folders_first_and_keeps_selection_by_id() {
        let mut files = Files::new();
        files.set_entries(
            &[
                entry(3, "b.txt", false),
                entry(4, "Я", true),
                entry(5, "а", true),
                entry(6, "A.txt", false),
            ],
            false,
            None,
        );
        let names: [&str; 4] = core::array::from_fn(|i| files.entries[i].name.as_str());
        assert_eq!(names, ["а", "Я", "A.txt", "b.txt"]);
        files.selected = Some(3);
        files.set_entries(
            &[entry(3, "b.txt", false), entry(7, "c", false)],
            false,
            None,
        );
        assert_eq!(files.selected_entry().unwrap().id, 3);
        assert_eq!(files.unique_name("c", ""), Name::new("c 2"));
        assert_eq!(
            files.unique_name("Новый файл", ".txt"),
            Name::new("Новый файл.txt")
        );
    }
    #[test]
    fn path_navigation_is_bounded() {
        let mut files = Files::new();
        assert_eq!(files.parent(), None);
        for id in 1..20 {
            files.enter(id, Name::new("x"));
        }
        assert_eq!(files.depth(), MAX_DEPTH);
        assert_eq!(files.directory(), (MAX_DEPTH - 1) as u16);
        files.leave();
        assert_eq!(files.parent(), Some((MAX_DEPTH - 3) as u16));
        assert_eq!(files.components().count(), MAX_DEPTH - 2);
    }
}
