//! The kernel's own translation tables (ADR 0019, 0033): built from frames of
//! the allocator on top of the boot tables `boot.s` installed, and switched to:
//! the image, a linear map of RAM and a window for device registers, all in the
//! upper half. The lower half is dropped.
//!
//! The pure part (descriptors, permissions, mapping, auditing) is `kernelvm`,
//! tested on the host. What is here is the part that touches system registers
//! and memory through the way the kernel reads physical addresses
//! (`super::pointer_to`).

// An error here ends the boot with a report, and there is no heap to box the
// audit it carries in.
#![allow(clippy::result_large_err)]

use core::arch::asm;
use core::fmt::{self, Write};
use core::ops::Range;

use devicetree::Region;

use kernelvm::{
    DeviceWindow, Expectation, ImageLayout, LayoutError, MapError, Mapper, MemoryTypes,
    Permissions, Table, TableMemory, TableRead, Window,
};
use physmem::FrameAllocator;

use super::{physical, pointer_to};
use crate::board::{Board, InterruptWiring};
use crate::log::klog;

unsafe extern "C" {
    safe static __image_start: u8;
    safe static __text_end: u8;
    safe static __rodata_end: u8;
    safe static __data_end: u8;
    safe static __stack_bottom: u8;
    safe static __stack_top: u8;
    safe static __exception_stack_bottom: u8;
    safe static __exception_stack_top: u8;
}

macro_rules! physical_of {
    ($symbol:ident) => {
        physical(core::ptr::addr_of!($symbol) as usize) as u64
    };
}

/// The parts of the image, as the linker placed them.
fn layout() -> Result<ImageLayout, LayoutError> {
    ImageLayout::new(
        physical_of!(__image_start)..physical_of!(__text_end),
        physical_of!(__text_end)..physical_of!(__rodata_end),
        physical_of!(__rodata_end)..physical_of!(__data_end),
        physical_of!(__stack_bottom)..physical_of!(__stack_top),
        physical_of!(__exception_stack_bottom)..physical_of!(__exception_stack_top),
    )
}

pub enum Error {
    /// `MAIR_EL1` lacks Normal write-back or Device-nGnRnE memory.
    MemoryTypes,
    Layout(LayoutError),
    Map(MapError),
    /// The tables `boot.s` built do not describe the image as `ImageLayout`
    /// does: the report says how.
    BootTables(kernelvm::Audit),
    /// The new upper half does not translate a part of the image to its frames.
    NewTables(&'static str),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MemoryTypes => f.write_str("MAIR_EL1 holds no Normal or Device memory type"),
            Self::Layout(error) => write!(f, "{error}"),
            Self::Map(error) => write!(f, "{error}"),
            Self::BootTables(report) => write!(f, "the boot tables are wrong: {report:?}"),
            Self::NewTables(what) => write!(f, "the new tables are wrong: {what}"),
        }
    }
}

impl From<LayoutError> for Error {
    fn from(error: LayoutError) -> Self {
        Self::Layout(error)
    }
}

impl From<MapError> for Error {
    fn from(error: MapError) -> Self {
        Self::Map(error)
    }
}

/// Tables read by physical address, at the address `pointer_to` gives: the
/// entry's identity map before `init` switches, the linear map after.
struct PhysicalTables;

impl TableRead for PhysicalTables {
    fn table(&self, address: u64) -> &Table {
        // SAFETY: callers only pass the roots of live translation tables and
        // the addresses found in them. Those are tables in RAM that the map
        // the kernel runs on covers at `pointer_to(address)`: the boot tables
        // (in the image, which only the entry's map and the image's own
        // mapping reach, so they are read before the switch) or the tables
        // `init` built from frames (in the linear map, which includes every
        // frame the allocator hands out). Nothing writes while they are read.
        unsafe { &*(pointer_to(address) as *const Table) }
    }
}

