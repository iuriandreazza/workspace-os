// Entry point, with two conventions (ADR 0013):
//
// - QEMU `virt` loading the image directly: the MMU is off, the level is EL1 by
//   default, EL2 with `virtualization=on` and EL3 with `secure=on`, x0 holds the
//   device tree address or zero, and x1 is anything.
// - The bootloader: x1 holds the entry magic "WSOSENTR" and x0 the address of
//   the boot info. The processor is the boot processor, already at EL1 with
//   the MMU and the caches on and the identity map installed, so none of the
//   work the direct path does to get there is done: no parking by MPIDR, no
//   drop from a higher level, and SCTLR_EL1 is left alone.
.section .text.boot
.global _start
_start:
    // x0 is the device tree address (direct) or the boot info address
    // (bootloader); QEMU's ELF loader leaves it zero. Keep it in x19, which
    // nothing below touches, and hand it to kernel_main. x21 records which
    // convention was used. The code below only uses x0 to x2.
    mov  x19, x0
    // x2 and x3 must be zero on the bootloader path; kernel_main reports them.
    orr  x22, x2, x3

    // Interrupts stay masked until the kernel installs an exception table.
    msr  daifset, #0xf

    // The entry magic is checked first, before any register is overwritten.
    movz x0, #0x5753, lsl #48
    movk x0, #0x4f53, lsl #32
    movk x0, #0x454e, lsl #16
    movk x0, #0x5452
    cmp  x1, x0
    cset x21, eq
    cbnz x21, .Lcommon

    // Only the boot core runs the kernel: there is a single boot stack.
    // On QEMU `virt` the boot core is the one with affinity 0. This read has
    // to happen before the drop to EL1, where MPIDR_EL1 reads VMPIDR_EL2.
    mrs  x0, mpidr_el1
    and  x0, x0, #0xffffff
    cbnz x0, .Lpark

    // The kernel runs in EL1 whatever level it was started at.
    mrs  x0, CurrentEL
    lsr  x0, x0, #2
    cmp  x0, #3
    b.eq .Lfrom_el3
    cmp  x0, #2
    b.eq .Lfrom_el2
    b    .Lel1

.Lfrom_el3:
    // RW: lower levels are AArch64. HCE: hvc is enabled. Bits 5:4 are RES1.
    // NS: the kernel runs
    // in the non-secure world.
    mov  x0, #((1 << 10) | (1 << 8) | (3 << 4) | (1 << 0))
    msr  scr_el3, x0
    msr  cptr_el3, xzr              // FP and SIMD do not trap to EL3
    // HCR_EL2.RW resets to an unknown value, so when EL2 exists the kernel goes
    // through it: EL1 could otherwise come up as AArch32.
    mrs  x0, id_aa64pfr0_el1
    ubfx x0, x0, #8, #4
    cbz  x0, 1f
    mov  x1, #0x3c9                 // EL2h, DAIF masked
    adr  x2, .Lfrom_el2
    b    2f
1:
    mov  x1, #0x3c5                 // EL1h, DAIF masked
    adr  x2, .Lel1
2:
    msr  spsr_el3, x1
    msr  elr_el3, x2
    eret

.Lfrom_el2:
    // SCTLR_EL2 resets to an unknown value: set the RES1 bits, nothing else.
    movz x0, #0x30c5, lsl #16
    movk x0, #0x0830
    msr  sctlr_el2, x0
    mov  x0, #(1 << 31)             // HCR_EL2: RW only, EL1 is AArch64
    msr  hcr_el2, x0
    // Reset values are unknown: FP, SIMD, debug and trace accesses must not trap
    // to EL2, where no handler exists.
    mov  x0, #0x33ff                // CPTR_EL2: RES1 bits only
    msr  cptr_el2, x0
    msr  mdcr_el2, xzr
    msr  hstr_el2, xzr
    // EL1 reads of MIDR_EL1 and MPIDR_EL1 return these.
    mrs  x0, midr_el1
    msr  vpidr_el2, x0
    mrs  x0, mpidr_el1
    msr  vmpidr_el2, x0
    mov  x0, #3                     // EL1 may read the counter and the timer
    msr  cnthctl_el2, x0
    msr  cntvoff_el2, xzr
    mov  x0, #0x3c5                 // EL1h, DAIF masked
    msr  spsr_el2, x0
    adr  x0, .Lel1
    msr  elr_el2, x0
    eret

.Lel1:
    // SCTLR_EL1 is unknown after a drop from a higher level: RES1 bits only,
    // so the MMU and the caches are off. The boot protocol guarantees that
    // they were off already: nothing is cleaned from a data cache here.
    movz x0, #0x30d0, lsl #16
    movk x0, #0x0800
    msr  sctlr_el1, x0
    isb

