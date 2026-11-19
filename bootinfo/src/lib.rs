//! The handoff between the bootloader and the kernel (ADR 0013).
//!
//! One definition of the layout, shared by both sides so that they cannot
//! disagree: the bootloader builds a [`BootInfo`] and a memory map, the kernel
//! reads them back with [`BootInfo::from_address`] and [`MemoryMap`]. The
//! blob comes from outside the kernel's own code, so every read is checked.
//!
//! The boot menu's logic ([`menu`]) is here for the same reason.
//!
//! The bootloader also needs two pieces of plain computation that live here so
//! that the host can test them: the conversion of the firmware's memory map
//! ([`memmap`]) and the page tables of the identity map the kernel starts on
//! ([`paging`]).

#![cfg_attr(not(test), no_std)]

pub mod config;
pub mod elf;
mod handoff;
pub mod initrd;
mod memmap;
pub mod menu;
mod paging;
pub mod pe;

pub use handoff::{Handoff, HandoffError, Rejected, check, select_rsdp, usable_console};
pub use memmap::{BuildError, EfiDescriptor, build_memory_map, carve_kernel, kind_from_efi};
pub use paging::{PAGE_SIZE, PageError, Table, TableAttributes, build_identity_map, tables_needed};

use core::mem::{offset_of, size_of};

/// The ASCII bytes `WSOSBOOT`, first in the structure.
pub const MAGIC: u64 = u64::from_le_bytes(*b"WSOSBOOT");
/// `x1` when the bootloader enters the kernel: the ASCII bytes `WSOSENTR`.
pub const ENTRY_MAGIC: u64 = 0x5753_4f53_454e_5452;

pub const VERSION_MAJOR: u16 = 1;
pub const VERSION_MINOR: u16 = 0;

/// The frozen header: no future version changes it.
pub const HEADER_SIZE: usize = 32;
/// A `size` above this is bogus: the structure is a hundred-odd bytes today.
const MAX_SIZE: usize = 64 * 1024;
/// A memory map above this many bytes is bogus.
const MAX_MAP_BYTES: usize = 1 << 20;

pub const ENTRY_SIZE: usize = size_of::<MemoryEntry>();

pub const CONSOLE_NONE: u32 = 0;
pub const CONSOLE_PL011: u32 = 1;
pub const CONSOLE_NS16550_MMIO: u32 = 2;
pub const CONSOLE_NS16550_PORT: u32 = 3;

pub const MODE_NORMAL: u32 = 0;
pub const MODE_RECOVERY: u32 = 1;

/// The name of a `boot_mode` for the log; a mode this version does not know
/// is reported, not rejected, so that a newer bootloader still boots.
pub fn boot_mode_name(mode: u32) -> &'static str {
    match mode {
        MODE_NORMAL => "normal",
        MODE_RECOVERY => "recovery",
        _ => "unknown",
    }
}

pub const ARCH_AARCH64: u32 = 1;
pub const ARCH_X86_64: u32 = 2;

/// The structure, version 1.0. Every field is naturally aligned and the
/// layout is C, so the byte offsets are the ones the ADR tables give.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BootInfo {
    pub magic: u64,
    pub version_major: u16,
    pub version_minor: u16,
    /// Size of the whole structure in bytes.
    pub size: u32,
    pub console_kind: u32,
    pub reserved0: u32,
    pub console_base: u64,
    pub boot_mode: u32,
    pub arch: u32,
    pub boot_cpu: u64,
    pub memory_map: u64,
    pub memory_map_count: u64,
    pub memory_map_entry_size: u32,
    pub reserved1: u32,
    pub device_tree: u64,
    pub device_tree_size: u64,
    pub rsdp: u64,
    pub kernel_base: u64,
    pub kernel_size: u64,
    pub efi_system_table: u64,
}

const _: () = assert!(offset_of!(BootInfo, boot_mode) == HEADER_SIZE);
const _: () = assert!(size_of::<BootInfo>() == 120);

