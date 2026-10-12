.intel_syntax noprefix
.code16
.section .text
.global _start
_start:
    cli
    .byte 0xea
    .word normalized, 0
normalized:
    xor ax, ax
    mov ds, ax
    mov es, ax
    mov ss, ax
    mov sp, 0x7c00
    cld
    sti
    mov byte ptr [drive], dl
    mov bx, 0x55aa
    mov ah, 0x41
    int 0x13
    jc failed
    cmp bx, 0xaa55
    jne failed
    test cx, 1
    jz failed
    mov bp, 3
retry:
    mov word ptr [packet + 2], 32
    mov si, offset packet
    mov dl, byte ptr [drive]
    mov ah, 0x42
    int 0x13
    jnc loaded
    xor ah, ah
    mov dl, byte ptr [drive]
    int 0x13
    dec bp
    jnz retry
failed:
    mov si, offset error
print:
    lodsb
    test al, al
    jz stopped
    mov ah, 0x0e
    mov bx, 7
    int 0x10
    jmp print
stopped:
    cli
    hlt
    jmp stopped
loaded:
    mov dl, byte ptr [drive]
    .byte 0xea
    .word 0x8000, 0
.balign 4
packet:
    .byte 16, 0
    .word 32, 0x8000, 0
    .quad 1
drive: .byte 0
error: .asciz "hpOS: BIOS disk extensions/read failed"
