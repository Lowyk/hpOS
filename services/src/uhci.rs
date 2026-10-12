//! USB 1.1 host controller (UHCI) driver for one full-speed device: the
//! same transfers as the EHCI driver, built from one descriptor per packet.
//! Registers are I/O ports reached through the kernel (syscall 12).
use crate::{
    host::{self, Host, Port, TIMEOUT, wait},
    runtime,
};
use core::{
    ptr::{read_volatile, write_volatile},
    sync::atomic::{Ordering, fence},
};
use hpos_service_protocol::NET_DMA;

// Layout in the upper half of the DMA window (the EHCI driver uses the lower).
const FRAME_LIST: usize = 0x10000;
const CONTROL_QH: usize = 0x11000;
const OUT_QH: usize = 0x11010;
const IN_QH: usize = 0x11020;
const CONTROL_TD: usize = 0x11100;
const CONTROL_TDS: usize = 160;
const IN_TD: usize = 0x12000;
const OUT_TD: usize = 0x13000;
const BULK_TDS: usize = 256;
const SETUP: usize = 0x14000;
const CONTROL_BUFFER: usize = 0x15000;
const CONTROL_BYTES: usize = 0x1000;
const OUT_BUFFER: usize = 0x18000;
const IN_BUFFER: usize = 0x1c000;
const BULK_BYTES: usize = 0x4000;

// Registers.
const USBCMD: u64 = 0x00;
const USBSTS: u64 = 0x02;
const USBINTR: u64 = 0x04;
const FRNUM: u64 = 0x06;
const FRBASEADD: u64 = 0x08;
const SOFMOD: u64 = 0x0c;
const PORTSC: u64 = 0x10;

const RUN: u32 = 1 << 0;
const HCRESET: u32 = 1 << 1;
const GRESET: u32 = 1 << 2;
const CONFIGURED: u32 = 1 << 6;
const MAX_PACKET_64: u32 = 1 << 7;
const HALTED: u32 = 1 << 5;

const PORT_CONNECTED: u32 = 1 << 0;
const PORT_CONNECT_CHANGE: u32 = 1 << 1;
const PORT_ENABLED: u32 = 1 << 2;
const PORT_ENABLE_CHANGE: u32 = 1 << 3;
const PORT_LOW_SPEED: u32 = 1 << 8;
const PORT_RESET: u32 = 1 << 9;
const PORT_CHANGES: u32 = PORT_CONNECT_CHANGE | PORT_ENABLE_CHANGE;

// Transfer descriptor.
const TERMINATE: u32 = 1;
const QUEUE: u32 = 2;
const DEPTH_FIRST: u32 = 4;
const ACTIVE: u32 = 1 << 23;
/// Stalled, data buffer error, babble, CRC/timeout, bit stuffing.
const ERRORS: u32 = 0x76 << 16;
const IOC: u32 = 1 << 24;
const LOW_SPEED: u32 = 1 << 26;
const THREE_ERRORS: u32 = 3 << 27;
const SHORT_PACKET_DETECT: u32 = 1 << 29;
const PID_SETUP: u32 = 0x2d;
const PID_IN: u32 = 0x69;
const PID_OUT: u32 = 0xe1;

pub struct Controller {
    slot: u64,
    io: u64,
    dma: u64,
    pub id: u32,
    low_speed: bool,
    address: u8,
    max_packet: u16,
    bulk_in: (u8, u16),
    bulk_out: (u8, u16),
    in_toggle: bool,
    out_toggle: bool,
    in_count: usize,
    out_count: usize,
    in_queued: bool,
    out_queued: bool,
}

fn actual(status: u32) -> usize {
    ((status & 0x7ff) + 1) as usize & 0x7ff
}

