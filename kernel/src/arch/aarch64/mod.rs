use core::arch::{asm, global_asm};
use core::ops::Range;
use core::sync::atomic::{AtomicU64, Ordering};

// The constants are the ones `kernelvm` builds descriptors and register values
// from, so that the boot code and the Rust that replaces its tables agree.
global_asm!(
    include_str!("boot.s"),
    DIRECT_ENTRY_MAIR = const kernelvm::DIRECT_ENTRY_MAIR,
    NORMAL_PAGE_BITS = const kernelvm::NORMAL_PAGE_BITS,
    NORMAL_BLOCK_BITS = const kernelvm::NORMAL_BLOCK_BITS,
    DEVICE_BLOCK_BITS = const kernelvm::DEVICE_BLOCK_BITS,
    TABLE_BITS = const kernelvm::TABLE_BITS,
    READ_ONLY = const kernelvm::READ_ONLY,
    PXN = const kernelvm::PRIVILEGED_EXECUTE_NEVER,
    TCR_BOTH_HALVES = const kernelvm::TCR_BOTH_HALVES,
    TCR_UPPER_MASK = const kernelvm::TCR_UPPER_HALF_MASK,
    TCR_UPPER_HALF = const kernelvm::TCR_UPPER_HALF,
    WXN = const kernelvm::SCTLR_WXN,
    MONITOR_CALL_GICV2_FOR_NON_SECURE = const MONITOR_CALL_GICV2_FOR_NON_SECURE,
    MONITOR_CALL_GICV3_FOR_NON_SECURE = const MONITOR_CALL_GICV3_FOR_NON_SECURE,
    SELFTEST_DIRTY_TCR2 = const cfg!(feature = "selftest-dirty-tcr2") as u64,
);
global_asm!(include_str!("exceptions.s"));

mod exceptions;
pub mod gic_cpu;
pub mod mmu;
pub mod timer;

// Addresses the linker script defines; only their addresses mean anything.
unsafe extern "C" {
    safe static __image_start: u8;
    safe static __stack_bottom: u8;
    safe static __stack_top: u8;
    safe static __exception_stack_top: u8;
    safe static __stack_probe_start: u8;
    safe static __stack_probe_end: u8;
}

macro_rules! address_of {
    ($symbol:ident) => {
        core::ptr::addr_of!($symbol) as usize
    };
}

/// The physical address of a symbol: the linker gives every symbol its
/// virtual address, the physical one is a fixed offset below.
fn physical(address: usize) -> usize {
    kernelvm::physical_of(address as u64) as usize
}

/// What the kernel adds to a physical address to read it (ADR 0033): zero while
/// the entry's identity map is the way in (the bootloader's, or the boot code's
/// on the direct path), and the start of the linear map once that is live.
static POINTER_OFFSET: AtomicU64 = AtomicU64::new(0);

/// The address at which the kernel reads physical `address`. Every physical
/// address the kernel dereferences goes through here, so that dropping the
/// identity map is one change and not a hunt.
pub fn pointer_to(address: u64) -> usize {
    address.wrapping_add(POINTER_OFFSET.load(Ordering::Relaxed)) as usize
}

/// From now on physical addresses are read through the linear map.
fn read_physical_through_the_linear_map() {
    POINTER_OFFSET.store(kernelvm::linear_of(0), Ordering::Relaxed);
}

/// Physical addresses of the loaded kernel: code, data and `.bss`.
pub fn kernel_image() -> Range<usize> {
    physical(address_of!(__image_start))..physical(address_of!(__stack_bottom))
}

/// Physical addresses of the boot stack, which sits right after the image.
pub fn boot_stack() -> Range<usize> {
    physical(address_of!(__stack_bottom))..physical(address_of!(__stack_top))
}

/// Physical addresses of the guard page above the boot stack and the exception
/// stack above it (ADR 0026), both reserved with the image.
pub fn exception_stack() -> Range<usize> {
    physical(address_of!(__stack_top))..physical(address_of!(__exception_stack_top))
}

