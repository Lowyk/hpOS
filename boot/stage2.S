#include "layout.h"
.intel_syntax noprefix
.code16
.section .text
.global _start
_start:
    cli
    xor ax, ax
    mov ds, ax
    mov es, ax
    mov ss, ax
    mov sp, 0x7c00
    cld
    sti
    mov byte ptr [drive], dl
    int 0x12
    cmp ax, 576
    jb memory_error
    xor eax, eax
    mov di, 0x6000
    mov cx, 32
    rep stosw
    mov dword ptr [0x6000], 0x48504f53
    mov dword ptr [0x6004], 3
    mov dword ptr [0x6008], 0x5000
    movzx eax, byte ptr [drive]
    mov dword ptr [0x6030], eax
    # Shift held at boot (BIOS keyboard flags): start without touching the disk.
    test byte ptr [0x417], 3
    jz no_safe_storage
    or dword ptr [0x6034], 1
no_safe_storage:

    mov eax, 0x80000000
    cpuid
    cmp eax, 0x80000001
    jb cpu_error
    mov eax, 0x80000001
    cpuid
    test edx, 0x20000000
    jz cpu_error

    # Collect the BIOS E820 map before leaving real mode.
    xor ebx, ebx
    mov di, 0x5000
e820_next:
    mov dword ptr es:[di + 20], 1
    mov eax, 0xe820
    mov edx, 0x534d4150
    mov ecx, 24
    int 0x15
    jc e820_done
    cmp eax, 0x534d4150
    jne memory_error
    cmp ecx, 20
    jb memory_error
    inc dword ptr [0x6010]
    add di, 24
    cmp dword ptr [0x6010], 64
    jb e820_continue
    test ebx, ebx
    jnz memory_error
    jmp e820_done
e820_continue:
    test ebx, ebx
    jnz e820_next
e820_done:
    cmp dword ptr [0x6010], 0
    je memory_error

    # Read the kernel in 32 KiB transfers into a fixed bounce buffer below 1 MiB,
    # then let the BIOS block-move service copy each piece to its final address.
read_next:
    mov ax, word ptr [sectors_left]
    test ax, ax
    jz read_done
    cmp ax, 64
    jbe read_count
    mov ax, 64
read_count:
    mov word ptr [transfer_count], ax
    mov bp, 3
read_retry:
    mov ax, word ptr [transfer_count]
    mov word ptr [packet + 2], ax
    mov si, offset packet
    mov dl, byte ptr [drive]
    mov ah, 0x42
    int 0x13
    jnc read_copy
    xor ah, ah
    mov dl, byte ptr [drive]
    int 0x13
    dec bp
    jnz read_retry
    mov si, offset disk_message
    jmp fail
read_copy:
    mov eax, dword ptr [copy_target]
    mov word ptr [move_gdt + 0x1a], ax
    shr eax, 16
    mov byte ptr [move_gdt + 0x1c], al
    mov byte ptr [move_gdt + 0x1f], ah
    mov cx, word ptr [transfer_count]
    shl cx, 8
    mov si, offset move_gdt
    mov ah, 0x87
    int 0x15
    jc copy_error
    test ah, ah
    jnz copy_error
    movzx eax, word ptr [transfer_count]
    sub word ptr [sectors_left], ax
    add dword ptr [packet + 8], eax
    shl eax, 9
    add dword ptr [copy_target], eax
    jmp read_next
copy_error:
    mov si, offset copy_message
    jmp fail
read_done:
    # Fast A20 gate; verify that addresses no longer alias.
    in al, 0x92
    or al, 2
    and al, 0xfe
    out 0x92, al
    call a20_test
    jnc a20_ready
    mov ax, 0x2401
    int 0x15
    call a20_test
    jc a20_error
a20_ready:
    call video
    cli
    lgdt [gdt_pointer]
    mov eax, cr0
    or eax, 1
    mov cr0, eax
    .byte 0xea
    .word protected, 0x08

# Preserve both test bytes, including when A20 is still disabled.
a20_test:
    push es
    mov ax, 0xffff
    mov es, ax
    mov al, byte ptr [0x4000]
    mov ah, byte ptr es:[0x4010]
    push ax
    mov byte ptr [0x4000], 0
    mov byte ptr es:[0x4010], 0xff
    cmp byte ptr [0x4000], 0xff
    pop ax
    mov byte ptr es:[0x4010], ah
    mov byte ptr [0x4000], al
    pop es
    je a20_off
    clc
    ret
a20_off:
    stc
    ret

