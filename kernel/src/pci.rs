//! PCI configuration space (mechanism #1) and the USB host controllers
//! handed to the network service: slot 0 is the USB 2.0 (EHCI) controller,
//! slots 1..=4 the USB 1.1 (UHCI) companions.
use core::{
    arch::asm,
    sync::atomic::{AtomicU32, AtomicU64, Ordering},
};

pub const SLOTS: usize = 5;
const NONE: u32 = u32::MAX;
/// bus << 16 | device << 11 | function << 8, per slot.
static LOCATION: [AtomicU32; SLOTS] = [const { AtomicU32::new(NONE) }; SLOTS];
/// EHCI: register base (physical memory). UHCI: I/O port base.
static BASE: [AtomicU64; SLOTS] = [const { AtomicU64::new(0) }; SLOTS];
static ID: [AtomicU32; SLOTS] = [const { AtomicU32::new(0) }; SLOTS];
/// Bytes of I/O space of a UHCI controller.
const UHCI_PORTS: u16 = 0x20;

fn out32(port: u16, value: u32) {
    unsafe {
        asm!("out dx, eax", in("dx") port, in("eax") value, options(nomem, nostack, preserves_flags));
    }
}
fn in32(port: u16) -> u32 {
    let value: u32;
    unsafe {
        asm!("in eax, dx", in("dx") port, out("eax") value, options(nomem, nostack, preserves_flags));
    }
    value
}
fn out16(port: u16, value: u16) {
    unsafe {
        asm!("out dx, ax", in("dx") port, in("ax") value, options(nomem, nostack, preserves_flags));
    }
}
fn in16(port: u16) -> u16 {
    let value: u16;
    unsafe {
        asm!("in ax, dx", in("dx") port, out("ax") value, options(nomem, nostack, preserves_flags));
    }
    value
}
/// IF clear.
pub fn read(location: u32, offset: u8) -> u32 {
    out32(0xcf8, 0x8000_0000 | location | (offset as u32 & 0xfc));
    in32(0xcfc)
}
/// IF clear.
pub fn write(location: u32, offset: u8, value: u32) {
    out32(0xcf8, 0x8000_0000 | location | (offset as u32 & 0xfc));
    out32(0xcfc, value);
}

/// Boot CPU, IF clear. Finds the first EHCI controller with a usable 32-bit
/// memory BAR and up to four UHCI controllers with an I/O BAR.
pub fn initialize() {
    let mut uhci = 1;
    for bus in 0..256u32 {
        for device in 0..32u32 {
            let base = bus << 16 | device << 11;
            if read(base, 0) & 0xffff == 0xffff {
                continue;
            }
            let functions = if read(base, 0x0c) >> 16 & 0x80 != 0 { 8 } else { 1 };
            for function in 0..functions {
                let location = base | function << 8;
                let id = read(location, 0);
                if id & 0xffff == 0xffff {
                    continue;
                }
                match read(location, 0x08) >> 8 {
                    0x0c0320 if LOCATION[0].load(Ordering::Relaxed) == NONE => {
                        let bar = read(location, 0x10);
                        let mut registers = (bar & !0xf) as u64;
                        if bar >> 1 & 3 == 2 {
                            registers |= (read(location, 0x14) as u64) << 32;
                        }
                        if bar & 1 != 0 || registers == 0 || registers >= 0x1_0000_0000 {
                            continue;
                        }
                        record(0, location, registers, id);
                    }
                    0x0c0300 if uhci < SLOTS => {
                        let bar = read(location, 0x20);
                        let io = (bar & 0xffe0) as u64;
                        if bar & 1 == 0 || io == 0 {
                            continue;
                        }
                        record(uhci, location, io, id);
                        uhci += 1;
                    }
                    _ => {}
                }
            }
        }
    }
}
fn record(slot: usize, location: u32, base: u64, id: u32) {
    BASE[slot].store(base, Ordering::Relaxed);
    ID[slot].store(id, Ordering::Relaxed);
    LOCATION[slot].store(location, Ordering::Relaxed);
}
/// Location and base (see [`BASE`]) of controller `slot`, if present.
pub fn controller(slot: usize) -> Option<(u32, u64)> {
    let location = LOCATION.get(slot)?.load(Ordering::Relaxed);
    (location != NONE).then(|| (location, BASE[slot].load(Ordering::Relaxed)))
}
pub fn id(slot: usize) -> u32 {
    ID.get(slot).map_or(0, |id| id.load(Ordering::Relaxed))
}
pub fn ehci() -> Option<(u32, u64)> {
    controller(0)
}
/// Registers the network service may change: command/status and the
/// capability and legacy-support area. BARs stay fixed, so the mapped
/// register page and I/O ranges cannot be moved elsewhere.
pub fn writable(offset: u64) -> bool {
    offset == 0x04 || (0x40..0x100).contains(&offset)
}
/// Whether `port`..`port + width` lies inside a UHCI controller's registers.
fn uhci_port(port: u64, width: u64) -> bool {
    (1..SLOTS).any(|slot| {
        controller(slot).is_some_and(|(_, io)| port >= io && port + width <= io + UHCI_PORTS as u64)
    })
}
/// UHCI register access for the network service: op 0..=2 read 8/16/32
/// bits, 3..=5 write them. Returns `None` for ports outside UHCI registers.
pub fn port_io(port: u64, op: u64, value: u32) -> Option<u32> {
    let width = 1 << (op % 3);
    if op > 5 || !uhci_port(port, width) || port % width != 0 {
        return None;
    }
    let port = port as u16;
    Some(match op {
        0 => crate::input(port) as u32,
        1 => in16(port) as u32,
        2 => in32(port),
        3 => {
            crate::out(port, value as u8);
            0
        }
        4 => {
            out16(port, value as u16);
            0
        }
        _ => {
            out32(port, value);
            0
        }
    })
}