/// The last 8 KiB of `.bss`, which the linker reserves: where a stack that ran
/// past its guard page would write first.
#[cfg(feature = "selftest-stack-overflow")]
fn stack_probe() -> *mut u64 {
    address_of!(__stack_probe_start) as *mut u64
}

#[cfg(feature = "selftest-stack-overflow")]
fn stack_probe_words() -> usize {
    (address_of!(__stack_probe_end) - address_of!(__stack_probe_start)) / size_of::<u64>()
}

#[cfg(feature = "selftest-stack-overflow")]
const STACK_PROBE_PATTERN: u64 = 0x57AC_0000_0000_0000;

#[cfg(feature = "selftest-stack-overflow")]
fn fill_stack_probe() {
    for word in 0..stack_probe_words() {
        // SAFETY: the probe is reserved `.bss` that nothing else uses, mapped
        // read and write, and `word` is within it.
        unsafe {
            stack_probe()
                .add(word)
                .write_volatile(STACK_PROBE_PATTERN | word as u64)
        };
    }
}

/// Whether every word of the probe still holds what `fill_stack_probe` wrote.
#[cfg(feature = "selftest-stack-overflow")]
pub fn stack_probe_intact() -> bool {
    (0..stack_probe_words()).all(|word| {
        // SAFETY: as in `fill_stack_probe`.
        unsafe { stack_probe().add(word).read_volatile() == STACK_PROBE_PATTERN | word as u64 }
    })
}

#[cfg(feature = "selftest-stack-overflow")]
pub use exceptions::overflow_stack;
#[cfg(feature = "selftest-compiler-trap")]
pub use exceptions::raise_compiler_trap;
#[cfg(feature = "selftest-nested-exception")]
pub use exceptions::raise_nested_exception;
#[cfg(feature = "selftest-unhandled-class")]
pub use exceptions::raise_unhandled_class;
#[cfg(feature = "selftest-unhandled-vector")]
pub use exceptions::raise_unhandled_vector;
pub use exceptions::{
    Handover, SVC_EXIT, SVC_SAMPLE_PSTATE, SVC_YIELD, Side, TrapFrame, current_el,
    exit_current_thread, run_interrupt_selftest, run_selftest, switch_selftest, yield_now,
};

/// The calls the boot code's monitor answers when the kernel was started at
/// EL3 (ADR 0035): give the non-secure world the GIC. They are the Arm SMC
/// Calling Convention's range for vendor-specific EL3 monitors.
const MONITOR_CALL_GICV2_FOR_NON_SECURE: u32 = 0x8700_0000;
const MONITOR_CALL_GICV3_FOR_NON_SECURE: u32 = 0x8700_0001;

/// What the monitor answered if it was not success.
type MonitorAnswer = Result<(), i64>;

fn call_monitor(function: u32, first: u64, second: u64) -> MonitorAnswer {
    let answer: i64;
    // SAFETY: the callers only ask where the boot code installed the monitor.
    // It writes the registers the call names and returns, clobbering at most
    // what the SMC Calling Convention lets a callee clobber.
    unsafe {
        asm!(
            "smc #0",
            inlateout("x0") u64::from(function) => answer,
            in("x1") first,
            in("x2") second,
            clobber_abi("C"),
        )
    };
    if answer == 0 { Ok(()) } else { Err(answer) }
}

/// Asks the monitor, which only exists where the kernel was started at EL3, to
/// make the boot core's SGIs and PPIs Group 1 and open the priority mask in
/// the GICv2 whose distributor and CPU interface are at the given physical
/// addresses.
pub fn give_gicv2_to_non_secure(distributor: u64, cpu_interface: u64) -> MonitorAnswer {
    call_monitor(
        MONITOR_CALL_GICV2_FOR_NON_SECURE,
        distributor,
        cpu_interface,
    )
}

/// Asks the monitor to put the GICv3 whose distributor is at the given
/// physical address in a single security state, in which the non-secure
/// kernel can configure it.
pub fn give_gicv3_to_non_secure(distributor: u64) -> MonitorAnswer {
    call_monitor(MONITOR_CALL_GICV3_FOR_NON_SECURE, distributor, 0)
}

