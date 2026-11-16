#![no_std]
#![no_main]

mod arch;
mod board;
mod drivers;
mod memory_image;

use core::fmt::Write;
use core::panic::PanicInfo;

#[unsafe(no_mangle)]
pub extern "C" fn kernel_main() -> ! {
    memory_image::assert_initialised();

    let mut console = board::console();
    let _ = writeln!(
        console,
        "workspace-os kernel: booted on aarch64 in EL{}",
        arch::current_el()
    );

    arch::run_selftest();
    let _ = writeln!(console, "workspace-os kernel: resumed after breakpoint");

    #[cfg(feature = "selftest-unhandled-class")]
    arch::raise_unhandled_class();
    #[cfg(feature = "selftest-compiler-trap")]
    arch::raise_compiler_trap();
    #[cfg(feature = "selftest-unhandled-vector")]
    arch::raise_unhandled_vector();

    arch::halt()
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    let mut console = board::console();
    let _ = writeln!(console, "kernel panic: {info}");
    arch::halt()
}