/// The most tables `init` builds: the root, a level 1, a level 2 and a level 3
/// table for the image, the device window's, the empty root for the lower half,
/// and, per region of RAM, up to two level 2 and two level 3 tables at its edges
/// and a level 1 table for each 512 GiB it touches (whole gigabytes are blocks
/// in the level 1 tables). The boot map of a firmware has a few dozen regions,
/// so this is generous; more regions fail with `OutOfFrames`.
const MAX_TABLES: usize = 512;

/// Tables taken from the frame allocator, which the kernel reaches through
/// `pointer_to`.
struct FrameTables<'a, 'f> {
    frames: &'a mut FrameAllocator<'f>,
    owned: [u64; MAX_TABLES],
    count: usize,
}

impl TableRead for FrameTables<'_, '_> {
    fn table(&self, address: u64) -> &Table {
        PhysicalTables.table(address)
    }
}

impl TableMemory for FrameTables<'_, '_> {
    fn allocate(&mut self) -> Option<u64> {
        let slot = self.owned.get_mut(self.count)?;
        let address = self.frames.alloc()?;
        *slot = address;
        self.count += 1;
        let line_size = data_cache_line_size();
        for line in (address..address + kernelvm::PAGE_SIZE).step_by(line_size) {
            // SAFETY: invalidating a line of a frame that was just handed out
            // discards nothing anyone needs; it keeps a stale dirty line from
            // being written back over the table later.
            unsafe {
                asm!("dc ivac, {}", in(reg) pointer_to(line), options(nostack, preserves_flags))
            };
        }
        // SAFETY: the frame was just handed out, so nothing else uses it, and
        // the map the kernel runs on covers RAM at `pointer_to(address)`: the
        // bootloader's map, or on the direct path the boot map (1 GiB blocks,
        // and the image's own 2 MiB, tail included), then the linear map.
        unsafe { core::ptr::write(pointer_to(address) as *mut Table, Table::EMPTY) };
        Some(address)
    }

    fn table_mut(&mut self, address: u64) -> &mut Table {
        // SAFETY: the table is one of those `allocate` handed out, so this is
        // the only reference to it while the mapper holds `&mut self`.
        unsafe { &mut *(pointer_to(address) as *mut Table) }
    }
}

impl FrameTables<'_, '_> {
    /// Makes the tables visible to the walker: clean and invalidate to the point
    /// of coherency by address, then wait for it.
    fn publish(&self) {
        let line_size = data_cache_line_size();
        for &table in &self.owned[..self.count] {
            for line in (table..table + kernelvm::PAGE_SIZE).step_by(line_size) {
                clean_and_invalidate(pointer_to(line) as u64);
            }
        }
        // SAFETY: a barrier.
        unsafe { asm!("dsb ish", options(nostack, preserves_flags)) };
    }
}

/// Cleans and invalidates the line holding the virtual `address` to the point of
/// coherency. It changes no data; the invalidate matters when the walker's
/// attributes are cacheable and the kernel's own writes were not (the direct
/// path runs with `C` clear), so a stale line must not survive.
fn clean_and_invalidate(address: u64) {
    // SAFETY: cleaning and invalidating a line of RAM the kernel owns changes
    // no data.
    unsafe { asm!("dc civac, {}", in(reg) address, options(nostack, preserves_flags)) };
}

/// The smallest data cache line, in bytes: `CTR_EL0.DminLine` is its log2 in
/// words.
fn data_cache_line_size() -> usize {
    let ctr: u64;
    // SAFETY: reading `CTR_EL0` has no side effects.
    unsafe { asm!("mrs {}, ctr_el0", out(reg) ctr, options(nomem, nostack, preserves_flags)) };
    4 << ((ctr >> 16) & 0xf)
}

fn register_ttbr(register: u64) -> u64 {
    register & 0x0000_ffff_ffff_f000
}

fn read_mair() -> u64 {
    let mair: u64;
    // SAFETY: reading `MAIR_EL1` has no side effects.
    unsafe { asm!("mrs {}, mair_el1", out(reg) mair, options(nomem, nostack, preserves_flags)) };
    mair
}

