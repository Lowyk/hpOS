//! Live display-mode switching through VBE 4F02h, limited to the modes the
//! loader validated at boot. A mode is switchable only if its framebuffer
//! starts at the boot mode's address, so the desktop's mapping stays valid.
use core::{
    ptr::{addr_of, addr_of_mut, read_volatile, write_volatile},
    sync::atomic::{AtomicUsize, Ordering},
};
use hpos_boot_protocol::{BootInfo, MAX_MODES, MODE_TABLE, VideoMode};

const EMPTY: VideoMode = VideoMode {
    number: 0,
    width: 0,
    height: 0,
    pitch: 0,
    framebuffer: 0,
    score: 0,
};
static mut MODES: [VideoMode; MAX_MODES as usize] = [EMPTY; MAX_MODES as usize];
static COUNT: AtomicUsize = AtomicUsize::new(0);
static CURRENT: AtomicUsize = AtomicUsize::new(0);
/// Bit n set: mode n can be switched to.
static USABLE: AtomicUsize = AtomicUsize::new(0);
const MAX_FRAMEBUFFER: u64 = 0x100_0000;

fn mode(index: usize) -> VideoMode {
    unsafe { addr_of!(MODES).cast::<VideoMode>().add(index).read() }
}

/// Boot CPU, before user processes exist.
pub fn initialize(info: &BootInfo) {
    let count = info.mode_count.min(MAX_MODES) as usize;
    if info.framebuffer_len().is_none() || count == 0 {
        return;
    }
    let mut usable = 0;
    let mut current = None;
    for index in 0..count {
        let m = unsafe { read_volatile((MODE_TABLE as *const VideoMode).add(index)) };
        unsafe {
            addr_of_mut!(MODES).cast::<VideoMode>().add(index).write(m);
        }
        let bytes = m.pitch as u64 * m.height as u64;
        if m.framebuffer as u64 == info.framebuffer
            && (800..=1920).contains(&m.width)
            && (600..=1200).contains(&m.height)
            && m.pitch as u32 >= m.width as u32 * 4
            && bytes <= MAX_FRAMEBUFFER
            && info.framebuffer + bytes <= 0x1_0000_0000
        {
            usable |= 1 << index;
        }
        if m.width as u32 == info.width
            && m.height as u32 == info.height
            && m.pitch as u32 == info.pitch
        {
            current.get_or_insert(index);
        }
    }
    let Some(current) = current else {
        return;
    };
    COUNT.store(count, Ordering::Relaxed);
    CURRENT.store(current, Ordering::Relaxed);
    USABLE.store(usable | 1 << current, Ordering::Relaxed);
}

fn usable(index: usize) -> bool {
    index < COUNT.load(Ordering::Relaxed) && USABLE.load(Ordering::Relaxed) & (1 << index) != 0
}
/// Bytes of framebuffer to map so that every switchable mode fits.
pub fn map_bytes(boot: u64) -> u64 {
    (0..COUNT.load(Ordering::Relaxed))
        .filter(|i| usable(*i))
        .map(|i| mode(i).pitch as u64 * mode(i).height as u64)
        .fold(boot, u64::max)
}
/// Bytes of backbuffer needed for the largest switchable mode.
pub fn back_bytes(boot: u64) -> u64 {
    (0..COUNT.load(Ordering::Relaxed))
        .filter(|i| usable(*i))
        .map(|i| mode(i).width as u64 * mode(i).height as u64 * 4)
        .fold(boot, u64::max)
}
pub fn count() -> usize {
    COUNT.load(Ordering::Relaxed)
}
pub fn current() -> usize {
    CURRENT.load(Ordering::Relaxed)
}
/// width | height << 16 | usable << 32, or None past the end.
pub fn describe(index: usize) -> Option<u64> {
    (index < count()).then(|| {
        let m = mode(index);
        m.width as u64 | (m.height as u64) << 16 | (usable(index) as u64) << 32
    })
}
/// Sets mode `index`; returns width | height << 16 | stride-in-pixels << 32.
pub fn switch(index: usize) -> Result<u64, ()> {
    if !usable(index) {
        return Err(());
    }
    let m = mode(index);
    crate::bios::set_video_mode(m.number).map_err(|_| ())?;
    CURRENT.store(index, Ordering::Relaxed);
    // Keep the fatal-error screen consistent with the real framebuffer layout.
    let info = 0x6000 as *mut BootInfo;
    unsafe {
        write_volatile(addr_of_mut!((*info).width), m.width as u32);
        write_volatile(addr_of_mut!((*info).height), m.height as u32);
        write_volatile(addr_of_mut!((*info).pitch), m.pitch as u32);
    }
    Ok(m.width as u64 | (m.height as u64) << 16 | ((m.pitch / 4) as u64) << 32)
}
