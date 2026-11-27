//! Exception handling for EL1: the context saved by `exceptions.s` and the
//! decoding of the syndrome.

use core::arch::asm;
use core::mem::{offset_of, size_of};

use crate::board;

/// Processor context saved on entry to a handled exception. The layout is
/// shared with `exceptions.s`. It is also a thread's saved state: every switch
/// returns from an exception into another thread's frame (ADR 0037).
#[derive(Clone, Copy)]
#[repr(C)]
pub struct TrapFrame {
    x: [u64; 31],
    sp_el0: u64,
    elr: u64,
    spsr: u64,
    esr: u64,
    far: u64,
    /// The stack pointer of the interrupted code: the handler runs on the
    /// exception stack (ADR 0026).
    sp: u64,
    _reserved: u64,
}

/// `SPSR_EL1` of a thread that starts: EL1 with `SP_ELx`, every DAIF bit and
/// the flags clear.
const SPSR_EL1H: u64 = 0b0101;
/// The bits of `SPSR_EL1` a new thread takes from a running one: all but the
/// flags (31:28), DAIF (9:6) and the mode (4:0).
const SPSR_INHERITED: u64 = !((0xf << 28) | (0xf << 6) | 0x1f);

impl TrapFrame {
    pub const ZERO: Self = Self {
        x: [0; 31],
        sp_el0: 0,
        elr: 0,
        spsr: 0,
        esr: 0,
        far: 0,
        sp: 0,
        _reserved: 0,
    };

    /// The frame of a thread that has not run yet: it starts at `entry` with
    /// `argument` in `x0`, on `stack_top`, with IRQs unmasked, and returning
    /// from `entry` goes to `returns_to`. `running_pstate` is the `SPSR` of
    /// kernel code that is running (`SVC_SAMPLE_PSTATE`): whatever the cores
    /// of `-cpu max` put in PSTATE besides the flags, DAIF and mode is kept.
    pub fn new_thread(
        entry: usize,
        argument: usize,
        stack_top: u64,
        running_pstate: u64,
        returns_to: usize,
    ) -> Self {
        assert!(
            stack_top.is_multiple_of(16),
            "a thread's stack pointer must be 16-byte aligned"
        );
        let mut frame = Self::ZERO;
        frame.x[0] = argument as u64;
        frame.x[30] = returns_to as u64;
        frame.elr = entry as u64;
        frame.sp = stack_top;
        frame.spsr = (running_pstate & SPSR_INHERITED) | SPSR_EL1H;
        frame
    }

    /// The saved `SPSR_EL1`.
    pub fn spsr(&self) -> u64 {
        self.spsr
    }
}

const _: () = {
    assert!(size_of::<TrapFrame>() == 304);
    assert!(offset_of!(TrapFrame, sp) == 288);
    assert!(offset_of!(TrapFrame, x) == 0);
    assert!(offset_of!(TrapFrame, sp_el0) == 248);
    assert!(offset_of!(TrapFrame, elr) == 256);
    assert!(offset_of!(TrapFrame, spsr) == 264);
    assert!(offset_of!(TrapFrame, esr) == 272);
    assert!(offset_of!(TrapFrame, far) == 280);
};

const EC_UNKNOWN: u64 = 0x00;
const EC_SVC_A64: u64 = 0x15;
const EC_INSTRUCTION_ABORT_LOWER: u64 = 0x20;
const EC_INSTRUCTION_ABORT: u64 = 0x21;
const EC_PC_ALIGNMENT: u64 = 0x22;
const EC_DATA_ABORT_LOWER: u64 = 0x24;
const EC_DATA_ABORT: u64 = 0x25;
const EC_SP_ALIGNMENT: u64 = 0x26;
const EC_BRK_A64: u64 = 0x3c;

/// The only BRK immediate the kernel resumes from. The compiler lowers
/// `abort` and `unreachable` to `brk #0x1`, and those must stay fatal.
const RESUMABLE_BREAKPOINT: u64 = 0x800;
const EC_WATCHPOINT_LOWER: u64 = 0x34;
const EC_WATCHPOINT: u64 = 0x35;

const INSTRUCTION_BYTES: u64 = 4;

