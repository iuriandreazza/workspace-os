//! The kernel's translation tables (ADR 0019): descriptors, the permissions of
//! each kind of memory, mapping, walking and auditing.
//!
//! Plain computation on tables the caller provides through [`TableMemory`], so
//! the host can build the maps the kernel builds and walk them. The unsafe part
//! (frames, system registers) stays in the kernel.
//!
//! Layout: 4 KiB granule, 39-bit address spaces in both halves
//! (`T0SZ = T1SZ = 25`), so every root is a level 1 table. Leaves are 4 KiB
//! pages and 2 MiB and 1 GiB blocks.

#![cfg_attr(not(test), no_std)]

use core::ops::Range;

pub const PAGE_SIZE: u64 = 4096;
pub const BLOCK_SIZE: u64 = 2 << 20;
const GIGABYTE: u64 = 1 << 30;
const ENTRIES: usize = 512;
/// Bits of virtual address each half translates.
pub const ADDRESS_BITS: u32 = 39;

/// The virtual address of the kernel image is its physical address plus this.
/// It is the start of the upper half, and `link.ld` defines the same number
/// (the boot smoke test compares them).
pub const KERNEL_OFFSET: u64 = 0xffff_ff80_0000_0000;

pub const fn virtual_of(physical: u64) -> u64 {
    physical.wrapping_add(KERNEL_OFFSET)
}

pub const fn physical_of(virtual_address: u64) -> u64 {
    virtual_address.wrapping_sub(KERNEL_OFFSET)
}

/// One translation table: 512 descriptors, aligned on its own size.
#[repr(C, align(4096))]
#[derive(Clone, Copy)]
pub struct Table(pub [u64; ENTRIES]);

impl Table {
    pub const EMPTY: Table = Table([0; ENTRIES]);
}

/// Where the tables live. `address` is physical; the kernel reaches it through
/// the map it runs on, the host through an arena.
pub trait TableRead {
    fn table(&self, address: u64) -> &Table;
}

pub trait TableMemory: TableRead {
    /// An empty table, or `None` when no frame is left.
    fn allocate(&mut self) -> Option<u64>;
    fn table_mut(&mut self, address: u64) -> &mut Table;
}

// ---- Descriptors ---------------------------------------------------------

pub const VALID: u64 = 1;
pub const TABLE_OR_PAGE: u64 = 2;
const ATTRIBUTE_INDEX_SHIFT: u32 = 2;
const ATTRIBUTE_INDEX_MASK: u64 = 7 << ATTRIBUTE_INDEX_SHIFT;
/// AP[1]: accessible from EL0. The kernel has no user space yet.
const USER_ACCESSIBLE: u64 = 1 << 6;
/// AP[2]: read only.
pub const READ_ONLY: u64 = 1 << 7;
pub const INNER_SHAREABLE: u64 = 3 << 8;
pub const OUTER_SHAREABLE: u64 = 2 << 8;
pub const ACCESS_FLAG: u64 = 1 << 10;
pub const PRIVILEGED_EXECUTE_NEVER: u64 = 1 << 53;
pub const UNPRIVILEGED_EXECUTE_NEVER: u64 = 1 << 54;

// The pieces of a descriptor that `boot.s` assembles by hand, with an output
// address and `AttrIndx` added: the tests check them against `leaf`.
/// An entry that points to a table.
pub const TABLE_BITS: u64 = VALID | TABLE_OR_PAGE;
/// A Normal page the kernel can read, with execute allowed: text once
/// `READ_ONLY` is added, and the base of the other kinds.
pub const NORMAL_PAGE_BITS: u64 =
    TABLE_BITS | INNER_SHAREABLE | ACCESS_FLAG | UNPRIVILEGED_EXECUTE_NEVER;
/// A Normal block, read and write, never executed.
pub const NORMAL_BLOCK_BITS: u64 =
    VALID | INNER_SHAREABLE | ACCESS_FLAG | UNPRIVILEGED_EXECUTE_NEVER | PRIVILEGED_EXECUTE_NEVER;
