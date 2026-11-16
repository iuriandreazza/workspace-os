#![no_std]
#![no_main]

mod arch;
mod board;
mod drivers;
mod memory;
mod memory_image;

use core::fmt::Write;
use core::panic::PanicInfo;

#[unsafe(no_mangle)]
pub extern "C" fn kernel_main(device_tree_address: usize) -> ! {
    memory_image::assert_initialised();

    let board = match board::discover(device_tree_address) {
        Ok(board) => board,
        // Without the device tree there is no console to report on.
        Err(_) => arch::halt(),
    };

    let mut console = board::console();
    let _ = writeln!(
        console,
        "workspace-os kernel: booted on aarch64 in EL{}",
        arch::current_el()
    );

    let _ = writeln!(
        console,
        "workspace-os kernel: memory {} MiB at {:#x}",
        board.memory.size >> 20,
        board.memory.base
    );
    let _ = writeln!(
        console,
        "workspace-os kernel: uart pl011 at {:#x}",
        board.uart.base
    );

    let mut frames = match memory::init(&board) {
        Ok(frames) => frames,
        Err(error) => panic!("physical memory: {error}"),
    };
    memory::report(&frames, &board, &mut console);
    memory::selftest(&mut frames, &board, &mut console);

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