fn read_tcr() -> u64 {
    let tcr: u64;
    // SAFETY: reading `TCR_EL1` has no side effects.
    unsafe { asm!("mrs {}, tcr_el1", out(reg) tcr, options(nomem, nostack, preserves_flags)) };
    tcr
}

fn read_ttbr1() -> u64 {
    let ttbr: u64;
    // SAFETY: reading `TTBR1_EL1` has no side effects.
    unsafe { asm!("mrs {}, ttbr1_el1", out(reg) ttbr, options(nomem, nostack, preserves_flags)) };
    register_ttbr(ttbr)
}

fn memory_types() -> Result<MemoryTypes, Error> {
    MemoryTypes::from_mair(read_mair()).ok_or(Error::MemoryTypes)
}

fn size(range: &Range<u64>) -> u64 {
    range.end - range.start
}

/// Checks that the boot tables, which `boot.s` builds in assembly the host
/// cannot test, map each part of the image to its own frames with its own
/// permissions and the Normal memory type.
fn check_boot_tables(layout: &ImageLayout, types: MemoryTypes) -> Result<(), Error> {
    // The tables of phase A cover 2 MiB from the start of the image: the part
    // after the stacks is read and write too, and the guard pages are empty.
    let tail = layout.exception_stack.end..layout.text.start + kernelvm::BLOCK_SIZE;
    let window = Window {
        virt: kernelvm::virtual_of(layout.text.start)..kernelvm::virtual_of(tail.end),
        offset: Some(kernelvm::KERNEL_OFFSET),
    };
    let [text, rodata, data, stack, exception_stack] = layout.parts();
    let known = [
        text,
        rodata,
        data,
        stack,
        exception_stack,
        (tail.clone(), Permissions::ReadWrite),
    ];
    let report = kernelvm::audit(
        &PhysicalTables,
        read_ttbr1(),
        kernelvm::UPPER_BASE,
        &Expectation {
            types,
            window: Some(&window),
            forbidden: &(0..0),
            device: &[],
            known: Some(&known),
        },
    );
    // Nothing else is mapped: the boot tables hold the image and no more.
    let strays = kernelvm::strays(
        &PhysicalTables,
        read_ttbr1(),
        kernelvm::UPPER_BASE,
        core::slice::from_ref(&window.virt),
    );
    if strays.0 != 0 {
        return Err(Error::BootTables(kernelvm::Audit {
            wrong_section: strays.1,
            ..report
        }));
    }
    let expected = (
        size(&layout.text),
        size(&layout.rodata),
        size(&layout.data) + size(&layout.stack) + size(&layout.exception_stack) + size(&tail),
    );
    let found = (report.text, report.read_only, report.read_write);
    if !report.is_clean() || report.device != 0 || found != expected {
        return Err(Error::BootTables(report));
    }
    Ok(())
}

/// The pages that hold `region`.
fn pages(region: Region) -> Range<u64> {
    let start = region.base & !(kernelvm::PAGE_SIZE - 1);
    let end = (region.base + region.size).next_multiple_of(kernelvm::PAGE_SIZE);
    start..end
}

/// The pages the console's registers and the interrupt controller's two blocks
/// occupy: the only Device memory the device window holds, in the order they
/// are mapped.
fn device_pages(board: &Board, wiring: &InterruptWiring) -> [Range<u64>; 3] {
    let [first, second] = wiring.controller.blocks();
    [pages(board.uart), pages(first), pages(second)]
}

/// Reads `ID_AA64MMFR3_EL1`, which is zero where it does not exist.
fn read_mmfr3() -> u64 {
    let mmfr3: u64;
    // SAFETY: reading an ID register has no side effects; one the processor
    // does not have is reserved and reads as zero. It is named by encoding.
    unsafe {
        asm!("mrs {}, S3_0_C0_C7_3", out(reg) mmfr3, options(nomem, nostack, preserves_flags))
    };
    mmfr3
}

/// `TCR2_EL1.PIE` (bit 1) and `.D128` (bit 5): permission indirection and 128-bit descriptors,
/// neither of which the kernel's tables use.
const TCR2_PIE: u64 = 1 << 1;
const TCR2_D128: u64 = 1 << 5;