/// A Device block, read and write, never executed.
pub const DEVICE_BLOCK_BITS: u64 =
    VALID | OUTER_SHAREABLE | ACCESS_FLAG | UNPRIVILEGED_EXECUTE_NEVER | PRIVILEGED_EXECUTE_NEVER;
const ADDRESS_MASK: u64 = 0x0000_ffff_ffff_f000;
const BLOCK_2M_MASK: u64 = 0x0000_ffff_ffe0_0000;
const BLOCK_1G_MASK: u64 = 0x0000_ffff_c000_0000;

/// What a piece of memory may be used for. No variant is both writable and
/// executable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Permissions {
    /// Kernel code: read and execute.
    Text,
    /// Constants: read only, never executed.
    ReadOnly,
    /// Data, `.bss` and stacks: read and write, never executed.
    ReadWrite,
    /// Device registers: read and write, never executed, no caching.
    Device,
}

/// The indexes into `MAIR_EL1` of the two memory types the kernel uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MemoryTypes {
    pub normal_index: u8,
    pub device_index: u8,
}

/// `MAIR_EL1` the kernel installs itself when it enters with the MMU off:
/// Device-nGnRnE at index 0 and Normal write-back at index 1.
pub const DIRECT_ENTRY_MAIR: u64 = 0xff00;

impl MemoryTypes {
    /// The indexes in `mair` that hold Normal write-back memory (`0xff`) and
    /// Device-nGnRnE (`0x00`), or `None` if either is missing. The bootloader
    /// leaves `MAIR_EL1` as the firmware had it (ADR 0013), so the kernel finds
    /// the indexes instead of assuming them.
    pub fn from_mair(mair: u64) -> Option<Self> {
        let find = |wanted: u8| (0..8u8).find(|i| (mair >> (8 * i)) as u8 == wanted);
        Some(Self {
            normal_index: find(0xff)?,
            device_index: find(0x00)?,
        })
    }

    fn attribute_bits(&self, permissions: Permissions) -> u64 {
        let (index, shareability) = match permissions {
            Permissions::Device => (self.device_index, OUTER_SHAREABLE),
            _ => (self.normal_index, INNER_SHAREABLE),
        };
        (u64::from(index) << ATTRIBUTE_INDEX_SHIFT) | shareability
    }
}

/// A leaf descriptor: a page at level 3, a block at level 2 or 1.
fn leaf(types: &MemoryTypes, output: u64, permissions: Permissions, level: u8) -> u64 {
    let shape = if level == 3 {
        VALID | TABLE_OR_PAGE
    } else {
        VALID
    };
    let (access, privileged_execute_never) = match permissions {
        Permissions::Text => (READ_ONLY, 0),
        Permissions::ReadOnly => (READ_ONLY, PRIVILEGED_EXECUTE_NEVER),
        Permissions::ReadWrite | Permissions::Device => (0, PRIVILEGED_EXECUTE_NEVER),
    };
    let mask = match level {
        1 => BLOCK_1G_MASK,
        2 => BLOCK_2M_MASK,
        _ => ADDRESS_MASK,
    };
    (output & mask)
        | shape
        | types.attribute_bits(permissions)
        | access
        | ACCESS_FLAG
        | privileged_execute_never
        | UNPRIVILEGED_EXECUTE_NEVER
}

fn table_descriptor(address: u64) -> u64 {
    (address & ADDRESS_MASK) | VALID | TABLE_OR_PAGE
}

// ---- Registers -----------------------------------------------------------

