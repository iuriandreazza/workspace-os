//! UEFI bootloader (ADR 0008, ADR 0013): loads the kernel from the EFI system
//! partition, collects what the firmware knows, leaves boot services and enters
//! the kernel with a `BootInfo`.
//!
//! The structure of the handoff and the checks on it are in the `bootinfo`
//! crate, where the host can test them; this file is the part that talks to
//! the firmware. The way Limine converts the memory map and enters a kernel
//! was the reference (see `docs/k2b-bootloader-research.md`); no code is copied.

#![no_std]
#![no_main]

mod cpu;
mod menu;
mod other;
mod ui;

use core::slice;
use core::time::Duration;

use bootinfo::menu::Action;
use bootinfo::{
    ARCH_AARCH64, BootInfo, CONSOLE_PL011, ENTRY_MAGIC, EfiDescriptor, MODE_NORMAL, MemoryEntry,
    MemoryMap, PAGE_SIZE, PageError, Table, TableAttributes, elf,
};
use uefi::boot::{self, AllocateType, MemoryType};
use uefi::mem::memory_map::MemoryMap as _;
use uefi::prelude::*;
use uefi::proto::loaded_image::LoadedImage;
use uefi::proto::media::file::{File, FileAttribute, FileInfo, FileMode, RegularFile};
use uefi::proto::media::fs::SimpleFileSystem;
use uefi::table::cfg::ConfigTableEntry;
use uefi::{cstr16, guid};

/// Where the kernel is on the EFI system partition.
const KERNEL_PATH: &uefi::CStr16 = cstr16!("\\KERNEL.ELF");
/// The entries of other systems (ADR 0025).
const CONFIG_PATH: &uefi::CStr16 = cstr16!("\\WSOS.CFG");
/// A configuration file is some lines; a bigger one is not one.
const MAX_CONFIG_BYTES: usize = 64 * 1024;
const DEVICE_TREE_GUID: uefi::Guid = guid!("b1b621d5-f19c-41a5-830b-d9152c69aae0");
const PL011_COMPATIBLE: &str = "arm,pl011";
/// Room for the memory map the kernel receives. The firmware's has some
/// dozens of entries.
const MAP_CAPACITY: usize = 2048;
/// Tables on top of the bound for the preliminary map: allocating the buffers
/// and leaving boot services can split a few entries after the bound was taken.
const EXTRA_TABLES: usize = 64;

/// Anything that stops the boot before boot services are gone.
enum Failure {
    Firmware(uefi::Error),
    Message(&'static str),
    /// The firmware publishes ACPI (at this root pointer) and no device tree.
    AcpiOnly(u64),
    /// There is no kernel file on the partition.
    KernelMissing,
    /// A step on the kernel file failed: what was being done, and why.
    KernelFile(&'static str, Status),
    Elf(elf::ElfError),
    Map(bootinfo::BuildError),
    Table(PageError),
}

impl From<uefi::Error> for Failure {
    fn from(error: uefi::Error) -> Self {
        Self::Firmware(error)
    }
}

impl core::fmt::Display for Failure {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Firmware(error) => write!(f, "firmware: {error}"),
            Self::Message(text) => f.write_str(text),
            Self::AcpiOnly(rsdp) => write!(
                f,
                "the firmware offers ACPI only (rsdp {rsdp:#x}): the kernel has no ACPI reader yet (on QEMU, use -machine virt,acpi=off)"
            ),
            Self::KernelMissing => {
                write!(f, "{KERNEL_PATH} is not on the EFI system partition")
            }
            Self::KernelFile(action, status) => {
                write!(f, "cannot {action} {KERNEL_PATH}: {status:?}")
            }
            Self::Elf(error) => write!(f, "kernel image: {error}"),
            Self::Map(error) => write!(f, "{error}"),
            Self::Table(error) => write!(f, "{error}"),
        }
    }
}

type Outcome<T> = Result<T, Failure>;

fn pages_for(bytes: usize) -> usize {
    bytes.div_ceil(PAGE_SIZE as usize)
}

/// Memory the kernel receives, so it is of the type the map turns into
/// "bootloader reclaimable". Zeroed.
fn allocate(pages: usize) -> Outcome<&'static mut [u8]> {
    Ok(allocate_pages(pages)?)
}

