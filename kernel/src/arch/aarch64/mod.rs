use core::arch::{asm, global_asm};

global_asm!(include_str!("boot.s"));
global_asm!(include_str!("exceptions.s"));

pub fn halt() -> ! {
    loop {
        // SAFETY: `wfe` only parks the core until the next event.
        unsafe { asm!("wfe", options(nomem, nostack, preserves_flags)) };
    }
}

/// Entered from the exception vector table, which passes ESR_EL1 and ELR_EL1.
#[unsafe(no_mangle)]
extern "C" fn unhandled_exception(syndrome: u64, return_address: u64) -> ! {
    panic!("unhandled exception: syndrome {syndrome:#x}, return address {return_address:#x}");
}
