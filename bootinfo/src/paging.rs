//! The identity map the kernel starts on (ADR 0013, "Processor state at
//! entry, aarch64"): every address maps to itself, memory the map describes as
//! RAM is Normal write-back cacheable and everything else is Device, with
//! execute never.
//!
//! Plain computation on a pool of tables, so the host can walk the result and
//! check it. The tables are addressed by the pool's own addresses: the
//! bootloader runs identity mapped, so a table's address is its physical one.
//!
//! Layout: 4 KiB granule. Everything below 512 GiB starts as 1 GiB Device
//! blocks; the regions that are memory split those down to 2 MiB blocks and,
//! at the edges, 4 KiB pages. Where the regime has more than 39 bits the root
//! is a level 0 table, and every further 512 GiB that holds memory gets a level
//! 1 table of its own, made when the first region reaches it, likewise Device
//! blocks first (ADR 0030): RAM above 512 GiB is described too, because the
//! kernel reads what the bootloader put there through this map. The memory-type indexes are not chosen here: the
//! caller passes the ones the live `MAIR_EL1` already holds, so the map can be
//! installed without touching `MAIR_EL1` or `TCR_EL1`.

use crate::MemoryEntry;

pub const PAGE_SIZE: u64 = 4096;
const BLOCK_2M: u64 = 2 << 20;
const BLOCK_1G: u64 = 1 << 30;
/// What one level 1 table covers: 512 GiB.
const SPAN: u64 = 512 * BLOCK_1G;
const ENTRIES: usize = 512;

/// One translation table: 512 descriptors, aligned on its own size.
#[repr(C, align(4096))]
#[derive(Clone, Copy)]
pub struct Table(pub [u64; ENTRIES]);

impl Table {
    pub const EMPTY: Table = Table([0; ENTRIES]);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageError {
    /// The pool holds fewer tables than the map needs.
    PoolExhausted,
    /// An address is beyond what the translation regime covers.
    OutOfRange,
    /// An entry overlaps one that is already mapped.
    Overlap,
    /// The translation regime has fewer than 31 or more than 48 address bits.
    UnsupportedAddressBits,
}

impl core::fmt::Display for PageError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            Self::PoolExhausted => "ran out of page table memory",
            Self::OutOfRange => "memory lies beyond the addresses the translation regime covers",
            Self::Overlap => "memory map entries overlap in the page tables",
            Self::UnsupportedAddressBits => "unsupported translation address width",
        })
    }
}

/// Memory type indexes into `MAIR_EL1` and the width of the address space the
/// live `TCR_EL1` translates (`64 - T0SZ`).
#[derive(Clone, Copy, Debug)]
pub struct TableAttributes {
    pub normal_index: u8,
    pub device_index: u8,
    pub address_bits: u8,
}

impl TableAttributes {
    /// The indexes in `mair` that hold Normal write-back memory (`0xff`) and
    /// Device-nGnRnE (`0x00`), or `None` if either is missing.
    pub fn from_mair(mair: u64, address_bits: u8) -> Option<Self> {
        let find = |wanted: u8| (0..8u8).find(|i| (mair >> (8 * i)) as u8 == wanted);
        Some(Self {
            normal_index: find(0xff)?,
            device_index: find(0x00)?,
            address_bits,
        })
    }
}

const VALID: u64 = 1;
const TABLE_OR_PAGE: u64 = 2;
const ACCESS_FLAG: u64 = 1 << 10;
const PRIVILEGED_EXECUTE_NEVER: u64 = 1 << 53;
const UNPRIVILEGED_EXECUTE_NEVER: u64 = 1 << 54;
const INNER_SHAREABLE: u64 = 3 << 8;
const OUTER_SHAREABLE: u64 = 2 << 8;
const ADDRESS_MASK: u64 = 0x0000_ffff_ffff_f000;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Type {
    Normal,
    Device,
}

