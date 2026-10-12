use crate::memory::{MemoryManager, Window};
use core::{
    arch::{asm, global_asm},
    ptr::{addr_of_mut, write},
    sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering},
};
use hpos_ipc::{Broker, Rights};
use hpos_scheduler::RoundRobin;

const THREADS: usize = 8;
const IDLE: usize = 7;
const LAST_USER: usize = 6;
const INPUT: usize = 3;
const DISPLAY: usize = 4;
const FILES: usize = 5;
const NET: usize = 6;
/// One isolation/capability probe bit per user process.
const ALL_PROBES: u8 = (1 << LAST_USER) - 1;

#[repr(C)]
struct Context {
    regs: [u64; 15],
    rip: u64,
    cs: u64,
    flags: u64,
    rsp: u64,
    ss: u64,
}
#[repr(C)]
struct FaultContext {
    regs: [u64; 15],
    error: u64,
    rip: u64,
    cs: u64,
    flags: u64,
    rsp: u64,
    ss: u64,
}
#[repr(C, align(16))]
struct FxState([u8; 512]);
const _: () = {
    assert!(core::mem::size_of::<Context>() == 160);
    assert!(core::mem::offset_of!(Context, rip) == 120);
    assert!(core::mem::offset_of!(Context, rsp) == 144);
    assert!(core::mem::align_of::<FxState>() == 16);
    assert!(core::mem::size_of::<FaultContext>() == 168);
};
struct Runtime {
    policy: RoundRobin<THREADS>,
    contexts: [u64; THREADS],
    fx: [FxState; THREADS],
    roots: [u64; THREADS],
    kernel_stacks: [u64; THREADS],
    data: [u64; THREADS],
    reports: [u64; THREADS],
    ipc: Broker<6, 5, 7, 32>,
    disk_window: [u64; crate::disk::WINDOW_FRAMES],
    /// Physical address of the network service's DMA window.
    dma: u64,
}
static mut RUNTIME: Runtime = Runtime {
    policy: RoundRobin::new(IDLE, 2),
    contexts: [0; THREADS],
    fx: [const { FxState([0; 512]) }; THREADS],
    roots: [0; THREADS],
    kernel_stacks: [0; THREADS],
    data: [0; THREADS],
    reports: [0; THREADS],
    ipc: Broker::empty(),
    disk_window: [0; crate::disk::WINDOW_FRAMES],
    dma: 0,
};

#[unsafe(no_mangle)]
static pit_ticks: AtomicU64 = AtomicU64::new(0);
#[unsafe(no_mangle)]
static worker_a: AtomicU64 = AtomicU64::new(0);
#[unsafe(no_mangle)]
static worker_b: AtomicU64 = AtomicU64::new(0);
#[unsafe(no_mangle)]
static probe_failed: AtomicU8 = AtomicU8::new(0);
static SWITCHES: AtomicU64 = AtomicU64::new(0);
static MESSAGES: AtomicU64 = AtomicU64::new(0);
static ISOLATION: AtomicU8 = AtomicU8::new(0);
static CAP_DENIED: AtomicU8 = AtomicU8::new(0);
static LAST_FAULT: AtomicU64 = AtomicU64::new(0);
/// The first user process that died: thread id, vector (0x80 = gave up via
/// syscall 4), instruction pointer and faulting address.
static CRASH: [AtomicU64; 4] = [const { AtomicU64::new(0) }; 4];
fn record_crash(id: usize, vector: u64, rip: u64, address: u64) {
    if CRASH[0].load(Ordering::Relaxed) == 0 {
        CRASH[1].store(vector, Ordering::Relaxed);
        CRASH[2].store(rip, Ordering::Relaxed);
        CRASH[3].store(address, Ordering::Relaxed);
        CRASH[0].store(id as u64, Ordering::Relaxed);
    }
}
pub fn crash() -> Option<[u64; 4]> {
    let id = CRASH[0].load(Ordering::Relaxed);
    (id != 0).then(|| {
        [
            id,
            CRASH[1].load(Ordering::Relaxed),
            CRASH[2].load(Ordering::Relaxed),
            CRASH[3].load(Ordering::Relaxed),
        ]
    })
}
static INPUT_REPORT: AtomicU64 = AtomicU64::new(0);
static DISPLAY_REPORT: AtomicU64 = AtomicU64::new(0);
static DISPLAY_TICKS: AtomicU64 = AtomicU64::new(0);
static MOUSE_EVENTS: AtomicU64 = AtomicU64::new(0);
static INPUT_EVENTS: AtomicU64 = AtomicU64::new(0);
static MEMORY_REQUEST: AtomicBool = AtomicBool::new(false);
static MEMORY_CHECKS: AtomicU64 = AtomicU64::new(0);
static FREE_MIB: AtomicU64 = AtomicU64::new(0);
static NX_AVAILABLE: AtomicBool = AtomicBool::new(false);
static DISPLAY_FAILED: AtomicBool = AtomicBool::new(false);
static FS_REPORT: AtomicU64 = AtomicU64::new(0);
static NET_REPORT: AtomicU64 = AtomicU64::new(0);

