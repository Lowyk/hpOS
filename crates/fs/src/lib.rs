//! HPFS: a small copy-on-write file system for one hpOS data partition.
//!
//! Layout, in 4 KiB blocks: block 0 holds the superblock, blocks 1..9 hold 256
//! fixed 128-byte inodes, the rest is file data. Directories own no blocks:
//! a child names its parent inode. The free-block bitmap is not stored; it is
//! rebuilt from the inodes at mount, so an interrupted update can only leak
//! blocks until the next mount, never cross-link them.
//!
//! File contents are replaced copy-on-write: new data blocks are written first,
//! then the single 512-byte inode sector. Until that sector write completes the
//! old contents stay intact.
#![no_std]

pub const SECTOR: usize = 512;
pub const BLOCK: usize = 4096;
pub const SECTORS_PER_BLOCK: u64 = (BLOCK / SECTOR) as u64;
pub const INODES: usize = 256;
pub const INODE_SIZE: usize = 128;
pub const INODE_BLOCKS: u32 = (INODES * INODE_SIZE / BLOCK) as u32;
pub const DATA_START: u32 = 1 + INODE_BLOCKS;
pub const DIRECT: usize = 16;
pub const PER_INDIRECT: usize = BLOCK / 2;
pub const MAX_FILE: usize = (DIRECT + PER_INDIRECT) * BLOCK;
pub const NAME_MAX: usize = 63;
pub const MAX_BLOCKS: usize = 65536;
pub const MAGIC: [u8; 8] = *b"HPFS0001";
pub const VERSION: u32 = 1;
pub const ROOT: u16 = 0;
const INODE_SECTOR: u64 = SECTORS_PER_BLOCK;

/// Sector-addressed storage relative to the start of the volume.
pub trait Device {
    fn read(&mut self, sector: u64, buffer: &mut [u8]) -> Result<(), Error>;
    fn write(&mut self, sector: u64, buffer: &[u8]) -> Result<(), Error>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Io,
    NotFormatted,
    Corrupt,
    NotMounted,
    NotFound,
    NotDirectory,
    IsDirectory,
    Exists,
    InvalidName,
    NoSpace,
    NoInodes,
    NotEmpty,
    TooLarge,
    Invalid,
}
impl Error {
    pub fn code(self) -> u32 {
        self as u32 + 1
    }
    pub fn from_code(code: u32) -> Option<Self> {
        use Error::*;
        [
            Io,
            NotFormatted,
            Corrupt,
            NotMounted,
            NotFound,
            NotDirectory,
            IsDirectory,
            Exists,
            InvalidName,
            NoSpace,
            NoInodes,
            NotEmpty,
            TooLarge,
            Invalid,
        ]
        .get((code as usize).checked_sub(1)?)
        .copied()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    File = 1,
    Directory = 2,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Info {
    pub id: u16,
    pub kind: Kind,
    pub parent: u16,
    pub size: u32,
    pub stamp: u64,
    pub name: [u8; NAME_MAX],
    pub name_len: u8,
}
impl Info {
    pub fn name(&self) -> &str {
        core::str::from_utf8(&self.name[..self.name_len as usize]).unwrap_or("?")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stats {
    pub blocks: u32,
    pub free_blocks: u32,
    pub used_inodes: u32,
    pub errors: u32,
}

pub fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = !0u32;
    for byte in bytes {
        crc ^= *byte as u32;
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb8_8320 & (crc & 1).wrapping_neg());
        }
    }
    !crc
}

pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= NAME_MAX
        && name != "."
        && name != ".."
        && !name
            .chars()
            .any(|c| c == '/' || (c as u32) < 0x20 || c == '\u{7f}')
}

