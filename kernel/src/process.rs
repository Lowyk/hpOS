use crate::memory::USER_BASE;
use core::{
    arch::{asm, global_asm},
    ptr::{addr_of, addr_of_mut, write_unaligned},
    sync::atomic::{AtomicBool, Ordering},
};

pub const USER_CODE: u64 = 0x2b;
pub const USER_SS: u64 = 0x33;

pub struct Image {
    pub bytes: &'static [u8],
    pub executable_end: u64,
    pub readonly_end: u64,
    pub probe: u64,
    pub resume: u64,
}
include!(concat!(env!("HPOS_BUILD_DIR"), "/services.rs"));

#[repr(C, packed)]
struct Tss {
    reserved: u32,
    rsp: [u64; 3],
    reserved2: u64,
    ist: [u64; 7],
    reserved3: u64,
    reserved4: u16,
    io_map: u16,
}
static mut TSS: Tss = Tss {
    reserved: 0,
    rsp: [0; 3],
    reserved2: 0,
    ist: [0; 7],
    reserved3: 0,
    reserved4: 0,
    io_map: 104,
};
static mut GDT: [u64; 9] = [0; 9];
const _: () = {
    assert!(core::mem::size_of::<Tss>() == 104);
    assert!(core::mem::offset_of!(Tss, rsp) == 4);
    assert!(core::mem::offset_of!(Tss, ist) == 36);
};

static TASK_REGISTER: AtomicBool = AtomicBool::new(false);
#[repr(C, packed)]
struct Descriptor {
    limit: u16,
    base: u64,
}

/// Boot CPU, first thing in the kernel: replaces the loader's GDT, which lives
/// in low memory that the BIOS thunk reuses. Selectors keep their meaning.
pub unsafe fn install_gdt() {
    let base = addr_of!(TSS) as u64;
    let descriptor = 103 | ((base & 0xffffff) << 16) | (0x89 << 40) | ((base >> 24 & 0xff) << 56);
    unsafe {
        addr_of_mut!(GDT).write([
            0,
            0x00cf9a000000ffff,
            0x00cf92000000ffff,
            0x00af9a000000ffff,
            0x00cf92000000ffff,
            0x00affa000000ffff,
            0x00cff2000000ffff,
            descriptor,
            base >> 32,
        ]);
    }
    let pointer = Descriptor {
        limit: 71,
        base: addr_of!(GDT) as u64,
    };
    unsafe {
        asm!(
            "lgdt [{}]",
            "mov ax, 0x20",
            "mov ds, ax",
            "mov es, ax",
            "mov ss, ax",
            "push 0x18",
            "lea rax, [rip + 2f]",
            "push rax",
            "retfq",
            "2:",
            in(reg) &pointer,
            out("rax") _,
        );
    }
}

/// Boot-only; `install_gdt` must have run.
pub unsafe fn initialize(exception_stack: u64) {
    unsafe {
        write_unaligned(addr_of_mut!(TSS.rsp).cast::<u64>(), 0x90000);
        write_unaligned(addr_of_mut!(TSS.ist).cast::<u64>(), exception_stack);
        asm!("mov ax, 0x38", "ltr ax", out("ax") _, options(nostack));
    }
    TASK_REGISTER.store(true, Ordering::Relaxed);
}

/// After a BIOS call: makes TR's cached TSS state certain again. IF=0.
pub unsafe fn reload_task_register() {
    if !TASK_REGISTER.load(Ordering::Relaxed) {
        return;
    }
    unsafe {
        let descriptor = addr_of_mut!(GDT).cast::<u64>().add(7);
        descriptor.write_volatile(descriptor.read_volatile() & !(1 << 41));
        asm!("mov ax, 0x38", "ltr ax", out("ax") _, options(nostack));
    }
}
pub unsafe fn kernel_stack(top: u64) {
    unsafe {
        write_unaligned(addr_of_mut!(TSS.rsp).cast::<u64>(), top);
    }
}