pub fn publish_memory(stats: crate::memory::Stats) {
    MEMORY_CHECKS.store(stats.checks as u64, Ordering::Relaxed);
    FREE_MIB.store((stats.free_frames / 256) as u64, Ordering::Relaxed);
    NX_AVAILABLE.store(stats.nx, Ordering::Relaxed);
}
pub fn take_memory_request() -> bool {
    MEMORY_REQUEST.swap(false, Ordering::Relaxed)
}
pub fn display_ready() -> bool {
    DISPLAY_REPORT.load(Ordering::Relaxed) != 0
}
pub fn display_failed() -> bool {
    DISPLAY_FAILED.load(Ordering::Relaxed)
}

global_asm!(
    r#"
    .section .text,"ax"
    .macro save_context
    push rax
    push rbx
    push rcx
    push rdx
    push rbp
    push rsi
    push rdi
    push r8
    push r9
    push r10
    push r11
    push r12
    push r13
    push r14
    push r15
    .endm
    .global timer_entry
timer_entry:
    save_context
    mov esi, 1
    jmp schedule_common
    .global yield_entry
yield_entry:
    save_context
    xor esi, esi
schedule_common:
    cld
    mov rdi, rsp
    and rsp, -16
    call schedule_interrupt
restore_context:
    mov rsp, rax
    pop r15
    pop r14
    pop r13
    pop r12
    pop r11
    pop r10
    pop r9
    pop r8
    pop rdi
    pop rsi
    pop rbp
    pop rdx
    pop rcx
    pop rbx
    pop rax
    iretq

    .global syscall_entry
syscall_entry:
    save_context
    cld
    mov rdi, rsp
    and rsp, -16
    call syscall_interrupt
    jmp restore_context
    .global page_fault_entry
page_fault_entry:
    save_context
    mov esi, 14
    jmp fault_common
    .global general_protection_entry
general_protection_entry:
    save_context
    mov esi, 13
    jmp fault_common
    .global invalid_opcode_entry
invalid_opcode_entry:
    push 0
    save_context
    mov esi, 6
    jmp fault_common
    .global divide_error_entry
divide_error_entry:
    push 0
    save_context
    xor esi, esi
fault_common:
    cld
    mov rbx, rsp
    mov rdi, rsp
    and rsp, -16
    call fault_interrupt
    test rax, rax
    jnz restore_context
    mov rsp, rbx
    pop r15
    pop r14
    pop r13
    pop r12
    pop r11
    pop r10
    pop r9
    pop r8
    pop rdi
    pop rsi
    pop rbp
    pop rdx
    pop rcx
    pop rbx
    pop rax
    add rsp, 8
    iretq
"#
);

unsafe extern "C" {
    pub fn timer_entry();
    pub fn yield_entry();
    pub fn syscall_entry();
    pub fn page_fault_entry();
    pub fn general_protection_entry();
    pub fn invalid_opcode_entry();
    pub fn divide_error_entry();
}