/// Raw inode field access; `bytes` is exactly one 128-byte record.
mod inode {
    use super::*;
    pub fn kind(b: &[u8]) -> u8 {
        b[0]
    }
    pub fn parent(b: &[u8]) -> u16 {
        u16::from_le_bytes([b[2], b[3]])
    }
    pub fn size(b: &[u8]) -> u32 {
        u32::from_le_bytes([b[4], b[5], b[6], b[7]])
    }
    pub fn stamp(b: &[u8]) -> u64 {
        u64::from_le_bytes(b[8..16].try_into().unwrap())
    }
    pub fn name(b: &[u8]) -> &[u8] {
        &b[16..16 + (b[1] as usize).min(NAME_MAX)]
    }
    pub fn direct(b: &[u8], index: usize) -> u16 {
        u16::from_le_bytes([b[80 + index * 2], b[81 + index * 2]])
    }
    pub fn indirect(b: &[u8]) -> u16 {
        u16::from_le_bytes([b[112], b[113]])
    }
    pub fn set_direct(b: &mut [u8], index: usize, block: u16) {
        b[80 + index * 2..82 + index * 2].copy_from_slice(&block.to_le_bytes());
    }
    pub fn set_indirect(b: &mut [u8], block: u16) {
        b[112..114].copy_from_slice(&block.to_le_bytes());
    }
    pub fn set_header(b: &mut [u8], kind: u8, parent: u16, size: u32, stamp: u64) {
        b[0] = kind;
        b[2..4].copy_from_slice(&parent.to_le_bytes());
        b[4..8].copy_from_slice(&size.to_le_bytes());
        b[8..16].copy_from_slice(&stamp.to_le_bytes());
    }
    pub fn set_name(b: &mut [u8], name: &str) {
        b[16..80].fill(0);
        b[16..16 + name.len()].copy_from_slice(name.as_bytes());
        b[1] = name.len() as u8;
    }
    pub fn seal(b: &mut [u8]) {
        let crc = crc32(&b[..124]);
        b[124..128].copy_from_slice(&crc.to_le_bytes());
    }
    pub fn sealed(b: &[u8]) -> bool {
        crc32(&b[..124]) == u32::from_le_bytes([b[124], b[125], b[126], b[127]])
    }
}

pub struct Fs<D: Device> {
    device: D,
    table: [u8; INODES * INODE_SIZE],
    used: [u64; MAX_BLOCKS / 64],
    blocks: u32,
    free: u32,
    stamp: u64,
    errors: u32,
    mounted: bool,
    scratch: [u8; BLOCK],
    indirect: [u8; BLOCK],
    indirect_block: u16,
}

impl<D: Device> Fs<D> {
    /// Large state is meant to live in a static; methods work in place.
    pub const fn new(device: D) -> Self {
        Self {
            device,
            table: [0; INODES * INODE_SIZE],
            used: [0; MAX_BLOCKS / 64],
            blocks: 0,
            free: 0,
            stamp: 0,
            errors: 0,
            mounted: false,
            scratch: [0; BLOCK],
            indirect: [0; BLOCK],
            indirect_block: 0,
        }
    }
    pub fn device(&mut self) -> &mut D {
        &mut self.device
    }
    pub fn mounted(&self) -> bool {
        self.mounted
    }

    /// Writes an empty volume of `sectors` sectors and mounts it.
    pub fn format(&mut self, sectors: u64, label: &str) -> Result<(), Error> {
        let blocks = sectors / SECTORS_PER_BLOCK;
        if blocks <= DATA_START as u64 || blocks > MAX_BLOCKS as u64 || label.len() > 32 {
            return Err(Error::Invalid);
        }
        self.mounted = false;
        self.scratch.fill(0);
        for block in 1..DATA_START as u64 {
            self.device
                .write(block * SECTORS_PER_BLOCK, &self.scratch)?;
        }
        self.table.fill(0);
        let root = &mut self.table[..INODE_SIZE];
        inode::set_header(root, Kind::Directory as u8, ROOT, 0, 1);
        inode::seal(root);
        self.device.write(INODE_SECTOR, &self.table[..SECTOR])?;
        let mut sb = [0u8; SECTOR];
        sb[..8].copy_from_slice(&MAGIC);
        sb[8..12].copy_from_slice(&VERSION.to_le_bytes());
        sb[12..16].copy_from_slice(&(BLOCK as u32).to_le_bytes());
        sb[16..20].copy_from_slice(&(blocks as u32).to_le_bytes());
        sb[20..24].copy_from_slice(&(INODES as u32).to_le_bytes());
        sb[24..28].copy_from_slice(&1u32.to_le_bytes());
        sb[28..32].copy_from_slice(&DATA_START.to_le_bytes());
        sb[32..32 + label.len()].copy_from_slice(label.as_bytes());
        let crc = crc32(&sb[..508]);
        sb[508..].copy_from_slice(&crc.to_le_bytes());
        // The superblock goes last: a half-formatted volume is never mountable.
        self.device.write(0, &sb)?;
        self.mount()
    }