/// End offsets of the body fields, in order. A field is read only when its
/// whole extent lies inside the `size` the bootloader declared.
const BODY_FIELD_ENDS: [usize; 13] = [
    offset_of!(BootInfo, arch),
    offset_of!(BootInfo, boot_cpu),
    offset_of!(BootInfo, memory_map),
    offset_of!(BootInfo, memory_map_count),
    offset_of!(BootInfo, memory_map_entry_size),
    offset_of!(BootInfo, reserved1),
    offset_of!(BootInfo, device_tree),
    offset_of!(BootInfo, device_tree_size),
    offset_of!(BootInfo, rsdp),
    offset_of!(BootInfo, kernel_base),
    offset_of!(BootInfo, kernel_size),
    offset_of!(BootInfo, efi_system_table),
    size_of::<BootInfo>(),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    NullPointer,
    Misaligned,
    /// Fewer bytes than the frozen header.
    TooSmall,
    BadMagic,
    /// `size` is smaller than the frozen header, larger than any real
    /// structure, or larger than the bytes at hand.
    BadSize,
    /// A major version this reader does not know. The header is valid, so the
    /// console it names can be used to say so.
    UnsupportedVersion {
        major: u16,
        minor: u16,
        console_kind: u32,
        console_base: u64,
    },
    BadMemoryMap(MapError),
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::NullPointer => f.write_str("boot info address is null"),
            Self::Misaligned => f.write_str("boot info address is not 8-byte aligned"),
            Self::TooSmall => f.write_str("boot info is smaller than its header"),
            Self::BadMagic => f.write_str("boot info magic is wrong"),
            Self::BadSize => f.write_str("boot info size is invalid"),
            Self::UnsupportedVersion { major, minor, .. } => {
                write!(f, "unsupported boot info version {major}.{minor}")
            }
            Self::BadMemoryMap(error) => write!(f, "boot memory map: {error}"),
        }
    }
}

fn read_u16(bytes: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([bytes[at], bytes[at + 1]])
}

fn read_u32(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap_or_default())
}

fn read_u64(bytes: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap_or_default())
}

impl BootInfo {
    /// A structure with the header and the identity fields filled in, as
    /// version 1.0 of the layout.
    pub fn new() -> Self {
        Self {
            magic: MAGIC,
            version_major: VERSION_MAJOR,
            version_minor: VERSION_MINOR,
            size: size_of::<Self>() as u32,
            memory_map_entry_size: ENTRY_SIZE as u32,
            ..Self::default()
        }
    }

    /// The bytes the bootloader writes: the structure as laid out in memory.
    pub fn to_bytes(&self) -> [u8; size_of::<Self>()] {
        let mut bytes = [0u8; size_of::<Self>()];
        let mut put = |at: usize, value: &[u8]| bytes[at..at + value.len()].copy_from_slice(value);
        put(offset_of!(Self, magic), &self.magic.to_le_bytes());
        put(
            offset_of!(Self, version_major),
            &self.version_major.to_le_bytes(),
        );
        put(
            offset_of!(Self, version_minor),
            &self.version_minor.to_le_bytes(),
        );
        put(offset_of!(Self, size), &self.size.to_le_bytes());
        put(
            offset_of!(Self, console_kind),
            &self.console_kind.to_le_bytes(),
        );
        put(
            offset_of!(Self, console_base),
            &self.console_base.to_le_bytes(),
        );
        put(offset_of!(Self, boot_mode), &self.boot_mode.to_le_bytes());
        put(offset_of!(Self, arch), &self.arch.to_le_bytes());
        put(offset_of!(Self, boot_cpu), &self.boot_cpu.to_le_bytes());
        put(offset_of!(Self, memory_map), &self.memory_map.to_le_bytes());
        put(
            offset_of!(Self, memory_map_count),
            &self.memory_map_count.to_le_bytes(),
        );
        put(
            offset_of!(Self, memory_map_entry_size),
            &self.memory_map_entry_size.to_le_bytes(),
        );
        put(
            offset_of!(Self, device_tree),
            &self.device_tree.to_le_bytes(),
        );
        put(
            offset_of!(Self, device_tree_size),
            &self.device_tree_size.to_le_bytes(),
        );
        put(offset_of!(Self, rsdp), &self.rsdp.to_le_bytes());
        put(
            offset_of!(Self, kernel_base),
            &self.kernel_base.to_le_bytes(),
        );
        put(
            offset_of!(Self, kernel_size),
            &self.kernel_size.to_le_bytes(),
        );
        put(
            offset_of!(Self, efi_system_table),
            &self.efi_system_table.to_le_bytes(),
        );
        bytes
    }