/// The `svc` immediates the kernel gives its threads (ADR 0037). `svc #0` stays
/// unhandled. An SVC from EL0 arrives on another vector.
pub const SVC_YIELD: u64 = 0x100;
pub const SVC_EXIT: u64 = 0x101;
/// Hands the caller's saved `SPSR` to the scheduler, once, at start.
pub const SVC_SAMPLE_PSTATE: u64 = 0x102;

/// The breakpoint whose handler takes another exception, on the exception stack.
#[cfg(feature = "selftest-nested-exception")]
const NESTING_BREAKPOINT: u64 = 0x801;

/// ESR_EL1 split into its fields.
struct Syndrome {
    class: u64,
    iss: u64,
}

impl Syndrome {
    fn new(esr: u64) -> Self {
        Self {
            class: (esr >> 26) & 0x3f,
            iss: esr & 0x1ff_ffff,
        }
    }

    fn class_name(&self) -> &'static str {
        match self.class {
            EC_UNKNOWN => "unknown reason",
            0x01 => "trapped WFI or WFE",
            0x0e => "illegal execution state",
            EC_SVC_A64 => "SVC (AArch64)",
            0x16 => "HVC (AArch64)",
            0x17 => "SMC (AArch64)",
            0x18 => "trapped system register access",
            0x07 => "trapped floating point or SIMD access",
            EC_INSTRUCTION_ABORT_LOWER => "instruction abort from a lower level",
            EC_INSTRUCTION_ABORT => "instruction abort",
            EC_PC_ALIGNMENT => "PC alignment fault",
            EC_DATA_ABORT_LOWER => "data abort from a lower level",
            EC_DATA_ABORT => "data abort",
            EC_SP_ALIGNMENT => "SP alignment fault",
            0x2f => "SError",
            0x30 | 0x31 => "breakpoint (hardware)",
            0x32 | 0x33 => "software step",
            EC_WATCHPOINT_LOWER | EC_WATCHPOINT => "watchpoint",
            EC_BRK_A64 => "BRK (AArch64)",
            _ => "unrecognised exception class",
        }
    }

    /// FAR_EL1 is only defined for these classes, and for aborts only when the
    /// FnV bit (ISS bit 10) says the address is valid.
    fn far_is_valid(&self) -> bool {
        let fault_address_not_valid = self.iss & (1 << 10) != 0;
        match self.class {
            EC_PC_ALIGNMENT => true,
            EC_INSTRUCTION_ABORT_LOWER
            | EC_INSTRUCTION_ABORT
            | EC_DATA_ABORT_LOWER
            | EC_DATA_ABORT
            | EC_WATCHPOINT_LOWER
            | EC_WATCHPOINT => !fault_address_not_valid,
            _ => false,
        }
    }
}

struct Report<'a> {
    esr: u64,
    elr: u64,
    far: u64,
    syndrome: &'a Syndrome,
}

impl core::fmt::Display for Report<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "{} (class {:#04x}, iss {:#09x}, esr {:#x}) at {:#x}",
            self.syndrome.class_name(),
            self.syndrome.class,
            self.syndrome.iss,
            self.esr,
            self.elr
        )?;
        if self.syndrome.far_is_valid() {
            write!(f, ", fault address {:#x}", self.far)?;
        }
        Ok(())
    }
}

/// Entered from `sync_exception_entry` with the context of the interrupted code.
/// Returning resumes it at `frame.elr`.
#[unsafe(no_mangle)]
extern "C" fn handle_sync_exception(frame: &mut TrapFrame) {
    let syndrome = Syndrome::new(frame.esr);
    if syndrome.class == EC_BRK_A64 && syndrome.iss & 0xffff == RESUMABLE_BREAKPOINT {
        let mut console = board::console();
        crate::log::klog!(
            Info,
            console,
            "exception: breakpoint #{:#x} at {:#x}",
            syndrome.iss & 0xffff,
            frame.elr
        );
        // ELR points at the BRK itself: resuming there would trap again.
        frame.elr += INSTRUCTION_BYTES;
        return;
    }
    if syndrome.class == EC_SVC_A64 && crate::threads::handle_svc(syndrome.iss & 0xffff, frame) {
        // ELR already points past the SVC.
        return;
    }
    #[cfg(feature = "selftest-nested-exception")]
    if syndrome.class == EC_BRK_A64 && syndrome.iss & 0xffff == NESTING_BREAKPOINT {
        // SAFETY: the exception ends in a kernel panic, which is what the test checks.
        unsafe { asm!("brk #0x802") };
    }
    let report = Report {
        esr: frame.esr,
        elr: frame.elr,
        far: frame.far,
        syndrome: &syndrome,
    };
    panic!("unhandled synchronous exception: {report}");
}

