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
    SELFTEST_DIRTY_TCR2 = const cfg!(feature = "selftest-dirty-tcr2") as u64,
);
global_asm!(include_str!("exceptions.s"));

mod exceptions;
pub mod mmu;

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
pub use exceptions::{current_el, run_selftest};

pub fn halt() -> ! {
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
