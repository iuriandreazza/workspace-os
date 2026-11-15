// Entry point. QEMU `virt` jumps here with the MMU off, at EL1 by default.
.section .text.boot
.global _start
_start:
    // Interrupts stay masked until the kernel installs an exception table.
    msr  daifset, #0xf

    // Only the boot core runs the kernel: there is a single boot stack.
    mrs  x0, mpidr_el1
    and  x0, x0, #0xffffff
    cbnz x0, .Lpark

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
    bl   kernel_main
.Lpark:
    wfe
    b    .Lpark