/// Reports the registers `boot.s` cleared where they exist, and panics if one
/// holds a feature the kernel's tables do not use (ADR 0026). Nothing is read
/// that the processor lacks: the read would be UNDEFINED.
fn verify_translation_extensions(mut out: impl Write) {
    let mmfr3 = read_mmfr3();
    let s1pie = (mmfr3 >> 8) & 0xf != 0;
    let tcr2: Option<u64> = ((mmfr3 & 0xf) != 0).then(|| {
        let value: u64;
        // SAFETY: `TCRX` says `TCR2_EL1` exists; reading it has no side effects.
        unsafe {
            asm!("mrs {}, S3_0_C2_C0_3", out(reg) value, options(nomem, nostack, preserves_flags))
        };
        value
    });
    let sctlr2: Option<u64> = (((mmfr3 >> 4) & 0xf) != 0).then(|| {
        let value: u64;
        // SAFETY: `SCTLRX` says `SCTLR2_EL1` exists; reading it has no side effects.
        unsafe {
            asm!("mrs {}, S3_0_C1_C0_3", out(reg) value, options(nomem, nostack, preserves_flags))
        };
        value
    });
    klog!(
        Info,
        out,
        "mmu: permission indirection {}, TCR2_EL1 {}, SCTLR2_EL1 {}",
        if s1pie { "present" } else { "absent" },
        Register(tcr2),
        Register(sctlr2)
    );
    assert!(
        tcr2.is_none_or(|value| value & (TCR2_PIE | TCR2_D128) == 0),
        "mmu: TCR2_EL1 holds permission indirection or 128-bit descriptors"
    );
}

/// A system register's value, or that it does not exist.
struct Register(Option<u64>);

impl fmt::Display for Register {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            Some(value) => write!(f, "{value:#x}"),
            None => f.write_str("absent"),
        }
    }
}

/// Builds the kernel's tables from frames of `frames` and switches to them.
///
/// The upper half maps the image as the boot tables do, by section, so that
/// switching `TTBR1_EL1` changes no translation of the code running; the rest
/// of RAM, minus the image, in the linear map; and the console's registers and
/// the interrupt controller's in the device window. Then every physical address is read through the linear
/// map, the console is pointed at the device window, and the lower half is
/// dropped (ADR 0033). A pointer the kernel still holds to a physical address
/// faults from here on.
pub fn init(
    frames: &mut FrameAllocator,
    board: &Board,
    wiring: &InterruptWiring,
) -> Result<(), Error> {
    let types = memory_types()?;
    let layout = layout()?;
    check_boot_tables(&layout, types)?;

    let mut tables = FrameTables {
        frames,
        owned: [0; MAX_TABLES],
        count: 0,
    };
    let hole = layout.span();
    let (upper, console, controller) = {
        let mut mapper = Mapper::new(&mut tables, types)?;
        layout.map(&mut mapper, kernelvm::KERNEL_OFFSET)?;
        let mut result = Ok(());
        let regions = board.for_each_memory_region(|region| {
            if result.is_ok() {
                result = kernelvm::map_linear_region(
                    &mut mapper,
                    region.base..region.base.saturating_add(region.size),
                    &hole,
                );
            }
        });
        result?;
        // A region the device tree could not give is not an error here: the
        // allocator has failed on the same tree before this runs.
        let _ = regions;
        let mut devices = DeviceWindow::new();
        let console = devices.map(&mut mapper, board.uart.base, board.uart.size)?;
        let [first, second] = wiring.controller.blocks();
        let distributor = devices.map(&mut mapper, first.base, first.size)?;
        let second_block = devices.map(&mut mapper, second.base, second.size)?;
        (mapper.root(), console, (distributor, second_block))
    };
    // The root the lower half is pointed at when it is dropped: all zeros.
    let empty = tables.allocate().ok_or(MapError::OutOfFrames)?;
    // The first exception after the switch needs its stack and its vectors in
    // the new root: a root that lacks them hangs the boot with no report.
    check_new_root(upper, &layout, types)?;
    tables.publish();
    switch_upper_half(upper);
    super::read_physical_through_the_linear_map();
    crate::board::use_console_at(console as usize);
    crate::board::use_interrupt_controller_at(controller.0 as usize, controller.1 as usize);
    drop_lower_half(empty);
    Ok(())
}

