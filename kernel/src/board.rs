//! QEMU `virt` machine, the only board supported so far.

use crate::drivers::pl011::Pl011;

const UART0_BASE: usize = 0x0900_0000;

pub fn console() -> Pl011 {
    // SAFETY: QEMU `virt` maps a PL011 at `UART0_BASE`. The kernel is its only
    // user: the boot code parks every core but the boot core and masks
    // interrupts, so nothing else can drive the UART concurrently.
    unsafe { Pl011::new(UART0_BASE) }
}
