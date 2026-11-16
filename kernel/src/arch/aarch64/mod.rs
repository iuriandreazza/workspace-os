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
