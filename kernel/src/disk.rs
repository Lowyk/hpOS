//! The hpOS data partition on the boot drive: the only storage the kernel writes.
//!
//! A partition qualifies only when the boot drive's MBR lists it with type 0x7f
//! and its first sector carries the HPFS magic. Any other disk, including a
//! multi-boot stick that merely contains the hpOS image as a file, is left alone.
use crate::bios::{self, BOUNCE, BOUNCE_SECTORS, Op};
use core::{
    ptr::{copy_nonoverlapping, read_volatile},
    sync::atomic::{AtomicU8, AtomicU64, Ordering},
};

pub const WINDOW_FRAMES: usize = 16;
pub const MAX_SECTORS: u64 = (WINDOW_FRAMES as u64 * 4096) / 512;
const PARTITION_TYPE: u8 = 0x7f;
const MAGIC: &[u8; 8] = b"HPFS0001";

#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum State {
    Absent = 0,
    Ready = 1,
    Failed = 2,
}

static STATE: AtomicU8 = AtomicU8::new(State::Absent as u8);
static DRIVE: AtomicU8 = AtomicU8::new(0);
static START: AtomicU64 = AtomicU64::new(0);
static SECTORS: AtomicU64 = AtomicU64::new(0);
static ERRORS: AtomicU64 = AtomicU64::new(0);

pub fn state() -> u64 {
    STATE.load(Ordering::Relaxed) as u64
}
pub fn sectors() -> Option<u64> {
    (STATE.load(Ordering::Relaxed) != State::Absent as u8).then(|| SECTORS.load(Ordering::Relaxed))
}
pub fn errors() -> u64 {
    ERRORS.load(Ordering::Relaxed)
}

fn bounce(offset: usize) -> u8 {
    unsafe { read_volatile((BOUNCE as *const u8).add(offset)) }
}

/// Boot CPU, interrupts disabled, after the BIOS thunk is installed.
pub fn probe(drive: u8) -> State {
    DRIVE.store(drive, Ordering::Relaxed);
    if drive < 0x80 || bios::transfer(Op::Read, drive, 0, 1).is_err() {
        return State::Absent;
    }
    if bounce(510) != 0x55 || bounce(511) != 0xaa {
        return State::Absent;
    }
    let mut found = None;
    for entry in 0..4 {
        let base = 446 + entry * 16;
        let word = |o: usize| {
            u32::from_le_bytes([
                bounce(base + o),
                bounce(base + o + 1),
                bounce(base + o + 2),
                bounce(base + o + 3),
            ])
        };
        if bounce(base + 4) == PARTITION_TYPE && word(8) != 0 && word(12) >= 64 {
            found = Some((word(8) as u64, word(12) as u64));
            break;
        }
    }
    let Some((start, count)) = found else {
        return State::Absent;
    };
    if bios::transfer(Op::Read, drive, start, 1).is_err() || (0..8).any(|i| bounce(i) != MAGIC[i]) {
        return State::Absent;
    }
    START.store(start, Ordering::Relaxed);
    SECTORS.store(count, Ordering::Relaxed);
    STATE.store(State::Ready as u8, Ordering::Relaxed);
    State::Ready
}

/// Copies between the bounce buffer and a 64 KiB window of owned 4 KiB frames.
fn copy_window(frames: &[u64; WINDOW_FRAMES], window_offset: usize, bytes: usize, to_window: bool) {
    let mut done = 0;
    while done < bytes {
        let offset = window_offset + done;
        let frame = frames[offset / 4096];
        let within = offset % 4096;
        let n = (4096 - within).min(bytes - done);
        unsafe {
            let window = (frame as *mut u8).add(within);
            let bounce = (BOUNCE as *mut u8).add(done);
            if to_window {
                copy_nonoverlapping(bounce, window, n);
            } else {
                copy_nonoverlapping(window, bounce, n);
            }
        }
        done += n;
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Error {
    Unavailable,
    Invalid,
    Io,
}

/// Sector numbers are relative to the data partition; data lives in `frames`.
pub fn transfer(
    write: bool,
    sector: u64,
    count: u64,
    frames: &[u64; WINDOW_FRAMES],
) -> Result<(), Error> {
    if STATE.load(Ordering::Relaxed) == State::Absent as u8 {
        return Err(Error::Unavailable);
    }
    let total = SECTORS.load(Ordering::Relaxed);
    if count == 0 || count > MAX_SECTORS || sector.checked_add(count).is_none_or(|end| end > total)
    {
        return Err(Error::Invalid);
    }
    let drive = DRIVE.load(Ordering::Relaxed);
    let start = START.load(Ordering::Relaxed);
    let mut done = 0u64;
    while done < count {
        let n = (count - done).min(BOUNCE_SECTORS as u64);
        let offset = (done * 512) as usize;
        let bytes = (n * 512) as usize;
        if write {
            copy_window(frames, offset, bytes, false);
        }
        let op = if write { Op::Write } else { Op::Read };
        if bios::transfer(op, drive, start + sector + done, n as u16).is_err() {
            ERRORS.fetch_add(1, Ordering::Relaxed);
            STATE.store(State::Failed as u8, Ordering::Relaxed);
            return Err(Error::Io);
        }
        if !write {
            copy_window(frames, offset, bytes, true);
        }
        done += n;
    }
    STATE.store(State::Ready as u8, Ordering::Relaxed);
    Ok(())
}
