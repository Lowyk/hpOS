#![no_std]

use core::sync::atomic::{AtomicU8, AtomicUsize, Ordering};

pub const USER_BASE: u64 = 0xffff_ffff_8000_0000;
pub const USER_DATA: u64 = USER_BASE + 0x100000;
pub const USER_FRAMEBUFFER: u64 = USER_BASE + 0x1000000;
pub const USER_BACKBUFFER: u64 = USER_BASE + 0x2000000;
pub const STARTUP_ADDRESS: u64 = USER_DATA + 16;
/// 64 KiB request window shared by the desktop and the file-system service.
pub const FS_SHARED: u64 = USER_BASE + 0x400000;
/// 64 KiB disk transfer window, mapped only into the file-system service.
pub const FS_DISK: u64 = USER_BASE + 0x500000;
pub const WINDOW_BYTES: usize = 0x10000;
/// Zeroed working memory for the desktop; see [`heap`] for the split.
pub const DESKTOP_HEAP: u64 = USER_BASE + 0x600000;
pub const DESKTOP_HEAP_BYTES: usize = 0x400000;
/// Network state and fetched pages, shared by the desktop and the network service.
pub const NET_SHARED: u64 = USER_BASE + 0xa00000;
/// Physically consecutive memory the USB controller reads and writes; network service only.
pub const NET_DMA: u64 = USER_BASE + 0xc00000;
/// The USB 2.0 controller's register page, uncached; network service only.
pub const NET_REGISTERS: u64 = USER_BASE + 0xe00000;

/// Offsets inside [`DESKTOP_HEAP`].
pub mod heap {
    pub const SHELL: usize = 0;
    pub const SHELL_BYTES: usize = 0x100000;
    pub const MODEL: usize = 0x100000;
    pub const MODEL_BYTES: usize = 0x100000;
    pub const PAGE: usize = 0x200000;
    pub const PAGE_BYTES: usize = 0x200000;
}

/// Kernel status words readable by the desktop (syscall 5).
pub mod status {
    pub const COUNT: usize = 26;
    pub const DISK_STATE: usize = 20;
    pub const DISK_SECTORS: usize = 21;
    pub const FS_REPORT: usize = 22;
    pub const DISK_ERRORS: usize = 23;
    /// CMOS clock: bit 31 valid, hour << 16 | minute << 8 | second.
    pub const TIME: usize = 24;
    /// CMOS date: bit 31 valid, year << 16 | month << 8 | day.
    pub const DATE: usize = 25;
}

/// File-system requests: header at the start of `FS_SHARED`, payload after it.
pub mod fs {
    pub const DATA_OFFSET: usize = 4096;
    pub const DATA_BYTES: usize = super::WINDOW_BYTES - DATA_OFFSET;
    pub const NAME_BYTES: usize = 1024;
    pub const STAT: u32 = 1;
    pub const LIST: u32 = 2;
    pub const RESOLVE: u32 = 3;
    pub const CREATE: u32 = 4;
    pub const READ: u32 = 5;
    pub const WRITE: u32 = 6;
    pub const REMOVE: u32 = 7;
    pub const RENAME: u32 = 8;
    /// Status value when no HPFS volume is mounted (matches hpos-fs NotMounted).
    pub const NOT_MOUNTED: u32 = 4;
    pub const LIST_START: u64 = u64::MAX;

    #[repr(C)]
    #[derive(Clone, Copy)]
    pub struct Request {
        pub op: u32,
        pub status: u32,
        pub args: [u64; 4],
        pub results: [u64; 4],
        pub name_len: u32,
        pub reserved: u32,
        pub name: [u8; NAME_BYTES],
    }
    #[repr(C)]
    #[derive(Clone, Copy)]
    pub struct Entry {
        pub id: u16,
        pub kind: u8,
        pub name_len: u8,
        pub size: u32,
        pub stamp: u64,
        pub name: [u8; 64],
    }
    pub const ENTRY_BYTES: usize = core::mem::size_of::<Entry>();
    pub const MAX_ENTRIES: usize = DATA_BYTES / ENTRY_BYTES;
}

/// Network service protocol. The desktop sends `op | sequence << 8` over IPC
/// after filling `State::request`; the service publishes everything else in
/// the shared [`State`] page and bumps `version` on each change.
pub mod net {
    pub const SHARED_BYTES: usize = 0x100000;
    pub const DMA_BYTES: usize = 0x20000;
    pub const BODY_OFFSET: usize = 0x4000;
    pub const BODY_BYTES: usize = SHARED_BYTES - BODY_OFFSET;
    pub const URL_BYTES: usize = 1024;
    pub const LOG: usize = 64;

    /// Take over the USB controller and bring the link up.
    pub const CONNECT: u64 = 1;
    /// Fetch `request` (an http:// URL).
    pub const FETCH: u64 = 2;
    /// Ping `request` (host name or address).
    pub const PING: u64 = 3;
    /// Abandon the current fetch.
    pub const STOP: u64 = 4;