.Lcommon:
    // The code up to the jump below runs at the physical address the image was
    // loaded at, while the linker placed it in the upper half (ADR 0019):
    // `adrp` of a symbol therefore gives the symbol's physical address, and
    // `ldr x, =symbol` its virtual one. Nothing here uses the stack or a
    // vector table, and a fault in this stretch hangs.

    // Rust assumes zero-initialised statics, and the boot tables below are in
    // `.bss`.
    adrp x0, __bss_start
    add  x0, x0, :lo12:__bss_start
    adrp x1, __bss_end
    add  x1, x1, :lo12:__bss_end
1:
    cmp  x0, x1
    b.hs 2f
    str  xzr, [x0], #8
    b    1b
2:
    // The memory types the descriptors name: the indexes into MAIR_EL1 of
    // Normal write-back (0xff) and Device-nGnRnE (0x00). The bootloader leaves
    // MAIR_EL1 as the firmware had it, so they are looked up there; the direct
    // path installs its own below. x24 and x25 hold index << 2, the position
    // of AttrIndx in a descriptor, or -1 while not found.
    cbnz x21, 3f
    ldr  x0, ={DIRECT_ENTRY_MAIR}
    b    4f
3:
    mrs  x0, mair_el1
4:
    mov  x23, #0
    mov  x24, #-1
    mov  x25, #-1
5:
    lsr  x1, x0, x23
    and  x1, x1, #0xff
    lsr  x2, x23, #1
    cmp  x1, #0xff
    b.ne 6f
    cmn  x24, #1
    csel x24, x2, x24, eq
6:
    cbnz x1, 7f
    cmn  x25, #1
    csel x25, x2, x25, eq
7:
    add  x23, x23, #8
    cmp  x23, #64
    b.lo 5b
    // A MAIR_EL1 without both types cannot describe the kernel's memory.
    cmn  x24, #1
    b.eq .Lpark
    cmn  x25, #1
    b.eq .Lpark

    // Descriptor templates (the bits are kernelvm's, whose tests compare them
    // with what its own code builds; the boot audit checks the tables):
    //   x26  Normal page, read and execute for EL1 (text), AttrIndx included
    //   x27  Normal 2 MiB block, read and write, never executed
    //   x28  Device 1 GiB block, read and write, never executed
    ldr  x0, ={NORMAL_PAGE_BITS}
    orr  x26, x24, x0
    ldr  x0, ={NORMAL_BLOCK_BITS}
    orr  x27, x24, x0
    ldr  x0, ={DEVICE_BLOCK_BITS}
    orr  x28, x25, x0

    // The image's pages, at level 3: 512 entries cover 2 MiB from the start of
    // the image (link.ld checks that it fits). Text is read and execute,
    // .rodata read only, .data, .bss and the stack read and write, and the
    // guard page below the stack stays empty. Nothing is writable and
    // executable.
    adrp x9, __image_start
    adrp x10, boot_l3
    adrp x11, __text_end
    adrp x12, __rodata_end
    adrp x13, __data_end
    adrp x14, __stack_bottom
    adrp x15, __stack_top
    mov  x1, #0
8:
    add  x2, x9, x1, lsl #12
    mov  x3, #0
    cmp  x2, x11
    b.lo 10f                        // text
    cmp  x2, x12
    b.lo 11f                        // .rodata
    cmp  x2, x13
    b.lo 12f                        // .data and .bss
    cmp  x2, x14
    b.lo 13f                        // guard page
    cmp  x2, x15
    b.hs 13f                        // beyond the stack
    b    12f                        // stack
10:
    orr  x3, x2, x26
    orr  x3, x3, #{READ_ONLY}
    b    13f
11:
    orr  x3, x2, x26
    orr  x3, x3, #{READ_ONLY}
    orr  x3, x3, #{PXN}
    b    13f
12:
    orr  x3, x2, x26
    orr  x3, x3, #{PXN}
