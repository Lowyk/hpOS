ENTRY(_start)
PHDRS { text PT_LOAD FLAGS(5); rodata PT_LOAD FLAGS(4); data PT_LOAD FLAGS(6); }
SECTIONS {
  . = 0xffffffff80000000;
  .text : { KEEP(*(.text.entry)) *(.text .text.*) } :text
  . = ALIGN(4096);
  __text_end = .;
  .rodata : { *(.rodata .rodata.*) } :rodata
  . = ALIGN(4096);
  __rodata_end = .;
  .data : { *(.data .data.*) *(.got .got.*) } :data
  .bss : { *(.bss .bss.*) *(COMMON) } :data
  . = ALIGN(4096);
  __image_end = .;
  ASSERT(__image_end <= 0xffffffff80080000, "service image exceeds 512 KiB")
  /DISCARD/ : { *(.eh_frame*) *(.comment*) *(.note*) }
}
