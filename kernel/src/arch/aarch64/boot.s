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
    // From here on a fault ends in a kernel panic instead of a silent hang.
    adrp x0, exception_vectors
    add  x0, x0, :lo12:exception_vectors
    msr  vbar_el1, x0
    isb

    adrp x0, __stack_top
    add  x0, x0, :lo12:__stack_top
    mov  sp, x0

    // Rust assumes zero-initialised statics.
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
    mov  x0, x19
    mov  x1, x21
    mov  x2, x22
    bl   kernel_main
.Lpark:
    wfe
    b    .Lpark
