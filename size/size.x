/* The size bins' memory: large regions so every variant links; sizes are read from the sections. */
MEMORY
{
  FLASH : ORIGIN = 0x00000000, LENGTH = 16M
  RAM : ORIGIN = 0x20000000, LENGTH = 16M
}

ENTRY(_start)

SECTIONS
{
  .text : { KEEP(*(.text._start)) *(.text .text.*) } > FLASH
  .rodata : { *(.rodata .rodata.* .srodata .srodata.*) } > FLASH
  .data : { *(.data .data.* .sdata .sdata.*) } > RAM AT > FLASH
  .bss (NOLOAD) : { *(.bss .bss.* .sbss .sbss.* COMMON) } > RAM
  /DISCARD/ : { *(.ARM.exidx .ARM.exidx.*) }
}
