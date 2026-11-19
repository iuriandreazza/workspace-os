#![no_std]
#![no_main]

mod arch;
mod board;
mod drivers;
mod log;
mod memory;
mod memory_image;

use core::panic::PanicInfo;

use log::klog;

/// `handoff_address` is the device tree address (or zero) after a direct load
/// and the boot info address after the bootloader; `entered_by_bootloader` is
/// 1 when the boot code saw the entry magic, 0 otherwise. `extra_entry_registers`
/// is `x2 | x3` as they were at entry, which the bootloader sets to zero.
#[unsafe(no_mangle)]
pub extern "C" fn kernel_main(
    handoff_address: usize,
    entered_by_bootloader: usize,
    extra_entry_registers: usize,
) -> ! {
    memory_image::assert_initialised();

    let board = if entered_by_bootloader == 1 {
        match board::from_boot_info(handoff_address) {
            Ok(board) => board,
            Err((error, console_usable)) => {
                if console_usable {
                    klog!(Error, board::console(), "{error}");
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
    log::banner(&mut console, arch::current_el());
    klog!(Ok, console, "booted on aarch64 in EL{}", arch::current_el());

    match board.entry {
        board::Entry::Direct => {
            klog!(Ok, console, "entry: direct load");
        }
        board::Entry::Bootloader {
            version_major,
            version_minor,
            boot_mode,
            map_entries,
            boot_cpu,
            rsdp,
            efi_system_table,
        } => {
            klog!(
                Ok,
                console,
                "entry: bootloader, boot info {version_major}.{version_minor}, mode {boot_mode} ({}), {map_entries} memory map entries",
                bootinfo::boot_mode_name(boot_mode)
            );
            klog!(
                Info,
                console,
                "handoff: boot cpu {boot_cpu:#x}, rsdp {rsdp:#x}, efi system table {efi_system_table:#x}, x2|x3 {extra_entry_registers}"
            );
        }
    }
    if let Some(entries) = board.map_entries() {
        for entry in entries {
            klog!(
                Info,
                console,
                "memory map: {:#x}..{:#x} {}",
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
        // Firmware regions the kernel must keep (runtime services) are probed too.
        let runtime = board
            .map_entries()
            .into_iter()
            .flatten()
            .find(|entry| entry.kind() == bootinfo::MemoryKind::ReservedMapped)
            .map_or(-1, |entry| attribute(entry.base as usize));
        klog!(Info, console, "sctlr_el1 {:#x}", arch::system_control());
        klog!(Info, console, "mapping: runtime services {runtime:#04x}");
        klog!(
            Info,
            console,
            "mapping: kernel image {:#04x}, uart {:#04x}, boot info {:#04x}, tables at {:#x}",
            attribute(arch::kernel_image().start),
            attribute(board.uart.base as usize),
            attribute(handoff_address),
            arch::translation_root()
        );
    }
    klog!(
        Ok,
        console,
        "memory {} MiB at {:#x}",
        board.memory.size >> 20,
        board.memory.base
    );
    klog!(Ok, console, "uart pl011 at {:#x}", board.uart.base);

    let mut frames = match memory::init(&board) {
        Ok(frames) => frames,
        Err(error) => panic!("physical memory: {error}"),
    };
    memory::report(&frames, &board, &mut console);
    memory::selftest(&mut frames, &board, &mut console);

    if let Err(error) = arch::mmu::init(&mut frames, &board) {
        panic!("mmu: {error}");
    }
    arch::mmu::verify(&mut console);

    let entry = match board.entry {
        board::Entry::Direct => "direct load by the QEMU loader",
        board::Entry::Bootloader { .. } => "UEFI bootloader, boot info",
    };
    log::summary(
        &mut console,
        &log::Summary {
            entry,
            memory_mib: board.memory.size >> 20,
            free_frames: frames.free_frames(),
            total_frames: frames.memory_frames(),
            console: board.uart.base,
        },
    );

    arch::run_selftest();
    klog!(Ok, console, "resumed after breakpoint");

    #[cfg(feature = "selftest-write-text")]
    arch::mmu::write_to_text();
    #[cfg(feature = "selftest-execute-data")]
    arch::mmu::execute_data();
    #[cfg(feature = "selftest-execute-rodata")]
    arch::mmu::execute_rodata();
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
    klog!(Error, console, "kernel panic: {info}");
    arch::halt()
}
