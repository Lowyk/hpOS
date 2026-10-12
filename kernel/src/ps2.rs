use core::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use hpos_service_protocol::ByteQueue;

pub static KEYS: ByteQueue<256> = ByteQueue::new();
pub static MOUSE: ByteQueue<256> = ByteQueue::new();
static READY: AtomicBool = AtomicBool::new(false);
static ID: AtomicU8 = AtomicU8::new(0);
pub fn ready() -> bool {
    READY.load(Ordering::Relaxed)
}
pub fn id() -> u8 {
    ID.load(Ordering::Relaxed)
}

fn wait_write() -> bool {
    for _ in 0..100_000 {
        if crate::input(0x64) & 2 == 0 {
            return true;
        }
    }
    false
}
fn command(byte: u8) -> bool {
    if !wait_write() {
        return false;
    }
    crate::out(0x64, byte);
    true
}
fn data(byte: u8) -> bool {
    if !wait_write() {
        return false;
    }
    crate::out(0x60, byte);
    true
}
fn receive(aux: bool) -> Option<u8> {
    for _ in 0..100_000 {
        let status = crate::input(0x64);
        if status & 1 != 0 {
            let byte = crate::input(0x60);
            if status & 0xc0 == 0 && (status & 0x20 != 0) == aux {
                return Some(byte);
            }
        }
    }
    None
}
fn mouse_command(byte: u8) -> bool {
    for _ in 0..3 {
        if !command(0xd4) || !data(byte) {
            return false;
        }
        match receive(true) {
            Some(0xfa) => return true,
            Some(0xfe) => continue,
            _ => return false,
        }
    }
    false
}
fn sample(rate: u8) -> bool {
    mouse_command(0xf3) && mouse_command(rate)
}
/// Boot CPU, IF clear. Keyboard translation setting is preserved.
pub fn initialize() {
    if !command(0xad) {
        return;
    }
    command(0xa7);
    for _ in 0..256 {
        if crate::input(0x64) & 1 == 0 {
            break;
        }
        crate::input(0x60);
    }
    if !command(0x20) {
        command(0xae);
        return;
    }
    let Some(config) = receive(false) else {
        command(0xae);
        return;
    };
    let mut ready = command(0xa8) && mouse_command(0xf5) && mouse_command(0xf6);
    let mut id = 0;
    if ready {
        let extension = sample(200) && sample(100) && sample(80) && mouse_command(0xf2);
        ready = ready && extension;
        if extension {
            match receive(true) {
                Some(value) => id = value,
                None => ready = false,
            }
        }
        if id != 0 && id != 3 && id != 4 {
            ready = false;
        }
        ready =
            ready && sample(100) && mouse_command(0xe8) && mouse_command(2) && mouse_command(0xf4);
    }
    // IRQ1, optional IRQ12; re-enable both clocks and restore the keyboard port.
    let config = (config | 1 | if ready { 2 } else { 0 }) & !(0x30 | if ready { 0 } else { 2 });
    if !command(0x60) || !data(config) {
        ready = false;
    }
    command(0xae);
    if !ready {
        command(0xa7);
    }
    READY.store(ready, Ordering::Relaxed);
    ID.store(id, Ordering::Relaxed);
}

/// Moves pending controller bytes into the queues. IF clear.
///
/// Reprogramming the 8259 around a BIOS call clears its request register, so
/// an IRQ1/IRQ12 edge raised during the call is lost. The byte stays in the
/// controller's output buffer, and while it does the controller raises no
/// further interrupts: keyboard and mouse would go silent for good.
pub fn drain() {
    for _ in 0..16 {
        let status = crate::input(0x64);
        if status & 1 == 0 {
            break;
        }
        let byte = crate::input(0x60);
        if status & 0xc0 != 0 {
            continue;
        }
        if status & 0x20 != 0 {
            MOUSE.push(byte);
        } else {
            KEYS.push(byte);
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn ps2_interrupt(aux_irq: u64) {
    drain();
    if aux_irq != 0 {
        crate::out(0xa0, 0x20);
    }
    crate::out(0x20, 0x20);
}