/// Audits the image in the new root, before it is used, as `verify` does after:
/// every page of every part maps its own frame with its own permissions and
/// memory type, and the sizes are what the linker gave.
fn check_new_root(root: u64, layout: &ImageLayout, types: MemoryTypes) -> Result<(), Error> {
    let span = layout.span();
    let window = Window {
        virt: kernelvm::virtual_of(span.start)..kernelvm::virtual_of(span.end),
        offset: Some(kernelvm::KERNEL_OFFSET),
    };
    let parts = layout.parts();
    let report = kernelvm::audit(
        &PhysicalTables,
        root,
        kernelvm::UPPER_BASE,
        &Expectation {
            types,
            window: Some(&window),
            forbidden: &(0..0),
            device: &[],
            known: Some(&parts),
        },
    );
    let [text, rodata, data, stack, exception_stack] = &parts;
    let sizes = (report.text, report.read_only, report.read_write);
    let wanted = (
        size(&text.0),
        size(&rodata.0),
        size(&data.0) + size(&stack.0) + size(&exception_stack.0),
    );
    if !report.is_clean() || sizes != wanted {
        return Err(Error::NewTables("the image is not mapped by section"));
    }
    Ok(())
}

/// Replaces `TTBR1_EL1`, with the code running from the upper half: the new
/// tables map the image with the same page size, output address and
/// attributes as the old ones, so no translation in use changes (ADR 0019).
/// The regime is the same, set by `boot.s` (ADR 0033).
fn switch_upper_half(upper: u64) {
    // SAFETY: `upper` is a complete level 0 table `init` built and published,
    // mapping this very code and its stack with the translations the boot
    // tables gave them.
    unsafe {
        asm!(
            "msr ttbr1_el1, {upper}",
            "isb",
            "tlbi vmalle1",
            "dsb ish",
            "isb",
            upper = in(reg) upper,
            options(nostack)
        );
    }
}

/// Drops the lower half: translation walks through `TTBR0_EL1` are disabled
/// (`EPD0`), the register is pointed at a table with no entry, and the TLB is
/// invalidated. From the first `isb` no address below the upper half
/// translates, whatever the old tables held (ADR 0033). One block of register
/// operations, touching no memory.
fn drop_lower_half(empty: u64) {
    let disabled = read_tcr() | kernelvm::TCR_EPD0;
    // SAFETY: the code, its stack, the tables and every pointer the kernel
    // uses from here on are in the upper half, which this does not touch.
    unsafe {
        asm!(
            "msr tcr_el1, {disabled}",
            "isb",
            "msr ttbr0_el1, {empty}",
            "isb",
            "tlbi vmalle1",
            "dsb ish",
            "isb",
            disabled = in(reg) disabled,
            empty = in(reg) empty,
            options(nostack)
        );
    }
}

fn read_ttbr0() -> u64 {
    let ttbr: u64;
    // SAFETY: reading `TTBR0_EL1` has no side effects.
    unsafe { asm!("mrs {}, ttbr0_el1", out(reg) ttbr, options(nomem, nostack, preserves_flags)) };
    register_ttbr(ttbr)
}

/// The most RAM regions and hand-over addresses `verify` probes in the lower
/// half: the first page of each, up to this many.
const MAX_PROBES: usize = 64;