/// Boot CPU only, before enabling IRQ0. All normal thread entry points use SysV ABI.
pub unsafe fn initialize(
    memory: &mut MemoryManager,
    info: &hpos_boot_protocol::BootInfo,
) -> Result<(), &'static str> {
    let state = unsafe { &mut *addr_of_mut!(RUNTIME) };
    let mxcsr = 0x1f80u32;
    unsafe {
        asm!("clts", "fninit", "ldmxcsr [{}]", in(reg) &mxcsr, options(nostack));
    }
    for id in 0..THREADS {
        unsafe {
            asm!("fxsave64 [{}]", in(reg) state.fx[id].0.as_mut_ptr(), options(nostack));
        }
    }
    for id in 1..=LAST_USER {
        let mask = [
            state.fx[0].0[28],
            state.fx[0].0[29],
            state.fx[0].0[30],
            state.fx[0].0[31],
        ];
        state.fx[id].0.fill(0);
        state.fx[id].0[0..2].copy_from_slice(&0x037fu16.to_le_bytes());
        state.fx[id].0[24..28].copy_from_slice(&0x1f80u32.to_le_bytes());
        state.fx[id].0[28..32].copy_from_slice(&mask);
    }
    let exception_stack = memory.allocate_stack(0x1_0030_0000)?;
    unsafe {
        crate::process::initialize(exception_stack);
    }
    crate::enable_exception_stack();
    state.roots.fill(memory.root());
    state.kernel_stacks[0] = 0x90000;
    let shared = memory.allocate_window()?;
    state.disk_window = memory.allocate_window()?;
    let disk_window = state.disk_window;
    let mut heap = [0u64; hpos_service_protocol::DESKTOP_HEAP_BYTES / 4096];
    memory.allocate_frames(&mut heap)?;
    let mut net_shared = [0u64; hpos_service_protocol::net::SHARED_BYTES / 4096];
    memory.allocate_frames(&mut net_shared)?;
    let mut dma = [0u64; hpos_service_protocol::net::DMA_BYTES / 4096];
    memory.allocate_run(&mut dma)?;
    state.dma = dma[0];
    let registers = crate::pci::ehci().map(|(_, base)| [base & !0xfff]);
    for id in 1..THREADS {
        let top = memory.allocate_stack(0x1_0020_0000 + id as u64 * 0x10000)?;
        state.kernel_stacks[id] = top;
        let user = if id == IDLE {
            None
        } else {
            let window = |address, frames| Window {
                address,
                frames,
                device: false,
            };
            let display = [
                window(hpos_service_protocol::FS_SHARED, &shared),
                window(hpos_service_protocol::DESKTOP_HEAP, &heap),
                window(hpos_service_protocol::NET_SHARED, &net_shared),
            ];
            let files = [
                window(hpos_service_protocol::FS_SHARED, &shared),
                window(hpos_service_protocol::FS_DISK, &disk_window),
            ];
            let net = [
                window(hpos_service_protocol::NET_SHARED, &net_shared),
                window(hpos_service_protocol::NET_DMA, &dma),
                Window {
                    address: hpos_service_protocol::NET_REGISTERS,
                    frames: registers.as_ref().map_or(&[], |r| &r[..]),
                    device: true,
                },
            ];
            let windows: &[Window] = match id {
                DISPLAY => &display,
                FILES => &files,
                NET => &net,
                _ => &[],
            };
            Some(memory.create_process(
                &crate::process::image(id),
                if id == DISPLAY { Some(info) } else { None },
                windows,
            )?)
        };
        let rsp = user.map_or(top - 8, |p| p.user_stack - 8);
        let saved = top - core::mem::size_of::<Context>() as u64 - if id == IDLE { 8 } else { 0 };
        if let Some(process) = user {
            for earlier in 1..id {
                if state.roots[earlier] == process.root
                    || state.data[earlier] == process.data_physical
                {
                    return Err("PROCESS ISOLATION SETUP FAILED");
                }
            }
            state.roots[id] = process.root;
            state.data[id] = process.data_physical;
        }
        let mut context = Context {
            regs: [0; 15],
            rip: if id == IDLE {
                idle as *const () as u64
            } else {
                crate::memory::USER_BASE
            },
            cs: if user.is_some() {
                crate::process::USER_CODE
            } else {
                0x18
            },
            flags: 0x202,
            rsp,
            ss: if user.is_some() {
                crate::process::USER_SS
            } else {
                0x20
            },
        };
        context.regs[8] = id as u64; // RDI in the save_context push order.
        unsafe {
            if id == IDLE {
                write(rsp as *mut u64, thread_returned as *const () as u64);
            }
            write(saved as *mut Context, context);
        }
        state.contexts[id] = saved;
    }
    if state.roots[1] == state.roots[2] || state.data[1] == state.data[2] {
        return Err("PROCESS ISOLATION SETUP FAILED");
    }
    for pid in 0..2 {
        state
            .ipc
            .grant(pid, 0, 1 - pid, Rights::Send)
            .map_err(|_| "IPC SEND GRANT FAILED")?;
        state
            .ipc
            .grant(pid, 1, pid, Rights::Receive)
            .map_err(|_| "IPC RECEIVE GRANT FAILED")?;
    }
    // Endpoints: 2 input->desktop, 3 desktop->input, 4 desktop->files,
    // 5 files->desktop, 6 desktop->network.
    for (pid, handle, endpoint, rights) in [
        (2, 0, 2, Rights::Send),
        (2, 1, 3, Rights::Receive),
        (3, 0, 3, Rights::Send),
        (3, 1, 2, Rights::Receive),
        (3, 2, 4, Rights::Send),
        (3, 3, 5, Rights::Receive),
        (4, 0, 5, Rights::Send),
        (4, 1, 4, Rights::Receive),
        (3, 4, 6, Rights::Send),
        (5, 1, 6, Rights::Receive),
    ] {
        state
            .ipc
            .grant(pid, handle, endpoint, rights)
            .map_err(|_| "SERVICE GRANT FAILED")?;
    }
    for id in 0..IDLE {
        state
            .policy
            .add(id)
            .map_err(|_| "THREAD REGISTRATION FAILED")?;
    }
    state.policy.start(0).map_err(|_| "INITIAL THREAD FAILED")?;
    Ok(())
}