global_asm!(
    r#"
    .section .user_blob,"a"
    .global user_program_start
user_program_start:
    mov r15, rdi
    mov ax, 0x33
    mov ds, ax
    mov es, ax
    mov rbx, 0xffffffff80100000
    mov r14, 0x1122334455667700
    or r14, r15
    mov qword ptr [rbx], r14
    mov qword ptr [rsp - 8], r14
    .global user_kernel_probe
user_kernel_probe:
    mov rax, qword ptr [0x100000]
    mov eax, 4
    mov edi, 1
    int 0x80
    .global user_probe_resume
user_probe_resume:
    mov eax, 1
    mov edi, 1
    xor esi, esi
    int 0x80
    cmp rax, 3
    jne user_error
    mov r12, 1
user_cycle:
    cmp qword ptr [rbx], r14
    jne user_error
    cmp qword ptr [rsp - 8], r14
    jne user_error
    cmp r15, 1
    jne pong_receive
ping_send:
    mov eax, 1
    xor edi, edi
    mov rsi, r12
    int 0x80
    test rax, rax
    jz ping_receive
    cmp rax, 1
    jne user_error
    xor eax, eax
    mov edi, 1
    int 0x80
    jmp ping_send
ping_receive:
    mov eax, 2
    mov edi, 1
    int 0x80
    test rax, rax
    jz ping_validate
    cmp rax, 1
    jne user_error
    xor eax, eax
    mov edi, 1
    int 0x80
    jmp ping_receive
ping_validate:
    cmp rdx, r12
    jne user_error
    cmp rcx, 2
    jne user_error
    jmp user_report
pong_receive:
    mov eax, 2
    mov edi, 1
    int 0x80
    test rax, rax
    jz pong_validate
    cmp rax, 1
    jne user_error
    xor eax, eax
    mov edi, 1
    int 0x80
    jmp pong_receive
pong_validate:
    cmp rdx, r12
    jne user_error
    cmp rcx, 1
    jne user_error
pong_send:
    mov eax, 1
    xor edi, edi
    mov rsi, r12
    int 0x80
    test rax, rax
    jz user_report
    cmp rax, 1
    jne user_error
    xor eax, eax
    mov edi, 1
    int 0x80
    jmp pong_send
user_report:
    mov eax, 3
    mov rdi, r12
    int 0x80
    inc r12
    xor eax, eax
    mov edi, 5
    int 0x80
    jmp user_cycle
user_error:
    mov eax, 4
    mov edi, 2
    int 0x80
user_error_wait:
    xor eax, eax
    mov edi, 100
    int 0x80
    jmp user_error_wait
    .global user_program_end
user_program_end:
"#
);
unsafe extern "C" {
    static user_program_start: u8;
    static user_program_end: u8;
    static user_kernel_probe: u8;
    static user_probe_resume: u8;
}
fn ping_image() -> &'static [u8] {
    let start = addr_of!(user_program_start) as usize;
    let end = addr_of!(user_program_end) as usize;
    unsafe { core::slice::from_raw_parts(start as *const u8, end - start) }
}
pub fn image(id: usize) -> Image {
    if (3..=6).contains(&id) {
        let source = &SERVICES[id - 3];
        Image {
            bytes: source.bytes,
            executable_end: source.executable_end,
            readonly_end: source.readonly_end,
            probe: source.probe,
            resume: source.resume,
        }
    } else {
        Image {
            bytes: ping_image(),
            executable_end: 4096,
            readonly_end: 4096,
            probe: USER_BASE + addr_of!(user_kernel_probe) as u64
                - addr_of!(user_program_start) as u64,
            resume: USER_BASE + addr_of!(user_probe_resume) as u64
                - addr_of!(user_program_start) as u64,
        }
    }
}
pub fn valid_return(id: usize, rip: u64, rsp: u64) -> bool {
    rip >= USER_BASE
        && rip < USER_BASE + image(id).executable_end.min(image(id).bytes.len() as u64)
        && rsp >= crate::memory::USER_STACK_LOW
        && rsp <= crate::memory::USER_STACK_TOP
}
