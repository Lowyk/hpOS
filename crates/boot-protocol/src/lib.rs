#![no_std]

pub const MAGIC: u32 = 0x48504f53;
pub const VERSION: u32 = 3;
/// Physical address of the loader's VBE mode list (`MAX_MODES` entries of 16 bytes).
pub const MODE_TABLE: u64 = 0x6100;
pub const MAX_MODES: u32 = 12;
pub const FLAG_FRAMEBUFFER: u32 = 1;
pub const MAX_MEMORY_ENTRIES: u32 = 64;
/// Set when the user chose to boot without disk access (Shift or F2 in the loader).
pub const OPTION_NO_STORAGE: u32 = 1;

/// Physical addresses are explicit integers, independent of the host ABI.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct BootInfo {
    pub magic: u32,
    pub version: u32,
    pub memory_map: u64,
    pub memory_entries: u32,
    pub flags: u32,
    pub framebuffer: u64,
    pub width: u32,
    pub height: u32,
    pub pitch: u32,
    pub bpp: u32,
    /// BIOS drive number (DL) that loaded hpOS; persistent storage uses only this drive.
    pub boot_drive: u32,
    pub options: u32,
    /// Valid entries in the mode list; 0 in text mode.
    pub mode_count: u32,
    /// Index of the mode the loader set.
    pub mode_current: u32,
}

/// One validated 32-bit linear-framebuffer VBE mode offered by the BIOS.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VideoMode {
    pub number: u16,
    pub width: u16,
    pub height: u16,
    pub pitch: u16,
    pub framebuffer: u32,
    pub score: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct MemoryEntry {
    pub base: u64,
    pub length: u64,
    pub kind: u32,
    pub attributes: u32,
}

impl BootInfo {
    pub fn valid(&self) -> bool {
        self.magic == MAGIC
            && self.version == VERSION
            && self.memory_entries > 0
            && self.memory_entries <= MAX_MEMORY_ENTRIES
            && self.memory_map == 0x5000
    }

    /// The initial x86 loader maps only the first 4 GiB.
    pub fn framebuffer_len(&self) -> Option<usize> {
        if self.flags & FLAG_FRAMEBUFFER == 0
            || self.bpp != 32
            || self.width == 0
            || self.height == 0
            || self.framebuffer == 0
        {
            return None;
        }
        if self.pitch < self.width.checked_mul(4)? || self.pitch % 4 != 0 {
            return None;
        }
        let bytes = u64::from(self.pitch).checked_mul(u64::from(self.height))?;
        if self.framebuffer.checked_add(bytes)? > 0x1_0000_0000 {
            return None;
        }
        usize::try_from(bytes).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn info() -> BootInfo {
        BootInfo {
            magic: MAGIC,
            version: VERSION,
            memory_map: 0x5000,
            memory_entries: 1,
            flags: FLAG_FRAMEBUFFER,
            framebuffer: 0xe0000000,
            width: 1024,
            height: 768,
            pitch: 4096,
            bpp: 32,
            boot_drive: 0x80,
            options: 0,
            mode_count: 0,
            mode_current: 0,
        }
    }
    #[test]
    fn assembly_layout_matches() {
        assert_eq!(core::mem::size_of::<BootInfo>(), 64);
        assert_eq!(core::mem::offset_of!(BootInfo, mode_count), 56);
        assert_eq!(core::mem::size_of::<VideoMode>(), 16);
        assert_eq!(core::mem::offset_of!(BootInfo, boot_drive), 48);
        assert_eq!(core::mem::size_of::<MemoryEntry>(), 24);
        assert_eq!(core::mem::offset_of!(BootInfo, framebuffer), 24);
        assert_eq!(core::mem::offset_of!(BootInfo, width), 32);
    }
    #[test]
    fn rejects_bad_handoff() {
        let mut b = info();
        assert!(b.valid());
        b.version = 2;
        assert!(!b.valid());
        b = info();
        b.memory_entries = 65;
        assert!(!b.valid());
    }
    #[test]
    fn checks_framebuffer_bounds_and_format() {
        let mut b = info();
        assert_eq!(b.framebuffer_len(), Some(4096 * 768));
        b.pitch = 1;
        assert_eq!(b.framebuffer_len(), None);
        b = info();
        b.framebuffer = 0xffff0000;
        assert_eq!(b.framebuffer_len(), None);
        b = info();
        b.bpp = 24;
        assert_eq!(b.framebuffer_len(), None);
        b = info();
        b.flags = 0;
        assert_eq!(b.framebuffer_len(), None);
    }
}