#[unsafe(no_mangle)]
extern "C" fn schedule_interrupt(context: u64, timer: u64) -> u64 {
    // Interrupt gates keep IF clear while Runtime is mutably borrowed.
    let state = unsafe { &mut *addr_of_mut!(RUNTIME) };
    let current = state.policy.current();
    unsafe {
        asm!("fxsave64 [{}]", in(reg) state.fx[current].0.as_mut_ptr(), options(nostack));
    }
    state.contexts[current] = context;
    let next = if timer != 0 {
        state.policy.on_tick()
    } else {
        state.policy.reschedule()
    };
    pit_ticks.store(state.policy.ticks(), Ordering::Relaxed);
    SWITCHES.store(state.policy.switches(), Ordering::Relaxed);
    if timer != 0 {
        crate::out(0x20, 0x20);
    }
    finish_switch(state, next)
}

fn finish_switch(state: &mut Runtime, mut next: usize) -> u64 {
    loop {
        let target = state.contexts[next];
        if target == 0 {
            crate::halt();
        }
        if (1..=LAST_USER).contains(&next) {
            let frame = unsafe { &mut *(target as *mut Context) };
            if frame.cs != crate::process::USER_CODE
                || frame.ss != crate::process::USER_SS
                || !crate::process::valid_return(next, frame.rip, frame.rsp)
            {
                probe_failed.store(1, Ordering::Relaxed);
                LAST_FAULT.store(14, Ordering::Relaxed); // Vector + 1; zero means no fault.
                if next == DISPLAY {
                    DISPLAY_FAILED.store(true, Ordering::Relaxed);
                }
                let _ = state.policy.stop_current();
                next = state.policy.reschedule();
                continue;
            }
            frame.flags &= !((3 << 12) | (1 << 14) | (1 << 17));
            frame.flags |= 0x202;
        }
        break;
    }
    SWITCHES.store(state.policy.switches(), Ordering::Relaxed);
    let target = state.contexts[next];
    if target == 0 {
        crate::halt();
    }
    unsafe {
        crate::process::kernel_stack(state.kernel_stacks[next]);
        let root: u64;
        asm!("mov {}, cr3", out(reg) root, options(nomem, nostack));
        if root != state.roots[next] {
            asm!("mov cr3, {}", in(reg) state.roots[next], options(nostack));
        }
        asm!("fxrstor64 [{}]", in(reg) state.fx[next].0.as_ptr(), options(nostack));
    }
    target
}

