//! From the firmware's memory map to the entries of ADR 0013.
//!
//! The conversion follows Limine's table (the research is in
//! `docs/k2b-bootloader-research.md`): the numbers of the kinds and the
//! grouping of the EFI types are its, the code is ours, written from the
//! description in the ADR.

use crate::{MemoryEntry, MemoryKind, PAGE_SIZE};

/// One descriptor as the firmware returns it, reduced to what the conversion
/// needs. The firmware's own stride is only known per call, so the caller
/// reads the descriptors and hands them over one by one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EfiDescriptor {
    pub efi_type: u32,
    pub physical_start: u64,
    pub pages: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildError {
    /// The output holds fewer entries than the map needs.
    TooManyEntries,
    /// Two descriptors cover the same address.
    Overlap,
    /// A descriptor does not start on a page boundary or wraps the address space.
    BadDescriptor,
    /// The range handed to `carve_kernel` is not inside a single entry.
    KernelNotContained,
}

impl core::fmt::Display for BuildError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            Self::TooManyEntries => "the memory map has more entries than reserved room",
            Self::Overlap => "the firmware memory map overlaps itself",
            Self::BadDescriptor => "the firmware memory map has a malformed descriptor",
            Self::KernelNotContained => "the kernel range is not inside one memory map entry",
        })
    }
}

const EFI_LOADER_CODE: u32 = 1;
const EFI_LOADER_DATA: u32 = 2;
const EFI_BOOT_SERVICES_CODE: u32 = 3;
const EFI_BOOT_SERVICES_DATA: u32 = 4;
const EFI_RUNTIME_SERVICES_CODE: u32 = 5;
const EFI_RUNTIME_SERVICES_DATA: u32 = 6;
const EFI_CONVENTIONAL: u32 = 7;
const EFI_ACPI_RECLAIM: u32 = 9;
const EFI_ACPI_NVS: u32 = 10;

pub fn kind_from_efi(efi_type: u32) -> MemoryKind {
    match efi_type {
        EFI_CONVENTIONAL => MemoryKind::Usable,
        EFI_LOADER_CODE | EFI_LOADER_DATA | EFI_BOOT_SERVICES_CODE | EFI_BOOT_SERVICES_DATA => {
            MemoryKind::BootloaderReclaimable
        }
        EFI_RUNTIME_SERVICES_CODE | EFI_RUNTIME_SERVICES_DATA => MemoryKind::ReservedMapped,
        EFI_ACPI_RECLAIM => MemoryKind::AcpiReclaimable,
        EFI_ACPI_NVS => MemoryKind::AcpiNvs,
        _ => MemoryKind::Reserved,
    }
}

/// Converts the descriptors into `out`, sorted by base, with adjacent entries
/// of the same kind merged, and returns how many entries it wrote.
///
/// Descriptors with no pages are dropped. Any other malformed input is an
/// error: the map is the authority for RAM, so a guess here would hand the
/// kernel memory that is not free.
pub fn build_memory_map(
    descriptors: impl IntoIterator<Item = EfiDescriptor>,
    out: &mut [MemoryEntry],
) -> Result<usize, BuildError> {
    let mut len = 0;
    for descriptor in descriptors {
        if descriptor.pages == 0 {
            continue;
        }
        let length = descriptor
            .pages
            .checked_mul(PAGE_SIZE)
            .ok_or(BuildError::BadDescriptor)?;
        if !descriptor.physical_start.is_multiple_of(PAGE_SIZE)
            || descriptor.physical_start.checked_add(length).is_none()
        {
            return Err(BuildError::BadDescriptor);
        }
        let entry = MemoryEntry::new(
            descriptor.physical_start,
            length,
            kind_from_efi(descriptor.efi_type),
        );
        // Insertion sort: the firmware map is almost sorted already.
        let slot = out.get_mut(len).ok_or(BuildError::TooManyEntries)?;
        *slot = entry;
        len += 1;
        let mut at = len - 1;
        while at > 0 && out[at - 1].base > out[at].base {
            out.swap(at - 1, at);
            at -= 1;
        }
    }

    for pair in out[..len].windows(2) {
        if pair[0].end() > pair[1].base {
            return Err(BuildError::Overlap);
        }
    }

    // Merge neighbours of the same kind that touch.
    let mut merged = 0;
    for read in 0..len {
        let entry = out[read];
        if merged > 0 {
            let last = &mut out[merged - 1];
            if last.kind == entry.kind && last.end() == entry.base {
                last.length += entry.length;
                continue;
            }
        }
        out[merged] = entry;
        merged += 1;
    }
    Ok(merged)
}

/// Marks `base..base + length` as the Kernel entry, splitting the entry that
/// holds it, and returns the new number of entries. The range must lie inside
/// a single entry: the loader allocated it as one block.
pub fn carve_kernel(
    out: &mut [MemoryEntry],
    len: usize,
    base: u64,
    length: u64,
) -> Result<usize, BuildError> {
    let end = base
        .checked_add(length)
        .ok_or(BuildError::KernelNotContained)?;
    let index = out[..len]
        .iter()
        .position(|entry| entry.base <= base && end <= entry.end())
        .ok_or(BuildError::KernelNotContained)?;
    let holder = out[index];

    let mut pieces = [MemoryEntry::default(); 3];
    let mut count = 0;
    let mut push = |entry: MemoryEntry| {
        pieces[count] = entry;
        count += 1;
    };
    if holder.base < base {
        push(MemoryEntry {
            length: base - holder.base,
            ..holder
        });
    }
    push(MemoryEntry::new(base, length, MemoryKind::Kernel));
    if end < holder.end() {
        push(MemoryEntry {
            base: end,
            length: holder.end() - end,
            ..holder
        });
    }

    let new_len = len + count - 1;
    if new_len > out.len() {
        return Err(BuildError::TooManyEntries);
    }
    out.copy_within(index + 1..len, index + count);
    out[index..index + count].copy_from_slice(&pieces[..count]);
    Ok(new_len)
}
