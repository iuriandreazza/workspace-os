#![no_std]
#![no_main]

mod arch;
mod board;
mod drivers;
mod memory;
mod memory_image;

use core::fmt::Write;
use core::panic::PanicInfo;

/// `handoff_address` is the device tree address (or zero) after a direct load
/// and the boot info address after the bootloader; `entered_by_bootloader` is
/// 1 when the boot code saw the entry magic, 0 otherwise.
#[unsafe(no_mangle)]
pub extern "C" fn kernel_main(handoff_address: usize, entered_by_bootloader: usize) -> ! {
    memory_image::assert_initialised();

    let board = if entered_by_bootloader == 1 {
        match board::from_boot_info(handoff_address) {
            Ok(board) => board,
            Err((error, console_usable)) => {
                if console_usable {
                    let _ = writeln!(board::console(), "workspace-os kernel: {error}");
                }
                arch::halt()
            }
        }
    } else {
        match board::discover(handoff_address) {
            Ok(board) => board,
            // Without the device tree there is no console to report on.
            Err(_) => arch::halt(),
        }
    };

    let mut console = board::console();
    let _ = writeln!(
        console,
        "workspace-os kernel: booted on aarch64 in EL{}",
        arch::current_el()
    );

    match board.entry {
        board::Entry::Direct => {
            let _ = writeln!(console, "workspace-os kernel: entry: direct load");
        }
        board::Entry::Bootloader {
            version_major,
            version_minor,
            boot_mode,
            map_entries,
        } => {
            let _ = writeln!(
                console,
                "workspace-os kernel: entry: bootloader, boot info {version_major}.{version_minor}, mode {boot_mode}, {map_entries} memory map entries"
            );
        }
    }
    if let Some(entries) = board.map_entries() {
        for entry in entries {
            let _ = writeln!(
                console,
                "workspace-os kernel: memory map: {:#x}..{:#x} {}",
                entry.base,
                entry.end(),
                entry.kind().name()
            );
        }
        // The bootloader promises Normal cacheable memory for the kernel and
        // Device memory for the UART: ask the translation it installed.
        let attribute = |address| {
            arch::memory_attributes(address)
                .map(|a| a as i32)
                .unwrap_or(-1)
        };
        let _ = writeln!(
            console,
            "workspace-os kernel: mapping: kernel image {:#04x}, uart {:#04x}",
            attribute(arch::kernel_image().start),
            attribute(board.uart.base as usize)
        );
    }
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
