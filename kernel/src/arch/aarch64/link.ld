/* The kernel runs in the upper half of the address space (ADR 0019): the
   virtual address of everything is its physical address plus KERNEL_OFFSET.
   The physical address stays fixed, so the loaders are unchanged (ADR 0013):
   they place segments by p_paddr and enter at e_entry, which is physical
   because the first instructions run with the MMU off or on the bootloader's
   identity map, before the kernel's own tables exist. */
KERNEL_OFFSET = 0xffffff8000000000;
/* RAM on QEMU `virt` starts at 0x40000000. QEMU puts the device tree blob
   (1 MiB) there, but only when the kernel image does not overlap it, so
   the kernel is linked 2 MiB above the start of RAM. */
KERNEL_PHYSICAL_BASE = 0x40200000;
PAGE = 4096;

ENTRY(_start_physical)

SECTIONS
{
    . = KERNEL_OFFSET + KERNEL_PHYSICAL_BASE;
    __image_start = .;
    _start_physical = _start - KERNEL_OFFSET;

    /* Every section starts on a page: the page tables give each its own
       permissions (W^X), and a page cannot hold two kinds. */
    .text : AT(ADDR(.text) - KERNEL_OFFSET) {
        KEEP(*(.text.boot))
        *(.text .text.*)
    }
    . = ALIGN(PAGE);
    __text_end = .;

    .rodata : AT(ADDR(.rodata) - KERNEL_OFFSET) {
        *(.rodata .rodata.*)
    }
    . = ALIGN(PAGE);
    __rodata_end = .;

    .data : AT(ADDR(.data) - KERNEL_OFFSET) {
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
    . = ALIGN(PAGE);
    __data_end = .;

    /* One unmapped page below the stack: an overflow faults. The exception
       handler then works on the same stack, so this is not a full guard
       (ADR 0019, "Known limits"). */
    . += PAGE;

    /* 64 KiB boot stack; AArch64 requires a 16-byte aligned stack pointer.
       Its own NOLOAD section, so that the segment's size in memory covers the
       stack and the bootloader reserves it with the image (ADR 0013). */
    .stack (NOLOAD) : ALIGN(PAGE) {
        __stack_bottom = .;
        . += 0x10000;
        __stack_top = .;
    }

    /DISCARD/ : {
        *(.comment)
        *(.eh_frame*)
    }

    /* The boot code maps the image with one level 3 table, which covers 2 MiB
       aligned to 2 MiB. */
    ASSERT(KERNEL_PHYSICAL_BASE % 0x200000 == 0, "the image must start on a 2 MiB boundary")
    ASSERT(__stack_top - __image_start <= 0x200000, "the image no longer fits the boot tables (2 MiB)")
}
