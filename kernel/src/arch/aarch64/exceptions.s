// Exception vector table for EL1. No exception is handled yet: every entry
// ends in a kernel panic, so a fault is reported instead of hanging silently.
.section .text
.balign 0x800
.global exception_vectors
exception_vectors:
.rept 16
    b    unhandled_exception_entry
    .balign 0x80
.endr

unhandled_exception_entry:
    // The interrupted stack may be what faulted, and this path never returns,
    // so start again from the top of the boot stack.
    adrp x2, __stack_top
    add  x2, x2, :lo12:__stack_top
    mov  sp, x2

    mrs  x0, esr_el1
    mrs  x1, elr_el1
    b    unhandled_exception