    pub mod stage {
        pub const OFF: u32 = 0;
        pub const NO_CONTROLLER: u32 = 1;
        pub const STARTING: u32 = 2;
        pub const NO_DEVICE: u32 = 3;
        pub const DEVICE: u32 = 4;
        pub const LINK: u32 = 5;
        pub const ADDRESS: u32 = 6;
        pub const ONLINE: u32 = 7;
        pub const FAILED: u32 = 8;
    }
    pub mod fetch {
        pub const IDLE: u32 = 0;
        pub const RESOLVING: u32 = 1;
        pub const CONNECTING: u32 = 2;
        pub const LOADING: u32 = 3;
        pub const DONE: u32 = 4;
        pub const FAILED: u32 = 5;
    }
    /// What a fetch ended with when it failed (`State::fetch_error`).
    pub mod error {
        pub const OFFLINE: u32 = 1;
        pub const BAD_URL: u32 = 2;
        pub const DNS: u32 = 3;
        pub const CONNECT: u32 = 4;
        pub const RESET: u32 = 5;
        pub const TIMEOUT: u32 = 6;
        pub const HTTPS: u32 = 7;
        pub const REDIRECTS: u32 = 8;
        pub const PROTOCOL: u32 = 9;
        pub const STOPPED: u32 = 10;
    }
    /// Body kinds (`State::content`).
    pub mod content {
        pub const OTHER: u32 = 0;
        pub const HTML: u32 = 1;
        pub const TEXT: u32 = 2;
    }
    /// Body encodings (`State::charset`).
    pub mod charset {
        pub const UTF8: u32 = 0;
        pub const WINDOWS_1251: u32 = 1;
        pub const LATIN1: u32 = 2;
        pub const KOI8R: u32 = 3;
        /// The server did not say; the desktop looks inside the page.
        pub const UNKNOWN: u32 = 0xff;
    }
    /// Log event codes; `a`, `b`, `c` meanings are listed per code.
    pub mod event {
        /// a = vendor | device << 16, b = ports, c = USB version (2 or 1).
        pub const CONTROLLER: u32 = 1;
        /// a = 1 if the BIOS let go, 0 if it had to be forced.
        pub const HANDOFF: u32 = 2;
        /// a = step that failed (1 halt, 2 reset, 3 run), b = USB version.
        pub const CONTROLLER_FAILED: u32 = 3;
        /// a = port, b = 1 on a USB 1.1 controller, c = which one.
        pub const PORT: u32 = 4;
        /// a = port, b = port status after reset, c = before; the device is
        /// not high speed and moves to a USB 1.1 controller.
        pub const SLOW_DEVICE: u32 = 5;
        /// a = vendor, b = product, c = number of configurations.
        pub const DEVICE: u32 = 6;
        /// a = step (1 descriptor, 2 address, 3 configuration, 4 set
        /// configuration) | configuration number << 4 | 1 << 8 on USB 1.1, b = transfer status (0x100 is a
        /// timeout), c = controller and port state from the driver.
        pub const USB_FAILED: u32 = 7;
        /// a, b, c = class << 16 | subclass << 8 | protocol of the first three
        /// different interfaces (0 if fewer).
        pub const NOT_RNDIS: u32 = 8;
        /// a = configuration value, b = control interface, c = data interface.
        pub const RNDIS: u32 = 9;
        /// a = step (1 initialize, 2 query, 3 filter), b = RNDIS status.
        pub const RNDIS_FAILED: u32 = 10;
        /// a, b = phone MAC (bytes 0-3, 4-5).
        pub const LINK_UP: u32 = 11;
        pub const DHCP_DISCOVER: u32 = 12;
        /// a = address, b = gateway, c = DNS server (network byte order packed big-endian).
        pub const LEASE: u32 = 13;
        pub const DHCP_TIMEOUT: u32 = 14;
        /// a = address.
        pub const RESOLVED: u32 = 15;
        /// a = address, b = port.
        pub const CONNECTING: u32 = 16;
        /// a = HTTP status, b = body bytes.
        pub const RESPONSE: u32 = 17;
        /// a = error code from [`super::error`].
        pub const FETCH_FAILED: u32 = 18;
        /// a = address, b = round trip in ms, c = sequence.
        pub const PONG: u32 = 19;
        /// a = address, c = sequence.
        pub const PING_LOST: u32 = 20;
        pub const DISCONNECTED: u32 = 21;
        /// a = transfer errors so far.
        pub const USB_ERROR: u32 = 22;
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    pub struct Event {
        pub code: u32,
        pub a: u32,
        pub b: u32,
        pub c: u32,
    }
    #[repr(C)]
    pub struct State {
        pub version: u32,
        pub stage: u32,
        pub address: [u8; 4],
        pub gateway: [u8; 4],
        pub dns: [u8; 4],
        pub mask: [u8; 4],
        pub mac: [u8; 6],
        pub phone_mac: [u8; 6],
        pub vendor: u16,
        pub product: u16,
        pub received: u32,
        pub sent: u32,
        /// Total events ever logged; the newest is `log[(events - 1) % LOG]`.
        pub events: u32,
        pub log: [Event; LOG],
        /// Written by the desktop before an IPC command; copied by the service,
        /// which then sets `taken` to `request_sequence`.
        pub request_sequence: u32,
        pub taken: u32,
        pub request_len: u32,
        pub request: [u8; URL_BYTES],
        pub fetch_sequence: u32,
        pub fetch: u32,
        pub fetch_error: u32,
        /// Bytes received so far.
        pub progress: u32,
        pub status: u32,
        pub content: u32,
        pub charset: u32,
        pub body_len: u32,
        /// Final URL after redirects.
        pub url_len: u32,
        pub url: [u8; URL_BYTES],
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Startup {
    pub framebuffer: u64,
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub reserved: u32,
    pub backbuffer: u64,
}

/// One producer (IRQ), one consumer (input service). Capacity is N - 1.
pub struct ByteQueue<const N: usize> {
    bytes: [AtomicU8; N],
    head: AtomicUsize,
    tail: AtomicUsize,
    dropped: AtomicUsize,
}
impl<const N: usize> ByteQueue<N> {
    pub const fn new() -> Self {
        assert!(N >= 2);
        Self {
            bytes: [const { AtomicU8::new(0) }; N],
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
            dropped: AtomicUsize::new(0),
        }
    }
    pub fn push(&self, byte: u8) -> bool {
        let head = self.head.load(Ordering::Relaxed);
        let next = (head + 1) % N;
        if next == self.tail.load(Ordering::Acquire) {
            self.dropped.fetch_add(1, Ordering::Relaxed);
            return false;
        }
        self.bytes[head].store(byte, Ordering::Relaxed);
        self.head.store(next, Ordering::Release);
        true
    }
    pub fn pop(&self) -> Option<u8> {
        let tail = self.tail.load(Ordering::Relaxed);
        if tail == self.head.load(Ordering::Acquire) {
            return None;
        }
        let byte = self.bytes[tail].load(Ordering::Relaxed);
        self.tail.store((tail + 1) % N, Ordering::Release);
        Some(byte)
    }
    pub fn dropped(&self) -> usize {
        self.dropped.load(Ordering::Relaxed)
    }
}
impl<const N: usize> Default for ByteQueue<N> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn full_queue_does_not_overwrite_and_wraps_in_order() {
        let q = ByteQueue::<4>::new();
        assert_eq!(q.pop(), None);
        for b in [1, 2, 3] {
            assert!(q.push(b));
        }
        assert!(!q.push(4));
        assert_eq!(q.dropped(), 1);
        assert_eq!(q.pop(), Some(1));
        assert!(q.push(4));
        for b in [2, 3, 4] {
            assert_eq!(q.pop(), Some(b));
        }
        assert_eq!(q.pop(), None);
    }
    #[test]
    fn startup_has_fixed_layout_and_fits_private_page() {
        assert_eq!(core::mem::size_of::<Startup>(), 32);
        assert_eq!(core::mem::offset_of!(Startup, backbuffer), 24);
        assert_eq!(core::mem::offset_of!(Startup, stride), 16);
        assert!(STARTUP_ADDRESS + 24 < USER_DATA + 4096);
        assert_eq!(USER_FRAMEBUFFER % 4096, 0);
    }
    #[test]
    fn fs_windows_do_not_overlap_other_user_regions() {
        assert_eq!(core::mem::size_of::<fs::Request>(), 1104);
        assert!(core::mem::size_of::<fs::Request>() <= fs::DATA_OFFSET);
        assert_eq!(fs::ENTRY_BYTES, 80);
        assert!(FS_SHARED >= USER_BASE + 0x302000);
        assert!(DESKTOP_HEAP >= FS_DISK + WINDOW_BYTES as u64);
        assert!(DESKTOP_HEAP + DESKTOP_HEAP_BYTES as u64 <= USER_FRAMEBUFFER);
        assert_eq!(FS_DISK, FS_SHARED + WINDOW_BYTES as u64 * 16);
        assert!(FS_DISK + (WINDOW_BYTES as u64) < USER_FRAMEBUFFER);
    }
    #[test]
    fn net_windows_fit_between_the_heap_and_the_framebuffer() {
        assert_eq!(NET_SHARED, DESKTOP_HEAP + DESKTOP_HEAP_BYTES as u64);
        assert!(NET_SHARED + net::SHARED_BYTES as u64 <= NET_DMA);
        assert!(NET_DMA + net::DMA_BYTES as u64 <= NET_REGISTERS);
        assert!(NET_REGISTERS + 4096 <= USER_FRAMEBUFFER);
        assert!(core::mem::size_of::<net::State>() <= net::BODY_OFFSET);
        assert_eq!(heap::PAGE + heap::PAGE_BYTES, DESKTOP_HEAP_BYTES);
        assert_eq!(heap::MODEL + heap::MODEL_BYTES, heap::PAGE);
    }
}