impl Controller {
    /// UHCI controller `index` (0..=3), if the kernel found one.
    pub fn open(index: u64) -> Option<Self> {
        let slot = index + 1;
        let info = runtime::call(11, slot << 8, 0);
        if info.status != 0 {
            return None;
        }
        Some(Self {
            slot,
            io: info.sender & 0xffff_ffff,
            dma: info.word,
            id: runtime::call(11, 3 | slot << 8, 0).word as u32,
            low_speed: false,
            address: 0,
            max_packet: 8,
            bulk_in: (0, 64),
            bulk_out: (0, 64),
            in_toggle: false,
            out_toggle: false,
            in_count: 0,
            out_count: 0,
            in_queued: false,
            out_queued: false,
        })
    }
    fn read16(&self, register: u64) -> u32 {
        runtime::call(12, self.io + register, 1 << 32).word as u32
    }
    fn write16(&self, register: u64, value: u32) {
        let _ = runtime::call(12, self.io + register, 4 << 32 | (value & 0xffff) as u64);
    }
    fn write32(&self, register: u64, value: u32) {
        let _ = runtime::call(12, self.io + register, 5 << 32 | value as u64);
    }
    fn write8(&self, register: u64, value: u32) {
        let _ = runtime::call(12, self.io + register, 3 << 32 | (value & 0xff) as u64);
    }
    fn phys(&self, offset: usize) -> u32 {
        (self.dma + offset as u64) as u32
    }
    fn dma_read(offset: usize) -> u32 {
        unsafe { read_volatile((NET_DMA as usize + offset) as *const u32) }
    }
    fn dma_write(offset: usize, value: u32) {
        unsafe { write_volatile((NET_DMA as usize + offset) as *mut u32, value) }
    }
    fn buffer(offset: usize, len: usize) -> &'static mut [u8] {
        unsafe { core::slice::from_raw_parts_mut((NET_DMA as usize + offset) as *mut u8, len) }
    }

    /// Takes the controller from the BIOS, resets it and starts the frame list.
    pub fn start(&mut self) -> bool {
        let command = host::config_read(self.slot, 0x04) & 0xffff;
        host::config_write(self.slot, 0x04, command | 0x05);
        // USBLEGSUP: no SMIs or keyboard emulation; clear their status.
        host::config_write(self.slot, 0xc0, 0x8f00);
        self.write16(USBCMD, GRESET);
        runtime::sleep(2);
        self.write16(USBCMD, 0);
        runtime::sleep(1);
        self.write16(USBCMD, HCRESET);
        if !wait(50, || self.read16(USBCMD) & HCRESET == 0) {
            return false;
        }
        self.write16(USBINTR, 0);
        self.write16(USBSTS, 0x3f);
        for queue in [CONTROL_QH, OUT_QH, IN_QH] {
            Self::dma_write(queue + 4, TERMINATE);
        }
        Self::dma_write(CONTROL_QH, self.phys(OUT_QH) | QUEUE);
        Self::dma_write(OUT_QH, self.phys(IN_QH) | QUEUE);
        Self::dma_write(IN_QH, TERMINATE);
        let head = self.phys(CONTROL_QH) | QUEUE;
        for frame in 0..1024 {
            Self::dma_write(FRAME_LIST + frame * 4, head);
        }
        fence(Ordering::SeqCst);
        self.write32(FRBASEADD, self.phys(FRAME_LIST));
        self.write16(FRNUM, 0);
        self.write8(SOFMOD, 64);
        self.write16(USBCMD, RUN | CONFIGURED | MAX_PACKET_64);
        wait(5, || self.read16(USBSTS) & HALTED == 0)
    }
    pub fn port(&self, port: u32) -> u32 {
        self.read16(PORTSC + 2 * port as u64)
    }
    fn set_port(&self, port: u32, value: u32) {
        self.write16(PORTSC + 2 * port as u64, value & !PORT_CHANGES);
    }
    pub fn connected(&self, port: u32) -> bool {
        self.port(port) & PORT_CONNECTED != 0
    }
    /// Returns and acknowledges a connect-status change.
    pub fn take_change(&self, port: u32) -> bool {
        let value = self.port(port);
        if value & PORT_CONNECT_CHANGE != 0 {
            self.write16(PORTSC + 2 * port as u64, (value & !PORT_CHANGES) | PORT_CONNECT_CHANGE);
            return true;
        }
        false
    }
    pub fn reset_port(&mut self, port: u32) -> Port {
        let value = self.port(port);
        if value & PORT_CONNECTED == 0 {
            return Port::Empty;
        }
        self.set_port(port, value | PORT_RESET);
        runtime::sleep(6);
        let value = self.port(port);
        self.set_port(port, value & !PORT_RESET);
        runtime::sleep(1);
        for _ in 0..10 {
            let value = self.port(port);
            if value & PORT_CONNECTED == 0 {
                return Port::Empty;
            }
            if value & PORT_ENABLED != 0 {
                runtime::sleep(2);
                return Port::Ready {
                    low_speed: value & PORT_LOW_SPEED != 0,
                };
            }
            if value & PORT_CHANGES != 0 {
                self.write16(PORTSC + 2 * port as u64, value);
            }
            self.set_port(port, value | PORT_ENABLED);
            runtime::sleep(1);
        }
        Port::Empty
    }

    /// One descriptor. `max` is the packet size; 0 for a zero-length packet.
    #[allow(clippy::too_many_arguments)]
    fn descriptor(&self, at: usize, next: Option<usize>, pid: u32, endpoint: u8, toggle: bool, buffer: usize, max: usize, extra: u32) {
        Self::dma_write(at, next.map_or(TERMINATE, |n| self.phys(n) | DEPTH_FIRST));
        Self::dma_write(at + 12, self.phys(buffer));
        let max_len = if max == 0 { 0x7ff } else { (max - 1) as u32 };
        let token = pid
            | (self.address as u32) << 8
            | (endpoint as u32) << 15
            | (toggle as u32) << 19
            | max_len << 21;
        Self::dma_write(at + 8, token);
        let control = ACTIVE | THREE_ERRORS | extra | if self.low_speed { LOW_SPEED } else { 0 };
        fence(Ordering::SeqCst);
        Self::dma_write(at + 4, control);
    }
    fn status(at: usize) -> u32 {
        Self::dma_read(at + 4)
    }
    fn max_len(at: usize) -> usize {
        ((Self::dma_read(at + 8) >> 21) + 1) as usize & 0x7ff
    }
    fn start_queue(&self, queue: usize, first: usize) {
        fence(Ordering::SeqCst);
        Self::dma_write(queue + 4, self.phys(first));
    }
    fn stop_queue(queue: usize) {
        Self::dma_write(queue + 4, TERMINATE);
    }

    fn control(&mut self, setup: [u8; 8], data_in: bool, len: usize) -> Result<usize, u32> {
        Self::buffer(SETUP, 8).copy_from_slice(&setup);
        let max = self.max_packet.max(8) as usize;
        let len = len.min((CONTROL_TDS - 2) * max).min(CONTROL_BYTES);
        let data = len.div_ceil(max);
        let status = CONTROL_TD + (data + 1) * 16;
        self.descriptor(CONTROL_TD, Some(CONTROL_TD + 16), PID_SETUP, 0, false, SETUP, 8, 0);
        for n in 0..data {
            let at = CONTROL_TD + (n + 1) * 16;
            let chunk = max.min(len - n * max);
            let (pid, extra) = if data_in { (PID_IN, SHORT_PACKET_DETECT) } else { (PID_OUT, 0) };
            self.descriptor(at, Some(at + 16), pid, 0, n % 2 == 0, CONTROL_BUFFER + n * max, chunk, extra);
        }
        let status_pid = if data > 0 && data_in { PID_OUT } else { PID_IN };
        self.descriptor(status, None, status_pid, 0, true, CONTROL_BUFFER, 0, IOC);
        self.start_queue(CONTROL_QH, CONTROL_TD);
        let mut moved = 0;
        let mut outcome = Err(TIMEOUT);
        let done = wait(50, || {
            moved = 0;
            for n in 0..data + 2 {
                let at = CONTROL_TD + n * 16;
                let s = Self::status(at);
                if s & ACTIVE != 0 {
                    return false;
                }
                if s & ERRORS != 0 {
                    outcome = Err(s >> 16 & 0xff);
                    return true;
                }
                if (1..=data).contains(&n) {
                    moved += actual(s);
                    if data_in && actual(s) < Self::max_len(at) {
                        // A short packet stops the queue: go on to the status stage.
                        let s = Self::status(status);
                        if s & ACTIVE != 0 {
                            Self::dma_write(CONTROL_QH + 4, self.phys(status));
                            return false;
                        }
                        if s & ERRORS != 0 {
                            outcome = Err(s >> 16 & 0xff);
                        } else {
                            outcome = Ok(moved);
                        }
                        return true;
                    }
                }
            }
            outcome = Ok(moved);
            true
        });
        if !done {
            Self::stop_queue(CONTROL_QH);
            return Err(TIMEOUT);
        }
        if outcome.is_err() {
            Self::stop_queue(CONTROL_QH);
        }
        outcome
    }
    /// Builds a bulk chain of one descriptor per packet; returns its length.
    fn bulk(&mut self, input: bool, len: usize) -> usize {
        let (endpoint, max) = if input { self.bulk_in } else { self.bulk_out };
        let max = max.clamp(8, 64) as usize;
        let count = len.div_ceil(max).clamp(1, BULK_TDS);
        let (first, buffer, pid, toggle) = if input {
            (IN_TD, IN_BUFFER, PID_IN, self.in_toggle)
        } else {
            (OUT_TD, OUT_BUFFER, PID_OUT, self.out_toggle)
        };
        for n in 0..count {
            let at = first + n * 16;
            let next = (n + 1 < count).then_some(at + 16);
            let chunk = max.min(len.saturating_sub(n * max));
            let extra = if input { SHORT_PACKET_DETECT } else { 0 } | if next.is_none() { IOC } else { 0 };
            self.descriptor(at, next, pid, endpoint, toggle ^ (n % 2 == 1), buffer + n * max, chunk, extra);
        }
        self.start_queue(if input { IN_QH } else { OUT_QH }, first);
        count
    }
}