fn vector_description(index: u64) -> &'static str {
    match index {
        0 => "synchronous, current EL with SP_EL0",
        1 => "IRQ, current EL with SP_EL0",
        2 => "FIQ, current EL with SP_EL0",
        3 => "SError, current EL with SP_EL0",
        5 => "IRQ, current EL with SP_ELx",
        6 => "FIQ, current EL with SP_ELx",
        7 => "SError, current EL with SP_ELx",
        8 => "synchronous, lower EL (AArch64)",
        9 => "IRQ, lower EL (AArch64)",
        10 => "FIQ, lower EL (AArch64)",
        11 => "SError, lower EL (AArch64)",
        12..=15 => "lower EL (AArch32)",
        16 => "nested exception on the exception stack",
        _ => "unknown vector",
    }
}

/// Entered from `unhandled_exception_entry`, which passes the vector index and
/// ESR_EL1, ELR_EL1 and FAR_EL1 on a freshly reset stack.
#[unsafe(no_mangle)]
extern "C" fn unhandled_exception(vector: u64, esr: u64, elr: u64, far: u64) -> ! {
    // IRQ and FIQ do not update ESR_EL1 and FAR_EL1: decoding them would report
    // an older, unrelated exception.
    let is_synchronous_or_serror = matches!(vector % 4, 0 | 3);
    if !is_synchronous_or_serror {
        panic!(
            "unhandled exception: {} at {elr:#x}",
            vector_description(vector)
        );
    }
    let syndrome = Syndrome::new(esr);
    let report = Report {
        esr,
        elr,
        far,
        syndrome: &syndrome,
    };
    panic!(
        "unhandled exception: {}: {report}",
        vector_description(vector)
    );
}

/// Entered from `irq_exception_entry` with the context of the interrupted code
/// saved on the exception stack. Returning resumes it where it was.
#[unsafe(no_mangle)]
extern "C" fn handle_irq_exception(frame: &mut TrapFrame) {
    // The tick is fully ended before the scheduler looks at it (ADR 0037).
    if crate::interrupts::handle_irq() {
        crate::threads::on_tick(frame);
    }
}

/// The running thread gives up the rest of its slice. Everything it had live
/// comes back when it is resumed.
pub fn yield_now() {
    // SAFETY: the synchronous handler switches to another thread and returns
    // here with every register restored. Memory may change meanwhile, so the
    // asm is not `nomem`.
    unsafe { asm!("svc #{imm}", imm = const SVC_YIELD, options(nostack)) };
}

/// Where a thread's entry returns to: ends the thread.
pub extern "C" fn exit_current_thread() -> ! {
    loop {
        // SAFETY: the handler frees the thread and never returns to it.
        unsafe { asm!("svc #{imm}", imm = const SVC_EXIT, options(nostack)) };
    }
}

unsafe extern "C" {
    fn exception_selftest() -> u64;
    fn interrupt_selftest() -> u64;
    fn yield_selftest() -> u64;
    fn yield_selftest_other() -> u64;
    fn tick_switch_selftest() -> u64;
    fn tick_switch_selftest_other() -> u64;
}

/// How the context test hands the processor to another thread.
#[derive(Clone, Copy)]
pub enum Handover {
    /// The thread yields.
    Yield,
    /// The thread waits, IRQs unmasked, for a tick that ends its slice.
    Tick,
}

/// Which of the two pattern sets of the context test to run: the boot thread
/// and the thread it switches to use different values.
#[derive(Clone, Copy)]
pub enum Side {
    Own,
    Other,
}