impl TableAttributes {
    fn leaf(&self, kind: Type, address: u64, level: u8) -> u64 {
        // Level 3 entries are "pages" (bits 1:0 = 0b11), blocks above are 0b01.
        let shape = if level == 3 {
            VALID | TABLE_OR_PAGE
        } else {
            VALID
        };
        let (index, shareability, execute_never) = match kind {
            Type::Normal => (self.normal_index, INNER_SHAREABLE, 0),
            Type::Device => (
                self.device_index,
                OUTER_SHAREABLE,
                PRIVILEGED_EXECUTE_NEVER | UNPRIVILEGED_EXECUTE_NEVER,
            ),
        };
        (address & ADDRESS_MASK_FOR_LEAF[level as usize])
            | shape
            | (u64::from(index) << 2)
            | shareability
            | ACCESS_FLAG
            | execute_never
    }
}

/// Output address bits of a leaf descriptor, by level: a 1 GiB block keeps
/// bits 47:30, a 2 MiB block 47:21, a page 47:12.
const ADDRESS_MASK_FOR_LEAF: [u64; 4] = [
    0,
    0x0000_ffff_c000_0000,
    0x0000_ffff_ffe0_0000,
    ADDRESS_MASK,
];

fn is_table(descriptor: u64) -> bool {
    descriptor & 3 == 3
}

fn is_leaf_block(descriptor: u64) -> bool {
    descriptor & 3 == 1
}

struct Builder<'a> {
    pool: &'a mut [Table],
    used: usize,
    attributes: TableAttributes,
    /// Index in the pool of the root table.
    root: usize,
    /// Whether the root is a level 0 table over level 1 tables, one per 512 GiB.
    /// Otherwise the root is the one level 1 table.
    root_is_level0: bool,
}

impl Builder<'_> {
    fn pool_base(&self) -> u64 {
        self.pool.as_ptr() as u64
    }

    fn address_of(&self, table: usize) -> u64 {
        self.pool_base() + (table as u64) * PAGE_SIZE
    }

    fn index_of(&self, descriptor: u64) -> usize {
        (((descriptor & ADDRESS_MASK) - self.pool_base()) / PAGE_SIZE) as usize
    }

    fn alloc(&mut self) -> Result<usize, PageError> {
        let table = self.used;
        *self.pool.get_mut(table).ok_or(PageError::PoolExhausted)? = Table::EMPTY;
        self.used += 1;
        Ok(table)
    }

    /// The level 1 table that covers the 512 GiB `span` (0 for the first),
    /// made with 1 GiB Device blocks if no region has reached it yet.
    fn level1(&mut self, span: usize) -> Result<usize, PageError> {
        if !self.root_is_level0 {
            return if span == 0 {
                Ok(self.root)
            } else {
                Err(PageError::OutOfRange)
            };
        }
        let descriptor = self.pool[self.root].0[span];
        if is_table(descriptor) {
            return Ok(self.index_of(descriptor));
        }
        let table = self.alloc()?;
        let base = span as u64 * SPAN;
        for gigabyte in 0..ENTRIES {
            self.pool[table].0[gigabyte] =
                self.attributes
                    .leaf(Type::Device, base + gigabyte as u64 * BLOCK_1G, 1);
        }
        self.pool[self.root].0[span] = self.address_of(table) | VALID | TABLE_OR_PAGE;
        Ok(table)
    }

    /// Replaces the block at `table[slot]` with a table of 512 blocks (or
    /// pages) of the next level, all Device, and returns that table. If the
    /// slot already holds a table, returns it.
    fn descend(&mut self, table: usize, slot: usize, level: u8) -> Result<usize, PageError> {
        let descriptor = self.pool[table].0[slot];
        if is_table(descriptor) {
            return Ok(self.index_of(descriptor));
        }
        if !is_leaf_block(descriptor) {
            return Err(PageError::Overlap);
        }
        if descriptor & (7 << 2) != u64::from(self.attributes.device_index) << 2 {
            // A block of Normal memory: something is mapped there already.
            return Err(PageError::Overlap);
        }
        let base = descriptor & ADDRESS_MASK_FOR_LEAF[level as usize];
        let child_level = level + 1;
        let size = if child_level == 3 {
            PAGE_SIZE
        } else {
            BLOCK_2M
        };
        let child = self.alloc()?;
        for i in 0..ENTRIES {
            self.pool[child].0[i] =
                self.attributes
                    .leaf(Type::Device, base + (i as u64) * size, child_level);
        }
        self.pool[table].0[slot] = self.address_of(child) | VALID | TABLE_OR_PAGE;
        Ok(child)
    }

    fn map_range(&mut self, base: u64, end: u64) -> Result<(), PageError> {
        let mut address = base;
        while address < end {
            let level1 = self.level1((address / SPAN) as usize)?;
            let l1_slot = ((address / BLOCK_1G) % ENTRIES as u64) as usize;
            let l2 = self.descend(level1, l1_slot, 1)?;
            let l2_slot = ((address / BLOCK_2M) % ENTRIES as u64) as usize;
            if address.is_multiple_of(BLOCK_2M) && end - address >= BLOCK_2M {
                if self.pool[l2].0[l2_slot] & 3 != 1
                    || self.pool[l2].0[l2_slot] & (7 << 2)
                        != u64::from(self.attributes.device_index) << 2
                {
                    return Err(PageError::Overlap);
                }
                self.pool[l2].0[l2_slot] = self.attributes.leaf(Type::Normal, address, 2);
                address += BLOCK_2M;
            } else {
                let l3 = self.descend(l2, l2_slot, 2)?;
                let l3_slot = ((address / PAGE_SIZE) % ENTRIES as u64) as usize;
                if self.pool[l3].0[l3_slot] & (7 << 2)
                    != u64::from(self.attributes.device_index) << 2
                {
                    return Err(PageError::Overlap);
                }
                self.pool[l3].0[l3_slot] = self.attributes.leaf(Type::Normal, address, 3);
                address += PAGE_SIZE;
            }
        }
        Ok(())
    }
}

