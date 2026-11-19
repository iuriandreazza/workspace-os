//! The kernel's own translation tables (ADR 0019): built from frames of the
//! allocator on top of the boot tables `boot.s` installed, and switched to.
//!
//! The pure part (descriptors, permissions, mapping, auditing) is `kernelvm`,
//! tested on the host. What is here is the part that touches memory by
//! physical address and system registers.

use core::arch::asm;
use core::fmt::{self, Write};
use core::ops::Range;

use kernelvm::{
    ImageLayout, LayoutError, MapError, Mapper, MemoryTypes, Table, TableMemory, TableRead,
};
use physmem::FrameAllocator;

use super::physical;
use crate::board::Board;
use crate::log::klog;

unsafe extern "C" {
    safe static __image_start: u8;
    safe static __text_end: u8;
    safe static __rodata_end: u8;
    safe static __data_end: u8;
    safe static __stack_bottom: u8;
    safe static __stack_top: u8;
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
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MemoryTypes => f.write_str("MAIR_EL1 holds no Normal or Device memory type"),
            Self::Layout(error) => write!(f, "{error}"),
            Self::Map(error) => write!(f, "{error}"),
            Self::BootTables(report) => write!(f, "the boot tables are wrong: {report:?}"),
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

/// Tables read by physical address, through whichever map covers RAM at that
/// address: the boot map before `init` and the window it builds after.
struct PhysicalTables;

impl TableRead for PhysicalTables {
    fn table(&self, address: u64) -> &Table {
        // SAFETY: callers only pass the roots of live translation tables and
        // the addresses found in them. Those are tables in RAM that the map
        // the kernel runs on covers at the physical address: the boot tables
        // (in the image, which only the boot map and the upper half reach, so
        // they are read before `switch`) or the tables `init` built from
        // frames (in the lower window). Nothing writes while they are read.
        unsafe { &*(address as usize as *const Table) }
    }
}

/// The most tables `init` builds: the two roots, a level 2 and a level 3 table
/// for the image, and, per region of RAM, up to two level 2 and two level 3
/// tables at its edges (whole gigabytes are blocks in the root). The boot map
/// of a firmware has a few dozen regions, so this is generous; more regions
/// fail with `OutOfFrames`.
const MAX_TABLES: usize = 512;

/// Tables taken from the frame allocator, which the kernel reaches at their
/// physical address.
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
            unsafe { asm!("dc ivac, {}", in(reg) line, options(nostack, preserves_flags)) };
        }
        // SAFETY: the frame was just handed out, so nothing else uses it. The
        // map the kernel runs on covers RAM at its physical address, except
        // the free part of the image's own 2 MiB on the direct path before the
        // switch; the allocator hands out the lowest frames first, which lie
        // below the image on the machines this runs on (a limit, ADR 0019).
        unsafe { core::ptr::write(address as usize as *mut Table, Table::EMPTY) };
        Some(address)
    }

    fn table_mut(&mut self, address: u64) -> &mut Table {
        // SAFETY: the table is one of those `allocate` handed out, so this is
        // the only reference to it while the mapper holds `&mut self`.
        unsafe { &mut *(address as usize as *mut Table) }
    }
}

