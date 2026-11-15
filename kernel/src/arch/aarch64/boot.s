// Entry point. QEMU `virt` jumps here at EL1 with the MMU off.
.section .text.boot
.global _start
_start:
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
3:
    wfe
    b    3b