/// An upper bound on the tables `build_identity_map` needs for these entries:
/// the root, the level 1 table, and per memory entry one level 2 table for
/// each gigabyte it touches plus up to two level 3 tables for its ends, and a
/// level 1 table for each 512 GiB it touches beyond the first.
pub fn tables_needed(entries: &[MemoryEntry]) -> usize {
    2 + entries
        .iter()
        .filter(|entry| entry.kind().is_normal_memory())
        .map(|entry| {
            (entry.length / BLOCK_1G) as usize + 2 + 2 + (entry.length / SPAN) as usize + 2
        })
        .sum::<usize>()
}

/// Builds the identity map in `pool` and returns the value for `TTBR0_EL1`:
/// the address of the root table. The root is the level 0 table when the
/// regime has more than 39 address bits and the level 1 table otherwise, as
/// the live `TCR_EL1` expects.
pub fn build_identity_map(
    pool: &mut [Table],
    entries: &[MemoryEntry],
    attributes: TableAttributes,
) -> Result<u64, PageError> {
    if !(31..=48).contains(&attributes.address_bits) {
        return Err(PageError::UnsupportedAddressBits);
    }
    let root_is_level0 = attributes.address_bits > 39;
    let mut builder = Builder {
        pool,
        used: 0,
        attributes,
        root: 0,
        root_is_level0,
    };
    let root = builder.alloc()?;
    builder.root = root;

    // The first 512 GiB (or the whole regime, if it is smaller) start as Device
    // blocks; further spans are made when a region reaches them.
    let covered = 1u64 << attributes.address_bits;
    let level1 = if root_is_level0 {
        builder.level1(0)?
    } else {
        root
    };
    if !root_is_level0 {
        for gigabyte in 0..covered / BLOCK_1G {
            builder.pool[level1].0[gigabyte as usize] =
                attributes.leaf(Type::Device, gigabyte * BLOCK_1G, 1);
        }
    }

    for entry in entries
        .iter()
        .filter(|entry| entry.kind().is_normal_memory())
    {
        if entry.end() > covered {
            return Err(PageError::OutOfRange);
        }
        builder.map_range(entry.base, entry.end())?;
    }
    Ok(builder.address_of(root))
}
