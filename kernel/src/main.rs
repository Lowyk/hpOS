#![no_std]
#![no_main]

mod bios;
mod disk;
mod font;
mod memory;
mod process;
mod pci;
mod ps2;
mod rtc;
mod scheduler;
mod video;

use core::{
    arch::{asm, global_asm},
    ffi::c_void,
    panic::PanicInfo,
    ptr::{addr_of_mut, write_volatile},
};
use hpos_boot_protocol::BootInfo;
use hpos_desktop::{canvas::Canvas, view};

global_asm!(
    r#"
    .section .text.entry,"ax"
    .global _start
_start:
    cld
    call kernel_main
1:  cli
    hlt
    jmp 1b
    .global fault_entry
fault_entry:
    cli
    cld
    and rsp, -16
    call fatal_exception
2:  hlt
    jmp 2b
    .global keyboard_entry
keyboard_entry:
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
    xor esi, esi
    jmp ps2_entry_common
    .global mouse_entry
mouse_entry:
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
    mov esi, 1
ps2_entry_common:
    cld
    mov rbx, rsp
    and rsp, -16
    mov rdi, rsi
    call ps2_interrupt
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
    iretq
    .global spurious_master_entry
spurious_master_entry:
    push rax
    push rdx
    mov dx, 0x20
    mov al, 0x0b
    out dx, al
    in al, dx
    test al, 0x80
    jz spurious_master_done
    mov al, 0x20
    out dx, al
spurious_master_done:
    pop rdx
    pop rax
    iretq
    .global spurious_slave_entry
spurious_slave_entry:
    push rax
    push rdx
    mov dx, 0xa0
    mov al, 0x0b
    out dx, al
    in al, dx
    test al, 0x80
    jz spurious_slave_cascade
    mov al, 0x20
    out dx, al
spurious_slave_cascade:
    mov dx, 0x20
    mov al, 0x20
    out dx, al
    pop rdx
    pop rax
    iretq
"#
);

unsafe extern "C" {
    fn fault_entry();
    fn keyboard_entry();
    fn mouse_entry();
    fn spurious_master_entry();
    fn spurious_slave_entry();
}

#[repr(C, packed)]
#[derive(Clone, Copy)]
struct Gate {
    low: u16,
    selector: u16,
    ist: u8,
    flags: u8,
    middle: u16,
    high: u32,
    zero: u32,
}
const EMPTY_GATE: Gate = Gate {
    low: 0,
    selector: 0,
    ist: 0,
    flags: 0,
    middle: 0,
    high: 0,
    zero: 0,
};
static mut IDT: [Gate; 256] = [EMPTY_GATE; 256];

fn install_fault_handlers() {
    let entry = fault_entry as *const () as u64;
    let gate = Gate {
        low: entry as u16,
        selector: 0x18,
        ist: 0,
        flags: 0x8e,
        middle: (entry >> 16) as u16,
        high: (entry >> 32) as u32,
        zero: 0,
    };
    #[repr(C, packed)]
    struct Descriptor {
        limit: u16,
        base: u64,
    }
    unsafe {
        let table = addr_of_mut!(IDT).cast::<Gate>();
        for i in 0..256 {
            table.add(i).write(gate);
        }
        let keyboard = keyboard_entry as *const () as u64;
        table.add(0x21).write(Gate {
            low: keyboard as u16,
            middle: (keyboard >> 16) as u16,
            high: (keyboard >> 32) as u32,
            ..gate
        });
        for (vector, entry) in [
            (0x20, scheduler::timer_entry as *const () as u64),
            (0x2c, mouse_entry as *const () as u64),
            (0x27, spurious_master_entry as *const () as u64),
            (0x2f, spurious_slave_entry as *const () as u64),
            (0x30, scheduler::yield_entry as *const () as u64),
            (0, scheduler::divide_error_entry as *const () as u64),
            (6, scheduler::invalid_opcode_entry as *const () as u64),
            (13, scheduler::general_protection_entry as *const () as u64),
            (14, scheduler::page_fault_entry as *const () as u64),
        ] {
            table.add(vector).write(Gate {
                low: entry as u16,
                middle: (entry >> 16) as u16,
                high: (entry >> 32) as u32,
                ..gate
            });
        }
        let syscall = scheduler::syscall_entry as *const () as u64;
        table.add(0x80).write(Gate {
            low: syscall as u16,
            middle: (syscall >> 16) as u16,
            high: (syscall >> 32) as u32,
            flags: 0xee,
            ..gate
        });
        let descriptor = Descriptor {
            limit: 4095,
            base: table as u64,
        };
        asm!("lidt [{}]", in(reg) &descriptor, options(readonly, nostack, preserves_flags));
    }
}
fn enable_exception_stack() {
    unsafe {
        addr_of_mut!(IDT).cast::<Gate>().add(8).write(Gate {
            ist: 1,
            ..addr_of_mut!(IDT).cast::<Gate>().add(8).read()
        });
    }
}

