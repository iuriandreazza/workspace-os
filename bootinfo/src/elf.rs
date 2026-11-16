//! The part of an ELF64 file the bootloader needs to place a kernel: where each
//! loadable segment goes and where execution starts. The file comes from disk,
//! so every offset and size is checked against the bytes at hand.

use crate::PAGE_SIZE;

const EM_AARCH64: u16 = 183;
const PT_LOAD: u32 = 1;
const ELF_HEADER_SIZE: usize = 64;
const PROGRAM_HEADER_SIZE: usize = 56;
/// A kernel with more loadable segments than this is refused.
pub const MAX_SEGMENTS: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ElfError {
    TooSmall,
    NotElf64LittleEndian,
    NotAarch64Executable,
    BadProgramHeaders,
    NoLoadableSegment,
    TooManySegments,
    /// A segment reaches outside the file or wraps the address space.
    BadSegment,
    Overlap,
    Unaligned,
    /// The entry point is not inside a loaded segment.
    BadEntry,
}

impl core::fmt::Display for ElfError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            Self::TooSmall => "file is too small to be an ELF",
            Self::NotElf64LittleEndian => "not a 64-bit little-endian ELF",
            Self::NotAarch64Executable => "not an AArch64 executable",
            Self::BadProgramHeaders => "program headers lie outside the file",
            Self::NoLoadableSegment => "no loadable segment",
            Self::TooManySegments => "too many loadable segments",
            Self::BadSegment => "a segment lies outside the file or wraps the address space",
            Self::Overlap => "segments overlap in memory",
            Self::Unaligned => "the image does not start on a page boundary",
            Self::BadEntry => "the entry point is outside the loaded segments",
        })
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Segment {
    pub file_offset: u64,
    pub file_size: u64,
    pub address: u64,
    pub memory_size: u64,
}

/// Where the image goes: the segments, and the page-aligned span they cover.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LoadPlan {
    pub entry: u64,
    /// Lowest address of any segment, page aligned.
    pub base: u64,
    /// One past the highest byte of any segment.
    pub end: u64,
    segments: [Segment; MAX_SEGMENTS],
    count: usize,
}

impl LoadPlan {
    pub fn segments(&self) -> &[Segment] {
        &self.segments[..self.count]
    }

    /// Size in memory from `base`, `.bss` and the like included.
    pub fn memory_size(&self) -> u64 {
        self.end - self.base
    }

    /// Bytes of pages to allocate for the image.
    pub fn span(&self) -> u64 {
        self.end.div_ceil(PAGE_SIZE) * PAGE_SIZE - self.base
    }
}

fn u16_at(bytes: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([bytes[at], bytes[at + 1]])
}

fn u32_at(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap_or_default())
}

fn u64_at(bytes: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap_or_default())
}

pub fn plan(file: &[u8]) -> Result<LoadPlan, ElfError> {
    if file.len() < ELF_HEADER_SIZE {
        return Err(ElfError::TooSmall);
    }
    // 0x7f ELF, class 64, little endian, version 1.
    if file[..4] != *b"\x7fELF" || file[4] != 2 || file[5] != 1 || file[6] != 1 {
        return Err(ElfError::NotElf64LittleEndian);
    }
    let file_type = u16_at(file, 16);
    // ET_EXEC only: a position-independent kernel needs relocation, which the
    // loader does not do (ADR 0013, "Load address").
    if file_type != 2 || u16_at(file, 18) != EM_AARCH64 {
        return Err(ElfError::NotAarch64Executable);
    }
    let entry = u64_at(file, 24);
    let table_offset = u64_at(file, 32);
    let entry_size = u16_at(file, 54) as usize;
    let count = u16_at(file, 56) as usize;
    if entry_size < PROGRAM_HEADER_SIZE {
        return Err(ElfError::BadProgramHeaders);
    }
    let table_end = usize::try_from(table_offset)
        .ok()
        .and_then(|offset| entry_size.checked_mul(count)?.checked_add(offset))
        .ok_or(ElfError::BadProgramHeaders)?;
    if table_end > file.len() {
        return Err(ElfError::BadProgramHeaders);
    }
    let table_offset = table_offset as usize;

    let mut segments = [Segment::default(); MAX_SEGMENTS];
    let mut loaded = 0;
    for index in 0..count {
        let header = &file[table_offset + index * entry_size..];
        if u32_at(header, 0) != PT_LOAD {
            continue;
        }
        let segment = Segment {
            file_offset: u64_at(header, 8),
            address: u64_at(header, 24),
            file_size: u64_at(header, 32),
            memory_size: u64_at(header, 40),
        };
        // A segment with nothing in memory places nothing.
        if segment.memory_size == 0 {
            continue;
        }
        let in_file = segment
            .file_offset
            .checked_add(segment.file_size)
            .is_some_and(|end| end <= file.len() as u64);
        if segment.file_size > segment.memory_size
            || !in_file
            || segment.address.checked_add(segment.memory_size).is_none()
        {
            return Err(ElfError::BadSegment);
        }
        *segments.get_mut(loaded).ok_or(ElfError::TooManySegments)? = segment;
        loaded += 1;
    }
    if loaded == 0 {
        return Err(ElfError::NoLoadableSegment);
    }

    let found = &segments[..loaded];
    for (i, a) in found.iter().enumerate() {
        for b in &found[i + 1..] {
            if a.address < b.address + b.memory_size && b.address < a.address + a.memory_size {
                return Err(ElfError::Overlap);
            }
        }
    }
    let base = found.iter().map(|s| s.address).min().unwrap_or(0);
    let end = found
        .iter()
        .map(|s| s.address + s.memory_size)
        .max()
        .unwrap_or(0);
    if !base.is_multiple_of(PAGE_SIZE) {
        return Err(ElfError::Unaligned);
    }
    if !found
        .iter()
        .any(|s| s.address <= entry && entry < s.address + s.memory_size)
    {
        return Err(ElfError::BadEntry);
    }
    Ok(LoadPlan {
        entry,
        base,
        end,
        segments,
        count: loaded,
    })
}