    pub fn mount(&mut self) -> Result<(), Error> {
        self.mounted = false;
        self.indirect_block = 0;
        let mut sb = [0u8; SECTOR];
        self.device.read(0, &mut sb)?;
        if sb[..8] != MAGIC {
            return Err(Error::NotFormatted);
        }
        let word = |o: usize| u32::from_le_bytes(sb[o..o + 4].try_into().unwrap());
        if crc32(&sb[..508]) != word(508)
            || word(8) != VERSION
            || word(12) != BLOCK as u32
            || word(20) != INODES as u32
            || word(24) != 1
            || word(28) != DATA_START
            || word(16) <= DATA_START
            || word(16) as usize > MAX_BLOCKS
        {
            return Err(Error::Corrupt);
        }
        self.blocks = word(16);
        self.device.read(INODE_SECTOR, &mut self.table)?;
        self.errors = 0;
        self.stamp = 0;
        // Pass 1: drop records that are torn or out of range.
        for id in 0..INODES {
            let b = &self.table[id * INODE_SIZE..(id + 1) * INODE_SIZE];
            let kind = inode::kind(b);
            if kind == 0 {
                continue;
            }
            let ok = inode::sealed(b)
                && (kind == 1 || kind == 2)
                && (b[1] as usize) <= NAME_MAX
                && core::str::from_utf8(inode::name(b)).is_ok()
                && (inode::size(b) as usize) <= MAX_FILE
                && (inode::parent(b) as usize) < INODES
                && (id == 0 || b[1] != 0)
                && (kind == 1 || inode::size(b) == 0)
                && (0..DIRECT)
                    .map(|i| inode::direct(b, i))
                    .chain([inode::indirect(b)])
                    .all(|p| p == 0 || (p as u32 >= DATA_START && (p as u32) < self.blocks));
            if !ok {
                if id == ROOT as usize {
                    return Err(Error::Corrupt);
                }
                self.errors += 1;
                self.table[id * INODE_SIZE..(id + 1) * INODE_SIZE].fill(0);
            } else {
                self.stamp = self.stamp.max(inode::stamp(b));
            }
        }
        if inode::kind(&self.table[..INODE_SIZE]) != 2 {
            return Err(Error::Corrupt);
        }
        // Pass 2: orphans and parent cycles are shown under the root.
        for id in 1..INODES {
            if self.kind_raw(id) == 0 {
                continue;
            }
            let mut cursor = self.parent_raw(id) as usize;
            let mut steps = 0;
            let attached = loop {
                if self.kind_raw(cursor) != 2 || cursor == id || steps > INODES {
                    break false;
                }
                if cursor == ROOT as usize {
                    break true;
                }
                cursor = self.parent_raw(cursor) as usize;
                steps += 1;
            };
            if !attached {
                self.errors += 1;
                let b = &mut self.table[id * INODE_SIZE..(id + 1) * INODE_SIZE];
                b[2..4].copy_from_slice(&ROOT.to_le_bytes());
            }
        }
        // Pass 3: rebuild the bitmap; cross-linked or missing blocks are errors.
        self.used.fill(0);
        for block in 0..DATA_START {
            self.mark(block as u16);
        }
        for id in 1..INODES {
            if self.kind_raw(id) != 1 {
                continue;
            }
            if !self.claim_file_blocks(id)? {
                self.errors += 1;
                let b = &mut self.table[id * INODE_SIZE..(id + 1) * INODE_SIZE];
                b[0] = 0;
                // Blocks it did claim stay marked; they leak until reformat but are never reused twice.
            }
        }
        self.free = (DATA_START..self.blocks)
            .filter(|b| !self.is_used(*b as u16))
            .count() as u32;
        self.mounted = true;
        Ok(())
    }

    fn claim_file_blocks(&mut self, id: usize) -> Result<bool, Error> {
        let b = self.record(id);
        let count = (inode::size(b) as usize).div_ceil(BLOCK);
        let indirect = inode::indirect(b);
        let mut direct = [0u16; DIRECT];
        for (i, slot) in direct.iter_mut().enumerate() {
            *slot = inode::direct(b, i);
        }
        if (count > DIRECT) != (indirect != 0) {
            return Ok(false);
        }
        for (i, block) in direct.iter().copied().enumerate() {
            if (i < count) != (block != 0) || (block != 0 && self.is_used(block)) {
                return Ok(false);
            }
            if block != 0 {
                self.mark(block);
            }
        }
        if indirect != 0 {
            if self.is_used(indirect) {
                return Ok(false);
            }
            self.load_indirect(indirect)?;
            self.mark(indirect);
            for i in 0..PER_INDIRECT {
                let block = self.indirect_entry(i);
                let needed = DIRECT + i < count;
                if needed != (block != 0)
                    || (block != 0
                        && ((block as u32) < DATA_START
                            || block as u32 >= self.blocks
                            || self.is_used(block)))
                {
                    return Ok(false);
                }
                if block != 0 {
                    self.mark(block);
                }
            }
        }
        Ok(true)
    }