fn out(port: u16, value: u8) {
    unsafe {
        asm!("out dx, al", in("dx") port, in("al") value, options(nomem, nostack, preserves_flags));
    }
}
fn input(port: u16) -> u8 {
    let value: u8;
    unsafe {
        asm!("in al, dx", in("dx") port, out("al") value, options(nomem, nostack, preserves_flags));
    }
    value
}
fn serial_init() {
    out(0x3f9, 0);
    out(0x3fb, 0x80);
    out(0x3f8, 1);
    out(0x3f9, 0);
    out(0x3fb, 3);
    out(0x3fa, 0xc7);
    out(0x3fc, 0x0b);
}
fn keyboard_interrupts_init() {
    // Remap both 8259 controllers; enable IRQ0 (PIT), IRQ1 and IRQ12 (PS/2).
    bios::program_pic(0x20, 0x28, 0xf8, if ps2::ready() { 0xef } else { 0xff });
    bios::kernel_pic_installed();
}
fn log(message: &str) {
    for byte in message.bytes() {
        // An absent UART must never prevent booting.
        for _ in 0..4096 {
            if input(0x3fd) & 0x20 != 0 {
                break;
            }
        }
        out(0x3f8, byte);
    }
}
fn text_mode(message: &str) {
    for (index, byte) in message.bytes().take(80 * 25).enumerate() {
        unsafe {
            write_volatile((0xb8000 as *mut u16).add(index), 0x0f00 | u16::from(byte));
        }
    }
}
fn halt() -> ! {
    loop {
        unsafe {
            asm!("cli", "hlt", options(nomem, nostack));
        }
    }
}

#[unsafe(no_mangle)]
extern "C" fn fatal_exception() -> ! {
    log("hpOS: CPU exception\r\n");
    text_mode("hpOS: CPU exception. See serial diagnostic output.");
    halt()
}
fn fatal_fault(vector: u64, error: u64, rip: u64, address: u64) -> ! {
    log("hpOS: fatal kernel exception\r\n");
    let info = unsafe { &*(0x6000 as *const BootInfo) };
    if info.valid()
        && info.framebuffer_len().is_some()
        && info.width >= 800
        && info.height >= 600
        && !(address >= info.framebuffer
            && address < info.framebuffer + info.framebuffer_len().unwrap_or(0) as u64)
    {
        let screen = Screen {
            pixels: info.framebuffer as *mut u32,
            width: info.width as usize,
            height: info.height as usize,
            stride: info.pitch as usize / 4,
        };
        screen.draw(false, &MemoryDisplay::Failed("KERNEL EXCEPTION"));
        screen.details([
            ("VECTOR:", vector),
            ("ERROR:", error),
            ("RIP:", rip),
            ("ADDRESS:", address),
        ]);
    } else {
        text_mode("hpOS: fatal kernel exception. See serial log.");
    }
    halt()
}