fn allocate_pages(pages: usize) -> Result<&'static mut [u8], uefi::Error> {
    let pointer = boot::allocate_pages(AllocateType::AnyPages, MemoryType::LOADER_DATA, pages)?;
    // SAFETY: the firmware gave `pages` whole pages that nothing else uses and
    // that stay allocated for the rest of the boot.
    let bytes = unsafe { slice::from_raw_parts_mut(pointer.as_ptr(), pages * PAGE_SIZE as usize) };
    bytes.fill(0);
    Ok(bytes)
}

/// Why a file on the EFI system partition could not be read.
enum FileError {
    Missing,
    /// A step on the file failed: what was being done, and why.
    Step(&'static str, Status),
    Message(&'static str),
    Firmware(uefi::Error),
}

impl From<uefi::Error> for FileError {
    fn from(error: uefi::Error) -> Self {
        Self::Firmware(error)
    }
}

/// Reads a whole file of the partition the bootloader started from into
/// memory of the type the kernel receives. The buffer is page aligned.
fn read_file(path: &uefi::CStr16) -> Result<&'static mut [u8], FileError> {
    let image = boot::open_protocol_exclusive::<LoadedImage>(boot::image_handle())?;
    let device = image.device().ok_or(FileError::Message(
        "the bootloader was not started from a device",
    ))?;
    let mut fs = boot::open_protocol_exclusive::<SimpleFileSystem>(device)?;
    let mut root = fs.open_volume()?;
    let handle = root
        .open(path, FileMode::Read, FileAttribute::empty())
        .map_err(|e| match e.status() {
            Status::NOT_FOUND => FileError::Missing,
            status => FileError::Step("open", status),
        })?;
    let mut file: RegularFile = handle
        .into_regular_file()
        .ok_or(FileError::Message("it is a directory"))?;

    // `get_info` wants a buffer aligned for `FileInfo`.
    #[repr(align(8))]
    // The info holds the file's own name, up to 255 characters of two bytes.
    struct InfoBuffer([u8; 1024]);
    let mut info_buffer = InfoBuffer([0u8; 1024]);
    let size = file
        .get_info::<FileInfo>(&mut info_buffer.0)
        .map_err(|e| FileError::Step("read the size of", e.status()))?
        .file_size() as usize;
    let buffer = allocate_pages(pages_for(size.max(1)))?;
    match file.read(&mut buffer[..size]) {
        Ok(read) if read == size => Ok(&mut buffer[..size]),
        Ok(_) => {
            release(&mut buffer[..size]);
            Err(FileError::Message("short read"))
        }
        Err(error) => {
            release(&mut buffer[..size]);
            Err(FileError::Step("read", error.status()))
        }
    }
}

/// Gives the pages `read_file` took back to the firmware. The caller drops
/// every reference to them.
fn release(bytes: &mut [u8]) {
    if let Some(base) = core::ptr::NonNull::new(bytes.as_mut_ptr()) {
        // SAFETY: `read_file` allocated `pages_for(len.max(1))` whole pages at
        // this address, and the caller holds the only reference to them.
        let _ = unsafe { boot::free_pages(base, pages_for(bytes.len().max(1))) };
    }
}

fn read_kernel_file() -> Outcome<&'static mut [u8]> {
    read_file(KERNEL_PATH).map_err(|error| match error {
        FileError::Missing => Failure::KernelMissing,
        FileError::Step(action, status) => Failure::KernelFile(action, status),
        FileError::Message(text) => Failure::Message(text),
        FileError::Firmware(error) => Failure::Firmware(error),
    })
}

/// The entries of `\WSOS.CFG`, as text that lives for the rest of the boot.
/// No file is no entries; a file that cannot be read is said, and is no
/// entries either, so that it can never keep our own kernel from starting.
fn read_config() -> Option<&'static str> {
    let bytes = match read_file(CONFIG_PATH) {
        Ok(bytes) if bytes.len() <= MAX_CONFIG_BYTES => bytes,
        Ok(bytes) => {
            ui::error(format_args!(
                "{CONFIG_PATH} is over {MAX_CONFIG_BYTES} bytes: ignored"
            ));
            release(bytes);
            return None;
        }
        Err(FileError::Missing) => return None,
        Err(FileError::Step(step, status)) => {
            ui::error(format_args!("cannot {step} {CONFIG_PATH}: {status:?}"));
            return None;
        }
        Err(FileError::Message(text)) => {
            ui::error(format_args!("{CONFIG_PATH}: {text}"));
            return None;
        }
        Err(FileError::Firmware(error)) => {
            ui::error(format_args!("{CONFIG_PATH}: firmware: {error}"));
            return None;
        }
    };
    if core::str::from_utf8(bytes).is_err() {
        ui::error(format_args!("{CONFIG_PATH} is not text (UTF-8): ignored"));
        release(bytes);
        return None;
    }
    let bytes: &'static [u8] = bytes;
    core::str::from_utf8(bytes).ok()
}