    fn record(&self, id: usize) -> &[u8] {
        &self.table[id * INODE_SIZE..(id + 1) * INODE_SIZE]
    }
    fn kind_raw(&self, id: usize) -> u8 {
        self.table[id * INODE_SIZE]
    }
    fn parent_raw(&self, id: usize) -> u16 {
        inode::parent(self.record(id))
    }
    fn mark(&mut self, block: u16) {
        self.used[block as usize / 64] |= 1 << (block % 64);
    }
    fn unmark(&mut self, block: u16) {
        self.used[block as usize / 64] &= !(1 << (block % 64));
    }
    fn is_used(&self, block: u16) -> bool {
        self.used[block as usize / 64] & (1 << (block % 64)) != 0
    }
    fn allocate(&mut self) -> Result<u16, Error> {
        for block in DATA_START..self.blocks {
            if !self.is_used(block as u16) {
                self.mark(block as u16);
                self.free -= 1;
                return Ok(block as u16);
            }
        }
        Err(Error::NoSpace)
    }
    fn release(&mut self, block: u16) {
        if block as u32 >= DATA_START && block as u32 > 0 && self.is_used(block) {
            self.unmark(block);
            self.free += 1;
        }
    }
    fn load_indirect(&mut self, block: u16) -> Result<(), Error> {
        if self.indirect_block != block {
            self.indirect_block = 0;
            self.device
                .read(block as u64 * SECTORS_PER_BLOCK, &mut self.indirect)?;
            self.indirect_block = block;
        }
        Ok(())
    }
    fn indirect_entry(&self, index: usize) -> u16 {
        u16::from_le_bytes([self.indirect[index * 2], self.indirect[index * 2 + 1]])
    }
    fn write_record(&mut self, id: usize) -> Result<(), Error> {
        let sector = id * INODE_SIZE / SECTOR;
        self.device.write(
            INODE_SECTOR + sector as u64,
            &self.table[sector * SECTOR..(sector + 1) * SECTOR],
        )
    }
    fn live(&self, id: u16) -> Result<u8, Error> {
        if !self.mounted {
            return Err(Error::NotMounted);
        }
        match self.table.get(id as usize * INODE_SIZE) {
            Some(0) | None => Err(Error::NotFound),
            Some(kind) => Ok(*kind),
        }
    }
    fn directory(&self, id: u16) -> Result<(), Error> {
        match self.live(id)? {
            2 => Ok(()),
            _ => Err(Error::NotDirectory),
        }
    }
    fn next_stamp(&mut self) -> u64 {
        self.stamp += 1;
        self.stamp
    }

    pub fn stats(&self) -> Stats {
        Stats {
            blocks: self.blocks,
            free_blocks: self.free,
            used_inodes: (0..INODES).filter(|i| self.kind_raw(*i) != 0).count() as u32,
            errors: self.errors,
        }
    }

    pub fn info(&self, id: u16) -> Result<Info, Error> {
        let kind = self.live(id)?;
        let b = self.record(id as usize);
        let mut name = [0; NAME_MAX];
        let raw = inode::name(b);
        name[..raw.len()].copy_from_slice(raw);
        Ok(Info {
            id,
            kind: if kind == 2 {
                Kind::Directory
            } else {
                Kind::File
            },
            parent: inode::parent(b),
            size: inode::size(b),
            stamp: inode::stamp(b),
            name,
            name_len: raw.len() as u8,
        })
    }

    /// Children in inode order; pass the previous id to continue a listing.
    pub fn next_child(&self, dir: u16, after: Option<u16>) -> Result<Option<u16>, Error> {
        self.directory(dir)?;
        let start = after.map_or(1, |a| a as usize + 1);
        Ok((start..INODES)
            .find(|id| self.kind_raw(*id) != 0 && self.parent_raw(*id) == dir)
            .map(|id| id as u16))
    }