/// Runs the context test of `exceptions.s` across a switch to another thread:
/// registers, flags, `SP`, `SP_EL0` and the stack below `SP` loaded with the
/// values of `side`, the handover, then a check that they are all back. True if
/// anything was not. A tick handover returns after a `wfi` that may have woken
/// without a switch; the caller counts switches.
pub fn switch_selftest(handover: Handover, side: Side) -> bool {
    // SAFETY: each of these saves and restores every register it changes, and
    // the handover is an SVC or an interrupt that the kernel handles.
    let failed = unsafe {
        match (handover, side) {
            (Handover::Yield, Side::Own) => yield_selftest(),
            (Handover::Yield, Side::Other) => yield_selftest_other(),
            (Handover::Tick, Side::Own) => tick_switch_selftest(),
            (Handover::Tick, Side::Other) => tick_switch_selftest_other(),
        }
    };
    failed != 0
}

/// Takes a breakpoint and checks that the interrupted code resumes with its
/// registers, flags and stack pointer intact.
pub fn run_selftest() {
    // SAFETY: `exception_selftest` saves and restores every register it
    // changes, and the vector table installed by the boot code handles BRK.
    let failed = unsafe { exception_selftest() };
    assert!(failed == 0, "context changed across a handled exception");
}

/// Takes a timer interrupt with distinct values in every register, in the
/// flags and just below the stack pointer, and checks that all of them came
/// back. The caller has seen ticks arrive, so the wait ends; the pattern
/// below SP is what the interrupt's own stack must leave alone (ADR 0026).
pub fn run_interrupt_selftest() {
    // SAFETY: `interrupt_selftest` saves and restores every register it
    // changes, and masks IRQs again before it returns.
    let failed = unsafe { interrupt_selftest() };
    assert!(failed == 0, "context changed across an interrupt");
}

pub fn current_el() -> u64 {
    let current_el: u64;
    // SAFETY: reading CurrentEL has no side effect.
    unsafe {
        asm!("mrs {}, CurrentEL", out(reg) current_el, options(nomem, nostack, preserves_flags));
    }
    current_el >> 2
}

/// Overflows the boot stack by recursing. The first store below the stack hits
/// the guard page; the report must be made on the exception stack and leave the
/// `.bss` probe whole (ADR 0026).
#[cfg(feature = "selftest-stack-overflow")]
pub fn overflow_stack() {
    super::fill_stack_probe();
    let mut console = board::console();
    crate::log::klog!(Info, console, "selftest: overflowing the boot stack");
    recurse(0);
    panic!("selftest: the boot stack did not overflow");
}

/// The recursion cannot be folded into a loop: the frame is written through
/// `black_box` and read after the call.
#[cfg(feature = "selftest-stack-overflow")]
#[inline(never)]
fn recurse(depth: usize) -> usize {
    let mut frame = [0u8; 256];
    core::hint::black_box(&mut frame)[0] = depth as u8;
    let below = if core::hint::black_box(true) {
        recurse(depth + 1)
    } else {
        0
    };
    below + core::hint::black_box(frame[0]) as usize
}

/// The handler of this breakpoint takes a second exception while it runs on the
/// exception stack, which must be reported as nested.
#[cfg(feature = "selftest-nested-exception")]
pub fn raise_nested_exception() {
    // SAFETY: the exception ends in a kernel panic, which is what the test checks.
    unsafe { asm!("brk #0x801") };
}

/// SVC from the kernel is a class no handler exists for.
#[cfg(feature = "selftest-unhandled-class")]
pub fn raise_unhandled_class() {
    // SAFETY: the exception ends in a kernel panic, which is what the test checks.
    unsafe { asm!("svc #0") };
}

/// The kernel never runs on SP_EL0, so no vector entry handles a BRK taken there.
/// SP_EL1 is invalid when it happens, as after a stack overflow.
#[cfg(feature = "selftest-unhandled-vector")]
pub fn raise_unhandled_vector() {
    // SAFETY: the exception ends in a kernel panic, which is what the test checks.
    // SP_EL1 is made invalid first: the panic path must not rely on it.
    unsafe { asm!("mov x0, #0x10", "mov sp, x0", "msr spsel, #0", "brk #0", out("x0") _) };
}

/// `brk #0x1` is what the compiler emits for `abort` and `unreachable`: it must
/// stay fatal.
#[cfg(feature = "selftest-compiler-trap")]
pub fn raise_compiler_trap() {
    // SAFETY: the exception ends in a kernel panic, which is what the test checks.
    unsafe { asm!("brk #0x1") };
}
