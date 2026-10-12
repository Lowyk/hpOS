extern crate std;
use super::*;
use std::{boxed::Box, vec, vec::Vec};

struct Memory {
    bytes: Vec<u8>,
    writes: usize,
    fail_at: Option<usize>,
}
impl Memory {
    fn new(sectors: usize) -> Self {
        Self {
            bytes: vec![0; sectors * SECTOR],
            writes: 0,
            fail_at: None,
        }
    }
}
impl Device for Memory {
    fn read(&mut self, sector: u64, buffer: &mut [u8]) -> Result<(), Error> {
        let start = sector as usize * SECTOR;
        let end = start + buffer.len();
        if buffer.len() % SECTOR != 0 || end > self.bytes.len() {
            return Err(Error::Io);
        }
        buffer.copy_from_slice(&self.bytes[start..end]);
        Ok(())
    }
    fn write(&mut self, sector: u64, buffer: &[u8]) -> Result<(), Error> {
        let start = sector as usize * SECTOR;
        let end = start + buffer.len();
        if buffer.len() % SECTOR != 0 || end > self.bytes.len() {
            return Err(Error::Io);
        }
        if self.fail_at == Some(self.writes) {
            return Err(Error::Io);
        }
        self.writes += 1;
        self.bytes[start..end].copy_from_slice(buffer);
        Ok(())
    }
}

fn volume(sectors: usize) -> Box<Fs<Memory>> {
    let mut fs = Box::new(Fs::new(Memory::new(sectors)));
    fs.format(sectors as u64, "test").unwrap();
    fs
}
fn remount(fs: Box<Fs<Memory>>) -> Box<Fs<Memory>> {
    let bytes = core::mem::take(&mut Box::leak(fs).device.bytes);
    let mut fresh = Box::new(Fs::new(Memory {
        bytes,
        writes: 0,
        fail_at: None,
    }));
    fresh.mount().unwrap();
    fresh
}
fn pattern(len: usize, seed: u8) -> Vec<u8> {
    (0..len)
        .map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed))
        .collect()
}
fn contents(fs: &mut Fs<Memory>, id: u16) -> Vec<u8> {
    let size = fs.info(id).unwrap().size as usize;
    let mut out = vec![0; size];
    assert_eq!(fs.read(id, 0, &mut out).unwrap(), size);
    out
}

#[test]
fn unformatted_and_damaged_superblocks_are_rejected() {
    let mut fs = Box::new(Fs::new(Memory::new(1024)));
    assert_eq!(fs.mount(), Err(Error::NotFormatted));
    fs.format(1024, "x").unwrap();
    fs.device().bytes[16] ^= 1;
    assert_eq!(fs.mount(), Err(Error::Corrupt));
    assert_eq!(fs.resolve("/"), Err(Error::NotMounted));
}

#[test]
fn directories_files_and_paths_survive_remount() {
    let mut fs = volume(4096);
    let docs = fs.create(ROOT, "Документы", Kind::Directory).unwrap();
    let note = fs.create(docs, "Привет.txt", Kind::File).unwrap();
    fs.write(note, "Хуйня Полнейшая ОС".as_bytes()).unwrap();
    let mut fs = remount(fs);
    assert_eq!(fs.resolve("/Документы/Привет.txt"), Ok(note));
    assert_eq!(contents(&mut fs, note), "Хуйня Полнейшая ОС".as_bytes());
    assert_eq!(fs.info(note).unwrap().name(), "Привет.txt");
    assert_eq!(fs.info(note).unwrap().parent, docs);
    assert_eq!(fs.next_child(ROOT, None), Ok(Some(docs)));
    assert_eq!(fs.next_child(ROOT, Some(docs)), Ok(None));
    assert_eq!(fs.stats().errors, 0);
}

#[test]
fn names_are_validated_and_unique_per_directory() {
    let mut fs = volume(1024);
    for bad in ["", ".", "..", "a/b", "tab\there", &"я".repeat(32)] {
        assert_eq!(
            fs.create(ROOT, bad, Kind::File),
            Err(Error::InvalidName),
            "{bad:?}"
        );
    }
    let a = fs.create(ROOT, "a", Kind::Directory).unwrap();
    assert_eq!(fs.create(ROOT, "a", Kind::File), Err(Error::Exists));
    fs.create(a, "a", Kind::File).unwrap();
    let file = fs.lookup(a, "a").unwrap();
    assert_eq!(fs.create(file, "x", Kind::File), Err(Error::NotDirectory));
    assert_eq!(fs.read(a, 0, &mut [0; 4]), Err(Error::IsDirectory));
}

#[test]
fn large_files_use_the_indirect_block_and_partial_reads_work() {
    let mut fs = volume(16384);
    let id = fs.create(ROOT, "big", Kind::File).unwrap();
    let data = pattern(BLOCK * (DIRECT + 5) + 123, 7);
    let free = fs.stats().free_blocks;
    fs.write(id, &data).unwrap();
    assert_eq!(fs.stats().free_blocks, free - (DIRECT as u32 + 6) - 1);
    let mut fs = remount(fs);
    assert_eq!(contents(&mut fs, id), data);
    let mut part = [0u8; 5000];
    assert_eq!(fs.read(id, BLOCK * 17 - 10, &mut part), Ok(5000));
    assert_eq!(&part[..], &data[BLOCK * 17 - 10..BLOCK * 17 + 4990]);
    assert_eq!(fs.read(id, data.len(), &mut part), Ok(0));
    fs.write(id, b"short").unwrap();
    assert_eq!(fs.stats().free_blocks, free - 1);
    assert_eq!(contents(&mut remount(fs), id), b"short");
}