    /// Reads the structure from `bytes`, which holds at least what `size`
    /// declares. Fields past `size` take their default of zero; a field that
    /// is only partly inside `size` counts as past it.
    pub fn parse(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() < HEADER_SIZE {
            return Err(Error::TooSmall);
        }
        if read_u64(bytes, offset_of!(Self, magic)) != MAGIC {
            return Err(Error::BadMagic);
        }
        let size = read_u32(bytes, offset_of!(Self, size)) as usize;
        if !(HEADER_SIZE..=MAX_SIZE).contains(&size) || size > bytes.len() {
            return Err(Error::BadSize);
        }
        let major = read_u16(bytes, offset_of!(Self, version_major));
        let minor = read_u16(bytes, offset_of!(Self, version_minor));
        let console_kind = read_u32(bytes, offset_of!(Self, console_kind));
        let console_base = read_u64(bytes, offset_of!(Self, console_base));
        if major != VERSION_MAJOR {
            return Err(Error::UnsupportedVersion {
                major,
                minor,
                console_kind,
                console_base,
            });
        }

        // Zero everything past the last field that lies entirely inside `size`.
        let mut known = [0u8; size_of::<Self>()];
        let mut limit = HEADER_SIZE;
        for end in BODY_FIELD_ENDS {
            if end <= size {
                limit = end;
            }
        }
        let copy = limit.min(known.len());
        known[..copy].copy_from_slice(&bytes[..copy]);
        Ok(Self {
            magic: MAGIC,
            version_major: major,
            version_minor: minor,
            size: size as u32,
            console_kind,
            reserved0: 0,
            console_base,
            boot_mode: read_u32(&known, offset_of!(Self, boot_mode)),
            arch: read_u32(&known, offset_of!(Self, arch)),
            boot_cpu: read_u64(&known, offset_of!(Self, boot_cpu)),
            memory_map: read_u64(&known, offset_of!(Self, memory_map)),
            memory_map_count: read_u64(&known, offset_of!(Self, memory_map_count)),
            // Absent in a body that stops before it: the version 1.0 default.
            memory_map_entry_size: if limit > offset_of!(Self, memory_map_entry_size) {
                read_u32(&known, offset_of!(Self, memory_map_entry_size))
            } else {
                ENTRY_SIZE as u32
            },
            reserved1: 0,
            device_tree: read_u64(&known, offset_of!(Self, device_tree)),
            device_tree_size: read_u64(&known, offset_of!(Self, device_tree_size)),
            rsdp: read_u64(&known, offset_of!(Self, rsdp)),
            kernel_base: read_u64(&known, offset_of!(Self, kernel_base)),
            kernel_size: read_u64(&known, offset_of!(Self, kernel_size)),
            efi_system_table: read_u64(&known, offset_of!(Self, efi_system_table)),
        })
    }

    /// Reads the structure the bootloader left at `address`.
    ///
    /// # Safety
    ///
    /// `address` must be where the bootloader put the structure (`x0` on
    /// entry, with the entry magic in `x1`): readable memory holding as many
    /// bytes as the `size` field of the header declares, or at least a
    /// header.
    pub unsafe fn from_address(address: usize) -> Result<Self, Error> {
        if address == 0 {
            return Err(Error::NullPointer);
        }
        if !address.is_multiple_of(8) {
            return Err(Error::Misaligned);
        }
        // SAFETY: the caller guarantees at least the header is readable.
        let header = unsafe { core::slice::from_raw_parts(address as *const u8, HEADER_SIZE) };
        if read_u64(header, offset_of!(Self, magic)) != MAGIC {
            return Err(Error::BadMagic);
        }
        let size = read_u32(header, offset_of!(Self, size)) as usize;
        if !(HEADER_SIZE..=MAX_SIZE).contains(&size) {
            return Err(Error::BadSize);
        }
        // SAFETY: the caller guarantees `size` bytes are readable, and the
        // bounds were checked above.
        let bytes = unsafe { core::slice::from_raw_parts(address as *const u8, size) };
        Self::parse(bytes)
    }