/// Walks the tables the processor uses now and panics unless they hold what
/// ADR 0019 and 0033 promise: no page that is writable and executable, none
/// for EL0, the image mapped by section with the sizes the linker gave them, a
/// guard page that translates to nothing, the linear map holding RAM read and
/// write with no alias of the image, the device window holding only the
/// console's and the interrupt controller's registers, nothing outside those
/// three windows, a lower half with
/// no entry and walks through it disabled, and `SCTLR_EL1.WXN` set. The walk
/// reads the live registers and the live tables, not what `init` meant to
/// build. The lower half is also asked, by hardware, about addresses that were
/// mapped before the drop: `handoff_address` is the physical address the
/// bootloader (or loader) gave in `x0`.
pub fn verify(
    mut out: impl Write,
    board: &Board,
    wiring: &InterruptWiring,
    handoff_address: usize,
) {
    let types = memory_types().unwrap_or_else(|e| panic!("mmu: {e}"));
    let layout = layout().unwrap_or_else(|e| panic!("mmu: {e}"));
    let (upper_root, lower_root) = (read_ttbr1(), read_ttbr0());
    let parts = layout.parts();
    let span = layout.span();
    let image_window = Window {
        virt: kernelvm::virtual_of(span.start)..kernelvm::virtual_of(span.end),
        offset: Some(kernelvm::KERNEL_OFFSET),
    };
    let linear_window = Window {
        virt: kernelvm::LINEAR_BASE..kernelvm::LINEAR_BASE + kernelvm::LINEAR_SPAN,
        offset: Some(kernelvm::LINEAR_BASE),
    };
    let device_window = Window {
        virt: kernelvm::DEVICE_BASE..kernelvm::DEVICE_BASE + kernelvm::DEVICE_WINDOW,
        offset: None,
    };
    let device_ranges = device_pages(board, wiring);
    let device_known = device_ranges
        .clone()
        .map(|range| (range, Permissions::Device));
    let walk = |window: &Window, forbidden: &Range<u64>, device: &[Range<u64>], known| {
        kernelvm::audit(
            &PhysicalTables,
            upper_root,
            kernelvm::UPPER_BASE,
            &Expectation {
                types,
                window: Some(window),
                forbidden,
                device,
                known,
            },
        )
    };
    let upper = walk(&image_window, &(0..0), &[], Some(&parts));
    let linear = walk(&linear_window, &span, &[], None);
    let devices = walk(&device_window, &(0..0), &device_ranges, Some(&device_known));
    let strays = kernelvm::strays(
        &PhysicalTables,
        upper_root,
        kernelvm::UPPER_BASE,
        &[
            image_window.virt.clone(),
            linear_window.virt.clone(),
            device_window.virt.clone(),
        ],
    );
    let lower = kernelvm::strays(&PhysicalTables, lower_root, 0, &[]);
    klog!(
        Info,
        out,
        "mmu: upper half at {upper_root:#x}: text {:#x} bytes, read only {:#x}, read write {:#x}",
        upper.text,
        upper.read_only,
        upper.read_write
    );
    klog!(
        Info,
        out,
        "mmu: linear map at {:#x}: {:#x} bytes of RAM, read write, never executed, the image left out",
        kernelvm::LINEAR_BASE,
        linear.read_write
    );
    klog!(
        Info,
        out,
        "mmu: device window at {:#x}: {:#x} bytes of registers, console at {:#x}, distributor at {:#x}, second block at {:#x}",
        kernelvm::DEVICE_BASE,
        devices.device,
        crate::board::console_base(),
        crate::board::interrupt_controller_bases().0,
        crate::board::interrupt_controller_bases().1
    );
    assert!(upper.is_clean(), "mmu: upper half is not clean: {upper:?}");
    assert!(
        linear.is_clean(),
        "mmu: linear map is not clean: {linear:?}"
    );
    assert!(
        devices.is_clean(),
        "mmu: device window is not clean: {devices:?}"
    );
    assert!(
        strays.0 == 0,
        "mmu: {} leaves lie outside the image, the linear map and the devices, the first at {:#x}",
        strays.0,
        strays.1.unwrap_or(0)
    );
    assert!(
        upper.text == size(&layout.text)
            && upper.read_only == size(&layout.rodata)
            && upper.read_write
                == size(&layout.data) + size(&layout.stack) + size(&layout.exception_stack)
            && upper.device == 0,
        "mmu: the upper half does not map the image by section: {upper:?}"
    );
    assert!(
        linear.text == 0 && linear.read_only == 0 && linear.device == 0,
        "mmu: the linear map holds code, constants or devices: {linear:?}"
    );
    assert!(
        devices.text == 0
            && devices.read_only == 0
            && devices.read_write == 0
            && devices.device == device_ranges.iter().map(size).sum::<u64>(),
        "mmu: the device window holds more than the console's and the interrupt controller's registers: {devices:?}"
    );
    let guard = kernelvm::virtual_of(layout.data.end);
    assert!(
        kernelvm::translate(&PhysicalTables, upper_root, guard).is_none(),
        "mmu: the guard page below the stack is mapped"
    );
    assert!(
        super::system_control() & kernelvm::SCTLR_WXN != 0,
        "mmu: SCTLR_EL1.WXN is not set"
    );
    verify_lower_half_is_empty(
        &mut out,
        board,
        handoff_address,
        &layout,
        lower_root,
        lower.0,
    );
    verify_translation_extensions(&mut out);
    klog!(
        Ok,
        out,
        "mmu: every leaf maps its own frame with its own memory type"
    );
    klog!(
        Ok,
        out,
        "mmu: write xor execute holds: no page is writable and executable, WXN is set"
    );
}

