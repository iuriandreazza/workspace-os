#![no_std]
#![no_main]

mod arch;
mod board;
mod drivers;

use core::fmt::Write;
use core::panic::PanicInfo;

#[unsafe(no_mangle)]
pub extern "C" fn kernel_main() -> ! {
    let mut console = board::console();
    let _ = writeln!(console, "workspace-os kernel: booted on aarch64");
    arch::halt()
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    let mut console = board::console();
    let _ = writeln!(console, "kernel panic: {info}");
    arch::halt()
}