    /// The memory map this structure points at.
    ///
    /// # Safety
    ///
    /// The structure must come from the bootloader, so that `memory_map` points
    /// at `memory_map_count` entries that stay unchanged for as long as the
    /// result is used.
    pub unsafe fn memory_map(&self) -> Result<MemoryMap<'static>, Error> {
        let entry_size = self.memory_map_entry_size as usize;
        let count = usize::try_from(self.memory_map_count)
            .map_err(|_| Error::BadMemoryMap(MapError::TooLarge))?;
        let bytes = entry_size
            .checked_mul(count)
            .filter(|bytes| *bytes <= MAX_MAP_BYTES)
            .ok_or(Error::BadMemoryMap(MapError::TooLarge))?;
        if entry_size < ENTRY_SIZE {
            return Err(Error::BadMemoryMap(MapError::EntrySize));
        }
        let address = self.memory_map as usize;
        if bytes > 0
            && (address == 0 || !address.is_multiple_of(8) || address.checked_add(bytes).is_none())
        {
            return Err(Error::BadMemoryMap(MapError::BadAddress));
        }
        let entries = if bytes == 0 {
            &[]
        } else {
            // SAFETY: the caller guarantees the entries are readable and
            // unchanged; the size was bounded above.
            unsafe { core::slice::from_raw_parts(address as *const u8, bytes) }
        };
        Ok(MemoryMap {
            entries,
            entry_size,
        })
    }
}

/// What a region of the memory map is. The numbers are Limine's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum MemoryKind {
    Usable = 0,
    Reserved = 1,
    AcpiReclaimable = 2,
    AcpiNvs = 3,
    BadMemory = 4,
    BootloaderReclaimable = 5,
    Kernel = 6,
    ReservedMapped = 8,
}

impl MemoryKind {
    /// A number outside the table is something this reader cannot interpret:
    /// the safe reading is `Reserved`.
    pub fn from_u32(value: u32) -> Self {
        match value {
            0 => Self::Usable,
            2 => Self::AcpiReclaimable,
            3 => Self::AcpiNvs,
            4 => Self::BadMemory,
            5 => Self::BootloaderReclaimable,
            6 => Self::Kernel,
            8 => Self::ReservedMapped,
            _ => Self::Reserved,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Usable => "usable",
            Self::Reserved => "reserved",
            Self::AcpiReclaimable => "acpi-reclaimable",
            Self::AcpiNvs => "acpi-nvs",
            Self::BadMemory => "bad-memory",
            Self::BootloaderReclaimable => "bootloader-reclaimable",
            Self::Kernel => "kernel",
            Self::ReservedMapped => "reserved-mapped",
        }
    }

    /// RAM that something owns or may own later. Firmware regions that are not
    /// RAM (runtime services, MMIO, reserved) stay out of the kernel's map.
    pub fn is_ram(self) -> bool {
        matches!(
            self,
            Self::Usable | Self::BootloaderReclaimable | Self::AcpiReclaimable | Self::Kernel
        )
    }

    /// RAM that holds something the kernel may read or write, so the identity
    /// map makes it Normal cacheable memory. Everything else is Device.
    pub fn is_normal_memory(self) -> bool {
        !matches!(self, Self::Reserved | Self::BadMemory)
    }
}

/// One region of the memory map.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MemoryEntry {
    pub base: u64,
    pub length: u64,
    pub kind: u32,
    pub flags: u32,
}

impl MemoryEntry {
    pub fn new(base: u64, length: u64, kind: MemoryKind) -> Self {
        Self {
            base,
            length,
            kind: kind as u32,
            flags: 0,
        }
    }

    pub fn kind(&self) -> MemoryKind {
        MemoryKind::from_u32(self.kind)
    }

    /// One past the last byte; saturates instead of wrapping.
    pub fn end(&self) -> u64 {
        self.base.saturating_add(self.length)
    }

