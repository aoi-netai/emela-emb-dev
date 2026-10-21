/* Non-overlapping code/data aliases of one 128 KiB SRAM1 slice; no flash or ROM. */
OUTPUT_ARCH(riscv)
ENTRY(_start)

MEMORY
{
    IRAM (rx) : ORIGIN = 0x40380000, LENGTH = 64K
    DRAM (rw) : ORIGIN = 0x3fc90000, LENGTH = 48K
    STACK (rw) : ORIGIN = 0x3fc9c000, LENGTH = 16K
}

SECTIONS
{
    _iram_start = ORIGIN(IRAM);
    _iram_end = ORIGIN(IRAM) + LENGTH(IRAM);
    _dram_start = ORIGIN(DRAM);
    _dram_end = ORIGIN(DRAM) + LENGTH(DRAM);
    .text : ALIGN(64)
    {
        KEEP(*(.text.startup))
        KEEP(*(.text.trap))
        *(.text .text.*)
        *(.rodata .rodata.*)
        . = ALIGN(4);
    } > IRAM

    .data : ALIGN(4)
    {
        _data_start = .;
        *(.data .data.* .sdata .sdata.*)
        . = ALIGN(4);
        _data_end = .;
    } > DRAM

    __global_pointer$ = _data_start + 0x800;
    .bss (NOLOAD) : ALIGN(4)
    {
        _bss_start = .;
        *(.bss .bss.* .sbss .sbss.*)
        *(COMMON)
        . = ALIGN(4);
        _bss_end = .;
    } > DRAM

    .stack (NOLOAD) : ALIGN(16)
    {
        _stack_guard_start = .;
        . += 256;
        _stack_bottom = .;
        . = ORIGIN(STACK) + LENGTH(STACK);
        _stack_top = .;
    } > STACK

    /DISCARD/ : { *(.comment .note.* .eh_frame*) }
}

ASSERT(_bss_end <= _dram_end, "controller data exceeds SRAM slice")
ASSERT(_stack_bottom < _stack_top, "controller stack reservation is empty")