# Enumerate validated direct-color modes; prefer the EDID native size when offered.
video:
    mov dword ptr [0x3000], 0x32454256
    mov di, 0x3000
    mov ax, 0x4f00
    int 0x10
    cmp ax, 0x004f
    jne text_mode
    mov ax, word ptr [0x300e]
    mov word ptr [mode_cursor], ax
    mov ax, word ptr [0x3010]
    mov word ptr [mode_segment], ax
    mov word ptr [mode_budget], 512
    # Read base EDID through VBE/DDC; a bad or absent block is ignored.
    mov ax, 0x4f15
    mov bx, 1
    xor cx, cx
    xor dx, dx
    mov di, 0x3400
    int 0x10
    cmp ax, 0x004f
    jne mode_next
    cmp dword ptr [0x3400], 0xffffff00
    jne mode_next
    cmp dword ptr [0x3404], 0x00ffffff
    jne mode_next
    mov si, 0x3400
    mov cx, 128
    xor bl, bl
edid_sum:
    lodsb
    add bl, al
    loop edid_sum
    test bl, bl
    jnz mode_next
    cmp word ptr [0x3436], 0
    je mode_next
    movzx ax, byte ptr [0x3438]
    movzx bx, byte ptr [0x343a]
    and bx, 0xf0
    shl bx, 4
    or ax, bx
    mov word ptr [native_width], ax
    movzx ax, byte ptr [0x343b]
    movzx bx, byte ptr [0x343d]
    and bx, 0xf0
    shl bx, 4
    or ax, bx
    mov word ptr [native_height], ax
mode_next:
    dec word ptr [mode_budget]
    jz modes_collected
    mov ax, word ptr [mode_segment]
    mov fs, ax
    mov si, word ptr [mode_cursor]
    mov cx, word ptr fs:[si]
    add word ptr [mode_cursor], 2
    jnc mode_cursor_ok
    add word ptr [mode_segment], 0x1000
mode_cursor_ok:
    cmp cx, 0xffff
    je modes_collected
    mov word ptr [mode_number], cx
    mov di, 0x3200
    mov ax, 0x4f01
    int 0x10
    cmp ax, 0x004f
    jne mode_next
    mov ax, word ptr [0x3200]
    and ax, 0x0091
    cmp ax, 0x0091
    jne mode_next
    cmp byte ptr [0x3219], 32
    jne mode_next
    cmp byte ptr [0x321b], 6
    jne mode_next
    cmp word ptr [0x3212], 800
    jb mode_next
    cmp word ptr [0x3212], 1920
    ja mode_next
    cmp word ptr [0x3214], 600
    jb mode_next
    cmp word ptr [0x3214], 1200
    ja mode_next
    cmp byte ptr [0x321f], 8
    jne mode_next
    cmp byte ptr [0x3220], 16
    jne mode_next
    cmp byte ptr [0x3221], 8
    jne mode_next
    cmp byte ptr [0x3222], 8
    jne mode_next
    cmp byte ptr [0x3223], 8
    jne mode_next
    cmp byte ptr [0x3224], 0
    jne mode_next
    mov eax, dword ptr [0x3228]
    test eax, eax
    jz mode_next
    mov dword ptr [0x6018], eax
    movzx eax, word ptr [0x3212]
    mov dword ptr [0x6020], eax
    movzx eax, word ptr [0x3214]
    mov dword ptr [0x6024], eax
    movzx eax, word ptr [0x3210]
    mov dword ptr [0x6028], eax
    # VBE 3.0 has separate linear-framebuffer pitch and color masks.
    cmp word ptr [0x3004], 0x0300
    jb mode_pitch
    cmp byte ptr [0x3236], 8
    jne mode_next
    cmp byte ptr [0x3237], 16
    jne mode_next
    cmp byte ptr [0x3238], 8
    jne mode_next
    cmp byte ptr [0x3239], 8
    jne mode_next
    cmp byte ptr [0x323a], 8
    jne mode_next
    cmp byte ptr [0x323b], 0
    jne mode_next
    movzx eax, word ptr [0x3232]
    mov dword ptr [0x6028], eax
mode_pitch:
    mov eax, dword ptr [0x6020]
    shl eax, 2
    cmp dword ptr [0x6028], eax
    jb mode_next
    test dword ptr [0x6028], 3
    jnz mode_next
    mov eax, dword ptr [0x6028]
    mul dword ptr [0x6024]
    test edx, edx
    jnz mode_next
    cmp eax, 0x1000000
    ja mode_next
    add eax, dword ptr [0x6018]
    jc mode_next
    mov eax, dword ptr [0x6020]
    mul dword ptr [0x6024]
    mov bx, word ptr [0x6020]
    cmp bx, word ptr [native_width]
    jne mode_score_ready
    mov bx, word ptr [0x6024]
    cmp bx, word ptr [native_height]
    jne mode_score_ready
    or eax, 0x80000000