    pub fn lookup(&self, dir: u16, name: &str) -> Result<u16, Error> {
        self.directory(dir)?;
        (1..INODES)
            .find(|id| {
                self.kind_raw(*id) != 0
                    && self.parent_raw(*id) == dir
                    && inode::name(self.record(*id)) == name.as_bytes()
            })
            .map(|id| id as u16)
            .ok_or(Error::NotFound)
    }

    pub fn resolve(&self, path: &str) -> Result<u16, Error> {
        let mut id = ROOT;
        self.directory(id)?;
        for part in path.split('/').filter(|p| !p.is_empty()) {
            self.directory(id)?;
            id = self.lookup(id, part)?;
        }
        Ok(id)
    }

    pub fn create(&mut self, parent: u16, name: &str, kind: Kind) -> Result<u16, Error> {
        self.directory(parent)?;
        if !valid_name(name) {
            return Err(Error::InvalidName);
        }
        if self.lookup(parent, name).is_ok() {
            return Err(Error::Exists);
        }
        let id = (1..INODES)
            .find(|id| self.kind_raw(*id) == 0)
            .ok_or(Error::NoInodes)?;
        let stamp = self.next_stamp();
        let b = &mut self.table[id * INODE_SIZE..(id + 1) * INODE_SIZE];
        b.fill(0);
        inode::set_header(b, kind as u8, parent, 0, stamp);
        inode::set_name(b, name);
        inode::seal(b);
        if let Err(error) = self.write_record(id) {
            self.table[id * INODE_SIZE] = 0;
            return Err(error);
        }
        Ok(id as u16)
    }

    pub fn read(&mut self, id: u16, offset: usize, buffer: &mut [u8]) -> Result<usize, Error> {
        if self.live(id)? != 1 {
            return Err(Error::IsDirectory);
        }
        let size = inode::size(self.record(id as usize)) as usize;
        if offset >= size {
            return Ok(0);
        }
        let total = buffer.len().min(size - offset);
        let mut done = 0;
        while done < total {
            let position = offset + done;
            let index = position / BLOCK;
            let within = position % BLOCK;
            let block = self.block_at(id as usize, index)?;
            let n = (BLOCK - within).min(total - done);
            self.device
                .read(block as u64 * SECTORS_PER_BLOCK, &mut self.scratch)?;
            buffer[done..done + n].copy_from_slice(&self.scratch[within..within + n]);
            done += n;
        }
        Ok(total)
    }

    fn block_at(&mut self, id: usize, index: usize) -> Result<u16, Error> {
        let b = self.record(id);
        let block = if index < DIRECT {
            inode::direct(b, index)
        } else {
            let indirect = inode::indirect(b);
            if indirect == 0 || index - DIRECT >= PER_INDIRECT {
                return Err(Error::Corrupt);
            }
            self.load_indirect(indirect)?;
            self.indirect_entry(index - DIRECT)
        };
        if block == 0 {
            Err(Error::Corrupt)
        } else {
            Ok(block)
        }
    }

    /// Replaces the whole file. On any error the previous contents remain.
    pub fn write(&mut self, id: u16, data: &[u8]) -> Result<(), Error> {
        if self.live(id)? != 1 {
            return Err(Error::IsDirectory);
        }
        if data.len() > MAX_FILE {
            return Err(Error::TooLarge);
        }
        let count = data.len().div_ceil(BLOCK);
        let needed = count + usize::from(count > DIRECT);
        if needed > self.free as usize {
            return Err(Error::NoSpace);
        }
        let old: [u8; INODE_SIZE] = self.record(id as usize).try_into().unwrap();
        let mut new = old;
        for i in 0..DIRECT {
            inode::set_direct(&mut new, i, 0);
        }
        inode::set_indirect(&mut new, 0);
        // The indirect buffer holds the new pointer list while data is written.
        self.indirect_block = 0;
        self.indirect.fill(0);
        let result = self.write_blocks(&mut new, data, count);
        if let Err(error) = result {
            self.release_list(&new, count);
            self.indirect_block = 0;
            return Err(error);
        }
        let stamp = self.next_stamp();
        inode::set_header(&mut new, 1, inode::parent(&old), data.len() as u32, stamp);
        inode::seal(&mut new);
        let range = id as usize * INODE_SIZE..(id as usize + 1) * INODE_SIZE;
        self.table[range.clone()].copy_from_slice(&new);
        if let Err(error) = self.write_record(id as usize) {
            self.table[range].copy_from_slice(&old);
            self.release_list(&new, count);
            self.indirect_block = 0;
            return Err(error);
        }
        self.indirect_block = 0;
        self.release_file(&old)?;
        Ok(())
    }

