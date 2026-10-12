//! USB 2.0 host controller (EHCI) driver for one high-speed device: control
//! transfers on endpoint 0 and one bulk IN and one bulk OUT endpoint, all on
//! the asynchronous schedule, polled without interrupts.
use crate::{
    host::{self, Host, Port, TIMEOUT, wait},
    runtime,
};
use core::{
    ptr::{read_volatile, write_volatile},
    sync::atomic::{Ordering, fence},
};
use hpos_service_protocol::{NET_DMA, NET_REGISTERS};

// Layout of the DMA window. Controllers with 64-bit addressing (such as
// ICH7) use 68-byte queue heads and 52-byte transfer descriptors, whose
// extra words hold the upper halves of addresses; the slots leave room for
// them and those words stay zero. Buffers are page aligned.
const QH_BYTES: usize = 0x80;
const TD_BYTES: usize = 0x40;
const HEAD: usize = 0;
const CONTROL_QH: usize = HEAD + QH_BYTES;
const OUT_QH: usize = CONTROL_QH + QH_BYTES;
const IN_QH: usize = OUT_QH + QH_BYTES;
const SETUP_TD: usize = IN_QH + QH_BYTES;
const DATA_TD: usize = SETUP_TD + TD_BYTES;
const STATUS_TD: usize = DATA_TD + TD_BYTES;
const OUT_TD: usize = STATUS_TD + TD_BYTES;
const IN_TD: usize = OUT_TD + TD_BYTES;
const _: () = assert!(IN_TD + TD_BYTES <= SETUP);
const SETUP: usize = 0x1000;
const CONTROL_BUFFER: usize = 0x2000;
pub const CONTROL_BYTES: usize = 0x1000;
const OUT_BUFFER: usize = 0x4000;
const IN_BUFFER: usize = 0x8000;
pub const BULK_BYTES: usize = 0x4000;

// Operational registers.
const USBCMD: usize = 0x00;
const USBSTS: usize = 0x04;
const USBINTR: usize = 0x08;
const CTRLDSSEGMENT: usize = 0x10;
const ASYNCLISTADDR: usize = 0x18;
const CONFIGFLAG: usize = 0x40;
const PORTSC: usize = 0x44;

const RUN: u32 = 1 << 0;
const HCRESET: u32 = 1 << 1;
const ASYNC_ENABLE: u32 = 1 << 5;
const HALTED: u32 = 1 << 12;
const ASYNC_STATUS: u32 = 1 << 15;

const PORT_CONNECTED: u32 = 1 << 0;
const PORT_CONNECT_CHANGE: u32 = 1 << 1;
const PORT_ENABLED: u32 = 1 << 2;
const PORT_ENABLE_CHANGE: u32 = 1 << 3;
const PORT_OVERCURRENT_CHANGE: u32 = 1 << 5;
const PORT_RESET: u32 = 1 << 8;
const PORT_POWER: u32 = 1 << 12;
const PORT_OWNER: u32 = 1 << 13;
/// Write-1-to-clear bits that a read-modify-write must not echo back.
const PORT_CHANGES: u32 = PORT_CONNECT_CHANGE | PORT_ENABLE_CHANGE | PORT_OVERCURRENT_CHANGE;

// Transfer descriptor token.
const ACTIVE: u32 = 1 << 7;
const TD_HALTED: u32 = 1 << 6;
const PID_OUT: u32 = 0;
const PID_IN: u32 = 1;
const PID_SETUP: u32 = 2;
const TERMINATE: u32 = 1;

/// A controller step failed: 1 halt, 2 reset, 3 run.
pub struct StartError(pub u32);


pub struct Controller {
    capabilities: usize,
    operational: usize,
    dma: u64,
    pub ports: u32,
    pub id: u32,
    max_packet: u16,
    address: u8,
    bulk_in: (u8, u16),
    bulk_out: (u8, u16),
    in_queued: bool,
    out_queued: bool,
}

