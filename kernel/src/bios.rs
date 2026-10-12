//! Real-mode BIOS disk calls through the thunk in `boot/bios-thunk.S`.
//!
//! The thunk runs with interrupts handled by the BIOS: the PIC returns to its
//! BIOS vectors and masks, IRQ1/IRQ12 stay masked so the BIOS cannot consume
//! PS/2 bytes, and the PIT returns to 18.2 Hz for BIOS timeouts. Everything is
//! restored before the kernel sees another interrupt.
use core::{
    arch::asm,
    ptr::{read_volatile, write_volatile},
    sync::atomic::{AtomicBool, AtomicU8, Ordering},
};

pub const CODE_PAGE: u64 = 0x8000;
pub const DATA_PAGE: u64 = 0x9000;
pub const BOUNCE: u64 = 0x10000;
pub const BOUNCE_SECTORS: u16 = 64;
static BLOB: &[u8] = include_bytes!(concat!(env!("HPOS_BUILD_DIR"), "/thunk.bin"));

const REQUEST_OP: u64 = DATA_PAGE;
const REQUEST_DRIVE: u64 = DATA_PAGE + 1;
const REQUEST_STATUS: u64 = DATA_PAGE + 2;
const REQUEST_CARRY: u64 = DATA_PAGE + 3;
const REQUEST_COUNT: u64 = DATA_PAGE + 4;
const REQUEST_KIND: u64 = DATA_PAGE + 6;
const REQUEST_LBA: u64 = DATA_PAGE + 16;

static BIOS_MASTER_MASK: AtomicU8 = AtomicU8::new(0xff);
static BIOS_SLAVE_MASK: AtomicU8 = AtomicU8::new(0xff);
static KERNEL_PIC: AtomicBool = AtomicBool::new(false);
static INSTALLED: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Read = 0x42,
    Write = 0x43,
}

/// Boot CPU, IF=0, before the kernel page tables: low memory is still writable
/// and the BIOS PIC configuration is still in place.
pub unsafe fn install() {
    if BLOB.len() <= 0x1000 || BLOB.len() > 0x2000 {
        return;
    }
    for (offset, byte) in BLOB.iter().enumerate() {
        unsafe {
            write_volatile((CODE_PAGE as *mut u8).add(offset), *byte);
        }
    }
    BIOS_MASTER_MASK.store(crate::input(0x21), Ordering::Relaxed);
    BIOS_SLAVE_MASK.store(crate::input(0xa1), Ordering::Relaxed);
    INSTALLED.store(true, Ordering::Relaxed);
}

/// Remaps both 8259 controllers with the given vector bases and masks.
pub fn program_pic(master: u8, slave: u8, master_mask: u8, slave_mask: u8) {
    for (port, byte) in [
        (0x20, 0x11),
        (0xa0, 0x11),
        (0x21, master),
        (0xa1, slave),
        (0x21, 4),
        (0xa1, 2),
        (0x21, 1),
        (0xa1, 1),
        (0x21, master_mask),
        (0xa1, slave_mask),
    ] {
        crate::out(port, byte);
        crate::out(0x80, 0);
    }
}
pub fn kernel_pic_installed() {
    KERNEL_PIC.store(true, Ordering::Relaxed);
}

/// Moves `count` sectors between `lba` and the bounce buffer at 0x10000.
pub fn transfer(op: Op, drive: u8, lba: u64, count: u16) -> Result<(), u8> {
    if count == 0 || count > BOUNCE_SECTORS {
        return Err(0xff);
    }
    call(0, op as u8, drive, count, lba)
}
/// VBE function 4F02h: sets `mode` (with the linear-framebuffer bit added).
pub fn set_video_mode(mode: u16) -> Result<(), u8> {
    call(1, 0, 0, mode | 0x4000, 0)
}
fn call(kind: u8, op: u8, drive: u8, count: u16, lba: u64) -> Result<(), u8> {
    if !INSTALLED.load(Ordering::Relaxed) {
        return Err(0xff);
    }
    let flags: u64;
    unsafe {
        asm!("pushfq", "pop {}", "cli", out(reg) flags, options(nomem));
        write_volatile(REQUEST_KIND as *mut u8, kind);
        write_volatile(REQUEST_OP as *mut u8, op);
        write_volatile(REQUEST_DRIVE as *mut u8, drive);
        write_volatile(REQUEST_COUNT as *mut u16, count);
        write_volatile(REQUEST_LBA as *mut u64, lba);
    }
    let kernel_pic = KERNEL_PIC.load(Ordering::Relaxed);
    let (master_mask, slave_mask) = (crate::input(0x21), crate::input(0xa1));
    if kernel_pic {
        program_pic(
            0x08,
            0x70,
            BIOS_MASTER_MASK.load(Ordering::Relaxed) | 0x02,
            BIOS_SLAVE_MASK.load(Ordering::Relaxed) | 0x10,
        );
        crate::out(0x43, 0x36);
        crate::out(0x40, 0);
        crate::out(0x40, 0);
    }
    unsafe {
        let thunk: extern "C" fn() = core::mem::transmute(CODE_PAGE as usize);
        thunk();
    }
    if kernel_pic {
        program_pic(0x20, 0x28, master_mask, slave_mask);
        crate::scheduler::program_timer();
        crate::ps2::drain();
    }
    unsafe {
        crate::process::reload_task_register();
    }
    let (status, carry) = unsafe {
        (
            read_volatile(REQUEST_STATUS as *const u8),
            read_volatile(REQUEST_CARRY as *const u8),
        )
    };
    if flags & 0x200 != 0 {
        unsafe {
            asm!("sti", options(nomem, nostack));
        }
    }
    if carry == 0 && status == 0 {
        Ok(())
    } else {
        Err(status)
    }
}