    pub fn to_bytes(&self) -> [u8; ENTRY_SIZE] {
        let mut bytes = [0u8; ENTRY_SIZE];
        bytes[0..8].copy_from_slice(&self.base.to_le_bytes());
        bytes[8..16].copy_from_slice(&self.length.to_le_bytes());
        bytes[16..20].copy_from_slice(&self.kind.to_le_bytes());
        bytes[20..24].copy_from_slice(&self.flags.to_le_bytes());
        bytes
    }

    fn from_bytes(bytes: &[u8]) -> Self {
        Self {
            base: read_u64(bytes, 0),
            length: read_u64(bytes, 8),
            kind: read_u32(bytes, 16),
            flags: read_u32(bytes, 20),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MapError {
    TooLarge,
    EntrySize,
    BadAddress,
    Empty,
    EmptyEntry,
    Unsorted,
    Overlap,
    Misaligned,
    KernelEntry,
}

impl core::fmt::Display for MapError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            Self::TooLarge => "too large",
            Self::EntrySize => "entry size is below the 24 bytes of version 1.0",
            Self::BadAddress => "address is null or misaligned",
            Self::Empty => "no entries",
            Self::EmptyEntry => "an entry has no length or wraps the address space",
            Self::Unsorted => "entries are not sorted by base",
            Self::Overlap => "entries overlap",
            Self::Misaligned => "a usable or reclaimable entry is not 4 KiB aligned",
            Self::KernelEntry => "the kernel is not exactly one Kernel entry",
        })
    }
}

/// A read-only view of the entries. The stride is the `entry_size` the
/// bootloader declared, so a later minor version may enlarge an entry.
#[derive(Clone, Copy)]
pub struct MemoryMap<'a> {
    entries: &'a [u8],
    entry_size: usize,
}

impl<'a> MemoryMap<'a> {
    /// A view over `bytes`, which must hold whole entries of `entry_size`.
    pub fn from_bytes(bytes: &'a [u8], entry_size: usize) -> Result<Self, MapError> {
        if entry_size < ENTRY_SIZE {
            return Err(MapError::EntrySize);
        }
        if !bytes.len().is_multiple_of(entry_size) {
            return Err(MapError::TooLarge);
        }
        Ok(Self {
            entries: bytes,
            entry_size,
        })
    }

    pub fn len(&self) -> usize {
        self.entries.len() / self.entry_size
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = MemoryEntry> + 'a {
        self.entries
            .chunks_exact(self.entry_size)
            .map(MemoryEntry::from_bytes)
    }

    /// Checks the guarantees the bootloader gives (ADR 0013): entries have a
    /// length, are sorted by base and overlap nothing, Usable and
    /// BootloaderReclaimable entries are 4 KiB aligned, and the kernel image
    /// is one contiguous Kernel entry that covers `kernel_base` for
    /// `kernel_size` bytes.
    pub fn validate(&self, kernel_base: u64, kernel_size: u64) -> Result<(), MapError> {
        if self.is_empty() {
            return Err(MapError::Empty);
        }
        let mut previous_end = 0u64;
        let mut previous_base = 0u64;
        let mut kernel_entries = 0;
        for entry in self.iter() {
            if entry.length == 0 || entry.base.checked_add(entry.length).is_none() {
                return Err(MapError::EmptyEntry);
            }
            if entry.base < previous_base {
                return Err(MapError::Unsorted);
            }
            if entry.base < previous_end {
                return Err(MapError::Overlap);
            }
            previous_base = entry.base;
            previous_end = entry.end();
            let aligned =
                entry.base.is_multiple_of(PAGE_SIZE) && entry.length.is_multiple_of(PAGE_SIZE);
            if matches!(
                entry.kind(),
                MemoryKind::Usable | MemoryKind::BootloaderReclaimable
            ) && !aligned
            {
                return Err(MapError::Misaligned);
            }
            if entry.kind() == MemoryKind::Kernel {
                kernel_entries += 1;
                let size = kernel_size
                    .checked_add(PAGE_SIZE - 1)
                    .map(|end| end / PAGE_SIZE * PAGE_SIZE);
                if entry.base != kernel_base || Some(entry.length) != size {
                    return Err(MapError::KernelEntry);
                }
            }
        }
        if kernel_entries != 1 {
            return Err(MapError::KernelEntry);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