impl Controller {
    /// Finds the controller the kernel mapped for this service.
    pub fn open() -> Option<Self> {
        let info = runtime::call(11, 0, 0);
        if info.status != 0 {
            return None;
        }
        let capabilities = NET_REGISTERS as usize + (info.sender & 0xfff) as usize;
        let id = runtime::call(11, 3, 0).word as u32;
        let mut c = Self {
            capabilities,
            operational: 0,
            dma: info.word,
            ports: 0,
            id,
            max_packet: 64,
            address: 0,
            bulk_in: (0, 512),
            bulk_out: (0, 512),
            in_queued: false,
            out_queued: false,
        };
        c.operational = capabilities + (c.capability(0) & 0xff) as usize;
        c.ports = c.capability(4) & 0xf;
        Some(c)
    }
    fn capability(&self, offset: usize) -> u32 {
        unsafe { read_volatile((self.capabilities + offset) as *const u32) }
    }
    fn read(&self, offset: usize) -> u32 {
        unsafe { read_volatile((self.operational + offset) as *const u32) }
    }
    fn write(&self, offset: usize, value: u32) {
        unsafe { write_volatile((self.operational + offset) as *mut u32, value) }
    }
    fn config_read(offset: u64) -> u32 {
        host::config_read(0, offset)
    }
    fn config_write(offset: u64, value: u32) {
        host::config_write(0, offset, value)
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

    /// Takes the controller from the BIOS, resets it and starts the
    /// asynchronous schedule. Returns whether the BIOS let go on request.
    pub fn start(&mut self) -> Result<bool, StartError> {
        // Memory decoding and bus mastering.
        let command = Self::config_read(0x04) & 0xffff;
        Self::config_write(0x04, command | 0x06);
        let mut polite = true;
        let mut pointer = (self.capability(8) >> 8 & 0xff) as u64;
        let mut guard = 0;
        while pointer >= 0x40 && guard < 16 {
            guard += 1;
            let value = Self::config_read(pointer);
            if value & 0xff == 1 {
                // USBLEGSUP: ask for ownership and wait for the BIOS to drop it.
                Self::config_write(pointer, value | 1 << 24);
                if !wait(100, || Self::config_read(pointer) & 1 << 16 == 0) {
                    polite = false;
                    let value = Self::config_read(pointer);
                    Self::config_write(pointer, (value & !(1 << 16)) | 1 << 24);
                }
                // No more SMIs from this controller; clear their status bits.
                Self::config_write(pointer + 4, 0);
                Self::config_write(pointer + 4, 0xe000_0000);
            }
            pointer = (value >> 8 & 0xff) as u64;
        }
        self.write(USBCMD, self.read(USBCMD) & !RUN);
        if !wait(5, || self.read(USBSTS) & HALTED != 0) {
            return Err(StartError(1));
        }
        self.write(USBCMD, HCRESET);
        if !wait(50, || self.read(USBCMD) & HCRESET == 0) {
            return Err(StartError(2));
        }
        if self.capability(8) & 1 != 0 {
            self.write(CTRLDSSEGMENT, 0);
        }
        self.write(USBINTR, 0);
        self.write(USBSTS, 0x3f);
        // Head of the circular list: head -> control -> bulk out -> bulk in.
        self.queue_head(HEAD, CONTROL_QH, 0, 0, 64, true);
        Self::dma_write(HEAD + 0x18, TD_HALTED);
        self.queue_head(CONTROL_QH, OUT_QH, 0, 0, 64, false);
        self.queue_head(OUT_QH, IN_QH, 0, 1, 512, false);
        self.queue_head(IN_QH, HEAD, 0, 2, 512, false);
        fence(Ordering::SeqCst);
        self.write(ASYNCLISTADDR, self.phys(HEAD));
        self.write(USBCMD, 8 << 16 | ASYNC_ENABLE | RUN);
        if !wait(5, || self.read(USBSTS) & HALTED == 0) {
            return Err(StartError(3));
        }
        self.write(CONFIGFLAG, 1);
        runtime::sleep(2);
        if self.capability(4) & 1 << 4 != 0 {
            for port in 0..self.ports {
                let value = self.port(port);
                self.set_port(port, value | PORT_POWER);
            }
            runtime::sleep(3);
        }
        Ok(polite)
    }
    /// Writes a queue head. Control endpoints take the data toggle from each
    /// transfer descriptor; bulk ones keep it in the queue head.
    fn queue_head(&self, at: usize, next: usize, address: u8, endpoint: u8, max: u16, head: bool) {
        let control = endpoint == 0;
        let characteristics = address as u32
            | (endpoint as u32) << 8
            | 2 << 12
            | (control as u32) << 14
            | (head as u32) << 15
            | (max as u32) << 16;
        Self::dma_write(at, self.phys(next) | 2);
        Self::dma_write(at + 4, characteristics);
        Self::dma_write(at + 8, 1 << 30);
        Self::dma_write(at + 0x0c, 0);
        Self::dma_write(at + 0x10, TERMINATE);
        Self::dma_write(at + 0x14, TERMINATE);
        for offset in (0x18..QH_BYTES).step_by(4) {
            Self::dma_write(at + offset, 0);
        }
    }
    /// Changes queue heads with the asynchronous schedule stopped, so the
    /// controller holds no cached copy.
    fn with_schedule_stopped(&mut self, change: impl FnOnce(&mut Self)) {
        self.write(USBCMD, self.read(USBCMD) & !ASYNC_ENABLE);
        wait(5, || self.read(USBSTS) & ASYNC_STATUS == 0);
        change(self);
        fence(Ordering::SeqCst);
        self.write(USBCMD, self.read(USBCMD) | ASYNC_ENABLE);
        wait(5, || self.read(USBSTS) & ASYNC_STATUS != 0);
    }

    pub fn port(&self, port: u32) -> u32 {
        self.read(PORTSC + 4 * port as usize)
    }
    fn set_port(&self, port: u32, value: u32) {
        self.write(PORTSC + 4 * port as usize, value & !PORT_CHANGES);
    }
    pub fn connected(&self, port: u32) -> bool {
        self.port(port) & PORT_CONNECTED != 0
    }
    /// Returns and acknowledges a connect-status change.
    pub fn take_change(&self, port: u32) -> bool {
        let value = self.port(port);
        if value & PORT_CONNECT_CHANGE != 0 {
            self.write(PORTSC + 4 * port as usize, (value & !PORT_CHANGES) | PORT_CONNECT_CHANGE);
            return true;
        }
        false
    }
    /// Resets the port; a device that does not come up enabled after three
    /// tries is not high speed and goes to the companion controller.
    pub fn reset_port(&mut self, port: u32) -> Port {
        let before = self.port(port);
        if before & PORT_CONNECTED == 0 {
            return Port::Empty;
        }
        if before >> 10 & 3 == 1 {
            self.set_port(port, before | PORT_OWNER);
            return Port::Slow { before, after: before };
        }
        let mut after = before;
        for _ in 0..3 {
            let value = self.port(port);
            self.set_port(port, (value & !PORT_ENABLED) | PORT_RESET);
            runtime::sleep(6);
            let value = self.port(port);
            self.set_port(port, value & !PORT_RESET);
            wait(2, || self.port(port) & PORT_RESET == 0);
            // Enabling can lag behind the end of reset.
            wait(5, || self.port(port) & PORT_ENABLED != 0);
            after = self.port(port);
            if after & PORT_CONNECTED == 0 {
                return Port::Empty;
            }
            if after & PORT_ENABLED != 0 {
                runtime::sleep(2);
                return Port::Ready { low_speed: false };
            }
            runtime::sleep(5);
        }
        self.set_port(port, after | PORT_OWNER);
        Port::Slow { before, after }
    }

}

impl Host for Controller {
    fn reset_device(&mut self, _low_speed: bool) {
        self.address = 0;
        self.max_packet = 64;
        self.in_queued = false;
        self.out_queued = false;
        self.with_schedule_stopped(|c| {
            c.queue_head(CONTROL_QH, OUT_QH, 0, 0, 64, false);
            c.queue_head(OUT_QH, IN_QH, 0, 1, 512, false);
            c.queue_head(IN_QH, HEAD, 0, 2, 512, false);
        });
    }
    fn set_address(&mut self, address: u8, max_packet: u16) {
        self.address = address;
        self.max_packet = max_packet;
        self.with_schedule_stopped(|c| c.queue_head(CONTROL_QH, OUT_QH, address, 0, max_packet, false));
    }
    fn configure_bulk(&mut self, bulk_in: (u8, u16), bulk_out: (u8, u16)) {
        self.bulk_in = bulk_in;
        self.bulk_out = bulk_out;
        self.in_queued = false;
        self.out_queued = false;
        let address = self.address;
        self.with_schedule_stopped(|c| {
            c.queue_head(OUT_QH, IN_QH, address, bulk_out.0, bulk_out.1, false);
            c.queue_head(IN_QH, HEAD, address, bulk_in.0, bulk_in.1, false);
        });
    }