/// `TCR_EL1` fields for the lower half (`TTBR0_EL1`): 39 bits, 4 KiB granule,
/// write-back write-allocate, inner shareable, walks enabled.
pub const TCR_LOWER_HALF: u64 = 25 | (1 << 8) | (1 << 10) | (3 << 12);
/// The same for the upper half (`TTBR1_EL1`); `TG1` encodes 4 KiB as `0b10`.
pub const TCR_UPPER_HALF: u64 = (25 << 16) | (1 << 24) | (1 << 26) | (3 << 28) | (2 << 30);
/// `EPD0`: translation walks through `TTBR0_EL1` are disabled.
pub const TCR_EPD0: u64 = 1 << 7;
/// `EPD1`: the same for `TTBR1_EL1`.
pub const TCR_EPD1: u64 = 1 << 23;
/// Bits of `TCR_EL1` that describe the lower half, including `EPD0`.
pub const TCR_LOWER_HALF_MASK: u64 = 0xffff;
/// Bits of `TCR_EL1` that describe the upper half, including `A1`, `EPD1`.
pub const TCR_UPPER_HALF_MASK: u64 = 0xffff_0000;
/// Both halves, the value the direct path writes (plus `IPS`).
pub const TCR_BOTH_HALVES: u64 = TCR_LOWER_HALF | TCR_UPPER_HALF;

/// `SCTLR_EL1.WXN`: memory that is writable is never executable, whatever its
/// descriptor says.
pub const SCTLR_WXN: u64 = 1 << 19;

// ---- Mapping -------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MapError {
    /// No frame left for a table.
    OutOfFrames,
    /// An address is not in either half of a 39-bit regime.
    NotCanonical,
    /// An address or a length is not a multiple of the page size.
    Unaligned,
    /// Part of the range is already mapped.
    Overlap,
}

impl core::fmt::Display for MapError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            Self::OutOfFrames => "no frame left for a page table",
            Self::NotCanonical => "address outside the 39-bit translation regime",
            Self::Unaligned => "address or length is not page aligned",
            Self::Overlap => "range overlaps memory that is already mapped",
        })
    }
}

fn level_index(address: u64, level: u8) -> usize {
    let shift = 12 + 9 * (3 - u32::from(level));
    ((address >> shift) & 0x1ff) as usize
}

/// Bits 63:39 equal: the address is in the lower or the upper half.
fn is_canonical(address: u64) -> bool {
    let top = address >> ADDRESS_BITS;
    top == 0 || top == u64::MAX >> ADDRESS_BITS
}

/// A root table and the means to add to it.
pub struct Mapper<'a, M: TableMemory> {
    memory: &'a mut M,
    types: MemoryTypes,
    root: u64,
}

impl<'a, M: TableMemory> Mapper<'a, M> {
    pub fn new(memory: &'a mut M, types: MemoryTypes) -> Result<Self, MapError> {
        let root = memory.allocate().ok_or(MapError::OutOfFrames)?;
        Ok(Self {
            memory,
            types,
            root,
        })
    }

    /// The physical address of the level 1 table, the value for `TTBRn_EL1`.
    pub fn root(&self) -> u64 {
        self.root
    }

    /// The table `table[index]` points to, made empty if there is none yet.
    fn child(&mut self, table: u64, index: usize) -> Result<u64, MapError> {
        let descriptor = self.memory.table(table).0[index];
        if descriptor == 0 {
            let child = self.memory.allocate().ok_or(MapError::OutOfFrames)?;
            self.memory.table_mut(table).0[index] = table_descriptor(child);
            return Ok(child);
        }
        if descriptor & 3 == 3 {
            return Ok(descriptor & ADDRESS_MASK);
        }
        Err(MapError::Overlap)
    }

    fn set_leaf(
        &mut self,
        virtual_address: u64,
        physical: u64,
        permissions: Permissions,
        level: u8,
    ) -> Result<(), MapError> {
        let mut table = self.root;
        for walked in 1..level {
            table = self.child(table, level_index(virtual_address, walked))?;
        }
        let slot = &mut self.memory.table_mut(table).0[level_index(virtual_address, level)];
        let wanted = leaf(&self.types, physical, permissions, level);
        // Mapping the same thing twice is not a conflict: two regions of a
        // device tree may share a page once widened to whole pages.
        if *slot != 0 && *slot != wanted {
            return Err(MapError::Overlap);
        }
        *slot = wanted;
        Ok(())
    }

