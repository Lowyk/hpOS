#![no_std]
#![no_main]
mod runtime;
use core::ptr::{addr_of_mut, copy_nonoverlapping, read_volatile, write_volatile};
use hpos_fs::{Device, Error, Fs, Kind, SECTOR};
use hpos_service_protocol::{
    FS_DISK, FS_SHARED, WINDOW_BYTES,
    fs::{self, Entry, Request},
};

const WINDOW_SECTORS: usize = WINDOW_BYTES / SECTOR;

struct Disk;
impl Disk {
    fn call(op: u64, sector: u64, count: usize) -> Result<(), Error> {
        match runtime::call(9, sector, (op << 32) | count as u64).status {
            0 => Ok(()),
            4 => Err(Error::Invalid),
            _ => Err(Error::Io),
        }
    }
}
impl Device for Disk {
    fn read(&mut self, sector: u64, buffer: &mut [u8]) -> Result<(), Error> {
        if buffer.len() % SECTOR != 0 {
            return Err(Error::Invalid);
        }
        for (index, chunk) in buffer.chunks_mut(WINDOW_BYTES).enumerate() {
            Self::call(
                1,
                sector + (index * WINDOW_SECTORS) as u64,
                chunk.len() / SECTOR,
            )?;
            unsafe { copy_nonoverlapping(FS_DISK as *const u8, chunk.as_mut_ptr(), chunk.len()) };
        }
        Ok(())
    }
    fn write(&mut self, sector: u64, buffer: &[u8]) -> Result<(), Error> {
        if buffer.len() % SECTOR != 0 {
            return Err(Error::Invalid);
        }
        for (index, chunk) in buffer.chunks(WINDOW_BYTES).enumerate() {
            unsafe { copy_nonoverlapping(chunk.as_ptr(), FS_DISK as *mut u8, chunk.len()) };
            Self::call(
                2,
                sector + (index * WINDOW_SECTORS) as u64,
                chunk.len() / SECTOR,
            )?;
        }
        Ok(())
    }
}

static mut FILES: Fs<Disk> = Fs::new(Disk);

fn request() -> *mut Request {
    FS_SHARED as *mut Request
}
fn data() -> *mut u8 {
    (FS_SHARED as usize + fs::DATA_OFFSET) as *mut u8
}
fn name(r: &Request) -> Result<&str, Error> {
    let len = r.name_len as usize;
    if len > fs::NAME_BYTES {
        return Err(Error::InvalidName);
    }
    core::str::from_utf8(&r.name[..len]).map_err(|_| Error::InvalidName)
}
fn id(value: u64) -> Result<u16, Error> {
    u16::try_from(value).map_err(|_| Error::NotFound)
}

fn handle(files: &mut Fs<Disk>, r: &mut Request) -> Result<(), Error> {
    r.results = [0; 4];
    match r.op {
        fs::STAT => {
            let stats = files.stats();
            r.results = [
                stats.blocks as u64,
                stats.free_blocks as u64,
                stats.used_inodes as u64,
                stats.errors as u64,
            ];
            if !files.mounted() {
                return Err(Error::NotMounted);
            }
        }
        fs::LIST => {
            let dir = id(r.args[0])?;
            let mut after = if r.args[1] == fs::LIST_START {
                None
            } else {
                Some(id(r.args[1])?)
            };
            let mut count = 0;
            while count < fs::MAX_ENTRIES {
                let Some(child) = files.next_child(dir, after)? else {
                    break;
                };
                let info = files.info(child)?;
                let mut entry = Entry {
                    id: child,
                    kind: info.kind as u8,
                    name_len: info.name_len,
                    size: info.size,
                    stamp: info.stamp,
                    name: [0; 64],
                };
                entry.name[..info.name_len as usize]
                    .copy_from_slice(&info.name[..info.name_len as usize]);
                unsafe {
                    write_volatile((data() as *mut Entry).add(count), entry);
                }
                count += 1;
                after = Some(child);
            }
            let more = after.is_some()
                && count == fs::MAX_ENTRIES
                && files.next_child(dir, after)?.is_some();
            r.results = [
                count as u64,
                after.map_or(fs::LIST_START, u64::from),
                more as u64,
                0,
            ];
        }
        fs::RESOLVE => r.results[0] = files.resolve(name(r)?)? as u64,
        fs::CREATE => {
            let kind = match r.args[1] {
                1 => Kind::File,
                2 => Kind::Directory,
                _ => return Err(Error::Invalid),
            };
            let parent = id(r.args[0])?;
            let new = files.create(parent, name(r)?, kind)?;
            r.results[0] = new as u64;
        }
        fs::READ => {
            let file = id(r.args[0])?;
            let length = (r.args[2] as usize).min(fs::DATA_BYTES);
            let buffer = unsafe { core::slice::from_raw_parts_mut(data(), length) };
            let offset = usize::try_from(r.args[1]).map_err(|_| Error::Invalid)?;
            r.results[0] = files.read(file, offset, buffer)? as u64;
            r.results[1] = files.info(file)?.size as u64;
        }
        fs::WRITE => {
            let file = id(r.args[0])?;
            let length = r.args[1] as usize;
            if length > fs::DATA_BYTES {
                return Err(Error::TooLarge);
            }
            let buffer = unsafe { core::slice::from_raw_parts(data(), length) };
            files.write(file, buffer)?;
        }
        fs::REMOVE => files.remove(id(r.args[0])?)?,
        fs::RENAME => {
            let target = id(r.args[0])?;
            let parent = id(r.args[1])?;
            files.rename(target, parent, name(r)?)?
        }
        _ => return Err(Error::Invalid),
    }
    Ok(())
}

#[unsafe(no_mangle)]
extern "C" fn service_main() -> ! {
    // Holding both receive and send handles means the kernel granted this process its endpoints.
    if runtime::call(6, 0, 0).status != 3 || runtime::call(5, 0, 0).status != 3 {
        runtime::fail();
    }
    let files = unsafe { &mut *addr_of_mut!(FILES) };
    if runtime::call(9, 0, 0).status == 0 {
        // A damaged or unformatted volume stays unmounted; requests then report it.
        let _ = files.mount();
    }
    let mut sequence = 1;
    loop {
        let message = runtime::call(2, 1, 0);
        if message.status == 1 {
            runtime::report(sequence, 0);
            sequence = sequence.wrapping_add(1);
            runtime::sleep(1);
            continue;
        }
        if message.status != 0 || message.sender != 4 {
            runtime::fail();
        }
        let mut r = unsafe { read_volatile(request()) };
        let status = if files.mounted() || r.op == fs::STAT {
            match handle(files, &mut r) {
                Ok(()) => 0,
                Err(error) => error.code(),
            }
        } else {
            fs::NOT_MOUNTED
        };
        r.status = status;
        unsafe { write_volatile(request(), r) };
        while runtime::call(1, 0, message.word).status == 1 {
            runtime::sleep(1);
        }
    }
}