    fn control_in(&mut self, setup: [u8; 8], out: &mut [u8]) -> Result<usize, u32> {
        let len = out.len().min(CONTROL_BYTES);
        let n = self.control(setup, true, len)?;
        out[..n].copy_from_slice(&Self::buffer(CONTROL_BUFFER, n)[..n]);
        Ok(n)
    }
    fn control_out(&mut self, setup: [u8; 8], data: &[u8]) -> Result<(), u32> {
        let len = data.len().min(CONTROL_BYTES);
        Self::buffer(CONTROL_BUFFER, len).copy_from_slice(&data[..len]);
        self.control(setup, false, len).map(|_| ())
    }

    fn out_buffer(&mut self) -> Result<&'static mut [u8], u32> {
        if self.out_queued {
            if !wait(20, || Self::token(OUT_TD) & ACTIVE == 0) {
                self.out_queued = false;
                self.unstick(OUT_QH);
                return Err(TIMEOUT);
            }
            self.out_queued = false;
            let token = Self::token(OUT_TD);
            if token & TD_HALTED != 0 {
                return Err(token & 0xff);
            }
        }
        Ok(Self::buffer(OUT_BUFFER, BULK_BYTES))
    }
    fn submit(&mut self, len: usize) {
        self.transfer_descriptor(OUT_TD, None, PID_OUT, false, OUT_BUFFER, len.min(BULK_BYTES), true);
        self.activate(OUT_QH, OUT_TD);
        self.out_queued = true;
    }
    fn receive(&mut self) {
        if !self.in_queued {
            self.transfer_descriptor(IN_TD, None, PID_IN, false, IN_BUFFER, BULK_BYTES, true);
            self.activate(IN_QH, IN_TD);
            self.in_queued = true;
        }
    }
    fn received(&mut self) -> Option<Result<&'static [u8], u32>> {
        if !self.in_queued {
            return None;
        }
        let token = Self::token(IN_TD);
        if token & ACTIVE != 0 {
            return None;
        }
        self.in_queued = false;
        if token & TD_HALTED != 0 {
            return Some(Err(token & 0xff));
        }
        let n = BULK_BYTES - (token >> 16 & 0x7fff) as usize;
        Some(Ok(&Self::buffer(IN_BUFFER, BULK_BYTES)[..n]))
    }
    /// USBSTS in the low half, PORTSC in the high half.
    fn debug(&self, port: u32) -> u32 {
        (self.read(USBSTS) & 0xffff) | (self.port(port) & 0xffff) << 16
    }
    fn clear_halt(&mut self, input: bool) -> Result<(), u32> {
        let endpoint = if input { self.bulk_in.0 | 0x80 } else { self.bulk_out.0 };
        let setup = hpos_net::usb::setup(0x02, hpos_net::usb::CLEAR_FEATURE, 0, endpoint as u16, 0);
        let result = self.control_out(setup, &[]);
        let queue_head = if input { IN_QH } else { OUT_QH };
        self.unstick(queue_head);
        if input {
            self.in_queued = false;
        } else {
            self.out_queued = false;
        }
        result
    }
}