/// Copies the kernel's loadable segments to the addresses the image asks for.
/// Returns the plan, and leaves `plan.span()` bytes at `plan.base` allocated.
fn load_kernel(file: &[u8]) -> Outcome<elf::LoadPlan> {
    let plan = elf::plan(file).map_err(Failure::Elf)?;
    // Page zero cannot be a slice in Rust, and no firmware gives it away.
    if plan.base == 0 {
        return Err(Failure::Message("the kernel is linked at address 0"));
    }
    let pages = pages_for(plan.span() as usize);
    boot::allocate_pages(
        AllocateType::Address(plan.base),
        MemoryType::LOADER_DATA,
        pages,
    )
    .map_err(|error| {
        print_memory_map();
        ui::error(format_args!(
            "the kernel needs {:#x}..{:#x} and the firmware refused it ({})",
            plan.base,
            plan.base + pages as u64 * PAGE_SIZE,
            error.status()
        ));
        Failure::Message(
            "the kernel's load address is not free RAM (the machine needs more memory, or the firmware uses that address)",
        )
    })?;
    // SAFETY: the pages at `plan.base` were just allocated for this, and the
    // firmware identity maps memory while boot services run.
    let image =
        unsafe { slice::from_raw_parts_mut(plan.base as *mut u8, pages * PAGE_SIZE as usize) };
    image.fill(0);
    for segment in plan.segments() {
        let at = (segment.address - plan.base) as usize;
        let from = segment.file_offset as usize;
        image[at..at + segment.file_size as usize]
            .copy_from_slice(&file[from..from + segment.file_size as usize]);
    }
    Ok(plan)
}

/// Prints the firmware's memory map, for a failure that the map explains.
fn print_memory_map() {
    if let Ok(map) = boot::memory_map(MemoryType::LOADER_DATA) {
        for descriptor in map.entries() {
            ui::info(format_args!(
                "memory map: {:#x}..{:#x} {:?}",
                descriptor.phys_start,
                descriptor.phys_start + descriptor.page_count * PAGE_SIZE,
                descriptor.ty
            ));
        }
    }
}

/// What the firmware publishes besides memory.
struct Tables {
    device_tree: Option<(usize, usize)>,
    /// The firmware published a device tree, usable or not.
    device_tree_published: bool,
    rsdp: u64,
}

fn find_tables() -> Tables {
    uefi::system::with_config_table(|entries| {
        let find = |guid: uefi::Guid| {
            entries
                .iter()
                .find(|entry| entry.guid == guid)
                .map(|entry| entry.address as usize)
        };
        let device_tree = find(DEVICE_TREE_GUID).and_then(|address| {
            // SAFETY: the firmware published this address as its device tree;
            // the reader checks the blob before trusting any size in it.
            let fdt = unsafe { devicetree::Fdt::from_address(address) }.ok()?;
            Some((address, fdt.size()))
        });
        let rsdp = bootinfo::select_rsdp(
            find(ConfigTableEntry::ACPI2_GUID).map(|a| a as u64),
            find(ConfigTableEntry::ACPI_GUID).map(|a| a as u64),
        );
        Tables {
            device_tree,
            device_tree_published: find(DEVICE_TREE_GUID).is_some(),
            rsdp,
        }
    })
}

/// Writes straight to the PL011 UART. Boot services are gone, so the firmware's
/// console is too; this is the only way left to say why the boot stops.
struct Uart(usize);

const UART_FLAGS: usize = 0x18;
const UART_TX_FIFO_FULL: u32 = 1 << 5;

impl core::fmt::Write for Uart {
    fn write_str(&mut self, text: &str) -> core::fmt::Result {
        for byte in text.bytes() {
            // SAFETY: `self.0` is the PL011 the device tree named, mapped as
            // Device memory by the firmware and by our own map; the data
            // register is at offset 0 and the flag register at 0x18, whose bit
            // 5 says the transmit FIFO is full. Waiting on it keeps a message
            // of any length from being cut after the first FIFO's worth.
            unsafe {
                while core::ptr::read_volatile((self.0 + UART_FLAGS) as *const u32)
                    & UART_TX_FIFO_FULL
                    != 0
                {
                    core::hint::spin_loop();
                }
                core::ptr::write_volatile(self.0 as *mut u8, byte);
            }
        }
        Ok(())
    }
}

