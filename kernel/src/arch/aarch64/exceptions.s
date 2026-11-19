// Exception vector table for EL1.
//
// A synchronous exception taken from the kernel itself (current EL, SP_ELx)
// saves the whole processor context on the interrupted stack, calls the Rust
// handler and returns to the interrupted code. Every other entry is not
// handled yet and ends in a kernel panic that names the entry.
//
// Synchronous exceptions run on their own stack (ADR 0026), never on the one
// they interrupted, so an overflow of the boot stack is reported and writes
// nothing below the guard page. The interrupted SP is saved in the frame.

// Layout of `TrapFrame` in exceptions.rs, checked there with const asserts.
.equ FRAME_SIZE, 304
.equ FRAME_SP_EL0, 248
.equ FRAME_ELR, 256
.equ FRAME_SPSR, 264
.equ FRAME_ESR, 272
.equ FRAME_FAR, 280
.equ FRAME_SP, 288
// Size of .exception_stack in link.ld.
.equ EXCEPTION_STACK_SIZE, 0x4000

.macro unhandled index
    mov  x0, #\index
    b    unhandled_exception_entry
    .balign 0x80
.endm

.section .text
.balign 0x800
.global exception_vectors
exception_vectors:
    // Current EL with SP_EL0: the kernel does not run on SP_EL0.
    unhandled 0
    unhandled 1
    unhandled 2
    unhandled 3
    // Current EL with SP_ELx.
    b    sync_exception_entry
    .balign 0x80
    unhandled 5
    unhandled 6
    unhandled 7
    // Lower EL, AArch64: no user space yet.
    unhandled 8
    unhandled 9
    unhandled 10
    unhandled 11
    // Lower EL, AArch32: never supported.
    unhandled 12
    unhandled 13
    unhandled 14
    unhandled 15