/// The lower half translates nothing (ADR 0033): walks through `TTBR0_EL1` are
/// off, its root holds no entry, and the hardware, asked about every kind of
/// address that was mapped there before (the console's registers, the image,
/// the hand-over, the first page of each region of RAM), says the translation
/// faults. The same question about a linear address of that RAM is answered,
/// so a probe that could not succeed does not pass the others.
fn verify_lower_half_is_empty(
    mut out: impl Write,
    board: &Board,
    handoff_address: usize,
    layout: &ImageLayout,
    lower_root: u64,
    leaves: u64,
) {
    assert!(
        read_tcr() & kernelvm::TCR_EPD0 != 0,
        "mmu: TCR_EPD0 is clear: the lower half is walked"
    );
    assert!(
        leaves == 0,
        "mmu: the lower half's root at {lower_root:#x} holds {leaves} leaves"
    );
    let mut probes = [0u64; MAX_PROBES];
    let mut count = 0;
    let mut add = |address: u64| {
        // More regions than probes: the rest are not probed (the report says
        // how many were), rather than stopping a boot over a diagnostic.
        if count < MAX_PROBES {
            probes[count] = address;
            count += 1;
        }
    };
    add(board.uart.base);
    add(layout.text.start);
    add(layout.data.start);
    add(handoff_address as u64);
    if board.device_tree.size > 0 {
        add(board.device_tree.base);
    }
    // The control is a page of RAM the linear map holds: the first one outside
    // the image, which the map leaves out.
    let span = layout.span();
    let mut control_page = None;
    // As in `init`: a tree that cannot be walked ended the boot before this.
    let _ = board.for_each_memory_region(|region| {
        add(region.base);
        let page = if span.contains(&region.base) {
            span.end
        } else {
            region.base
        };
        if page < region.base.saturating_add(region.size) {
            control_page.get_or_insert(page);
        }
    });
    for &address in &probes[..count] {
        assert!(
            super::memory_attributes(address as usize).is_none(),
            "mmu: {address:#x} still translates in the lower half"
        );
    }
    let control = control_page.map_or(0, pointer_to);
    assert!(
        control_page.is_some() && super::memory_attributes(control).is_some(),
        "mmu: the linear address {control:#x} of RAM does not translate: the probes prove nothing"
    );
    klog!(
        Info,
        out,
        "mmu: lower half dropped: TCR_EL1.EPD0 set, root at {lower_root:#x} holds {leaves} leaves, {count} addresses translate to nothing"
    );
}

// ---- Pages added to the live upper half (ADR 0029, 0037) -----------------

