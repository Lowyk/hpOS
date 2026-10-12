use core::{
    arch::{asm, x86_64::__cpuid},
    ptr::{addr_of, addr_of_mut, read_volatile, write_volatile},
};
use hpos_boot_protocol::{BootInfo, MemoryEntry};
use hpos_memory::{FrameAllocator, Region};
use hpos_service_protocol::{Startup, USER_BACKBUFFER, USER_FRAMEBUFFER};
use hpos_x86_paging::{Error, HUGE_PAGE, Mapper, PAGE, Permissions, TableStore};

const WORDS: usize = 16384; // 4 GiB tracked, two 128 KiB bitmaps.
pub const WINDOW_FRAMES: usize = crate::disk::WINDOW_FRAMES;
const STACK_GUARD: u64 = 0x7f000;
/// Firmware, loader data and the kernel image; never handed out as frames.
const LOW_RESERVED: u64 = 0x400000;
const _: () = assert!(LOW_RESERVED % HUGE_PAGE == 0);
const SCRATCH: u64 = 0x1_0000_1000;
pub use hpos_service_protocol::USER_BASE;
pub const USER_DATA: u64 = USER_BASE + 0x100000;
const USER_STACK: u64 = USER_BASE + 0x200000;
/// 1 MiB per process; the pages below and above stay unmapped as guards.
pub const USER_STACK_PAGES: u64 = 256;
pub const USER_STACK_LOW: u64 = USER_STACK + PAGE;
pub const USER_STACK_TOP: u64 = USER_STACK + (USER_STACK_PAGES + 1) * PAGE;
static mut FRAMES: FrameAllocator<WORDS> = FrameAllocator::empty();

unsafe extern "C" {
    static __text_start: u8;
    static __text_end: u8;
    static __rodata_start: u8;
    static __rodata_end: u8;
    static __data_start: u8;
    static __kernel_end: u8;
}

struct PhysicalTables {
    frames: *mut FrameAllocator<WORDS>,
    tables: [u64; 64],
    count: usize,
    shared: usize,
}
impl PhysicalTables {
    // All allocation is boot/UI-only on one CPU. Stores never retain a mutable reference.
    fn frames(&mut self) -> &mut FrameAllocator<WORDS> {
        unsafe { &mut *self.frames }
    }
    fn check(&self, table: u64, index: usize) -> Result<(), Error> {
        if index >= 512 || !self.tables[..self.count].contains(&table) {
            Err(Error::CorruptTable)
        } else {
            Ok(())
        }
    }
}
impl TableStore for PhysicalTables {
    fn allocate_table(&mut self) -> Result<u64, Error> {
        if self.count == self.tables.len() {
            return Err(Error::OutOfMemory);
        }
        let frame = self.frames().allocate().ok_or(Error::OutOfMemory)?;
        for index in 0..512 {
            unsafe {
                write_volatile((frame as *mut u64).add(index), 0);
            }
        }
        self.tables[self.count] = frame;
        self.count += 1;
        Ok(frame)
    }
    fn read(&self, table: u64, index: usize) -> Result<u64, Error> {
        self.check(table, index)?;
        Ok(unsafe { read_volatile((table as *const u64).add(index)) })
    }
    fn write(&mut self, table: u64, index: usize, value: u64) -> Result<(), Error> {
        self.check(table, index)?;
        if self.tables[..self.shared].contains(&table) {
            return Err(Error::SupervisorConflict);
        }
        unsafe {
            write_volatile((table as *mut u64).add(index), value);
        }
        Ok(())
    }
}

#[derive(Clone, Copy)]
pub struct Stats {
    pub free_frames: usize,
    pub nx: bool,
    pub checks: usize,
}
pub struct MemoryManager {
    mapper: Mapper<PhysicalTables>,
    nx: bool,
    checks: usize,
}
/// Physical frames mapped at a fixed user address. Device registers are
/// mapped uncached.
pub struct Window<'a> {
    pub address: u64,
    pub frames: &'a [u64],
    pub device: bool,
}
#[derive(Clone, Copy)]
pub struct ProcessSpace {
    pub root: u64,
    pub user_stack: u64,
    pub data_physical: u64,
}

