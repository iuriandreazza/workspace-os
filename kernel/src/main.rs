#![no_std]
#![no_main]

extern crate alloc;

mod arch;
mod board;
mod drivers;
mod heap;
mod interrupts;
mod log;
mod memory;
mod memory_image;
mod threads;

use core::panic::PanicInfo;

use log::klog;

/// `handoff_address` is the device tree address (or zero) after a direct load
/// and the boot info address after the bootloader; `entered_by_bootloader` is
/// 1 when the boot code saw the entry magic, 0 otherwise. `extra_entry_registers`
/// is `x2 | x3` as they were at entry, which the bootloader sets to zero.
/// `entered_at_el3` is 1 when the boot code came down from EL3.
#[unsafe(no_mangle)]
pub extern "C" fn kernel_main(
    handoff_address: usize,
    entered_by_bootloader: usize,
    extra_entry_registers: usize,
    entered_at_el3: usize,
) -> ! {
    memory_image::assert_initialised();

    let board = if entered_by_bootloader == 1 {
        match board::from_boot_info(handoff_address) {
            Ok(board) => board,
            Err((error, console)) => {
                if let Some(base) = console {
                    board::use_console_at(base);
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

    // The console is at the physical address the hand-over names until the
    // kernel's tables are live (ADR 0033).
    board::use_console_at(board.uart.base as usize);
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

    // Read from the device tree before the switch: the controller's registers
    // are mapped into the device window with the rest.
    let wiring = match board.interrupt_wiring() {
        Ok(wiring) => wiring,
        Err(error) => panic!("interrupt controller: {error}"),
    };
    let before = board.handoff_digest();
    if let Err(error) = arch::mmu::init(&mut frames, &board, &wiring) {
        panic!("mmu: {error}");
    }
    // The lower half is gone: the hand-over is read again from its address
    // through the linear map (ADR 0033), and from here on it is this board the
    // kernel uses, never the one read through the entry's map. A linear map
    // that lost a region faults here, one that maps another frame reads other
    // bytes.
    let board = match board.read_again(handoff_address) {
        Ok(again) => again,
        Err(error) => panic!("handoff read again after the switch: {error}"),
    };
    let after = board.handoff_digest();
    assert!(
        before == after,
        "handoff read after the switch differs from the one before it"
    );
    klog!(
        Ok,
        console,
        "handoff: read again after the switch, {} bytes, digest {:#x}, as before it",
        after.bytes,
        after.value
    );
    arch::mmu::verify(&mut console, &board, &wiring, handoff_address);
    memory::linear_map_selftest(&board, frames.window(), &mut console);

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

    // The frame allocator moves into the heap, which grows from it.
    if let Err(error) = heap::init(frames) {
        panic!("heap: {error}");
    }
    heap::selftest(&mut console);

    // The memory the bootloader owned goes to the frame allocator (ADR 0033):
    // first everything the kernel keeps from it is copied into the heap, then
    // the frames are released and used, then the hand-over is digested again,
    // which a board still pointing into that memory would fail.
    let board = board.into_owned();
    let copied = board.handoff_digest();
    assert!(
        before == copied,
        "the hand-over copied into the heap differs from the one read"
    );
    klog!(
        Ok,
        console,
        "handoff: kept by the kernel, {} bytes, digest {:#x}, as before it",
        copied.bytes,
        copied.value
    );
    let mut reclaim = memory::Reclaim::plan(&board);
    heap::with_frames(|frames| reclaim.release(frames, &mut console));
    heap::with_frames(|frames| memory::reclaim_selftest(frames, &reclaim, &mut console));
    // Its lists are heap memory that has done its job.
    drop(reclaim);
    let used = board.handoff_digest();
    assert!(
        before == used,
        "the hand-over changed after the reclaimed memory was used"
    );
    klog!(
        Ok,
        console,
        "handoff: read again after the reclaimed memory was used, {} bytes, digest {:#x}, as before it",
        used.bytes,
        used.value
    );
    interrupts::selftest(&mut console, &wiring, entered_at_el3 == 1);
    threads::selftest(&mut console);
    #[cfg(feature = "selftest-heap-exhaustion")]
    heap::exhaust(&mut console);

    arch::run_selftest();
    klog!(Ok, console, "resumed after breakpoint");

    #[cfg(feature = "selftest-stack-overflow")]
    arch::overflow_stack();
    #[cfg(feature = "selftest-nested-exception")]
    arch::raise_nested_exception();
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

    threads::retire_boot_thread()
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    // A tick must not arrive while the report is written.
    arch::disable_interrupts();
    let mut console = board::console();
    klog!(Error, console, "kernel panic: {info}");
    klog!(
        Error,
        console,
        "panicked in thread {}.",
        threads::current_index()
    );
    #[cfg(feature = "selftest-thread-overflow")]
    klog!(
        Info,
        console,
        "selftest: neighbouring thread stack {}",
        if threads::canary_intact() {
            "intact"
        } else {
            "overwritten"
        }
    );
    #[cfg(feature = "selftest-stack-overflow")]
    klog!(
        Info,
        console,
        "selftest: .bss stack probe {}",
        if arch::stack_probe_intact() {
            "intact"
        } else {
            "overwritten"
        }
    );
    arch::halt()
}