13:
    str  x3, [x10, x1, lsl #3]
    add  x1, x1, #1
    cmp  x1, #512
    b.lo 8b

    // The upper half: level 1 -> level 2 -> the table above, at the indexes of
    // the image's virtual address.
    ldr  x0, =__image_start
    ubfx x1, x0, #30, #9
    ubfx x2, x0, #21, #9
    adrp x4, boot_l1_high
    adrp x5, boot_l2_high
    orr  x3, x5, #{TABLE_BITS}
    str  x3, [x4, x1, lsl #3]
    orr  x3, x10, #{TABLE_BITS}
    str  x3, [x5, x2, lsl #3]

    cbnz x21, 20f
    // The direct path also needs the lower half, since the instruction after
    // the MMU is switched on is still at its physical address and the kernel
    // reads the device tree and the UART there: 1 GiB blocks, the first
    // Device (QEMU `virt` keeps its devices below 1 GiB) and the rest
    // Normal, and the image's own gigabyte down to the table above, so that
    // the text is executable and nothing else is.
    adrp x4, boot_l1_low
    mov  x1, #0
14:
    lsl  x2, x1, #30
    orr  x3, x2, x27
    cmp  x1, #0
    csel x3, x28, x3, eq
    str  x3, [x4, x1, lsl #3]
    add  x1, x1, #1
    cmp  x1, #512
    b.lo 14b
    adrp x5, boot_l2_low
    ubfx x1, x9, #30, #9
    orr  x3, x5, #{TABLE_BITS}
    str  x3, [x4, x1, lsl #3]
    and  x6, x9, #0xffffffffc0000000   // first byte of the image's gigabyte
    mov  x1, #0
15:
    add  x2, x6, x1, lsl #21
    orr  x3, x2, x27
    str  x3, [x5, x1, lsl #3]
    add  x1, x1, #1
    cmp  x1, #512
    b.lo 15b
    ubfx x1, x9, #21, #9
    orr  x3, x10, #{TABLE_BITS}
    str  x3, [x5, x1, lsl #3]
20:
    // The tables reach memory before the hardware walks them: clean and
    // invalidate them by address (not by set and way, which the bootloader's
    // contract forbids), then order the stores.
    adrp x0, boot_tables_start
    adrp x1, boot_tables_end
    mrs  x2, ctr_el0                // DminLine: log2 of the smallest line, in words
    ubfx x2, x2, #16, #4
    mov  x3, #4
    lsl  x2, x3, x2
21:
    dc   civac, x0
    add  x0, x0, x2
    cmp  x0, x1
    b.lo 21b
    dsb  sy

    cbnz x21, 22f
    // The direct path owns every translation register.
    ldr  x0, ={DIRECT_ENTRY_MAIR}
    msr  mair_el1, x0
    ldr  x0, ={TCR_BOTH_HALVES}
    // IPS: the physical address size the processor has, at most 48 bits.
    mrs  x1, id_aa64mmfr0_el1
    and  x1, x1, #0xf
    mov  x2, #5
    cmp  x1, x2
    csel x1, x1, x2, lo
    orr  x0, x0, x1, lsl #32
    msr  tcr_el1, x0
    adrp x0, boot_l1_low
    msr  ttbr0_el1, x0
    b    23f
22:
    // The bootloader path keeps its identity map in TTBR0_EL1 and its lower
    // half of TCR_EL1; the upper half is ours.
    mrs  x0, tcr_el1
    mov  x1, #{TCR_UPPER_MASK}
    bic  x0, x0, x1
    ldr  x1, ={TCR_UPPER_HALF}
    orr  x0, x0, x1
    msr  tcr_el1, x0
23:
    adrp x0, boot_l1_high
    msr  ttbr1_el1, x0
    isb
    tlbi vmalle1
    dsb  nsh
    isb
    cbnz x21, 24f
    // MMU on; the caches stay off on this path, as they were (ADR 0019).
    movz x0, #0x30d0, lsl #16
    movk x0, #0x0801
    msr  sctlr_el1, x0
    isb
24:
    // Into the upper half. From here the PC, `adrp` and `bl` are virtual.
    ldr  x0, =.Lupper
    br   x0
.Lupper:
    adrp x0, exception_vectors
    add  x0, x0, :lo12:exception_vectors
    msr  vbar_el1, x0
    isb

    // The tables are in place: from now on whatever is writable is never
    // executable, whatever a descriptor says. This is not done earlier because
    // the bootloader's map holds the code it is still running from as both.
    mrs  x0, sctlr_el1
    orr  x0, x0, #{WXN}
    msr  sctlr_el1, x0
    isb
    tlbi vmalle1
    dsb  nsh
    isb

    adrp x0, __stack_top
    add  x0, x0, :lo12:__stack_top
    mov  sp, x0

    mov  x0, x19
    mov  x1, x21
    mov  x2, x22
    bl   kernel_main
.Lpark:
    wfe
    b    .Lpark

// Tables of the boot map, in .bss (zeroed above): page aligned, one page each.
.section .bss.boot_tables, "aw", @nobits
.balign 4096
boot_tables_start:
boot_l1_high:
    .skip 4096
boot_l2_high:
    .skip 4096
boot_l3:
    .skip 4096
boot_l1_low:
    .skip 4096
boot_l2_low:
    .skip 4096
boot_tables_end:
