//! What the network service needs from a USB host controller driver, and
//! helpers both drivers share.
use crate::runtime;

/// Transfer failure code for a timeout; other codes are controller status bits.
pub const TIMEOUT: u32 = 0x100;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Port {
    Empty,
    /// Reset and enabled: the device is ours.
    Ready { low_speed: bool },
    /// Not high speed: handed to a USB 1.1 companion controller.
    /// Port status before and after the reset, for the log.
    Slow { before: u32, after: u32 },
}

pub trait Host {
    /// Fresh device at address 0 on endpoint 0.
    fn reset_device(&mut self, low_speed: bool);
    fn set_address(&mut self, address: u8, max_packet: u16);
    fn control_in(&mut self, setup: [u8; 8], out: &mut [u8]) -> Result<usize, u32>;
    fn control_out(&mut self, setup: [u8; 8], data: &[u8]) -> Result<(), u32>;
    fn configure_bulk(&mut self, bulk_in: (u8, u16), bulk_out: (u8, u16));
    /// The bulk OUT buffer, once the previous OUT transfer has finished.
    fn out_buffer(&mut self) -> Result<&'static mut [u8], u32>;
    /// Sends the first `len` bytes of the OUT buffer.
    fn submit(&mut self, len: usize);
    /// Queues a bulk IN transfer if none is pending.
    fn receive(&mut self);
    /// A finished bulk IN transfer, if any: its data, or the error code.
    fn received(&mut self) -> Option<Result<&'static [u8], u32>>;
    /// Clears a stalled bulk endpoint and restarts its data toggle.
    fn clear_halt(&mut self, input: bool) -> Result<(), u32>;
    /// Controller and port state packed for the log after a failure.
    fn debug(&self, port: u32) -> u32;
}

pub fn ticks() -> u64 {
    runtime::call(5, 0, 0).word
}
/// Polls `ready` until it holds or `limit` ticks (10 ms each) pass.
pub fn wait(limit: u64, mut ready: impl FnMut() -> bool) -> bool {
    let start = ticks();
    loop {
        for _ in 0..2000 {
            if ready() {
                return true;
            }
            core::hint::spin_loop();
        }
        if ticks().wrapping_sub(start) > limit {
            return ready();
        }
        runtime::sleep(1);
    }
}
/// PCI configuration of controller `slot` (0 EHCI, 1..=4 UHCI).
pub fn config_read(slot: u64, offset: u64) -> u32 {
    runtime::call(11, 1 | slot << 8, offset).word as u32
}
pub fn config_write(slot: u64, offset: u64, value: u32) {
    let _ = runtime::call(11, 2 | slot << 8, offset << 32 | value as u64);
}