mode_score_ready:
    mov dword ptr [candidate_score], eax
    and eax, 0x7fffffff
    cmp eax, dword ptr [safe_area]
    jae safe_candidate_done
    mov dword ptr [safe_area], eax
    mov di, offset safe_mode
    call mode_entry_write
safe_candidate_done:
    xor bp, bp
    mov di, offset mode_table
mode_duplicate:
    cmp bp, word ptr [mode_count]
    jae mode_rank_start
    mov ax, word ptr [0x6020]
    cmp ax, word ptr [di + 2]
    jne mode_duplicate_next
    mov ax, word ptr [0x6024]
    cmp ax, word ptr [di + 4]
    je mode_next
mode_duplicate_next:
    inc bp
    add di, 16
    jmp mode_duplicate
mode_rank_start:
    xor bp, bp
    mov di, offset mode_table
mode_rank:
    cmp bp, word ptr [mode_count]
    jae mode_insert
    mov eax, dword ptr [candidate_score]
    cmp eax, dword ptr [di + 12]
    ja mode_insert
    inc bp
    add di, 16
    jmp mode_rank
mode_insert:
    cmp bp, 12
    jae mode_next
    mov bx, word ptr [mode_count]
    cmp bx, 12
    jb mode_shift
    mov bx, 11
mode_shift:
    cmp bx, bp
    jbe mode_store
    mov ax, bx
    shl ax, 4
    mov di, offset mode_table
    add di, ax
    mov si, di
    sub si, 16
    mov cx, 8
    rep movsw
    dec bx
    jmp mode_shift
mode_store:
    mov ax, bp
    shl ax, 4
    mov di, offset mode_table
    add di, ax
    call mode_entry_write
    cmp word ptr [mode_count], 12
    jae mode_next
    inc word ptr [mode_count]
    jmp mode_next
modes_collected:
    cmp word ptr [mode_count], 0
    je text_mode
    cmp word ptr [mode_count], 1
    je mode_choose
    mov ah, 0
    int 0x1a
    mov word ptr [menu_start], dx
mode_menu:
    mov ax, 3
    int 0x10
    mov si, offset video_message
    call video_print
    xor bp, bp
mode_menu_row:
    mov al, ' '
    cmp bp, word ptr [selected_mode]
    jne mode_menu_marker
    mov al, '>'
mode_menu_marker:
    call video_char
    mov al, ' '
    call video_char
    mov ax, bp
    shl ax, 4
    mov di, offset mode_table
    add di, ax
    mov ax, word ptr [di + 2]
    call video_number
    mov al, 'x'
    call video_char
    mov ax, word ptr [di + 4]
    call video_number
    test dword ptr [di + 12], 0x80000000
    jz mode_menu_line
    mov si, offset native_message
    call video_print
mode_menu_line:
    mov si, offset newline_message
    call video_print
    inc bp
    cmp bp, word ptr [mode_count]
    jb mode_menu_row
    mov si, offset video_keys_message
    call video_print
    mov si, offset storage_on_message
    test dword ptr [0x6034], 1
    jz storage_line
    mov si, offset storage_off_message
storage_line:
    call video_print
mode_menu_wait:
    mov ah, 1
    int 0x16
    jnz mode_menu_key
    cmp byte ptr [menu_touched], 0
    jne mode_menu_wait
    xor ah, ah
    int 0x1a
    sub dx, word ptr [menu_start]
    cmp dx, 91
    jb mode_menu_wait
    jmp mode_choose
mode_menu_key:
    xor ah, ah
    int 0x16
    mov byte ptr [menu_touched], 1
    cmp al, 13
    je mode_choose
    cmp al, 27
    jne mode_menu_navigation
    mov word ptr [selected_mode], 0
    jmp mode_choose
mode_menu_navigation:
    cmp ah, 0x3b
    je mode_menu_safe
    cmp ah, 0x3c
    jne mode_menu_arrows
    xor dword ptr [0x6034], 1
    jmp mode_menu
mode_menu_arrows:
    cmp ah, 0x48
    je mode_menu_up
    cmp ah, 0x50
    jne mode_menu_wait
    inc word ptr [selected_mode]
    mov ax, word ptr [selected_mode]
    cmp ax, word ptr [mode_count]
    jb mode_menu
    mov word ptr [selected_mode], 0
    jmp mode_menu