fn fail_on_uart(uart: usize, reason: impl core::fmt::Display) -> ! {
    use core::fmt::Write;
    let _ = writeln!(Uart(uart), "workspace-os bootloader: {reason}\r");
    loop {
        core::hint::spin_loop();
    }
}

fn run() -> Outcome<core::convert::Infallible> {
    ui::banner();
    ui::ok(format_args!("started at EL{}", cpu::current_el()));
    if cpu::current_el() == 2 && cpu::virtualization_host_extensions() {
        return Err(Failure::Message(
            "started at EL2 with the host extensions (E2H): not supported yet",
        ));
    }
    if !matches!(cpu::current_el(), 1 | 2) {
        return Err(Failure::Message("started at an unexpected exception level"));
    }

    // The entries of other systems come from the file on the partition; the
    // built-in two are first and never depend on it (ADR 0025).
    let mut others =
        [bootinfo::menu::Entry::kernel("", MODE_NORMAL); bootinfo::config::MAX_FILE_ENTRIES];
    let other_count = match read_config() {
        None => 0,
        Some(text) => match bootinfo::config::parse(text, &mut others) {
            Ok(count) => {
                ui::ok(format_args!("{CONFIG_PATH}: {count} entries"));
                count
            }
            Err(error) => {
                ui::error(format_args!(
                    "{CONFIG_PATH}, {error}: the built-in entries only"
                ));
                0
            }
        },
    };
    let mut entries = [bootinfo::menu::BUILT_IN[0]; bootinfo::menu::MAX_ENTRIES];
    let count = bootinfo::menu::with_built_in(&others[..other_count], &mut entries);

    let mut autoboot = core::num::NonZeroU32::new(bootinfo::menu::AUTOBOOT_SECONDS);
    loop {
        let menu =
            bootinfo::menu::Menu::new(&entries[..count], bootinfo::menu::DEFAULT_ENTRY, autoboot)
                .ok_or(Failure::Message("the boot menu has no entry to start"))?;
        let (entry, reason) = menu::choose(menu);
        match entry.action {
            Action::Kernel { boot_mode } => {
                ui::ok(format_args!(
                    "starting {} (boot mode {boot_mode}, {}), {reason}",
                    entry.label,
                    bootinfo::boot_mode_name(boot_mode)
                ));
                return boot_kernel(boot_mode);
            }
            other_system => {
                ui::ok(format_args!(
                    "starting {} ({}), {reason}",
                    entry.label,
                    other::kind_name(&other_system)
                ));
                match other::start(&other_system) {
                    Ok(status) => ui::info(format_args!(
                        "{} returned {status:?}: back to the menu",
                        entry.label
                    )),
                    Err(refusal) => ui::refused(entry.label, format_args!("{refusal}")),
                }
                // Somebody read this: no countdown starts a default entry
                // over what they are reading (and a failing default entry
                // would loop forever).
                autoboot = None;
            }
        }
    }
}

/// Loads the kernel and enters it with `boot_mode` in the boot info.
fn boot_kernel(boot_mode: u32) -> Outcome<core::convert::Infallible> {
    let kernel_file = read_kernel_file()?;
    let plan = load_kernel(kernel_file)?;
    #[cfg(feature = "selftest-retry-after-failure")]
    let outcome = if FIRST_RUN.swap(false, core::sync::atomic::Ordering::Relaxed) {
        Err(Failure::Message("forced failure of the first run"))
    } else {
        hand_over(&plan, boot_mode)
    };
    #[cfg(not(feature = "selftest-retry-after-failure"))]
    let outcome = hand_over(&plan, boot_mode);
    // Only a boot that failed gets here. The range is given back so that a
    // second run in the same firmware session (the next boot entry) can load
    // the kernel at the same address.
    if let Some(base) = core::ptr::NonNull::new(plan.base as *mut u8) {
        // SAFETY: `load_kernel` allocated exactly these pages and nothing
        // refers to them after the failure.
        let _ = unsafe { boot::free_pages(base, pages_for(plan.span() as usize)) };
    }
    outcome
}

#[cfg(feature = "selftest-retry-after-failure")]
static FIRST_RUN: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(true);