/// Unmasks IRQs on this core: from here an interrupt is taken whenever the
/// controller raises one.
pub fn enable_interrupts() {
    // SAFETY: the vector table and the exception stack exist from the first
    // instruction of the kernel's Rust, and the IRQ entry saves and restores
    // everything it touches. It is a compiler barrier on purpose.
    unsafe { asm!("msr daifclr, #2", options(nostack, preserves_flags)) };
}

/// `DAIF` as a value of the four bits D, A, I, F in bits 9:6.
pub fn daif() -> u64 {
    let daif: u64;
    // SAFETY: reading `DAIF` has no side effects.
    unsafe { asm!("mrs {}, daif", out(reg) daif, options(nomem, nostack, preserves_flags)) };
    daif
}

/// Whether IRQs are masked on this core.
pub fn irqs_masked() -> bool {
    daif() & DAIF_IRQ_MASK != 0
}

pub fn disable_interrupts() {
    // SAFETY: masking IRQs only delays them. It is a compiler barrier on purpose.
    unsafe { asm!("msr daifset, #2", options(nostack, preserves_flags)) };
}

/// Masks IRQs for as long as it lives and puts the mask back as it found it
/// (ADR 0037): it nests, and never unmasks a caller that had IRQs masked. On one
/// core this is the whole of mutual exclusion against a handler.
pub struct IrqGuard {
    irqs_were_masked: bool,
}

const DAIF_IRQ_MASK: u64 = 1 << 7;

impl IrqGuard {
    pub fn new() -> Self {
        let daif: u64;
        // SAFETY: reading `DAIF` has no side effects; masking IRQs only delays
        // them. The asm is not `nomem` on purpose: it is a compiler barrier.
        unsafe {
            asm!("mrs {}, daif", "msr daifset, #2", out(reg) daif, options(nostack, preserves_flags))
        };
        Self {
            irqs_were_masked: daif & DAIF_IRQ_MASK != 0,
        }
    }
}

impl Drop for IrqGuard {
    fn drop(&mut self) {
        if !self.irqs_were_masked {
            enable_interrupts();
        }
    }
}

pub fn halt() -> ! {
    disable_interrupts();
    loop {
        // SAFETY: `wfe` only parks the core until the next event.
        unsafe { asm!("wfe", options(nomem, nostack, preserves_flags)) };
    }
}

/// `TTBR0_EL1` without the ASID: the root of the table the kernel runs on.
pub fn translation_root() -> u64 {
    let ttbr: u64;
    // SAFETY: reading `TTBR0_EL1` has no side effects.
    unsafe { asm!("mrs {}, ttbr0_el1", out(reg) ttbr, options(nomem, nostack, preserves_flags)) };
    ttbr & 0x0000_ffff_ffff_f000
}

/// `SCTLR_EL1`: the MMU, cache and alignment settings the kernel runs with.
pub fn system_control() -> u64 {
    let sctlr: u64;
    // SAFETY: reading `SCTLR_EL1` has no side effects.
    unsafe { asm!("mrs {}, sctlr_el1", out(reg) sctlr, options(nomem, nostack, preserves_flags)) };
    sctlr
}

/// The memory attribute byte (as in `MAIR_EL1`) the current translation gives
/// `address` for a privileged read, or `None` if the translation faults. With
/// the MMU off it describes the default type of every access.
pub fn memory_attributes(address: usize) -> Option<u8> {
    let par: u64;
    // SAFETY: `at` only asks the hardware to translate and puts the answer in
    // `PAR_EL1`; it reads no memory at the address and cannot fault.
    unsafe {
        asm!(
            "at s1e1r, {address}",
            "isb",
            "mrs {par}, par_el1",
            address = in(reg) address,
            par = out(reg) par,
            options(nostack, preserves_flags)
        );
    }
    // Bit 0 is F: the translation aborted.
    (par & 1 == 0).then_some((par >> 56) as u8)
}