#[unsafe(no_mangle)]
extern "C" fn syscall_interrupt(context: u64) -> u64 {
    let frame = unsafe { &mut *(context as *mut Context) };
    let state = unsafe { &mut *addr_of_mut!(RUNTIME) };
    let current = state.policy.current();
    if !(1..=LAST_USER).contains(&current) || frame.cs != crate::process::USER_CODE {
        frame.regs[14] = 4;
        return context;
    }
    unsafe {
        asm!("fxsave64 [{}]", in(reg) state.fx[current].0.as_mut_ptr(), options(nostack));
    }
    state.contexts[current] = context;
    let number = frame.regs[14];
    let arg = frame.regs[8];
    frame.regs[14] = 0;
    let mut next = current;
    match number {
        0 => {
            if arg > 10000 || state.policy.sleep_current(arg).is_err() {
                frame.regs[14] = 4;
            } else {
                next = state.policy.reschedule();
            }
        }
        1 => match state.ipc.send(current - 1, arg, frame.regs[9]) {
            Ok(()) => {
                MESSAGES.fetch_add(1, Ordering::Relaxed);
                if current == INPUT {
                    if frame.regs[9] >> 63 != 0 {
                        MOUSE_EVENTS.fetch_add(1, Ordering::Relaxed);
                    } else {
                        INPUT_EVENTS.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
            Err(error) => {
                frame.regs[14] = ipc_error(error);
                if error == hpos_ipc::Error::Denied {
                    CAP_DENIED.fetch_or(1 << (current - 1), Ordering::Relaxed);
                }
            }
        },
        2 => match state.ipc.receive(current - 1, arg) {
            Ok(message) => {
                frame.regs[11] = message.word;
                frame.regs[12] = message.sender as u64 + 1;
            }
            Err(error) => {
                frame.regs[14] = ipc_error(error);
            }
        },
        3 => {
            let marker = unsafe { core::ptr::read_volatile(state.data[current] as *const u64) };
            if arg != state.reports[current].wrapping_add(1)
                || marker != (0x1122334455667700 | current as u64)
            {
                probe_failed.store(1, Ordering::Relaxed);
                frame.regs[14] = 4;
            } else {
                state.reports[current] = arg;
                match current {
                    1 => worker_a.store(arg, Ordering::Relaxed),
                    2 => worker_b.store(arg, Ordering::Relaxed),
                    INPUT => INPUT_REPORT.store(arg, Ordering::Relaxed),
                    DISPLAY => {
                        DISPLAY_REPORT.store(arg, Ordering::Relaxed);
                        DISPLAY_TICKS.store(frame.regs[9], Ordering::Relaxed);
                    }
                    FILES => FS_REPORT.store(arg, Ordering::Relaxed),
                    _ => NET_REPORT.store(arg, Ordering::Relaxed),
                }
            }
        }
        4 => {
            probe_failed.store(1, Ordering::Relaxed);
            if current >= INPUT {
                record_crash(current, 0x80, frame.rip, 0);
                if current == DISPLAY {
                    DISPLAY_FAILED.store(true, Ordering::Relaxed);
                }
                let _ = state.policy.stop_current();
                next = state.policy.reschedule();
            }
        }
        // The tick counter (arg 0) is open to every service; the rest is the desktop's.
        5 if current == DISPLAY || arg == 0 => {
            frame.regs[11] = match arg {
                0 => state.policy.ticks(),
                1 => state.policy.switches(),
                2 => worker_a.load(Ordering::Relaxed),
                3 => worker_b.load(Ordering::Relaxed),
                4 => MESSAGES.load(Ordering::Relaxed),
                5 => {
                    (ISOLATION.load(Ordering::Relaxed) == ALL_PROBES
                        && CAP_DENIED.load(Ordering::Relaxed) == ALL_PROBES)
                        as u64
                }
                6 => MEMORY_CHECKS.load(Ordering::Relaxed),
                7 => FREE_MIB.load(Ordering::Relaxed),
                8 => NX_AVAILABLE.load(Ordering::Relaxed) as u64,
                9 => probe_failed.load(Ordering::Relaxed) as u64,
                10 => INPUT_EVENTS.load(Ordering::Relaxed),
                11 => DISPLAY_REPORT.load(Ordering::Relaxed),
                12 => crate::ps2::KEYS.dropped() as u64,
                13 => LAST_FAULT.load(Ordering::Relaxed),
                14 => INPUT_REPORT.load(Ordering::Relaxed),
                15 => crate::ps2::ready() as u64,
                16 => crate::ps2::MOUSE.dropped() as u64,
                17 => MOUSE_EVENTS.load(Ordering::Relaxed),
                18 => crate::ps2::id() as u64,
                19 => DISPLAY_TICKS.load(Ordering::Relaxed),
                20 => crate::disk::state(),
                21 => crate::disk::sectors().unwrap_or(0),
                22 => FS_REPORT.load(Ordering::Relaxed),
                23 => crate::disk::errors(),
                24 => crate::rtc::now().0,
                25 => crate::rtc::now().1,
                _ => {
                    frame.regs[14] = 4;
                    0
                }
            };
        }
        6 if current == INPUT => {
            if arg == 1 {
                frame.regs[11] = crate::ps2::KEYS.dropped() as u64;
            } else if arg != 0 {
                frame.regs[14] = 4;
            } else if let Some(byte) = crate::ps2::KEYS.pop() {
                frame.regs[11] = byte as u64;
            } else {
                frame.regs[14] = 1;
            }
        }
        7 if current == INPUT => match arg {
            0 => {
                if let Some(byte) = crate::ps2::MOUSE.pop() {
                    frame.regs[11] = byte as u64;
                } else {
                    frame.regs[14] = 1;
                }
            }
            1 => frame.regs[11] = crate::ps2::id() as u64,
            2 => frame.regs[11] = crate::ps2::MOUSE.dropped() as u64,
            _ => frame.regs[14] = 4,
        },
        8 if current == DISPLAY => {
            MEMORY_REQUEST.store(true, Ordering::Relaxed);
        }
        // Disk: arg = partition-relative sector, word = op << 32 | count.
        // Data moves through the service's private 64 KiB window.
        9 if current == FILES => {
            let op = frame.regs[9] >> 32;
            let count = frame.regs[9] & 0xffff_ffff;
            let result = match op {
                0 => crate::disk::sectors()
                    .map(|sectors| frame.regs[11] = sectors)
                    .ok_or(crate::disk::Error::Unavailable),
                1 | 2 => crate::disk::transfer(op == 2, arg, count, &state.disk_window),
                _ => Err(crate::disk::Error::Invalid),
            };
            frame.regs[14] = match result {
                Ok(()) => 0,
                Err(crate::disk::Error::Invalid) => 4,
                Err(_) => 5,
            };
        }
        // Display modes: arg 0 count/current, 1 describe(word), 2 switch(word).
        10 if current == DISPLAY => match arg {
            0 => {
                frame.regs[11] =
                    crate::video::count() as u64 | (crate::video::current() as u64) << 8
            }
            1 => match crate::video::describe(frame.regs[9] as usize) {
                Some(word) => frame.regs[11] = word,
                None => frame.regs[14] = 4,
            },
            2 => match crate::video::switch(frame.regs[9] as usize) {
                Ok(word) => frame.regs[11] = word,
                Err(()) => frame.regs[14] = 5,
            },
            _ => frame.regs[14] = 4,
        },
        // USB controllers: arg = op | slot << 8 (slot 0 EHCI, 1..=4 UHCI).
        // op 0: DMA base and location | register base, 1 config read
        // (word = offset), 2 config write (word = offset << 32 | value), 3 id.
        11 if current == NET => match (arg & 0xff, crate::pci::controller((arg >> 8) as usize)) {
            (_, None) => frame.regs[14] = 5,
            (0, Some((location, base))) => {
                frame.regs[11] = state.dma;
                // EHCI: offset in the mapped register page; UHCI: I/O base.
                let base = if arg >> 8 == 0 { base & 0xfff } else { base };
                frame.regs[12] = (location as u64) << 32 | base;
            }
            (3, Some(_)) => frame.regs[11] = crate::pci::id((arg >> 8) as usize) as u64,
            (1, Some((location, _))) if frame.regs[9] < 0x100 && frame.regs[9] % 4 == 0 => {
                frame.regs[11] = crate::pci::read(location, frame.regs[9] as u8) as u64;
            }
            (2, Some((location, _))) if crate::pci::writable(frame.regs[9] >> 32) => {
                let offset = frame.regs[9] >> 32;
                if offset % 4 == 0 {
                    crate::pci::write(location, offset as u8, frame.regs[9] as u32);
                } else {
                    frame.regs[14] = 4;
                }
            }
            _ => frame.regs[14] = 4,
        },
        // UHCI registers: arg = port, word = op << 32 | value (see pci::port_io).
        12 if current == NET => match crate::pci::port_io(arg, frame.regs[9] >> 32, frame.regs[9] as u32) {
            Some(value) => frame.regs[11] = value as u64,
            None => frame.regs[14] = 4,
        },
        5..=12 => {
            frame.regs[14] = 3;
        }
        _ => {
            frame.regs[14] = 4;
        }
    }
    finish_switch(state, next)
}
fn ipc_error(error: hpos_ipc::Error) -> u64 {
    match error {
        hpos_ipc::Error::Full | hpos_ipc::Error::Empty => 1,
        hpos_ipc::Error::Denied => 3,
        _ => 2,
    }
}

#[unsafe(no_mangle)]
extern "C" fn fault_interrupt(context: u64, vector: u64) -> u64 {
    let frame = unsafe { &mut *(context as *mut FaultContext) };
    let address: u64;
    unsafe {
        asm!("mov {}, cr2", out(reg) address, options(nomem, nostack));
    }
    let state = unsafe { &mut *addr_of_mut!(RUNTIME) };
    let current = state.policy.current();
    if frame.cs & 3 == 0 {
        crate::fatal_fault(vector, frame.error, frame.rip, address);
    }
    if (1..=LAST_USER).contains(&current)
        && vector == 14
        && address == 0x100000
        && frame.error & 5 == 5
        && frame.rip == crate::process::image(current).probe
        && frame.ss == crate::process::USER_SS
        && crate::process::valid_return(current, frame.rip, frame.rsp)
    {
        ISOLATION.fetch_or(1 << (current - 1), Ordering::Relaxed);
        frame.rip = crate::process::image(current).resume;
        frame.flags &= !((3 << 12) | (1 << 14) | (1 << 17));
        frame.flags |= 0x202;
        return 0;
    }
    probe_failed.store(1, Ordering::Relaxed);
    LAST_FAULT.store(vector + 1, Ordering::Relaxed);
    record_crash(current, vector, frame.rip, address);
    if current == DISPLAY {
        DISPLAY_FAILED.store(true, Ordering::Relaxed);
    }
    let _ = state.policy.stop_current();
    let next = state.policy.reschedule();
    finish_switch(state, next)
}

#[unsafe(no_mangle)]
pub extern "C" fn scheduler_sleep(ticks: u64) {
    let flags: u64;
    unsafe {
        asm!("pushfq", "pop {}", "cli", out(reg) flags);
        {
            let state = &mut *addr_of_mut!(RUNTIME);
            if state.policy.sleep_current(ticks).is_err() {
                crate::halt();
            }
        }
        // Runtime's mutable borrow ends before another thread can enter the scheduler.
        asm!("int 0x30");
        if flags & 0x200 != 0 {
            asm!("sti", options(nostack));
        }
    }
}

extern "C" fn idle() -> ! {
    loop {
        unsafe {
            asm!("sti", "hlt", options(nostack));
        }
    }
}
extern "C" fn thread_returned() -> ! {
    crate::halt()
}

pub fn program_timer() {
    let divisor = (1_193_182u32 / 100) as u16;
    crate::out(0x43, 0x34);
    crate::out(0x40, divisor as u8);
    crate::out(0x40, (divisor >> 8) as u8);
}

pub fn failed() -> bool {
    probe_failed.load(Ordering::Relaxed) != 0
}