mode_menu_up:
    cmp word ptr [selected_mode], 0
    jne mode_menu_up_step
    mov ax, word ptr [mode_count]
    mov word ptr [selected_mode], ax
mode_menu_up_step:
    dec word ptr [selected_mode]
    jmp mode_menu
mode_menu_safe:
    mov ax, word ptr [mode_count]
    dec ax
    mov word ptr [selected_mode], ax
    shl ax, 4
    mov di, offset mode_table
    add di, ax
    mov si, offset safe_mode
    mov cx, 8
    rep movsw
    jmp mode_choose
mode_choose:
    mov ax, word ptr [mode_count]
    mov word ptr [mode_attempts], ax
mode_try:
    mov ax, word ptr [selected_mode]
    shl ax, 4
    mov di, offset mode_table
    add di, ax
    movzx eax, word ptr [di + 2]
    mov dword ptr [0x6020], eax
    movzx eax, word ptr [di + 4]
    mov dword ptr [0x6024], eax
    movzx eax, word ptr [di + 6]
    mov dword ptr [0x6028], eax
    mov eax, dword ptr [di + 8]
    mov dword ptr [0x6018], eax
    mov bx, word ptr [di]
    or bx, 0x4000
    mov ax, 0x4f02
    int 0x10
    cmp ax, 0x004f
    je mode_success
    dec word ptr [mode_attempts]
    jz text_mode
    inc word ptr [selected_mode]
    mov ax, word ptr [selected_mode]
    cmp ax, word ptr [mode_count]
    jb mode_try
    mov word ptr [selected_mode], 0
    jmp mode_try
mode_success:
    mov dword ptr [0x602c], 32
    mov dword ptr [0x6014], 1
    # Hand the validated mode list to the kernel for live switching.
    movzx eax, word ptr [mode_count]
    mov dword ptr [0x6038], eax
    movzx eax, word ptr [selected_mode]
    mov dword ptr [0x603c], eax
    mov si, offset mode_table
    mov di, 0x6100
    mov cx, 12 * 8
    rep movsw
    ret
mode_entry_write:
    mov ax, word ptr [mode_number]
    mov word ptr [di], ax
    mov ax, word ptr [0x6020]
    mov word ptr [di + 2], ax
    mov ax, word ptr [0x6024]
    mov word ptr [di + 4], ax
    mov ax, word ptr [0x6028]
    mov word ptr [di + 6], ax
    mov eax, dword ptr [0x6018]
    mov dword ptr [di + 8], eax
    mov eax, dword ptr [candidate_score]
    mov dword ptr [di + 12], eax
    ret
video_number:
    pusha
    mov bx, 10
    xor cx, cx
video_digits:
    xor dx, dx
    div bx
    push dx
    inc cx
    test ax, ax
    jnz video_digits
video_digit:
    pop ax
    add al, '0'
    call video_char
    loop video_digit
    popa
    ret
video_print:
    lodsb
    test al, al
    jz video_print_done
    call video_char
    jmp video_print
video_print_done:
    ret
video_char:
    pusha
    mov bx, 7
    mov ah, 0x0e
    int 0x10
    popa
    ret
text_mode:
    mov dword ptr [0x6014], 0
    mov ax, 3
    int 0x10
    ret
cpu_error:
    mov si, offset cpu_message
    jmp fail
memory_error:
    mov si, offset memory_message
    jmp fail
a20_error:
    mov si, offset a20_message
fail:
    lodsb
    test al, al
    jz halt
    mov ah, 0x0e
    mov bx, 7
    int 0x10
    jmp fail
halt:
    cli
    hlt
    jmp halt

.code32
protected:
    mov ax, 0x10
    mov ds, ax
    mov es, ax
    mov ss, ax
    mov fs, ax
    mov gs, ax
    mov esp, 0x90000
    cld
    # Check that E820 offers usable RAM for the kernel and its bootstrap stack.
    mov esi, 0x5000
    mov ecx, dword ptr [0x6010]
ram_next:
    cmp dword ptr [esi + 16], 1
    jne ram_skip
    test dword ptr [esi + 20], 1
    jz ram_skip
    cmp dword ptr [esi + 4], 0
    jne ram_skip
    cmp dword ptr [esi], 0x100000
    ja ram_skip
    mov eax, dword ptr [esi]
    xor edx, edx
    add eax, dword ptr [esi + 8]
    adc edx, dword ptr [esi + 12]
    test edx, edx
    jnz ram_ok
    cmp eax, KERNEL_END
    jae ram_ok