sync_exception_entry:
    // There is no stack to save a register on yet. x0 is parked in TPIDR_EL1,
    // which nothing else uses, and exchanged with SP by three instructions
    // that need no third register: afterwards SP is the top of the exception
    // stack and x0 the interrupted SP.
    msr  tpidr_el1, x0
    adrp x0, __exception_stack_top
    add  x0, x0, :lo12:__exception_stack_top
    add  sp, sp, x0
    sub  x0, sp, x0
    sub  sp, sp, x0
    // An exception taken on the exception stack itself (the handler or the
    // report faulted) must not reuse it: the interrupted SP is within one
    // stack size below the top, or in the guard page below that (the handler
    // overflowed it). SP exactly at the bottom of the guard page is the empty
    // boot stack, not an overflow.
    sub  x0, sp, x0
    cmp  x0, #(EXCEPTION_STACK_SIZE + 0x1000)
    b.lo nested_exception_entry
    sub  x0, sp, x0
    sub  sp, sp, #FRAME_SIZE
    str  x0, [sp, #FRAME_SP]
    mrs  x0, tpidr_el1
    stp  x0, x1, [sp, #0]
    stp  x2, x3, [sp, #16]
    stp  x4, x5, [sp, #32]
    stp  x6, x7, [sp, #48]
    stp  x8, x9, [sp, #64]
    stp  x10, x11, [sp, #80]
    stp  x12, x13, [sp, #96]
    stp  x14, x15, [sp, #112]
    stp  x16, x17, [sp, #128]
    stp  x18, x19, [sp, #144]
    stp  x20, x21, [sp, #160]
    stp  x22, x23, [sp, #176]
    stp  x24, x25, [sp, #192]
    stp  x26, x27, [sp, #208]
    stp  x28, x29, [sp, #224]
    str  x30, [sp, #240]
    mrs  x0, sp_el0
    mrs  x1, elr_el1
    stp  x0, x1, [sp, #FRAME_SP_EL0]
    mrs  x0, spsr_el1
    mrs  x1, esr_el1
    stp  x0, x1, [sp, #FRAME_SPSR]
    mrs  x0, far_el1
    str  x0, [sp, #FRAME_FAR]

    // Scrub everything the frame holds, so that the restore below is the only
    // source of the interrupted values and a missing restore cannot go unseen.
    mov  x0, sp
.irp n, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29
    mov  x\n, xzr
.endr
    msr  sp_el0, xzr
    msr  spsr_el1, xzr
    msr  elr_el1, xzr
    bl   handle_sync_exception

    // ELR and SPSR first: they need scratch registers that are reloaded after.
    ldp  x0, x1, [sp, #FRAME_SP_EL0]
    msr  sp_el0, x0
    msr  elr_el1, x1
    ldr  x0, [sp, #FRAME_SPSR]
    msr  spsr_el1, x0
    ldp  x2, x3, [sp, #16]
    ldp  x4, x5, [sp, #32]
    ldp  x6, x7, [sp, #48]
    ldp  x8, x9, [sp, #64]
    ldp  x10, x11, [sp, #80]
    ldp  x12, x13, [sp, #96]
    ldp  x14, x15, [sp, #112]
    ldp  x16, x17, [sp, #128]
    ldp  x18, x19, [sp, #144]
    ldp  x20, x21, [sp, #160]
    ldp  x22, x23, [sp, #176]
    ldp  x24, x25, [sp, #192]
    ldp  x26, x27, [sp, #208]
    ldp  x28, x29, [sp, #224]
    ldr  x30, [sp, #240]
    // x0 and x1 are the last to come back, x0 through TPIDR_EL1, once SP is
    // the interrupted one again.
    ldr  x0, [sp, #0]
    msr  tpidr_el1, x0
    ldr  x1, [sp, #8]
    ldr  x0, [sp, #FRAME_SP]
    mov  sp, x0
    mrs  x0, tpidr_el1
    eret

nested_exception_entry:
    mov  x0, #16
unhandled_exception_entry:
    // The interrupted stack may be what faulted, and this path never returns,
    // so start again from the top of the exception stack. x0 holds the entry
    // index.
    adrp x4, __exception_stack_top
    add  x4, x4, :lo12:__exception_stack_top
    mov  sp, x4

    mrs  x1, esr_el1
    mrs  x2, elr_el1
    mrs  x3, far_el1
    b    unhandled_exception

// Takes the resumable breakpoint with distinct values in x0 to x30 and in the flags, then
// checks that every one came back, with SP and SP_EL0. Returns 0 on success.
.global exception_selftest
exception_selftest:
    stp  x18, x19, [sp, #-112]!
    stp  x20, x21, [sp, #16]
    stp  x22, x23, [sp, #32]
    stp  x24, x25, [sp, #48]
    stp  x26, x27, [sp, #64]
    stp  x28, x29, [sp, #80]
    str  x30, [sp, #96]
    mrs  x0, sp_el0
    str  x0, [sp, #104]

    // N and C set, Z and V clear: no flag keeps its reset value by chance.
    movz x30, #0xa000, lsl #16
    msr  nzcv, x30
.irp n, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28
    ldr  x\n, =(0x0123456789abcdef + \n * 0x1111111)
.endr
    mov  x29, sp
    ldr  x30, =0x0badc0ffee0ddf00
    msr  sp_el0, x30
    add  x30, sp, #16
    brk  #0x800

    // Branches on the flags need no scratch register, so check them first.
    b.pl .Lselftest_failed
    b.cc .Lselftest_failed
    b.eq .Lselftest_failed
    b.vs .Lselftest_failed
    add  x29, x29, #16
    cmp  x30, x29
    b.ne .Lselftest_failed
    sub  x29, x29, #16
    mov  x30, sp
    cmp  x30, x29
    b.ne .Lselftest_failed
    mrs  x30, sp_el0
    ldr  x29, =0x0badc0ffee0ddf00
    cmp  x30, x29
    b.ne .Lselftest_failed
.irp n, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28
    ldr  x30, =(0x0123456789abcdef + \n * 0x1111111)
    cmp  x\n, x30
    b.ne .Lselftest_failed
.endr
    mov  x0, #0
    b    .Lselftest_done
.Lselftest_failed:
    mov  x0, #1
.Lselftest_done:
    mov  x1, x0
    ldr  x0, [sp, #104]
    msr  sp_el0, x0
    mov  x0, x1
    ldp  x20, x21, [sp, #16]
    ldp  x22, x23, [sp, #32]
    ldp  x24, x25, [sp, #48]
    ldp  x26, x27, [sp, #64]
    ldp  x28, x29, [sp, #80]
    ldr  x30, [sp, #96]
    ldp  x18, x19, [sp], #112
    ret
