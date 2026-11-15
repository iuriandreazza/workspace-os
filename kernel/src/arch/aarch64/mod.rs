use core::arch::{asm, global_asm};

global_asm!(include_str!("boot.s"));

pub fn halt() -> ! {
    loop {
        // SAFETY: `wfe` only parks the core until the next event.
        unsafe { asm!("wfe", options(nomem, nostack, preserves_flags)) };
    }
}