    /// Maps `length` bytes at `virtual_address` to `physical`, with 1 GiB
    /// and 2 MiB blocks wherever both addresses and the rest of the length allow it and
    /// pages elsewhere. Nothing already mapped may be touched: break before
    /// make is the caller's business.
    pub fn map_range(
        &mut self,
        virtual_address: u64,
        physical: u64,
        length: u64,
        permissions: Permissions,
    ) -> Result<(), MapError> {
        if !(virtual_address | physical | length).is_multiple_of(PAGE_SIZE) {
            return Err(MapError::Unaligned);
        }
        if length == 0 {
            return Ok(());
        }
        let last = virtual_address
            .checked_add(length - 1)
            .ok_or(MapError::NotCanonical)?;
        if !is_canonical(virtual_address) || !is_canonical(last) {
            return Err(MapError::NotCanonical);
        }
        self.map_with_blocks_up_to(virtual_address, physical, length, permissions, GIGABYTE)
    }

    /// `map_range` with blocks no bigger than `largest`. Where a block meets a
    /// table that is already there (an earlier range mapped part of it with
    /// smaller pieces), the block is mapped with smaller pieces too, which
    /// accepts what is identical and refuses what is not.
    fn map_with_blocks_up_to(
        &mut self,
        virtual_address: u64,
        physical: u64,
        length: u64,
        permissions: Permissions,
        largest: u64,
    ) -> Result<(), MapError> {
        let mut done = 0;
        while done < length {
            let (at, to) = (virtual_address + done, physical + done);
            let size = [GIGABYTE, BLOCK_SIZE]
                .into_iter()
                .find(|&size| {
                    size <= largest
                        && at.is_multiple_of(size)
                        && to.is_multiple_of(size)
                        && length - done >= size
                })
                .unwrap_or(PAGE_SIZE);
            let level = match size {
                GIGABYTE => 1,
                BLOCK_SIZE => 2,
                _ => 3,
            };
            match self.set_leaf(at, to, permissions, level) {
                Err(MapError::Overlap) if size > PAGE_SIZE && self.is_table_at(at, level) => {
                    let smaller = if size == GIGABYTE {
                        BLOCK_SIZE
                    } else {
                        PAGE_SIZE
                    };
                    self.map_with_blocks_up_to(at, to, size, permissions, smaller)?;
                }
                other => other?,
            }
            done += size;
        }
        Ok(())
    }

    /// Whether the entry for `virtual_address` at `level` is a table.
    fn is_table_at(&self, virtual_address: u64, level: u8) -> bool {
        let mut table = self.root;
        for walked in 1..level {
            let descriptor = self.memory.table(table).0[level_index(virtual_address, walked)];
            if descriptor & 3 != 3 {
                return false;
            }
            table = descriptor & ADDRESS_MASK;
        }
        self.memory.table(table).0[level_index(virtual_address, level)] & 3 == 3
    }
}

// ---- Reading -------------------------------------------------------------

/// What a leaf descriptor allows, read back from its bits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Leaf {
    pub output: u64,
    pub writable: bool,
    /// Executable at EL1 by the descriptor alone: `PXN` clear.
    pub executable: bool,
    pub user_accessible: bool,
    pub user_executable: bool,
    pub attribute_index: u8,
}

impl Leaf {
    fn decode(descriptor: u64, level: u8) -> Self {
        let mask = match level {
            1 => BLOCK_1G_MASK,
            2 => BLOCK_2M_MASK,
            _ => ADDRESS_MASK,
        };
        Self {
            output: descriptor & mask,
            writable: descriptor & READ_ONLY == 0,
            executable: descriptor & PRIVILEGED_EXECUTE_NEVER == 0,
            user_accessible: descriptor & USER_ACCESSIBLE != 0,
            user_executable: descriptor & UNPRIVILEGED_EXECUTE_NEVER == 0,
            attribute_index: ((descriptor & ATTRIBUTE_INDEX_MASK) >> ATTRIBUTE_INDEX_SHIFT) as u8,
        }
    }
}

fn span(level: u8) -> u64 {
    match level {
        1 => GIGABYTE,
        2 => BLOCK_SIZE,
        _ => PAGE_SIZE,
    }
}

