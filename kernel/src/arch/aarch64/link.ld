ENTRY(_start)

SECTIONS
{
    /* RAM on QEMU `virt` starts at 0x40000000. QEMU puts the device tree blob
       (1 MiB) there, but only when the kernel image does not overlap it, so
       the kernel is linked 2 MiB above the start of RAM. */
    . = 0x40200000;
    __image_start = .;

    .text : {
        KEEP(*(.text.boot))
        *(.text .text.*)
    }

    .rodata : ALIGN(8) {
        *(.rodata .rodata.*)
    }

    .data : ALIGN(8) {
        *(.data .data.*)
    }

    /* 8-byte alignment at both ends: the boot code clears .bss one word at a time. */
    .bss (NOLOAD) : ALIGN(8) {
        __bss_start = .;
        *(.bss .bss.*)
        *(COMMON)
        . = ALIGN(8);
        __bss_end = .;
    }

    /* 64 KiB boot stack; AArch64 requires a 16-byte aligned stack pointer.
       Its own NOLOAD section, so that the segment's size in memory covers the
       stack and the bootloader reserves it with the image (ADR 0013). */
    .stack (NOLOAD) : ALIGN(16) {
        __stack_bottom = .;
        . += 0x10000;
        __stack_top = .;
    }

    /DISCARD/ : {
        *(.comment)
        *(.eh_frame*)
    }
}
