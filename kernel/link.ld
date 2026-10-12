ENTRY(_start)
PHDRS {
  text PT_LOAD FLAGS(5);
  rodata PT_LOAD FLAGS(4);
  data PT_LOAD FLAGS(6);
}
SECTIONS {
  . = 0x100000;
  __kernel_start = .;
  __text_start = .;
  .text : { KEEP(*(.text.entry)) *(.text .text.*) } :text
  . = ALIGN(4096);
  __text_end = .;
  __rodata_start = .;
  .rodata : { *(.rodata .rodata.*) KEEP(*(.user_blob)) } :rodata
  . = ALIGN(4096);
  __rodata_end = .;
  __data_start = .;
  .data : { *(.data .data.*) *(.got .got.*) } :data
  .bss : { *(.bss .bss.*) *(COMMON) } :data
  . = ALIGN(4096);
  __kernel_end = .;
  ASSERT(__kernel_end <= 0x3F0000, "kernel exceeds the reserved first 4 MiB")
  /DISCARD/ : { *(.eh_frame*) *(.comment*) *(.note*) }
}