/// The leaf that translates `virtual_address` under `root`, with the output
/// address of that very byte, or `None` if the translation faults.
pub fn translate<M: TableRead>(memory: &M, root: u64, virtual_address: u64) -> Option<Leaf> {
    if !is_canonical(virtual_address) {
        return None;
    }
    let mut table = root;
    for level in 1..=3u8 {
        let descriptor = memory.table(table).0[level_index(virtual_address, level)];
        match (descriptor & 3, level) {
            (3, 1 | 2) => table = descriptor & ADDRESS_MASK,
            (3, 3) | (1, 1 | 2) => {
                let mut found = Leaf::decode(descriptor, level);
                found.output |= virtual_address & (span(level) - 1);
                return Some(found);
            }
            _ => return None,
        }
    }
    None
}

/// What a walk over every translation of a root found.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Audit {
    /// Bytes mapped, by what the descriptor allows.
    pub text: u64,
    pub read_only: u64,
    pub read_write: u64,
    pub device: u64,
    /// The first virtual address of a leaf that can be written and executed.
    pub writable_executable: Option<u64>,
    /// The first virtual address of a leaf EL0 can reach or execute.
    pub user_reachable: Option<u64>,
    /// The first virtual address whose output lies in the forbidden range.
    pub aliased: Option<u64>,
    /// Entries that are neither empty, a table, a block nor a page.
    pub malformed: u64,
}

impl Audit {
    /// Nothing is writable and executable, nothing is for EL0, nothing aliases
    /// the forbidden range and every entry is well formed.
    pub fn is_clean(&self) -> bool {
        self.writable_executable.is_none()
            && self.user_reachable.is_none()
            && self.aliased.is_none()
            && self.malformed == 0
    }
}

struct Walk<'a> {
    types: MemoryTypes,
    forbidden: &'a Range<u64>,
    audit: Audit,
}

impl Walk<'_> {
    fn leaf(&mut self, descriptor: u64, level: u8, at: u64) {
        let found = Leaf::decode(descriptor, level);
        let size = span(level);
        match (found.writable, found.executable) {
            (true, true) => {
                self.audit.writable_executable.get_or_insert(at);
            }
            (false, true) => self.audit.text += size,
            (false, false) => self.audit.read_only += size,
            (true, false) if found.attribute_index == self.types.device_index => {
                self.audit.device += size;
            }
            (true, false) => self.audit.read_write += size,
        }
        if found.user_accessible || found.user_executable {
            self.audit.user_reachable.get_or_insert(at);
        }
        if found.output < self.forbidden.end && self.forbidden.start < found.output + size {
            self.audit.aliased.get_or_insert(at);
        }
    }

    fn table<M: TableRead>(&mut self, memory: &M, table: u64, level: u8, base: u64) {
        for (index, &descriptor) in memory.table(table).0.iter().enumerate() {
            let at = base + index as u64 * span(level);
            match (descriptor & 3, level) {
                (0, _) => {}
                (3, 1 | 2) => self.table(memory, descriptor & ADDRESS_MASK, level + 1, at),
                (3, 3) | (1, 1 | 2) => self.leaf(descriptor, level, at),
                _ => self.audit.malformed += 1,
            }
        }
    }
}

/// Walks every entry under `root`, whose lowest address is `base` (0 for the
/// lower half, [`KERNEL_OFFSET`] for the upper one). `forbidden` is a range of
/// physical addresses nothing under this root may map.
pub fn audit<M: TableRead>(
    memory: &M,
    root: u64,
    base: u64,
    types: MemoryTypes,
    forbidden: &Range<u64>,
) -> Audit {
    let mut walk = Walk {
        types,
        forbidden,
        audit: Audit::default(),
    };
    walk.table(memory, root, 1, base);
    walk.audit
}

// ---- The kernel's maps ---------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayoutError {
    /// A boundary is not a multiple of the page size.
    Unaligned,
    /// The sections overlap or are out of order.
    Misordered,
    /// The stack does not start a page above the data.
    NoGuardPage,
}