/// Everything after the kernel is loaded, up to the jump into it, with
/// `boot_mode` in the boot info.
fn hand_over(plan: &elf::LoadPlan, boot_mode: u32) -> Outcome<core::convert::Infallible> {
    ui::ok(format_args!(
        "kernel loaded at {:#x}..{:#x}, entry {:#x}",
        plan.base, plan.end, plan.entry
    ));

    let tables = find_tables();
    let Some((device_tree, device_tree_size)) = tables.device_tree else {
        return Err(if tables.device_tree_published {
            Failure::Message("the device tree the firmware published is not valid")
        } else if tables.rsdp != 0 {
            Failure::AcpiOnly(tables.rsdp)
        } else {
            Failure::Message("the firmware publishes neither a device tree nor ACPI")
        });
    };
    // SAFETY: the blob was checked in `find_tables`.
    let fdt = unsafe { devicetree::Fdt::from_address(device_tree) }
        .map_err(|_| Failure::Message("the device tree vanished"))?;
    let uart = fdt
        .device(PL011_COMPATIBLE)
        .map_err(|_| Failure::Message("the device tree names no PL011 console"))?;
    ui::ok(format_args!(
        "device tree at {device_tree:#x} ({device_tree_size} bytes), acpi rsdp {:#x}, pl011 at {:#x}",
        tables.rsdp, uart.base
    ));

    // Everything the kernel will read is allocated now: after boot services
    // are gone nothing can be.
    let device_tree_copy = allocate(pages_for(device_tree_size))?;
    // SAFETY: `find_tables` checked that `device_tree_size` bytes are the blob.
    device_tree_copy[..device_tree_size].copy_from_slice(unsafe {
        slice::from_raw_parts(device_tree as *const u8, device_tree_size)
    });
    let info_page = allocate(1)?;
    let map_address = allocate(pages_for(MAP_CAPACITY * bootinfo::ENTRY_SIZE))?.as_ptr() as usize;
    // SAFETY: page aligned, zeroed memory big enough for `MAP_CAPACITY`
    // entries, which this function alone uses until the jump.
    let entries =
        unsafe { slice::from_raw_parts_mut(map_address as *mut MemoryEntry, MAP_CAPACITY) };

    let translation = cpu::translation();
    if !translation.granule_4k {
        return Err(Failure::Message(
            "the firmware translates with a granule other than 4 KiB",
        ));
    }
    let attributes = TableAttributes::from_mair(translation.mair, translation.address_bits).ok_or(
        Failure::Message(
            "the firmware's memory attributes lack Device-nGnRnE or Normal write-back",
        ),
    )?;

    // A preliminary map sizes the page table pool, and building the tables
    // from it now reports a map they cannot describe while the firmware can
    // still print.
    let preliminary = boot::memory_map(MemoryType::LOADER_DATA)?;
    let count = convert(&preliminary, entries)?;
    let tables_wanted = bootinfo::tables_needed(&entries[..count]) + EXTRA_TABLES;
    let pool_bytes = allocate(pages_for(tables_wanted * PAGE_SIZE as usize))?;
    let pool_address = pool_bytes.as_ptr() as usize;
    // SAFETY: page aligned, zeroed memory, as many tables as were asked for,
    // used by this function alone until the jump.
    let pool = unsafe { slice::from_raw_parts_mut(pool_address as *mut Table, tables_wanted) };
    bootinfo::build_identity_map(pool, &entries[..count], attributes).map_err(Failure::Table)?;
    drop(preliminary);

    let mut info = BootInfo::new();
    info.console_kind = CONSOLE_PL011;
    info.console_base = uart.base;
    info.boot_mode = boot_mode;
    info.arch = ARCH_AARCH64;
    info.boot_cpu = cpu::boot_cpu();
    info.memory_map = map_address as u64;
    info.device_tree = device_tree_copy.as_ptr() as u64;
    info.device_tree_size = device_tree_size as u64;
    info.rsdp = tables.rsdp;
    info.kernel_base = plan.base;
    info.kernel_size = plan.memory_size();
    info.efi_system_table =
        uefi::table::system_table_raw().map_or(0, |table| table.as_ptr() as u64);
    // Only a kernel that must refuse the structure is booted with this.
    #[cfg(feature = "selftest-unknown-boot-info-version")]
    {
        info.version_major = bootinfo::VERSION_MAJOR + 1;
    }
    let info_address = info_page.as_ptr() as u64;

    let usable_bytes: u64 = entries[..count]
        .iter()
        .filter(|entry| entry.kind() == bootinfo::MemoryKind::Usable)
        .map(|entry| entry.length)
        .sum();
    ui::summary(&ui::Summary {
        kernel_base: plan.base,
        kernel_end: plan.end,
        entry: plan.entry,
        usable_mib: usable_bytes >> 20,
        regions: count,
        console: uart.base,
        device_tree_bytes: device_tree_size,
        exception_level: cpu::current_el(),
    });
    ui::info(format_args!("page tables at {pool_address:#x}"));
    ui::go(format_args!("leaving boot services, entering the kernel"));
    let uart_base = uart.base as usize;

    // SAFETY: nothing below calls the firmware, and nothing uses boot
    // services memory in a way that depends on it staying valid, except this
    // function's own stack and code, which the map keeps mapped.
    let final_map = unsafe { boot::exit_boot_services(Some(MemoryType::LOADER_DATA)) };
    // Not before the call: the firmware's exit notifications may wait on an
    // interrupt. Once it returns, its handlers must not run while the map is
    // rebuilt.
    cpu::mask_interrupts();

    let count =
        convert(&final_map, entries).unwrap_or_else(|reason| fail_on_uart(uart_base, reason));
    let count = bootinfo::carve_kernel(entries, count, plan.base, plan.span())
        .unwrap_or_else(|reason| fail_on_uart(uart_base, reason));
    info.memory_map_count = count as u64;
    // `entries` has the layout of the wire format, so the map is complete; the
    // kernel's own checks are run here too, so a broken map stops the boot
    // with a message instead of in the kernel.
    // SAFETY: a shared view of the entries just written, which nothing writes
    // while it lives.
    let written = unsafe {
        slice::from_raw_parts(entries.as_ptr().cast::<u8>(), count * bootinfo::ENTRY_SIZE)
    };
    let map = MemoryMap::from_bytes(written, bootinfo::ENTRY_SIZE).and_then(|map| {
        map.validate(info.kernel_base, info.kernel_size)
            .map(|()| map)
    });
    if let Err(reason) = map {
        fail_on_uart(
            uart_base,
            format_args!("the final memory map is invalid: {reason}"),
        );
    }
    info_page[..core::mem::size_of::<BootInfo>()].copy_from_slice(&info.to_bytes());

    // The tables are built from the final map: the buffers, the pool and the
    // kernel are in it with their final types.
    let root = bootinfo::build_identity_map(pool, &entries[..count], attributes)
        .unwrap_or_else(|reason| fail_on_uart(uart_base, reason));

    cpu::clean_to_coherency(pool_address, pool_address + pool.len() * PAGE_SIZE as usize);
    cpu::clean_to_coherency(
        info_address as usize,
        info_address as usize + PAGE_SIZE as usize,
    );
    cpu::clean_to_coherency(map_address, map_address + count * bootinfo::ENTRY_SIZE);
    cpu::sync_instructions(plan.base as usize, plan.end as usize);

    // SAFETY: boot services are gone; `root` identity maps the code, the
    // stack, the kernel, the boot info and the map with the live memory types;
    // `plan.entry` is inside the loaded kernel.
    unsafe { cpu::enter_kernel(plan.entry, info_address, root, ENTRY_MAGIC) }
}

fn convert(map: &impl uefi::mem::memory_map::MemoryMap, out: &mut [MemoryEntry]) -> Outcome<usize> {
    bootinfo::build_memory_map(
        map.entries().map(|descriptor| EfiDescriptor {
            efi_type: descriptor.ty.0,
            physical_start: descriptor.phys_start,
            pages: descriptor.page_count,
        }),
        out,
    )
    .map_err(Failure::Map)
}

#[entry]
fn main() -> Status {
    #[cfg(feature = "selftest-retry-after-failure")]
    let result = match run() {
        Err(failure) => {
            ui::failure(format_args!("{failure}"));
            run()
        }
        done => done,
    };
    #[cfg(not(feature = "selftest-retry-after-failure"))]
    let result = run();
    match result {
        Ok(never) => match never {},
        Err(failure) => {
            ui::failure(format_args!("{failure}"));
            boot::stall(Duration::from_secs(5));
            match failure {
                Failure::Firmware(error) => error.status(),
                Failure::KernelFile(_, status) => status,
                Failure::KernelMissing => Status::NOT_FOUND,
                _ => Status::LOAD_ERROR,
            }
        }
    }
}
