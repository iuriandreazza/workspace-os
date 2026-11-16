use core::arch::{asm, global_asm};
use core::ops::Range;

global_asm!(include_str!("boot.s"));
global_asm!(include_str!("exceptions.s"));

mod exceptions;

// Addresses the linker script defines; only their addresses mean anything.
unsafe extern "C" {
    safe static __image_start: u8;
    safe static __stack_bottom: u8;
    safe static __stack_top: u8;
}

macro_rules! address_of {
    ($symbol:ident) => {
        core::ptr::addr_of!($symbol) as usize
    };
}

/// Physical addresses of the loaded kernel: code, data and `.bss`.
pub fn kernel_image() -> Range<usize> {
    address_of!(__image_start)..address_of!(__stack_bottom)
}

/// Physical addresses of the boot stack, which sits right after the image.
pub fn boot_stack() -> Range<usize> {
    address_of!(__stack_bottom)..address_of!(__stack_top)
}

#[cfg(feature = "selftest-compiler-trap")]
pub use exceptions::raise_compiler_trap;
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