impl MemoryManager {
    pub fn root(&self) -> u64 {
        self.mapper.root()
    }
    /// Boot-only construction. Physical backing stays owned until process teardown exists.
    /// Zeroed frames for a fixed shared or I/O window; owned for the system lifetime.
    pub fn allocate_window(&mut self) -> Result<[u64; WINDOW_FRAMES], &'static str> {
        let mut frames = [0u64; WINDOW_FRAMES];
        self.allocate_frames(&mut frames)?;
        Ok(frames)
    }
    pub fn allocate_frames(&mut self, frames: &mut [u64]) -> Result<(), &'static str> {
        for frame in frames.iter_mut() {
            let physical = self
                .mapper
                .store_mut()
                .frames()
                .allocate()
                .ok_or("NO MEMORY FOR SERVICE WINDOW")?;
            for index in 0..PAGE as usize / 8 {
                unsafe {
                    write_volatile((physical as *mut u64).add(index), 0);
                }
            }
            *frame = physical;
        }
        Ok(())
    }
    /// Zeroed, physically consecutive frames for device DMA.
    pub fn allocate_run(&mut self, frames: &mut [u64]) -> Result<(), &'static str> {
        let first = self
            .mapper
            .store_mut()
            .frames()
            .allocate_run(frames.len())
            .ok_or("NO MEMORY FOR DEVICE BUFFERS")?;
        for (index, frame) in frames.iter_mut().enumerate() {
            *frame = first + index as u64 * PAGE;
            for word in 0..PAGE as usize / 8 {
                unsafe {
                    write_volatile((*frame as *mut u64).add(word), 0);
                }
            }
        }
        Ok(())
    }
    pub fn create_process(
        &mut self,
        image: &crate::process::Image,
        display: Option<&BootInfo>,
        windows: &[Window],
    ) -> Result<ProcessSpace, &'static str> {
        let code = image.bytes;
        if code.is_empty()
            || code.len() > 0x80000
            || image.executable_end % PAGE != 0
            || image.readonly_end % PAGE != 0
            || image.executable_end > image.readonly_end
        {
            return Err("INVALID USER IMAGE");
        }
        let kernel_entry = self
            .mapper
            .root_entry(0)
            .map_err(|_| "KERNEL ROOT LOOKUP FAILED")?;
        let master = self.mapper.store_mut();
        let store = PhysicalTables {
            frames: master.frames,
            tables: master.tables,
            count: master.count,
            shared: master.count,
        };
        let mut mapper = Mapper::new(store, self.nx).map_err(|_| "NO MEMORY FOR PROCESS ROOT")?;
        mapper
            .install_supervisor_entry(0, kernel_entry)
            .map_err(|_| "KERNEL ROOT IMPORT FAILED")?;
        let mut data_physical = 0;
        let code_pages = code.len().div_ceil(PAGE as usize);
        for index in 0..code_pages + 1 + USER_STACK_PAGES as usize {
            let physical = mapper
                .store_mut()
                .frames()
                .allocate()
                .ok_or("NO MEMORY FOR USER PAGE")?;
            for byte in 0..PAGE as usize {
                unsafe {
                    write_volatile((physical as *mut u8).add(byte), 0);
                }
            }
            let (address, permissions) = if index < code_pages {
                let offset = index * PAGE as usize;
                for (byte, value) in code[offset..(offset + PAGE as usize).min(code.len())]
                    .iter()
                    .enumerate()
                {
                    unsafe {
                        write_volatile((physical as *mut u8).add(byte), *value);
                    }
                }
                (
                    USER_BASE + offset as u64,
                    if (offset as u64) < image.executable_end {
                        Permissions::CODE
                    } else if (offset as u64) < image.readonly_end {
                        Permissions::READ_ONLY
                    } else {
                        Permissions::DATA
                    },
                )
            } else if index == code_pages {
                data_physical = physical;
                (USER_DATA, Permissions::DATA)
            } else {
                (
                    USER_STACK + (index - code_pages) as u64 * PAGE,
                    Permissions::DATA,
                )
            };
            mapper
                .map_user_4k(address, physical, permissions)
                .map_err(|_| "USER PAGE MAPPING FAILED")?;
        }
        let mut startup = Startup {
            framebuffer: 0,
            width: 0,
            height: 0,
            stride: 0,
            reserved: 0,
            backbuffer: 0,
        };
        if let Some(info) = display {
            if let Some(boot_length) = info.framebuffer_len() {
                // Cover every mode the desktop may switch to later.
                let length = crate::video::map_bytes(boot_length as u64) as usize;
                if info.width > 1920 || info.height > 1200 || length > 0x1000000 {
                    return Err("DISPLAY EXCEEDS BOOT LIMITS");
                }
                let first = info.framebuffer & !(PAGE - 1);
                let end = (info.framebuffer + length as u64).div_ceil(PAGE) * PAGE;
                let mut permissions = Permissions::DATA;
                permissions.uncached = true;
                for offset in (0..end - first).step_by(PAGE as usize) {
                    mapper
                        .map_user_4k(USER_FRAMEBUFFER + offset, first + offset, permissions)
                        .map_err(|_| "DISPLAY MAPPING FAILED")?;
                }
                startup = Startup {
                    framebuffer: USER_FRAMEBUFFER + (info.framebuffer - first),
                    width: info.width,
                    height: info.height,
                    stride: info.pitch / 4,
                    reserved: 0,
                    backbuffer: USER_BACKBUFFER,
                };
                let bytes = crate::video::back_bytes(info.width as u64 * info.height as u64 * 4);
                for offset in (0..bytes.div_ceil(PAGE) * PAGE).step_by(PAGE as usize) {
                    let frame = mapper
                        .store_mut()
                        .frames()
                        .allocate()
                        .ok_or("NO MEMORY FOR BACKBUFFER")?;
                    for index in 0..PAGE as usize / 8 {
                        unsafe {
                            write_volatile((frame as *mut u64).add(index), 0);
                        }
                    }
                    mapper
                        .map_user_4k(USER_BACKBUFFER + offset, frame, Permissions::DATA)
                        .map_err(|_| "BACKBUFFER MAPPING FAILED")?;
                }
                if !mapper
                    .translate(USER_FRAMEBUFFER)
                    .map_err(|_| "DISPLAY ACCESS CHECK FAILED")?
                    .is_some_and(|m| m.user_accessible && m.writable && m.physical == first)
                {
                    return Err("DISPLAY ACCESS CHECK FAILED");
                }
            }
        } else if mapper
            .translate(USER_FRAMEBUFFER)
            .map_err(|_| "DISPLAY GRANT CHECK FAILED")?
            .is_some()
        {
            return Err("DISPLAY EXPOSED TO OTHER PROCESS");
        }
        for window in windows {
            let mut permissions = Permissions::DATA;
            permissions.uncached = window.device;
            for (index, frame) in window.frames.iter().enumerate() {
                mapper
                    .map_user_4k(window.address + index as u64 * PAGE, *frame, permissions)
                    .map_err(|_| "SERVICE WINDOW MAPPING FAILED")?;
            }
        }
        unsafe {
            core::ptr::write((data_physical + 16) as *mut Startup, startup);
        }
        let code_mapping = mapper
            .translate(USER_BASE)
            .map_err(|_| "USER CODE LOOKUP FAILED")?
            .ok_or("USER CODE MISSING")?;
        let kernel_mapping = mapper
            .translate(0x100000)
            .map_err(|_| "KERNEL CODE LOOKUP FAILED")?
            .ok_or("KERNEL CODE MISSING")?;
        if !code_mapping.user_accessible || code_mapping.writable || kernel_mapping.user_accessible
        {
            return Err("USER PAGE ACCESS CHECK FAILED");
        }
        for address in [USER_STACK, USER_STACK_TOP] {
            if mapper
                .translate(address)
                .map_err(|_| "USER GUARD LOOKUP FAILED")?
                .is_some()
            {
                return Err("USER STACK GUARD IS MAPPED");
            }
        }
        Ok(ProcessSpace {
            root: mapper.root(),
            user_stack: USER_STACK_TOP,
            data_physical,
        })
    }
    /// Called once, on the boot CPU with interrupts disabled, before other users of FRAMES.
    pub unsafe fn initialize(info: &BootInfo) -> Result<Self, &'static str> {
        let entries = unsafe {
            core::slice::from_raw_parts(
                info.memory_map as *const MemoryEntry,
                info.memory_entries as usize,
            )
        };
        let framebuffer = match info.framebuffer_len() {
            Some(size) => Region {
                start: info.framebuffer,
                end: info.framebuffer + size as u64,
            },
            None => Region { start: 0, end: 0 },
        };
        let frames = unsafe { &mut *addr_of_mut!(FRAMES) };
        frames
            .initialize(
                entries,
                &[
                    Region {
                        start: 0,
                        end: LOW_RESERVED,
                    },
                    framebuffer,
                ],
            )
            .map_err(|_| "INVALID BIOS MEMORY MAP")?;
        let store = PhysicalTables {
            frames: addr_of_mut!(FRAMES),
            tables: [0; 64],
            count: 0,
            shared: 0,
        };
        // Keep compatibility with toolchains whose CPUID intrinsic is unsafe.
        #[allow(unused_unsafe)]
        let nx = unsafe { __cpuid(0x80000001) }.edx & (1 << 20) != 0;
        let mut mapper = Mapper::new(store, nx).map_err(|_| "NO MEMORY FOR PAGE TABLES")?;
        let text_start = addr_of!(__text_start) as u64;
        let text_end = addr_of!(__text_end) as u64;
        let rodata_start = addr_of!(__rodata_start) as u64;
        let rodata_end = addr_of!(__rodata_end) as u64;
        if addr_of!(__kernel_end) as u64 > LOW_RESERVED {
            return Err("KERNEL EXCEEDS RESERVED MEMORY");
        }
        for physical in (PAGE..LOW_RESERVED).step_by(PAGE as usize) {
            if physical == STACK_GUARD {
                continue;
            }
            let mut permissions = if (physical >= text_start && physical < text_end)
                || physical == crate::bios::CODE_PAGE
            {
                Permissions::CODE
            } else if physical >= rodata_start && physical < rodata_end {
                Permissions::READ_ONLY
            } else {
                Permissions::DATA
            };
            permissions.uncached = physical == 0xb8000 || overlaps(physical, PAGE, framebuffer);
            mapper
                .map_4k(physical, physical, permissions)
                .map_err(|_| "LOW MEMORY MAPPING FAILED")?;
        }
        for physical in (LOW_RESERVED..0x1_0000_0000).step_by(HUGE_PAGE as usize) {
            let mut permissions = Permissions::DATA;
            permissions.uncached = overlaps(physical, HUGE_PAGE, framebuffer);
            mapper
                .map_2m(physical, physical, permissions)
                .map_err(|_| "RAM MAPPING FAILED")?;
        }
        // This verifies guard pages and permissions before changing CR3.
        if mapper.translate(0).map_err(|_| "BAD NULL GUARD")?.is_some()
            || mapper
                .translate(STACK_GUARD)
                .map_err(|_| "BAD STACK GUARD")?
                .is_some()
        {
            return Err("GUARD PAGE IS MAPPED");
        }
        let code = mapper
            .translate(text_start)
            .map_err(|_| "BAD CODE MAPPING")?
            .ok_or("MISSING CODE MAPPING")?;
        let data = mapper
            .translate(addr_of!(__data_start) as u64)
            .map_err(|_| "BAD DATA MAPPING")?
            .ok_or("MISSING DATA MAPPING")?;
        let ro = mapper
            .translate(rodata_start)
            .map_err(|_| "BAD RODATA MAPPING")?
            .ok_or("MISSING RODATA MAPPING")?;
        if code.writable
            || !code.executable
            || !data.writable
            || ro.writable
            || (nx && (data.executable || ro.executable))
        {
            return Err("BAD PAGE PERMISSIONS");
        }
        if nx {
            unsafe {
                enable_nx();
            }
        }
        let root = mapper.root();
        unsafe {
            asm!("mov cr3, {}", in(reg) root, options(nostack));
            let mut cr0: u64;
            asm!("mov {}, cr0", out(reg) cr0, options(nomem, nostack));
            cr0 |= 1 << 16; // WP makes read-only mappings apply to supervisor writes.
            asm!("mov cr0, {}", in(reg) cr0, options(nostack));
        }
        let mut manager = Self {
            mapper,
            nx,
            checks: 0,
        };
        manager.self_test()?;
        Ok(manager)
    }

    /// Tests an actual writable virtual alias, unmaps it, then returns its owned frame.
    pub fn self_test(&mut self) -> Result<(), &'static str> {
        let frame = self
            .mapper
            .store_mut()
            .frames()
            .allocate()
            .ok_or("NO FRAME FOR MEMORY CHECK")?;
        if self
            .mapper
            .map_4k(SCRATCH, frame, Permissions::DATA)
            .is_err()
        {
            let _ = self.mapper.store_mut().frames().release(frame);
            return Err("SCRATCH MAPPING FAILED");
        }
        unsafe {
            asm!("invlpg [{}]", in(reg) SCRATCH, options(nostack));
        }
        let valid = unsafe {
            let virtual_page = SCRATCH as *mut u64;
            write_volatile(virtual_page, 0x48504f534d454d31);
            write_volatile(virtual_page.add(511), 0x1122334455667788);
            read_volatile(frame as *const u64) == 0x48504f534d454d31
                && read_volatile((frame as *const u64).add(511)) == 0x1122334455667788
                && read_volatile(virtual_page) == 0x48504f534d454d31
        };
        if self
            .mapper
            .unmap_4k(SCRATCH)
            .map_err(|_| "SCRATCH UNMAP FAILED")?
            != Some(frame)
        {
            return Err("SCRATCH FRAME MISMATCH");
        }
        unsafe {
            asm!("invlpg [{}]", in(reg) SCRATCH, options(nostack));
        }
        let before = self.mapper.store_mut().frames().free_frames();
        self.mapper
            .store_mut()
            .frames()
            .release(frame)
            .map_err(|_| "FRAME RELEASE FAILED")?;
        if !valid
            || self
                .mapper
                .translate(SCRATCH)
                .map_err(|_| "SCRATCH LOOKUP FAILED")?
                .is_some()
            || self.mapper.store_mut().frames().free_frames() != before + 1
        {
            return Err("MEMORY CHECK FAILED");
        }
        self.checks += 1;
        Ok(())
    }

    pub fn stats(&mut self) -> Stats {
        Stats {
            free_frames: self.mapper.store_mut().frames().free_frames(),
            nx: self.nx,
            checks: self.checks,
        }
    }

    /// Keeps both surrounding pages unmapped. Stacks stay owned until thread teardown exists.
    pub fn allocate_stack(&mut self, base: u64) -> Result<u64, &'static str> {
        const PAGES: usize = 4;
        if base % PAGE != 0
            || base < 0x1_0020_0000
            || base.checked_add((PAGES as u64 + 2) * PAGE).is_none()
        {
            return Err("INVALID STACK ADDRESS");
        }
        for offset in 0..PAGES + 2 {
            if self
                .mapper
                .translate(base + offset as u64 * PAGE)
                .map_err(|_| "STACK LOOKUP FAILED")?
                .is_some()
            {
                return Err("STACK REGION ALREADY MAPPED");
            }
        }
        let mut frames = [0u64; PAGES];
        let mut allocated = 0;
        for (index, frame) in frames.iter_mut().enumerate() {
            let Some(physical) = self.mapper.store_mut().frames().allocate() else {
                break;
            };
            for byte in 0..PAGE as usize {
                unsafe {
                    write_volatile((physical as *mut u8).add(byte), 0);
                }
            }
            let address = base + (index as u64 + 1) * PAGE;
            if self
                .mapper
                .map_4k(address, physical, Permissions::DATA)
                .is_err()
            {
                let _ = self.mapper.store_mut().frames().release(physical);
                break;
            }
            unsafe {
                asm!("invlpg [{}]", in(reg) address, options(nostack));
            }
            *frame = physical;
            allocated += 1;
        }
        if allocated != PAGES {
            for (index, frame) in frames[..allocated].iter().enumerate() {
                let address = base + (index as u64 + 1) * PAGE;
                let _ = self.mapper.unmap_4k(address);
                unsafe {
                    asm!("invlpg [{}]", in(reg) address, options(nostack));
                }
                let _ = self.mapper.store_mut().frames().release(*frame);
            }
            return Err("NO MEMORY FOR THREAD STACK");
        }
        Ok(base + (PAGES as u64 + 1) * PAGE)
    }
}

fn overlaps(start: u64, size: u64, region: Region) -> bool {
    region.start < region.end && start < region.end && start + size > region.start
}
unsafe fn enable_nx() {
    let low: u32;
    let high: u32;
    unsafe {
        asm!("rdmsr", in("ecx") 0xc0000080u32, out("eax") low, out("edx") high, options(nomem, nostack));
        asm!("wrmsr", in("ecx") 0xc0000080u32, in("eax") low | (1 << 11), in("edx") high, options(nostack));
    }
}