impl FrameTables<'_, '_> {
    /// Makes the tables visible to the walker: clean and invalidate to the point
    /// of coherency by address, then wait for it.
    fn publish(&self) {
        let line_size = data_cache_line_size();
        for &table in &self.owned[..self.count] {
            for line in (table..table + kernelvm::PAGE_SIZE).step_by(line_size) {
                // SAFETY: cleaning and invalidating a line of RAM the kernel
                // owns changes no data. The invalidate matters when the
                // walker's attributes are cacheable and the kernel's own
                // writes were not (the direct path runs with `C` clear), so
                // a stale line must not survive.
                unsafe { asm!("dc civac, {}", in(reg) line, options(nostack, preserves_flags)) };
            }
        }
        // SAFETY: a barrier.
        unsafe { asm!("dsb ish", options(nostack, preserves_flags)) };
    }
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
/// cannot test, give the image the permissions `ImageLayout` does.
fn check_boot_tables(layout: &ImageLayout, types: MemoryTypes) -> Result<(), Error> {
    let report = kernelvm::audit(
        &PhysicalTables,
        read_ttbr1(),
        kernelvm::KERNEL_OFFSET,
        types,
        &(0..0),
    );
    let expected = (
        size(&layout.text),
        size(&layout.rodata),
        size(&layout.data) + size(&layout.stack),
    );
    let found = (report.text, report.read_only, report.read_write);
    if !report.is_clean() || report.device != 0 || found != expected {
        return Err(Error::BootTables(report));
    }
    Ok(())
}

/// Builds the kernel's tables from frames of `frames` and switches to them.
///
/// The upper half maps the image as the boot tables do, by section, so that
/// switching `TTBR1_EL1` changes no translation of the code running; the lower
/// half becomes a window onto RAM and the UART, without the image, which the
/// kernel's pointers to the boot info, the device tree and its own frames
/// need until they become virtual (ADR 0019).
pub fn init(frames: &mut FrameAllocator, board: &Board) -> Result<(), Error> {
    let types = memory_types()?;
    let layout = layout()?;
    check_boot_tables(&layout, types)?;

    let mut tables = FrameTables {
        frames,
        owned: [0; MAX_TABLES],
        count: 0,
    };
    let upper = {
        let mut mapper = Mapper::new(&mut tables, types)?;
        layout.map(&mut mapper, kernelvm::KERNEL_OFFSET)?;
        mapper.root()
    };
    let hole = layout.span();
    let lower = {
        let mut mapper = Mapper::new(&mut tables, types)?;
        let mut result = Ok(());
        let regions = board.for_each_memory_region(|region| {
            if result.is_ok() {
                result = kernelvm::map_ram_region(
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
        kernelvm::map_device(&mut mapper, board.uart.base, board.uart.size)?;
        mapper.root()
    };
    tables.publish();
    switch(lower, upper);
    Ok(())
}

/// Replaces `TTBR0_EL1` and `TTBR1_EL1`.
///
/// The lower half changes shape (bootloader's map, or the boot code's blocks,
/// for the kernel's), so it is torn down and made: translation walks through
/// `TTBR0_EL1` are disabled (`EPD0`), the register is written, the TLB is
/// invalidated, and walks are enabled again, with the lower half of `TCR_EL1`
/// the kernel's. No memory outside the upper half is touched in between, which
/// is why this is one block of register operations.
///
/// The upper half is switched with the code running from it: the new tables
/// map the image with the same page size, output address and attributes as
/// the old ones, so no translation in use changes (ADR 0019).
fn switch(lower: u64, upper: u64) {
    let current = read_tcr();
    let enabled = (current & !kernelvm::TCR_LOWER_HALF_MASK) | kernelvm::TCR_LOWER_HALF;
    let disabled = enabled | kernelvm::TCR_EPD0;
    // SAFETY: both roots are complete tables `init` built and published, the
    // upper half maps this very code and its stack with the translations the
    // boot tables gave them, and nothing between the first and the last
    // instruction touches memory the lower half maps.
    unsafe {
        asm!(
            "msr tcr_el1, {disabled}",
            "isb",
            "msr ttbr0_el1, {lower}",
            "isb",
            "tlbi vmalle1",
            "dsb ish",
            "isb",
            "msr tcr_el1, {enabled}",
            "isb",
            "msr ttbr1_el1, {upper}",
            "isb",
            "tlbi vmalle1",
            "dsb ish",
            "isb",
            disabled = in(reg) disabled,
            enabled = in(reg) enabled,
            lower = in(reg) lower,
            upper = in(reg) upper,
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

/// Walks the tables the processor uses now and panics unless they hold what
/// ADR 0019 promises: no page that is writable and executable, none for EL0,
/// the image mapped by section with the sizes the linker gave them, a guard
/// page that translates to nothing, no alias of the image in the lower half,
/// and `SCTLR_EL1.WXN` set. The walk reads the live registers and the live
/// tables, not what `init` meant to build.
pub fn verify(mut out: impl Write) {
    let types = memory_types().unwrap_or_else(|e| panic!("mmu: {e}"));
    let layout = layout().unwrap_or_else(|e| panic!("mmu: {e}"));
    let (upper_root, lower_root) = (read_ttbr1(), read_ttbr0());
    let upper = kernelvm::audit(
        &PhysicalTables,
        upper_root,
        kernelvm::KERNEL_OFFSET,
        types,
        &(0..0),
    );
    let lower = kernelvm::audit(&PhysicalTables, lower_root, 0, types, &layout.span());
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
        "mmu: lower half at {lower_root:#x}: read write {:#x} bytes, device {:#x}, text {:#x}",
        lower.read_write,
        lower.device,
        lower.text
    );
    assert!(upper.is_clean(), "mmu: upper half is not clean: {upper:?}");
    assert!(lower.is_clean(), "mmu: lower half is not clean: {lower:?}");
    assert!(
        upper.text == size(&layout.text)
            && upper.read_only == size(&layout.rodata)
            && upper.read_write == size(&layout.data) + size(&layout.stack)
            && upper.device == 0,
        "mmu: the upper half does not map the image by section: {upper:?}"
    );
    assert!(
        lower.text == 0 && lower.read_only == 0,
        "mmu: the lower half holds code or constants: {lower:?}"
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
    klog!(
        Ok,
        out,
        "mmu: write xor execute holds: no page is writable and executable, WXN is set"
    );
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