ram_skip:
    add esi, 24
    loop ram_next
    jmp protected_halt
ram_ok:

    # PML4 -> PDPT -> four page directories, identity-mapped with 2 MiB pages.
    mov edi, 0x70000
    xor eax, eax
    mov ecx, 0x6000 / 4
    rep stosd
    mov dword ptr [0x70000], 0x71003
    mov edi, 0x71000
    mov eax, 0x72003
    mov ecx, 4
pdpt_next:
    mov dword ptr [edi], eax
    add eax, 0x1000
    add edi, 8
    loop pdpt_next
    mov edi, 0x72000
    mov eax, 0x83
    mov ecx, 2048
pd_next:
    mov dword ptr [edi], eax
    add eax, 0x200000
    add edi, 8
    loop pd_next
    # MMIO framebuffer pages are uncached until a measured PAT/WC policy exists.
    test dword ptr [0x6014], 1
    jz paging_ready
    mov esi, dword ptr [0x6018]
    shr esi, 21
    mov eax, dword ptr [0x6028]
    mul dword ptr [0x6024]
    add eax, dword ptr [0x6018]
    dec eax
    shr eax, 21
    sub eax, esi
    inc eax
    lea edi, [esi * 8 + 0x72000]
framebuffer_page:
    or dword ptr [edi], 0x18
    add edi, 8
    dec eax
    jnz framebuffer_page
paging_ready:
    mov eax, cr4
    or eax, 0x620
    mov cr4, eax
    mov eax, 0x70000
    mov cr3, eax
    mov ecx, 0xc0000080
    rdmsr
    or eax, 0x100
    wrmsr
    mov eax, cr0
    and eax, 0xfffffffb
    or eax, 0x80000003
    mov cr0, eax
    .byte 0xea
    .long long_mode
    .word 0x18
protected_halt:
    mov word ptr [0xb8000], 0x4f45
    cli
    hlt
    jmp protected_halt

.code64
long_mode:
    mov ax, 0x20
    mov ds, ax
    mov es, ax
    mov ss, ax
    xor eax, eax
    mov fs, ax
    mov gs, ax
    mov rsp, 0x90000
    xor rbp, rbp
    mov rdi, 0x6000
    mov rax, KERNEL_ENTRY
    jmp rax
long_halt:
    cli
    hlt
    jmp long_halt

.balign 8
gdt:
    .quad 0
    .quad 0x00cf9a000000ffff
    .quad 0x00cf92000000ffff
    .quad 0x00af9a000000ffff
    .quad 0x00cf92000000ffff
gdt_end:
gdt_pointer:
    .word gdt_end - gdt - 1
    .long gdt
.balign 4
packet:
    .byte 16, 0
    .word 0, 0, 0x1000
    .quad KERNEL_LBA
sectors_left: .word KERNEL_SECTORS
.balign 8
# INT 15h AH=87h descriptor table: source is the bounce buffer at 0x10000.
move_gdt:
    .quad 0, 0
    .word 0xffff, 0x0000
    .byte 0x01, 0x93, 0x00, 0x00
    .word 0xffff, 0x0000
    .byte 0x10, 0x93, 0x00, 0x00
    .quad 0, 0
copy_target: .long 0x100000
transfer_count: .word 0
mode_cursor: .word 0
mode_number: .word 0
mode_budget: .word 0
mode_segment: .word 0
mode_count: .word 0
selected_mode: .word 0
mode_attempts: .word 0
native_width: .word 0
native_height: .word 0
menu_start: .word 0
menu_touched: .byte 0
.balign 4
candidate_score: .long 0
safe_area: .long 0xffffffff
safe_mode: .space 16, 0
mode_table: .space 12 * 16, 0
video_message: .asciz "hpOS 0.10 - Huynya Polneyshaya OS / DISPLAY MODE\r\n\r\n"
native_message: .asciz "  NATIVE"
newline_message: .asciz "\r\n"
video_keys_message: .asciz "\r\nUp/Down: select  Enter: boot  F1: safe mode\r\nEsc: default. Default starts after 5 seconds.\r\n"
storage_on_message: .asciz "F2: disk access ON (Shift at boot also disables it)\r\n"
storage_off_message: .asciz "F2: disk access OFF - files and settings are not saved\r\n"
drive: .byte 0
cpu_message: .asciz "hpOS: x86_64 CPU required"
memory_message: .asciz "hpOS: invalid or oversized E820 map"
disk_message: .asciz "hpOS: kernel read failed"
copy_message: .asciz "hpOS: kernel copy above 1 MiB failed"
a20_message: .asciz "hpOS: cannot enable A20"