    fn write_blocks(
        &mut self,
        new: &mut [u8; INODE_SIZE],
        data: &[u8],
        count: usize,
    ) -> Result<(), Error> {
        for index in 0..count {
            let block = self.allocate()?;
            if index < DIRECT {
                inode::set_direct(new, index, block);
            } else {
                let i = index - DIRECT;
                self.indirect[i * 2..i * 2 + 2].copy_from_slice(&block.to_le_bytes());
            }
            let chunk = &data[index * BLOCK..((index + 1) * BLOCK).min(data.len())];
            self.scratch[..chunk.len()].copy_from_slice(chunk);
            self.scratch[chunk.len()..].fill(0);
            self.device
                .write(block as u64 * SECTORS_PER_BLOCK, &self.scratch)?;
        }
        if count > DIRECT {
            let block = self.allocate()?;
            inode::set_indirect(new, block);
            self.device
                .write(block as u64 * SECTORS_PER_BLOCK, &self.indirect)?;
        }
        Ok(())
    }

    /// Releases blocks recorded in a not-yet-committed inode and the indirect buffer.
    fn release_list(&mut self, new: &[u8; INODE_SIZE], count: usize) {
        for index in 0..count.min(DIRECT) {
            let block = inode::direct(new, index);
            if block != 0 {
                self.release(block);
            }
        }
        for i in 0..count.saturating_sub(DIRECT) {
            let block = self.indirect_entry(i);
            if block != 0 {
                self.release(block);
            }
        }
        let indirect = inode::indirect(new);
        if indirect != 0 {
            self.release(indirect);
        }
    }

    fn release_file(&mut self, record: &[u8; INODE_SIZE]) -> Result<(), Error> {
        let count = (inode::size(record) as usize).div_ceil(BLOCK);
        for index in 0..count.min(DIRECT) {
            self.release(inode::direct(record, index));
        }
        let indirect = inode::indirect(record);
        if indirect != 0 {
            self.load_indirect(indirect)?;
            for i in 0..count.saturating_sub(DIRECT).min(PER_INDIRECT) {
                let block = self.indirect_entry(i);
                self.release(block);
            }
            self.release(indirect);
            self.indirect_block = 0;
        }
        Ok(())
    }

    pub fn remove(&mut self, id: u16) -> Result<(), Error> {
        let kind = self.live(id)?;
        if id == ROOT {
            return Err(Error::Invalid);
        }
        if kind == 2 && self.next_child(id, None)?.is_some() {
            return Err(Error::NotEmpty);
        }
        let old: [u8; INODE_SIZE] = self.record(id as usize).try_into().unwrap();
        self.table[id as usize * INODE_SIZE..(id as usize + 1) * INODE_SIZE].fill(0);
        if let Err(error) = self.write_record(id as usize) {
            self.table[id as usize * INODE_SIZE..(id as usize + 1) * INODE_SIZE]
                .copy_from_slice(&old);
            return Err(error);
        }
        if kind == 1 {
            self.release_file(&old)?;
        }
        Ok(())
    }

    pub fn rename(&mut self, id: u16, parent: u16, name: &str) -> Result<(), Error> {
        let kind = self.live(id)?;
        if id == ROOT {
            return Err(Error::Invalid);
        }
        self.directory(parent)?;
        if !valid_name(name) {
            return Err(Error::InvalidName);
        }
        if kind == 2 {
            let mut cursor = parent;
            for _ in 0..=INODES {
                if cursor == id {
                    return Err(Error::Invalid);
                }
                if cursor == ROOT {
                    break;
                }
                cursor = self.parent_raw(cursor as usize);
            }
        }
        if let Ok(existing) = self.lookup(parent, name) {
            if existing != id {
                return Err(Error::Exists);
            }
        }
        let old: [u8; INODE_SIZE] = self.record(id as usize).try_into().unwrap();
        let stamp = self.next_stamp();
        let b = &mut self.table[id as usize * INODE_SIZE..(id as usize + 1) * INODE_SIZE];
        b[2..4].copy_from_slice(&parent.to_le_bytes());
        b[8..16].copy_from_slice(&stamp.to_le_bytes());
        inode::set_name(b, name);
        inode::seal(b);
        if let Err(error) = self.write_record(id as usize) {
            self.table[id as usize * INODE_SIZE..(id as usize + 1) * INODE_SIZE]
                .copy_from_slice(&old);
            return Err(error);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