/// Maps up to `pages` pages from `at` in the live upper half, each to a frame
/// of `frames`, read and write and never executed, and returns how many it
/// mapped. It stops at the first frame or table frame it cannot get, and the
/// pages mapped before stay mapped.
///
/// The slots are empty before, so nothing is replaced: no break-before-make and
/// no TLB invalidation (an empty entry is not cached). What the walker needs is
/// the new descriptors in memory it can see, so the lines of every descriptor
/// written are cleaned and then a barrier and `isb` make the pages usable.
pub fn map_pages(frames: &mut FrameAllocator, at: u64, pages: usize) -> usize {
    // `init` read the memory types and stopped the boot if they were missing,
    // so this cannot fail on a kernel that got here; zero would be reported as
    // a heap with no frames.
    let Ok(types) = memory_types() else {
        return 0;
    };
    let root = read_ttbr1();
    let mut tables = FrameTables {
        frames,
        owned: [0; MAX_TABLES],
        count: 0,
    };
    let mut mapped = 0;
    while mapped < pages {
        let Some(frame) = tables.frames.alloc() else {
            break;
        };
        let page = at + mapped as u64 * kernelvm::PAGE_SIZE;
        let result = Mapper::resume(&mut tables, types, root).map_range(
            page,
            frame,
            kernelvm::PAGE_SIZE,
            Permissions::ReadWrite,
        );
        if result.is_err() {
            // The frame was never mapped, so giving it back is sound.
            let _ = tables.frames.free(frame);
            break;
        }
        mapped += 1;
    }
    tables.publish();
    for page in 0..mapped {
        let page = at + page as u64 * kernelvm::PAGE_SIZE;
        for descriptor in kernelvm::walked_descriptors(&PhysicalTables, root, page)
            .into_iter()
            .flatten()
        {
            clean_and_invalidate(pointer_to(descriptor) as u64);
        }
    }
    // SAFETY: barriers: the descriptors are visible to the walker before the
    // first use of the new pages.
    unsafe { asm!("dsb ish", "isb", options(nostack, preserves_flags)) };
    mapped
}

/// What the live upper half says about the page at `virtual_address`: the frame
/// it maps and its permissions (`None` for a combination the kernel never
/// builds, such as writable and executable), or nothing if it is unmapped.
pub fn upper_page(virtual_address: u64) -> Option<(u64, Option<Permissions>)> {
    let types = memory_types().ok()?;
    let leaf = kernelvm::translate(&PhysicalTables, read_ttbr1(), virtual_address)?;
    Some((leaf.output, leaf.permissions(types)))
}

// ---- Boots that must end in a fault --------------------------------------

/// Writes to the first byte of the kernel's code, as the byte it already holds
/// so a mapping that wrongly allows it changes nothing. The write must fault.
#[cfg(feature = "selftest-write-text")]
pub fn write_to_text() {
    let target = core::ptr::addr_of!(__image_start) as *mut u8;
    let mut console = crate::board::console();
    klog!(
        Info,
        console,
        "selftest: writing to kernel code at {target:p}"
    );
    // SAFETY: the address is mapped; the write is expected to fault, and if it
    // does not it stores the byte that is already there.
    unsafe { target.write_volatile(target.read_volatile()) };
    panic!("selftest: the kernel's code is writable");
}

/// Jumps to the first byte of the page `target` starts. It must fault.
#[cfg(any(feature = "selftest-execute-data", feature = "selftest-execute-rodata"))]
fn jump_to(target: *const u8, what: &str) {
    let mut console = crate::board::console();
    klog!(Info, console, "selftest: jumping to {what} at {target:p}");
    // SAFETY: the jump is expected to fault; the exception handler reports it.
    unsafe { asm!("br {}", in(reg) target) };
    panic!("selftest: {what} can be executed");
}

#[cfg(feature = "selftest-execute-data")]
pub fn execute_data() {
    jump_to(core::ptr::addr_of!(__rodata_end), ".data")
}

#[cfg(feature = "selftest-execute-rodata")]
pub fn execute_rodata() {
    jump_to(core::ptr::addr_of!(__text_end), ".rodata")
}