struct Screen {
    pixels: *mut u32,
    width: usize,
    height: usize,
    stride: usize,
}
enum MemoryDisplay {
    Initializing,
    Ready,
    Failed(&'static str),
}
impl Screen {
    fn hex(&self, x: usize, y: usize, value: u64) {
        let mut digits = [b'0'; 16];
        for (index, byte) in digits.iter_mut().enumerate() {
            *byte = b"0123456789ABCDEF"[((value >> (4 * (15 - index))) & 15) as usize];
        }
        self.text(
            x,
            y,
            core::str::from_utf8(&digits).unwrap_or("?"),
            0xffffff,
            2,
        );
    }
    /// Four labelled hex values below the boot-screen message.
    fn details(&self, rows: [(&str, u64); 4]) {
        let x = self.width / 2 - 180;
        let y = self.height / 2 + 120;
        self.rectangle(x - 12, y - 10, 384, 116, 0x1e2536);
        for (n, (label, value)) in rows.iter().enumerate() {
            self.text(x, y + n * 26, label, 0xffffff, 2);
            self.hex(x + 120, y + n * 26, *value);
        }
    }
    fn rectangle(&self, x: usize, y: usize, width: usize, height: usize, color: u32) {
        let right = x.saturating_add(width).min(self.width);
        let bottom = y.saturating_add(height).min(self.height);
        for row in y..bottom {
            for col in x..right {
                unsafe {
                    write_volatile(self.pixels.add(row * self.stride + col), color);
                }
            }
        }
    }
    fn text(&self, mut x: usize, y: usize, text: &str, color: u32, scale: usize) {
        for c in text.bytes() {
            for (row, bits) in font::glyph(c).iter().enumerate() {
                for col in 0..5 {
                    if bits & (1 << (4 - col)) != 0 {
                        self.rectangle(x + col * scale, y + row * scale, scale, scale, color);
                    }
                }
            }
            x += 6 * scale;
        }
    }
    /// Branded boot screen; the drawing is shared with the desktop crate.
    fn draw(&self, _light: bool, memory: &MemoryDisplay) {
        let Some(canvas) =
            (unsafe { Canvas::new(self.pixels, self.width, self.height, self.stride) })
        else {
            return;
        };
        match memory {
            MemoryDisplay::Initializing => view::boot_screen(&canvas, 1, "Подготовка памяти..."),
            MemoryDisplay::Ready => {
                view::boot_screen(&canvas, 2, "Память в порядке, запуск служб...")
            }
            MemoryDisplay::Failed(error) => view::boot_screen(&canvas, 3, error),
        }
    }
}

#[unsafe(no_mangle)]
extern "C" fn kernel_main(info: *const BootInfo) -> ! {
    install_fault_handlers();
    unsafe {
        process::install_gdt();
        bios::install();
    }
    serial_init();
    log("hpOS: entered x86_64 Rust kernel\r\n");
    // Only the fixed loader handoff address is accepted in this bootstrap.
    if info as usize != 0x6000 {
        log("hpOS: bad handoff pointer\r\n");
        halt();
    }
    let info = unsafe { &*info };
    if !info.valid() {
        log("hpOS: bad boot protocol\r\n");
        halt();
    }
    let screen = if info.framebuffer_len().is_some() && info.width >= 800 && info.height >= 600 {
        Some(Screen {
            pixels: info.framebuffer as *mut u32,
            width: info.width as usize,
            height: info.height as usize,
            stride: info.pitch as usize / 4,
        })
    } else {
        log("hpOS: VBE unavailable; text fallback\r\n");
        text_mode("hpOS 0.10: initializing physical and virtual memory...");
        None
    };
    let light = false;
    if let Some(screen) = &screen {
        screen.draw(light, &MemoryDisplay::Initializing);
    }
    log("hpOS: initializing physical allocator and protected paging\r\n");
    let mut manager = match unsafe { memory::MemoryManager::initialize(info) } {
        Ok(manager) => manager,
        Err(error) => {
            log(error);
            if let Some(screen) = &screen {
                screen.draw(light, &MemoryDisplay::Failed(error));
            } else {
                text_mode(error);
            }
            halt();
        }
    };
    if let Some(screen) = &screen {
        screen.draw(light, &MemoryDisplay::Ready);
    } else {
        text_mode("hpOS 0.10 MEMORY OK. Preparing user processes...");
    }
    log("hpOS: MEMORY OK; looking for the hpOS data partition\r\n");
    if info.options & hpos_boot_protocol::OPTION_NO_STORAGE != 0 {
        log("hpOS: disk access disabled at boot\r\n");
    } else if disk::probe(info.boot_drive as u8) == disk::State::Ready {
        log("hpOS: HPFS data partition found on the boot drive\r\n");
    } else {
        log("hpOS: no HPFS data partition; files stay unavailable\r\n");
    }
    video::initialize(info);
    pci::initialize();
    log("hpOS: preparing guarded thread stacks and FPU contexts\r\n");
    ps2::initialize();
    if let Err(error) = unsafe { scheduler::initialize(&mut manager, info) } {
        log(error);
        if let Some(screen) = &screen {
            screen.draw(light, &MemoryDisplay::Failed(error));
        } else {
            text_mode(error);
        }
        halt();
    }
    if let Some(screen) = &screen {
        screen.draw(light, &MemoryDisplay::Ready);
    }
    scheduler::publish_memory(manager.stats());
    scheduler::program_timer();
    keyboard_interrupts_init();
    log("hpOS: PIT, user processes and IPC ready\r\n");
    unsafe {
        asm!("sti", options(nostack));
    }
    let mut reported_display_failure = false;
    loop {
        if scheduler::take_memory_request() {
            if let Err(error) = manager.self_test() {
                unsafe {
                    asm!("cli", options(nostack));
                }
                log(error);
                if let Some(screen) = &screen {
                    screen.draw(light, &MemoryDisplay::Failed(error));
                } else {
                    text_mode(error);
                }
                halt();
            }
            scheduler::publish_memory(manager.stats());
            log("hpOS: repeated memory check passed\r\n");
        }
        if scheduler::display_failed() && !reported_display_failure {
            if let Some(screen) = &screen {
                screen.draw(light, &MemoryDisplay::Failed("DISPLAY SERVICE STOPPED"));
                if let Some([id, vector, rip, address]) = scheduler::crash() {
                    screen.details([
                        ("PROCESS:", id),
                        ("VECTOR:", vector),
                        ("RIP:", rip),
                        ("ADDRESS:", address),
                    ]);
                }
            }
            reported_display_failure = true;
        }
        if screen.is_none() {
            text_mode(if scheduler::failed() {
                "hpOS 0.10 SERVICE ERROR"
            } else if scheduler::display_ready() {
                "hpOS 0.10 USER SERVICES RUNNING / VBE UNAVAILABLE"
            } else {
                "hpOS 0.10 STARTING USER SERVICES"
            });
        }
        scheduler::scheduler_sleep(1);
    }
}

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    log("hpOS: Rust panic\r\n");
    halt()
}

#[unsafe(no_mangle)]
unsafe extern "C" fn memset(dst: *mut c_void, value: i32, count: usize) -> *mut c_void {
    for i in 0..count {
        unsafe {
            dst.cast::<u8>().add(i).write_volatile(value as u8);
        }
    }
    dst
}
#[unsafe(no_mangle)]
unsafe extern "C" fn memcpy(dst: *mut c_void, src: *const c_void, count: usize) -> *mut c_void {
    for i in 0..count {
        unsafe {
            dst.cast::<u8>()
                .add(i)
                .write_volatile(src.cast::<u8>().add(i).read_volatile());
        }
    }
    dst
}
#[unsafe(no_mangle)]
unsafe extern "C" fn memmove(dst: *mut c_void, src: *const c_void, count: usize) -> *mut c_void {
    if (dst as usize) < src as usize {
        unsafe {
            memcpy(dst, src, count);
        }
    } else {
        for i in (0..count).rev() {
            unsafe {
                dst.cast::<u8>()
                    .add(i)
                    .write_volatile(src.cast::<u8>().add(i).read_volatile());
            }
        }
    }
    dst
}