#[test]
fn failed_write_keeps_old_contents_and_frees_new_blocks() {
    let mut fs = volume(4096);
    let id = fs.create(ROOT, "settings", Kind::File).unwrap();
    fs.write(id, b"theme=dark").unwrap();
    let free = fs.stats().free_blocks;
    for failing in 0..4 {
        fs.device().fail_at = Some(fs.device().writes + failing);
        assert_eq!(fs.write(id, &pattern(BLOCK * 3, 1)), Err(Error::Io));
        fs.device().fail_at = None;
        assert_eq!(fs.stats().free_blocks, free);
        assert_eq!(contents(&mut fs, id), b"theme=dark");
    }
    let mut fs = remount(fs);
    assert_eq!(contents(&mut fs, id), b"theme=dark");
    assert_eq!(fs.stats().free_blocks, free);
}

#[test]
fn interrupted_write_before_inode_commit_only_leaks_until_remount() {
    let mut fs = volume(4096);
    let id = fs.create(ROOT, "f", Kind::File).unwrap();
    fs.write(id, b"old").unwrap();
    let free = fs.stats().free_blocks;
    // Data block reaches the disk, the inode sector does not: simulates power loss.
    fs.device().fail_at = Some(fs.device().writes + 1);
    assert!(fs.write(id, b"new").is_err());
    let mut fs = remount(fs);
    assert_eq!(contents(&mut fs, id), b"old");
    assert_eq!(fs.stats().free_blocks, free);
    assert_eq!(fs.stats().errors, 0);
}

#[test]
fn remove_and_rename_enforce_tree_rules() {
    let mut fs = volume(4096);
    let a = fs.create(ROOT, "a", Kind::Directory).unwrap();
    let b = fs.create(a, "b", Kind::Directory).unwrap();
    let f = fs.create(b, "f", Kind::File).unwrap();
    fs.write(f, &pattern(9000, 3)).unwrap();
    assert_eq!(fs.remove(a), Err(Error::NotEmpty));
    assert_eq!(fs.rename(a, b, "loop"), Err(Error::Invalid));
    assert_eq!(fs.rename(a, a, "self"), Err(Error::Invalid));
    assert_eq!(fs.remove(ROOT), Err(Error::Invalid));
    fs.rename(f, ROOT, "moved").unwrap();
    assert_eq!(fs.resolve("/moved"), Ok(f));
    assert_eq!(fs.rename(b, ROOT, "moved"), Err(Error::Exists));
    fs.rename(b, ROOT, "b2").unwrap();
    let free = fs.stats().free_blocks;
    fs.remove(f).unwrap();
    assert_eq!(fs.stats().free_blocks, free + 3);
    fs.remove(a).unwrap();
    let fs = remount(fs);
    assert_eq!(fs.resolve("/b2"), Ok(b));
    assert_eq!(fs.resolve("/a"), Err(Error::NotFound));
    assert_eq!(fs.stats().used_inodes, 2);
}

#[test]
fn space_and_inode_exhaustion_are_reported_without_damage() {
    let mut fs = volume((DATA_START as usize + 4) * 8);
    let id = fs.create(ROOT, "f", Kind::File).unwrap();
    assert_eq!(fs.write(id, &pattern(BLOCK * 5, 0)), Err(Error::NoSpace));
    fs.write(id, &pattern(BLOCK * 4, 0)).unwrap();
    assert_eq!(fs.write(id, &pattern(BLOCK * 4, 9)), Err(Error::NoSpace));
    assert_eq!(contents(&mut fs, id), pattern(BLOCK * 4, 0));
    let mut fs = volume(1024);
    for n in 1..INODES {
        fs.create(ROOT, &std::format!("{n}"), Kind::File).unwrap();
    }
    assert_eq!(fs.create(ROOT, "x", Kind::File), Err(Error::NoInodes));
    assert_eq!(fs.write(id, &vec![0; MAX_FILE + 1]), Err(Error::TooLarge));
}

#[test]
fn torn_inode_and_cross_links_are_isolated_at_mount() {
    let mut fs = volume(4096);
    let a = fs.create(ROOT, "a", Kind::File).unwrap();
    let b = fs.create(ROOT, "b", Kind::File).unwrap();
    let dir = fs.create(ROOT, "d", Kind::Directory).unwrap();
    let child = fs.create(dir, "c", Kind::File).unwrap();
    fs.write(a, b"aaaa").unwrap();
    fs.write(b, b"bbbb").unwrap();
    // Point b at a's block with a valid checksum: a cross-link.
    let table = INODE_SECTOR as usize * SECTOR;
    let bytes = &mut fs.device().bytes;
    let a_block =
        &bytes[table + a as usize * INODE_SIZE + 80..table + a as usize * INODE_SIZE + 82].to_vec();
    let record = table + b as usize * INODE_SIZE;
    bytes[record + 80..record + 82].copy_from_slice(a_block);
    let crc = crc32(&bytes[record..record + 124]);
    bytes[record + 124..record + 128].copy_from_slice(&crc.to_le_bytes());
    // Tear the directory record: its child becomes visible under the root.
    bytes[table + dir as usize * INODE_SIZE + 20] ^= 0xff;
    let mut fs = remount(fs);
    assert_eq!(fs.stats().errors, 3);
    assert_eq!(contents(&mut fs, a), b"aaaa");
    assert_eq!(fs.info(b), Err(Error::NotFound));
    assert_eq!(fs.info(dir), Err(Error::NotFound));
    assert_eq!(fs.info(child).unwrap().parent, ROOT);
}

#[test]
fn error_codes_round_trip() {
    for code in 1..=14 {
        assert_eq!(Error::from_code(code).unwrap().code(), code);
    }
    assert_eq!(Error::from_code(0), None);
    assert_eq!(Error::from_code(15), None);
}
