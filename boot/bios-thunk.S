# Long mode -> real mode BIOS disk call and back.
# Copied by the kernel to 0x8000 (code page) / 0x9000 (data page). The kernel
# fills the request at 0x9000, disables interrupts, reprograms the PIC/PIT for
# the BIOS and calls _start with the SysV ABI. Every page touched here is
# identity-mapped in all hpOS address spaces.
.intel_syntax noprefix
.section .text
.global _start
.code64
_start:
    push rbx
    push rbp
    push r12
    push r13
    push r14
    push r15
    pushfq
    cli
    mov qword ptr [saved_rsp], rsp
    mov rax, cr3
    mov qword ptr [saved_cr3], rax
    mov rax, cr0
    mov qword ptr [saved_cr0], rax
    sgdt [saved_gdt]
    sidt [saved_idt]
    mov word ptr [saved_ds], ds
    mov word ptr [saved_es], es
    mov word ptr [saved_fs], fs
    mov word ptr [saved_gs], gs
    mov word ptr [saved_ss], ss
    lgdt [thunk_gdt_pointer]
    push 0x10
    mov eax, offset compat
    push rax
    retfq

.code32
compat:
    mov ax, 0x18
    mov ds, ax
    mov es, ax
    mov ss, ax
    mov fs, ax
    mov gs, ax
    # Clearing PG in compatibility mode leaves IA-32e mode.
    mov eax, cr0
    and eax, 0x7fffffff
    mov cr0, eax
    mov ecx, 0xc0000080
    rdmsr
    and eax, 0xfffffeff
    wrmsr
    .byte 0xea
    .long pm16
    .word 0x20

.code16
pm16:
    mov ax, 0x28
    mov ds, ax
    mov es, ax
    mov ss, ax
    mov fs, ax
    mov gs, ax
    mov eax, cr0
    and eax, 0xfffffffe
    mov cr0, eax
    .byte 0xea
    .word real, 0
real:
    xor ax, ax
    mov ds, ax
    mov es, ax
    mov fs, ax
    mov gs, ax
    mov ss, ax
    mov sp, 0x7c00
    lidt [real_idt]
    sti
    mov byte ptr [request_status], 0xff
    mov byte ptr [request_carry], 1
    cmp byte ptr [request_kind], 1
    je video_call
    mov bp, 3
disk_try:
    mov ax, word ptr [request_count]
    mov word ptr [request_dap + 2], ax
    mov ah, byte ptr [request_op]
    xor al, al
    mov dl, byte ptr [request_drive]
    mov si, offset request_dap
    int 0x13
    jnc disk_ok
    mov byte ptr [request_status], ah
    xor ah, ah
    mov dl, byte ptr [request_drive]
    int 0x13
    dec bp
    jnz disk_try
    jmp disk_done
# VBE set mode: BX comes from the request count field.
video_call:
    mov ax, 0x4f02
    mov bx, word ptr [request_count]
    int 0x10
    cmp ax, 0x004f
    je disk_ok
    mov byte ptr [request_status], ah
    jmp disk_done
disk_ok:
    mov byte ptr [request_status], 0
    mov byte ptr [request_carry], 0
disk_done:
    cli
    lgdt [thunk_gdt_pointer]
    mov eax, cr0
    or eax, 1
    mov cr0, eax
    .byte 0xea
    .word pm32, 0x10

.code32
pm32:
    mov ax, 0x18
    mov ds, ax
    mov es, ax
    mov ss, ax
    mov fs, ax
    mov gs, ax
    mov eax, dword ptr [saved_cr3]
    mov cr3, eax
    mov eax, cr4
    or eax, 0x20
    mov cr4, eax
    mov ecx, 0xc0000080
    rdmsr
    or eax, 0x100
    wrmsr
    # Restoring PG (with LME set) re-enters IA-32e compatibility mode.
    mov eax, dword ptr [saved_cr0]
    mov cr0, eax
    .byte 0xea
    .long lm64
    .word 0x08

.code64
lm64:
    lgdt [saved_gdt]
    lidt [saved_idt]
    mov rsp, qword ptr [saved_rsp]
    mov ax, word ptr [saved_ss]
    mov ss, ax
    mov ax, word ptr [saved_ds]
    mov ds, ax
    mov ax, word ptr [saved_es]
    mov es, ax
    mov ax, word ptr [saved_fs]
    mov fs, ax
    mov ax, word ptr [saved_gs]
    mov gs, ax
    push 0x18
    mov eax, offset finished
    push rax
    retfq
finished:
    popfq
    pop r15
    pop r14
    pop r13
    pop r12
    pop rbp
    pop rbx
    ret

# Data page at 0x9000. The kernel relies on the request offsets below.
.org 0x1000
request_op: .byte 0
request_drive: .byte 0
request_status: .byte 0
request_carry: .byte 0
request_count: .word 0
request_kind: .byte 0
.balign 8
request_dap:
    .byte 16, 0
    .word 0, 0, 0x1000
    .long 0, 0
.balign 8
saved_rsp: .long 0, 0
saved_cr3: .long 0, 0
saved_cr0: .long 0, 0
saved_gdt: .space 10, 0
saved_idt: .space 10, 0
saved_ds: .word 0
saved_es: .word 0
saved_fs: .word 0
saved_gs: .word 0
saved_ss: .word 0
real_idt:
    .word 0x3ff
    .long 0
.balign 8
thunk_gdt:
    .quad 0
    .quad 0x00af9a000000ffff
    .quad 0x00cf9a000000ffff
    .quad 0x00cf92000000ffff
    .quad 0x00009a000000ffff
    .quad 0x000092000000ffff
thunk_gdt_end:
thunk_gdt_pointer:
    .word thunk_gdt_end - thunk_gdt - 1
    .long thunk_gdt, 0
