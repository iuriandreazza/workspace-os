use core::arch::{asm, global_asm};

global_asm!(include_str!("boot.s"));
global_asm!(include_str!("exceptions.s"));

mod exceptions;

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