impl Host for Controller {
    fn reset_device(&mut self, low_speed: bool) {
        self.low_speed = low_speed;
        self.address = 0;
        self.max_packet = 8;
        self.in_queued = false;
        self.out_queued = false;
        for queue in [CONTROL_QH, OUT_QH, IN_QH] {
            Self::stop_queue(queue);
        }
    }
    fn set_address(&mut self, address: u8, max_packet: u16) {
        self.address = address;
        self.max_packet = max_packet;
    }
    fn control_in(&mut self, setup: [u8; 8], out: &mut [u8]) -> Result<usize, u32> {
        let n = self.control(setup, true, out.len())?;
        let n = n.min(out.len());
        out[..n].copy_from_slice(&Self::buffer(CONTROL_BUFFER, n)[..n]);
        Ok(n)
    }
    fn control_out(&mut self, setup: [u8; 8], data: &[u8]) -> Result<(), u32> {
        let len = data.len().min(CONTROL_BYTES);
        Self::buffer(CONTROL_BUFFER, len).copy_from_slice(&data[..len]);
        self.control(setup, false, len).map(|_| ())
    }
    fn configure_bulk(&mut self, bulk_in: (u8, u16), bulk_out: (u8, u16)) {
        self.bulk_in = bulk_in;
        self.bulk_out = bulk_out;
        self.in_toggle = false;
        self.out_toggle = false;
        self.in_queued = false;
        self.out_queued = false;
        Self::stop_queue(OUT_QH);
        Self::stop_queue(IN_QH);
    }
    fn out_buffer(&mut self) -> Result<&'static mut [u8], u32> {
        if self.out_queued {
            let last = OUT_TD + (self.out_count - 1) * 16;
            let count = self.out_count;
            let finished = wait(20, || {
                Self::status(last) & ACTIVE == 0
                    || (0..count).any(|n| Self::status(OUT_TD + n * 16) & ERRORS != 0)
            });
            self.out_queued = false;
            if !finished {
                Self::stop_queue(OUT_QH);
                return Err(TIMEOUT);
            }
            if let Some(s) = (0..count).map(|n| Self::status(OUT_TD + n * 16)).find(|s| s & ERRORS != 0) {
                Self::stop_queue(OUT_QH);
                return Err(s >> 16 & 0xff);
            }
            self.out_toggle ^= count % 2 == 1;
        }
        Ok(Self::buffer(OUT_BUFFER, BULK_BYTES))
    }
    fn submit(&mut self, len: usize) {
        self.out_count = self.bulk(false, len.min(BULK_BYTES));
        self.out_queued = true;
    }
    fn receive(&mut self) {
        if !self.in_queued {
            self.in_count = self.bulk(true, BULK_BYTES);
            self.in_queued = true;
        }
    }
    fn received(&mut self) -> Option<Result<&'static [u8], u32>> {
        if !self.in_queued {
            return None;
        }
        let mut finished = self.in_count;
        let mut bytes = 0;
        for n in 0..self.in_count {
            let at = IN_TD + n * 16;
            let s = Self::status(at);
            if s & ACTIVE != 0 {
                return None;
            }
            if s & ERRORS != 0 {
                self.in_queued = false;
                Self::stop_queue(IN_QH);
                return Some(Err(s >> 16 & 0xff));
            }
            bytes += actual(s);
            if actual(s) < Self::max_len(at) {
                finished = n + 1;
                break;
            }
        }
        self.in_queued = false;
        self.in_toggle ^= finished % 2 == 1;
        Some(Ok(&Self::buffer(IN_BUFFER, BULK_BYTES)[..bytes]))
    }
    /// USBSTS (bits 0-7), frame number (8-18, moves while the schedule
    /// runs), PORTSC (19-31).
    fn debug(&self, port: u32) -> u32 {
        (self.read16(USBSTS) & 0xff) | (self.read16(FRNUM) & 0x7ff) << 8 | (self.port(port) & 0x1fff) << 19
    }
    fn clear_halt(&mut self, input: bool) -> Result<(), u32> {
        let endpoint = if input { self.bulk_in.0 | 0x80 } else { self.bulk_out.0 };
        let setup = hpos_net::usb::setup(0x02, hpos_net::usb::CLEAR_FEATURE, 0, endpoint as u16, 0);
        let result = self.control_out(setup, &[]);
        if input {
            Self::stop_queue(IN_QH);
            self.in_toggle = false;
            self.in_queued = false;
        } else {
            Self::stop_queue(OUT_QH);
            self.out_toggle = false;
            self.out_queued = false;
        }
        result
    }
}