impl core::fmt::Display for LayoutError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            Self::Unaligned => "a kernel section does not start and end on a page",
            Self::Misordered => "kernel sections overlap or are out of order",
            Self::NoGuardPage => "no unmapped page between the data and the stack",
        })
    }
}

/// Where the linker put each part of the image, as physical addresses, each on
/// a page boundary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImageLayout {
    pub text: Range<u64>,
    pub rodata: Range<u64>,
    /// `.data` and `.bss`.
    pub data: Range<u64>,
    pub stack: Range<u64>,
}

impl ImageLayout {
    pub fn new(
        text: Range<u64>,
        rodata: Range<u64>,
        data: Range<u64>,
        stack: Range<u64>,
    ) -> Result<Self, LayoutError> {
        let layout = Self {
            text,
            rodata,
            data,
            stack,
        };
        let boundaries = [
            layout.text.start,
            layout.text.end,
            layout.rodata.start,
            layout.rodata.end,
            layout.data.start,
            layout.data.end,
            layout.stack.start,
            layout.stack.end,
        ];
        if !boundaries.iter().all(|b| b.is_multiple_of(PAGE_SIZE)) {
            return Err(LayoutError::Unaligned);
        }
        if boundaries.windows(2).any(|pair| pair[0] > pair[1]) || layout.text.is_empty() {
            return Err(LayoutError::Misordered);
        }
        if layout.stack.start < layout.data.end + PAGE_SIZE {
            return Err(LayoutError::NoGuardPage);
        }
        Ok(layout)
    }

    /// Everything the image occupies, the guard page included.
    pub fn span(&self) -> Range<u64> {
        self.text.start..self.stack.end
    }

    fn parts(&self) -> [(&Range<u64>, Permissions); 4] {
        [
            (&self.text, Permissions::Text),
            (&self.rodata, Permissions::ReadOnly),
            (&self.data, Permissions::ReadWrite),
            (&self.stack, Permissions::ReadWrite),
        ]
    }

    /// Maps each part to its physical address plus `offset`, with its own
    /// permissions. The guard page between the data and the stack stays empty.
    pub fn map<M: TableMemory>(
        &self,
        mapper: &mut Mapper<'_, M>,
        offset: u64,
    ) -> Result<(), MapError> {
        for (range, permissions) in self.parts() {
            mapper.map_range(
                range.start.wrapping_add(offset),
                range.start,
                range.end - range.start,
                permissions,
            )?;
        }
        Ok(())
    }
}

/// Maps one region of RAM to itself, read and write, never executed, except
/// for what lies in `hole` (the kernel image, which the upper half maps with
/// its own permissions and no alias may make writable). The region is widened
/// to whole pages.
pub fn map_ram_region<M: TableMemory>(
    mapper: &mut Mapper<'_, M>,
    region: Range<u64>,
    hole: &Range<u64>,
) -> Result<(), MapError> {
    if region.is_empty() {
        return Ok(());
    }
    let start = region.start & !(PAGE_SIZE - 1);
    let end = region
        .end
        .checked_add(PAGE_SIZE - 1)
        .ok_or(MapError::NotCanonical)?
        & !(PAGE_SIZE - 1);
    let below = start..end.min(hole.start);
    let above = start.max(hole.end)..end;
    for part in [below, above] {
        if part.start < part.end {
            mapper.map_range(
                part.start,
                part.start,
                part.end - part.start,
                Permissions::ReadWrite,
            )?;
        }
    }
    Ok(())
}

/// Maps the pages that hold `length` bytes of registers at `base` to
/// themselves as Device memory.
pub fn map_device<M: TableMemory>(
    mapper: &mut Mapper<'_, M>,
    base: u64,
    length: u64,
) -> Result<(), MapError> {
    let start = base & !(PAGE_SIZE - 1);
    let end = base
        .checked_add(length)
        .and_then(|end| end.checked_add(PAGE_SIZE - 1))
        .ok_or(MapError::NotCanonical)?
        & !(PAGE_SIZE - 1);
    mapper.map_range(start, start, end - start, Permissions::Device)
}

#[cfg(test)]
mod tests;