impl Controller {
    fn transfer_descriptor(&self, at: usize, next: Option<usize>, pid: u32, toggle: bool, buffer: usize, len: usize, last: bool) {
        Self::dma_write(at, next.map_or(TERMINATE, |n| self.phys(n)));
        Self::dma_write(at + 4, TERMINATE);
        let base = self.phys(buffer);
        Self::dma_write(at + 0x0c, base);
        for page in 1..5u32 {
            Self::dma_write(at + 0x0c + 4 * page as usize, (base & !0xfff) + page * 0x1000);
        }
        // Upper address halves (64-bit controllers): all buffers are below 4 GiB.
        for offset in (0x20..TD_BYTES).step_by(4) {
            Self::dma_write(at + offset, 0);
        }
        let token = ACTIVE
            | pid << 8
            | 3 << 10
            | (last as u32) << 15
            | (len as u32) << 16
            | (toggle as u32) << 31;
        fence(Ordering::SeqCst);
        Self::dma_write(at + 8, token);
    }
    /// Points an idle queue head at a new descriptor chain. The overlay token
    /// is cleaned first (keeping the data toggle), the link written last.
    fn activate(&self, queue_head: usize, first: usize) {
        let token = Self::dma_read(queue_head + 0x18);
        Self::dma_write(queue_head + 0x18, token & 1 << 31);
        Self::dma_write(queue_head + 0x14, TERMINATE);
        fence(Ordering::SeqCst);
        Self::dma_write(queue_head + 0x10, self.phys(first));
    }
    /// After a timeout: forget whatever the queue head was doing.
    fn unstick(&mut self, queue_head: usize) {
        self.with_schedule_stopped(|_| {
            Self::dma_write(queue_head + 0x0c, 0);
            Self::dma_write(queue_head + 0x10, TERMINATE);
            Self::dma_write(queue_head + 0x14, TERMINATE);
            Self::dma_write(queue_head + 0x18, 0);
        });
    }
    fn token(at: usize) -> u32 {
        Self::dma_read(at + 8)
    }

