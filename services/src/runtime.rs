use core::{
    arch::{asm, global_asm},
    ffi::c_void,
    panic::PanicInfo,
};

global_asm!(
    r#"
.section .text.entry,"ax"
.global _start
_start:
    mov r15, rdi
    mov ax, 0x33
    mov ds, ax
    mov es, ax
    mov rbx, 0xffffffff80100000
    mov r14, 0x1122334455667700
    or r14, r15
    mov [rbx], r14
.global user_kernel_probe
user_kernel_probe:
    mov rax, qword ptr [0x100000]
    mov eax, 4
    int 0x80
.global user_probe_resume
user_probe_resume:
    mov eax, 1
    mov edi, 1
    xor esi, esi
    int 0x80
    cmp rax, 3
    jne failed
    and rsp, -16
    call service_main
failed:
    mov eax, 4
    int 0x80
1:
    xor eax, eax
    mov edi, 100
    int 0x80
    jmp 1b
"#
);

pub struct Result {
    pub status: u64,
    pub word: u64,
    #[allow(dead_code)] // Only Receive returns a sender.
    pub sender: u64,
}
pub fn call(number: u64, arg: u64, word: u64) -> Result {
    let mut status = number;
    let value: u64;
    let sender: u64;
    unsafe {
        asm!("int 0x80", inout("rax") status, in("rdi") arg, in("rsi") word,
            lateout("rdx") value, lateout("rcx") sender, options(nostack));
    }
    Result {
        status,
        word: value,
        sender,
    }
}
pub fn sleep(ticks: u64) {
    let _ = call(0, ticks, 0);
}
pub fn report(sequence: u64, elapsed: u64) {
    if call(3, sequence, elapsed).status != 0 {
        fail();
    }
}
pub fn fail() -> ! {
    let _ = call(4, 0, 0);
    loop {
        sleep(100);
    }
}
#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    fail()
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
    if dst as usize <= src as usize {
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