    /// A control transfer; `data_in` selects the data stage direction.
    /// IN data lands in the control buffer; returns the bytes moved.
    fn control(&mut self, setup: [u8; 8], data_in: bool, len: usize) -> Result<usize, u32> {
        Self::buffer(SETUP, 8).copy_from_slice(&setup);
        let has_data = len > 0;
        self.transfer_descriptor(SETUP_TD, Some(if has_data { DATA_TD } else { STATUS_TD }), PID_SETUP, false, SETUP, 8, false);
        if has_data {
            let pid = if data_in { PID_IN } else { PID_OUT };
            self.transfer_descriptor(DATA_TD, Some(STATUS_TD), pid, true, CONTROL_BUFFER, len, false);
        }
        let status_pid = if has_data && data_in { PID_OUT } else { PID_IN };
        self.transfer_descriptor(STATUS_TD, None, status_pid, true, CONTROL_BUFFER, 0, true);
        fence(Ordering::SeqCst);
        self.activate(CONTROL_QH, SETUP_TD);
        let tds: &[usize] = if has_data { &[SETUP_TD, DATA_TD, STATUS_TD] } else { &[SETUP_TD, STATUS_TD] };
        let done = wait(50, || {
            Self::token(STATUS_TD) & ACTIVE == 0 || tds.iter().any(|td| Self::token(*td) & TD_HALTED != 0)
        });
        if !done {
            self.unstick(CONTROL_QH);
            return Err(TIMEOUT);
        }
        for td in tds {
            let token = Self::token(*td);
            if token & TD_HALTED != 0 {
                return Err(token & 0xff);
            }
        }
        Ok(if has_data { len - (Self::token(DATA_TD) >> 16 & 0x7fff) as usize } else { 0 })
    }
}
