//! Minimal flattened device tree (FDT) reader.
//!
//! Written for the kernel, which has no allocator when it first needs the
//! tree, so nothing here allocates. The blob comes from outside the kernel and
//! is treated as untrusted: every offset, length and string is checked against
//! the bounds of the block it lives in before it is used.
//!
//! Only what the kernel asks for is supported: the first `reg` entry of the
//! first usable node with a given `compatible` string, among the direct
//! children of the root node; every `reg` entry of every usable memory node;
//! and the regions the tree reserves (`/memreserve/` and the children of
//! `reserved-memory`). `reg` values are taken as CPU physical addresses, which
//! holds when the parent bus has no address translation; nodes below another
//! bus are not looked at.
//!
//! The memory and reservation walkers hand out raw ranges: a size of zero or a
//! range that wraps the address space is passed on as found, because deciding
//! what to do with a hostile range belongs to whoever uses it.

#![cfg_attr(not(test), no_std)]

use core::fmt;

const MAGIC: u32 = 0xd00d_feed;
const HEADER_SIZE: usize = 40;
/// The boot protocols cap the blob at 2 MiB; a larger `totalsize` is bogus.
const MAX_TOTAL_SIZE: usize = 2 * 1024 * 1024;
const SUPPORTED_VERSION: u32 = 17;

const TOKEN_BEGIN_NODE: u32 = 1;
const TOKEN_END_NODE: u32 = 2;
const TOKEN_PROP: u32 = 3;
const TOKEN_NOP: u32 = 4;
const TOKEN_END: u32 = 9;

/// Defaults from the devicetree specification, for a root with no cell counts.
const DEFAULT_ADDRESS_CELLS: u32 = 2;
const DEFAULT_SIZE_CELLS: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    NullPointer,
    Misaligned,
    BadMagic,
    BadTotalSize,
    UnsupportedVersion,
    BadHeader,
    Truncated,
    BadToken,
    BadString,
    UnsupportedCells,
    MalformedReg,
    UnsupportedRanges,
    NotFound,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = match self {
            Self::NullPointer => "device tree pointer is null",
            Self::Misaligned => "device tree is not 8-byte aligned",
            Self::BadMagic => "device tree magic number is wrong",
            Self::BadTotalSize => "device tree size is outside the allowed range",
            Self::UnsupportedVersion => "device tree version is not supported",
            Self::BadHeader => "device tree header points outside the blob",
            Self::Truncated => "device tree structure block is truncated",
            Self::BadToken => "device tree structure block has an invalid token",
            Self::BadString => "device tree holds an unterminated string",
            Self::UnsupportedCells => "device tree uses an unsupported cell count",
            Self::MalformedReg => "device tree reg property is malformed",
            Self::UnsupportedRanges => "device tree reserved-memory translates addresses",
            Self::NotFound => "device tree has no matching node",
        };
        f.write_str(text)
    }
}

/// An address range from a `reg` property.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Region {
    pub base: u64,
    pub size: u64,
}

#[derive(Clone, Copy)]
pub struct Fdt<'a> {
    blob: &'a [u8],
    size: usize,
    reserve_map: &'a [u8],
    structure: &'a [u8],
    strings: &'a [u8],
}

impl<'a> Fdt<'a> {
    /// Reads the blob at `address`.
    ///
    /// # Safety
    ///
    /// `address` must point at readable memory that stays unmodified for `'a`
    /// and extends at least as far as the `totalsize` its header claims, up to
    /// 2 MiB. The contents are validated; the location is not.
    pub unsafe fn from_address(address: usize) -> Result<Fdt<'a>, Error> {
        if address == 0 {
            return Err(Error::NullPointer);
        }
        if !address.is_multiple_of(8) {
            return Err(Error::Misaligned);
        }
        let header = address as *const u32;
        // SAFETY: the caller guarantees the memory at `address` is readable and
        // a header is 40 bytes; `address` is 4-byte aligned.
        let (magic, total_size) = unsafe {
            (
                u32::from_be(header.read()),
                u32::from_be(header.add(1).read()),
            )
        };
        if magic != MAGIC {
            return Err(Error::BadMagic);
        }
        let total_size = usize::try_from(total_size).map_err(|_| Error::BadTotalSize)?;
        if !(HEADER_SIZE..=MAX_TOTAL_SIZE).contains(&total_size) {
            return Err(Error::BadTotalSize);
        }
        // SAFETY: the caller guarantees `total_size` readable bytes at `address`,
        // checked above to be within the 2 MiB bound.
        let blob = unsafe { core::slice::from_raw_parts(address as *const u8, total_size) };
        Fdt::parse(blob)
    }

    /// Validates a blob held in a slice.
    pub fn parse(blob: &'a [u8]) -> Result<Fdt<'a>, Error> {
        if blob.len() < HEADER_SIZE {
            return Err(Error::BadTotalSize);
        }
        if read_u32(blob, 0)? != MAGIC {
            return Err(Error::BadMagic);
        }
        let total_size = to_usize(read_u32(blob, 4)?)?;
        if !(HEADER_SIZE..=MAX_TOTAL_SIZE).contains(&total_size) || total_size > blob.len() {
            return Err(Error::BadTotalSize);
        }
        let blob = &blob[..total_size];

        let version = read_u32(blob, 20)?;
        let last_compatible = read_u32(blob, 24)?;
        if version < SUPPORTED_VERSION || last_compatible > SUPPORTED_VERSION {
            return Err(Error::UnsupportedVersion);
        }

        let reserve_map_offset = to_usize(read_u32(blob, 16)?)?;
        let structure_offset = to_usize(read_u32(blob, 8)?)?;
        let strings_offset = to_usize(read_u32(blob, 12)?)?;
        let strings_size = to_usize(read_u32(blob, 32)?)?;
        let structure_size = to_usize(read_u32(blob, 36)?)?;
        if !structure_offset.is_multiple_of(4) || !structure_size.is_multiple_of(4) {
            return Err(Error::BadHeader);
        }
        // The reservation map is a list of 16-byte entries, 8-byte aligned.
        if !reserve_map_offset.is_multiple_of(8) {
            return Err(Error::BadHeader);
        }
        Ok(Fdt {
            blob,
            size: total_size,
            reserve_map: blob.get(reserve_map_offset..).ok_or(Error::BadHeader)?,
            structure: sub_block(blob, structure_offset, structure_size)?,
            strings: sub_block(blob, strings_offset, strings_size)?,
        })
    }

    /// The validated blob, as many bytes as the header claims.
    pub fn blob(&self) -> &'a [u8] {
        self.blob
    }

    /// Size in bytes the header claims for the whole blob.
    pub fn size(&self) -> usize {
        self.size
    }

    /// Calls `visit` for every `reg` entry of every enabled node whose
    /// `device_type` is `memory`, in tree order.
    pub fn for_each_memory_region(&self, mut visit: impl FnMut(Region)) -> Result<(), Error> {
        self.find_root_child(|node| {
            // A memory node with no `reg` describes nothing and is skipped.
            if node.device_type == Some(b"memory\0") && node.reg.is_some() {
                node.for_each_entry(&mut visit)?;
            }
            Ok(false)
        })
        .map(|_| ())
    }

    /// Calls `visit` for every range the tree marks as not available to the
    /// kernel: the `/memreserve/` entries and each enabled child of
    /// `reserved-memory` that has a `reg`. Children that only ask for a size
    /// (dynamic allocation) reserve nothing yet and are skipped.
    pub fn for_each_reserved_region(&self, mut visit: impl FnMut(Region)) -> Result<(), Error> {
        self.for_each_memreserve(&mut visit)?;
        self.for_each_reserved_memory_child(&mut visit)
    }

    fn for_each_memreserve(&self, visit: &mut impl FnMut(Region)) -> Result<(), Error> {
        let mut offset = 0;
        loop {
            // A map with no terminating (0, 0) entry runs off the blob.
            let base = read_u64(self.reserve_map, offset)?;
            let size = read_u64(self.reserve_map, offset + 8)?;
            if base == 0 && size == 0 {
                return Ok(());
            }
            visit(Region { base, size });
            offset += 16;
        }
    }

    fn for_each_reserved_memory_child(&self, visit: &mut impl FnMut(Region)) -> Result<(), Error> {
        let root = self.root_cells()?;
        let mut cells = root;
        let mut inside = false;
        let mut child = Node::empty(root);
        let mut tokens = Tokens::new(self);
        while let Some(token) = tokens.next_token()? {
            match token {
                Token::BeginNode { name } if tokens.depth == 2 => {
                    inside = name == b"reserved-memory";
                    cells = root;
                }
                Token::Property { name, value } if inside && tokens.depth == 2 => match name {
                    b"#address-cells" => cells.address = single_cell(value)?,
                    b"#size-cells" => cells.size = single_cell(value)?,
                    // An empty `ranges` is the identity; anything else
                    // translates addresses, which is not supported.
                    b"ranges" if !value.is_empty() => return Err(Error::UnsupportedRanges),
                    _ => {}
                },
                Token::BeginNode { .. } if inside && tokens.depth == 3 => {
                    child = Node::empty(cells);
                }
                Token::Property { name, value } if inside && tokens.depth == 3 => {
                    child.record(name, value);
                }
                // `depth` already counts the node this token closes as closed.
                Token::EndNode if inside && tokens.depth == 2 => {
                    if child.is_enabled() && child.reg.is_some() {
                        child.for_each_entry(&mut *visit)?;
                    }
                }
                Token::EndNode if tokens.depth == 1 => inside = false,
                Token::BeginNode { .. } | Token::Property { .. } | Token::EndNode => {}
            }
        }
        Ok(())
    }

    /// First region of the first enabled node whose `device_type` is `memory`.
    pub fn memory(&self) -> Result<Region, Error> {
        self.first_root_child_region(|node| node.device_type == Some(b"memory\0"))
    }

    /// First region of the first enabled node whose `compatible` list holds `compatible`.
    pub fn device(&self, compatible: &str) -> Result<Region, Error> {
        self.first_root_child_region(|node| node.is_compatible(compatible))
    }

    fn root_cells(&self) -> Result<Cells, Error> {
        let mut cells = Cells {
            address: DEFAULT_ADDRESS_CELLS,
            size: DEFAULT_SIZE_CELLS,
        };
        let mut tokens = Tokens::new(self);
        // Properties precede subnodes, so the root's cells are known by the
        // time the first child starts.
        while let Some(token) = tokens.next_token()? {
            match token {
                Token::BeginNode { .. } if tokens.depth > 1 => break,
                Token::Property { name, value } if tokens.depth == 1 => match name {
                    b"#address-cells" => cells.address = single_cell(value)?,
                    b"#size-cells" => cells.size = single_cell(value)?,
                    _ => {}
                },
                Token::BeginNode { .. } | Token::Property { .. } | Token::EndNode => {}
            }
        }
        Ok(cells)
    }

    fn first_root_child_region(&self, wanted: impl Fn(&Node<'a>) -> bool) -> Result<Region, Error> {
        let mut region = Err(Error::NotFound);
        self.find_root_child(|node| {
            if !wanted(node) {
                return Ok(false);
            }
            region = node.region();
            Ok(true)
        })?;
        region
    }

    /// Calls `visit` for each enabled direct child of the root, once its
    /// properties are known. `visit` returns `true` to stop the walk.
    fn find_root_child(
        &self,
        mut visit: impl FnMut(&Node<'a>) -> Result<bool, Error>,
    ) -> Result<(), Error> {
        let cells = self.root_cells()?;
        let mut tokens = Tokens::new(self);
        let mut current = Node::empty(cells);
        while let Some(token) = tokens.next_token()? {
            match token {
                Token::BeginNode { .. } if tokens.depth == 2 => current = Node::empty(cells),
                Token::Property { name, value } if tokens.depth == 2 => current.record(name, value),
                // `depth` already counts the node this token closes as closed.
                Token::EndNode if tokens.depth == 1 && current.is_enabled() => {
                    if visit(&current)? {
                        return Ok(());
                    }
                }
                Token::BeginNode { .. } | Token::Property { .. } | Token::EndNode => {}
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy)]
struct Cells {
    address: u32,
    size: u32,
}

/// The properties of one direct child of the root that the kernel cares about.
#[derive(Clone, Copy)]
struct Node<'a> {
    cells: Cells,
    device_type: Option<&'a [u8]>,
    compatible: Option<&'a [u8]>,
    status: Option<&'a [u8]>,
    reg: Option<&'a [u8]>,
}

impl<'a> Node<'a> {
    fn empty(cells: Cells) -> Self {
        Self {
            cells,
            device_type: None,
            compatible: None,
            status: None,
            reg: None,
        }
    }

    fn record(&mut self, name: &[u8], value: &'a [u8]) {
        match name {
            b"device_type" => self.device_type = Some(value),
            b"compatible" => self.compatible = Some(value),
            b"status" => self.status = Some(value),
            b"reg" => self.reg = Some(value),
            _ => {}
        }
    }

    /// A missing `status` means enabled; so do `okay` and `ok`.
    fn is_enabled(&self) -> bool {
        matches!(self.status, None | Some(b"okay\0") | Some(b"ok\0"))
    }

    fn is_compatible(&self, wanted: &str) -> bool {
        let Some(list) = self.compatible else {
            return false;
        };
        // A string list is NUL-terminated; anything else is not trusted.
        list.last() == Some(&0)
            && list[..list.len() - 1]
                .split(|&byte| byte == 0)
                .any(|entry| entry == wanted.as_bytes())
    }

    /// The `reg` bytes with the sizes of one address and of one whole entry.
    fn reg_layout(&self) -> Result<(&'a [u8], usize, usize), Error> {
        let reg = self.reg.ok_or(Error::MalformedReg)?;
        let (address_cells, size_cells) = (self.cells.address, self.cells.size);
        if !(1..=2).contains(&address_cells) || !(1..=2).contains(&size_cells) {
            return Err(Error::UnsupportedCells);
        }
        let address_bytes = to_usize(address_cells)? * 4;
        let entry_bytes = address_bytes + to_usize(size_cells)? * 4;
        Ok((reg, address_bytes, entry_bytes))
    }

    /// Every entry of `reg`, unchecked beyond the layout: a trailing partial
    /// entry is an error, but a size of zero or a wrapping range is passed on.
    fn for_each_entry(&self, mut visit: impl FnMut(Region)) -> Result<(), Error> {
        let (reg, address_bytes, entry_bytes) = self.reg_layout()?;
        if !reg.len().is_multiple_of(entry_bytes) {
            return Err(Error::MalformedReg);
        }
        for entry in reg.chunks_exact(entry_bytes) {
            visit(Region {
                base: read_cells(&entry[..address_bytes]),
                size: read_cells(&entry[address_bytes..]),
            });
        }
        Ok(())
    }

    fn region(&self) -> Result<Region, Error> {
        let (reg, address_bytes, entry_bytes) = self.reg_layout()?;
        if reg.len() < entry_bytes {
            return Err(Error::MalformedReg);
        }
        let base = read_cells(&reg[..address_bytes]);
        let size = read_cells(&reg[address_bytes..entry_bytes]);
        if size == 0 || base.checked_add(size).is_none() {
            return Err(Error::MalformedReg);
        }
        Ok(Region { base, size })
    }
}

enum Token<'a> {
    BeginNode { name: &'a [u8] },
    Property { name: &'a [u8], value: &'a [u8] },
    EndNode,
}

/// Walks the structure block. Ends at `FDT_END`; a block that runs out before
/// it, or closes more nodes than it opened, is an error.
struct Tokens<'a> {
    structure: &'a [u8],
    strings: &'a [u8],
    position: usize,
    depth: u32,
    finished: bool,
}

impl<'a> Tokens<'a> {
    fn new(fdt: &Fdt<'a>) -> Self {
        Self {
            structure: fdt.structure,
            strings: fdt.strings,
            position: 0,
            depth: 0,
            finished: false,
        }
    }

    fn next_token(&mut self) -> Result<Option<Token<'a>>, Error> {
        if self.finished {
            return Ok(None);
        }
        let structure = self.structure;
        loop {
            let token = read_u32(structure, self.position).map_err(|_| Error::Truncated)?;
            self.position += 4;
            match token {
                TOKEN_NOP => {}
                TOKEN_END => {
                    self.finished = true;
                    return if self.depth == 0 {
                        Ok(None)
                    } else {
                        Err(Error::Truncated)
                    };
                }
                TOKEN_BEGIN_NODE => {
                    let name = c_string(structure.get(self.position..).ok_or(Error::Truncated)?)?;
                    self.position = align4(self.position + name.len() + 1)?;
                    self.depth += 1;
                    return Ok(Some(Token::BeginNode { name }));
                }
                TOKEN_END_NODE => {
                    self.depth = self.depth.checked_sub(1).ok_or(Error::BadToken)?;
                    return Ok(Some(Token::EndNode));
                }
                TOKEN_PROP => {
                    let length = to_usize(
                        read_u32(structure, self.position).map_err(|_| Error::Truncated)?,
                    )?;
                    let name_offset = to_usize(
                        read_u32(structure, self.position + 4).map_err(|_| Error::Truncated)?,
                    )?;
                    let value_start = self.position + 8;
                    let value_end = value_start.checked_add(length).ok_or(Error::Truncated)?;
                    let value = structure
                        .get(value_start..value_end)
                        .ok_or(Error::Truncated)?;
                    let name = c_string(self.strings.get(name_offset..).ok_or(Error::BadString)?)?;
                    self.position = align4(value_end)?;
                    return Ok(Some(Token::Property { name, value }));
                }
                _ => return Err(Error::BadToken),
            }
        }
    }
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, Error> {
    let end = offset.checked_add(4).ok_or(Error::Truncated)?;
    let word = bytes.get(offset..end).ok_or(Error::Truncated)?;
    Ok(u32::from_be_bytes([word[0], word[1], word[2], word[3]]))
}

fn read_u64(bytes: &[u8], offset: usize) -> Result<u64, Error> {
    let end = offset.checked_add(8).ok_or(Error::Truncated)?;
    let word = bytes.get(offset..end).ok_or(Error::Truncated)?;
    Ok(read_cells(word))
}

fn to_usize(value: u32) -> Result<usize, Error> {
    usize::try_from(value).map_err(|_| Error::BadHeader)
}

fn align4(value: usize) -> Result<usize, Error> {
    value.checked_add(3).map(|v| v & !3).ok_or(Error::Truncated)
}

fn sub_block(blob: &[u8], offset: usize, size: usize) -> Result<&[u8], Error> {
    let end = offset.checked_add(size).ok_or(Error::BadHeader)?;
    blob.get(offset..end).ok_or(Error::BadHeader)
}

/// The bytes before the first NUL, which must exist.
fn c_string(bytes: &[u8]) -> Result<&[u8], Error> {
    let length = bytes
        .iter()
        .position(|&byte| byte == 0)
        .ok_or(Error::BadString)?;
    Ok(&bytes[..length])
}

fn single_cell(value: &[u8]) -> Result<u32, Error> {
    match value {
        [a, b, c, d] => Ok(u32::from_be_bytes([*a, *b, *c, *d])),
        _ => Err(Error::UnsupportedCells),
    }
}

/// Joins one or two big-endian cells; the caller passes 4 or 8 bytes.
fn read_cells(bytes: &[u8]) -> u64 {
    bytes
        .iter()
        .fold(0u64, |acc, &byte| (acc << 8) | u64::from(byte))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Builder {
        structure: Vec<u8>,
        strings: Vec<u8>,
        memreserve: Vec<(u64, u64)>,
    }

    impl Builder {
        fn new() -> Self {
            Self {
                structure: Vec::new(),
                strings: Vec::new(),
                memreserve: Vec::new(),
            }
        }

        fn word(&mut self, value: u32) {
            self.structure.extend_from_slice(&value.to_be_bytes());
        }

        fn pad(&mut self) {
            while !self.structure.len().is_multiple_of(4) {
                self.structure.push(0);
            }
        }

        fn begin(&mut self, name: &str) -> &mut Self {
            self.word(TOKEN_BEGIN_NODE);
            self.structure.extend_from_slice(name.as_bytes());
            self.structure.push(0);
            self.pad();
            self
        }

        fn end(&mut self) -> &mut Self {
            self.word(TOKEN_END_NODE);
            self
        }

        fn prop(&mut self, name: &str, value: &[u8]) -> &mut Self {
            let offset = self.strings.len() as u32;
            self.strings.extend_from_slice(name.as_bytes());
            self.strings.push(0);
            self.word(TOKEN_PROP);
            self.word(value.len() as u32);
            self.word(offset);
            self.structure.extend_from_slice(value);
            self.pad();
            self
        }

        fn cells(&mut self, name: &str, cells: &[u32]) -> &mut Self {
            let bytes: Vec<u8> = cells.iter().flat_map(|c| c.to_be_bytes()).collect();
            self.prop(name, &bytes)
        }

        fn finish(&mut self) -> Vec<u8> {
            self.word(TOKEN_END);
            let structure_offset = HEADER_SIZE + 16 * (self.memreserve.len() + 1);
            let strings_offset = structure_offset + self.structure.len();
            let total = strings_offset + self.strings.len();
            let mut blob = Vec::new();
            for word in [
                MAGIC,
                total as u32,
                structure_offset as u32,
                strings_offset as u32,
                HEADER_SIZE as u32,
                17,
                16,
                0,
                self.strings.len() as u32,
                self.structure.len() as u32,
            ] {
                blob.extend_from_slice(&word.to_be_bytes());
            }
            for (base, size) in &self.memreserve {
                blob.extend_from_slice(&base.to_be_bytes());
                blob.extend_from_slice(&size.to_be_bytes());
            }
            blob.extend_from_slice(&[0; 16]);
            blob.extend_from_slice(&self.structure);
            blob.extend_from_slice(&self.strings);
            blob
        }
    }

    fn qemu_like(memory_size: u32, uart_status: Option<&str>) -> Vec<u8> {
        let mut b = Builder::new();
        b.begin("");
        b.cells("#address-cells", &[2]).cells("#size-cells", &[2]);
        b.begin("memory@40000000");
        b.prop("device_type", b"memory\0")
            .cells("reg", &[0, 0x4000_0000, 0, memory_size]);
        b.end();
        b.begin("pl011@9000000");
        b.prop("compatible", b"arm,pl011\0arm,primecell\0")
            .cells("reg", &[0, 0x0900_0000, 0, 0x1000]);
        if let Some(status) = uart_status {
            b.prop("status", format!("{status}\0").as_bytes());
        }
        b.end();
        b.end();
        b.finish()
    }

    #[test]
    fn reads_memory_and_uart() {
        let blob = qemu_like(0x1000_0000, None);
        let fdt = Fdt::parse(&blob).unwrap();
        assert_eq!(
            fdt.memory(),
            Ok(Region {
                base: 0x4000_0000,
                size: 0x1000_0000
            })
        );
        assert_eq!(
            fdt.device("arm,pl011"),
            Ok(Region {
                base: 0x0900_0000,
                size: 0x1000
            })
        );
        assert_eq!(
            fdt.device("arm,primecell"),
            Ok(Region {
                base: 0x0900_0000,
                size: 0x1000
            })
        );
    }

    #[test]
    fn the_blob_is_what_the_header_claims_and_nothing_after_it() {
        let mut blob = qemu_like(0x1000_0000, None);
        let claimed = blob.len();
        blob.extend_from_slice(&[0xaa; 64]);
        let fdt = Fdt::parse(&blob).unwrap();
        assert_eq!(fdt.size(), claimed);
        assert_eq!(fdt.blob(), &blob[..claimed]);
    }

    #[test]
    fn memory_size_follows_the_blob() {
        for size in [0x0800_0000, 0x2000_0000] {
            let blob = qemu_like(size, None);
            assert_eq!(
                Fdt::parse(&blob).unwrap().memory().unwrap().size,
                u64::from(size)
            );
        }
    }

    #[test]
    fn missing_device_is_not_found() {
        let blob = qemu_like(0x1000_0000, None);
        assert_eq!(
            Fdt::parse(&blob).unwrap().device("arm,pl01"),
            Err(Error::NotFound)
        );
    }

    #[test]
    fn disabled_device_is_skipped() {
        let blob = qemu_like(0x1000_0000, Some("disabled"));
        assert_eq!(
            Fdt::parse(&blob).unwrap().device("arm,pl011"),
            Err(Error::NotFound)
        );
        let blob = qemu_like(0x1000_0000, Some("okay"));
        assert!(Fdt::parse(&blob).unwrap().device("arm,pl011").is_ok());
    }

    #[test]
    fn root_without_cell_counts_uses_the_specification_defaults() {
        let mut b = Builder::new();
        b.begin("");
        b.begin("memory");
        b.prop("device_type", b"memory\0")
            .cells("reg", &[0, 0x4000_0000, 0x0800_0000]);
        b.end().end();
        let blob = b.finish();
        assert_eq!(
            Fdt::parse(&blob).unwrap().memory(),
            Ok(Region {
                base: 0x4000_0000,
                size: 0x0800_0000
            })
        );
    }

    #[test]
    fn single_cell_addresses_are_read() {
        let mut b = Builder::new();
        b.begin("");
        b.cells("#address-cells", &[1]).cells("#size-cells", &[1]);
        b.begin("memory");
        b.prop("device_type", b"memory\0")
            .cells("reg", &[0x8000_0000, 0x4000_0000]);
        b.end().end();
        let blob = b.finish();
        assert_eq!(
            Fdt::parse(&blob).unwrap().memory(),
            Ok(Region {
                base: 0x8000_0000,
                size: 0x4000_0000
            })
        );
    }

    #[test]
    fn unsupported_cell_counts_are_rejected() {
        let mut b = Builder::new();
        b.begin("");
        b.cells("#address-cells", &[3]).cells("#size-cells", &[1]);
        b.begin("memory");
        b.prop("device_type", b"memory\0")
            .cells("reg", &[0, 0, 1, 1]);
        b.end().end();
        let blob = b.finish();
        assert_eq!(
            Fdt::parse(&blob).unwrap().memory(),
            Err(Error::UnsupportedCells)
        );
    }

    #[test]
    fn malformed_reg_is_rejected() {
        let cases: [&[u32]; 4] = [
            &[0, 0x4000_0000, 0],
            &[0, 0x4000_0000, 0, 0],
            &[0xffff_ffff, 0xffff_ffff, 0, 1],
            &[],
        ];
        for reg in cases {
            let mut b = Builder::new();
            b.begin("")
                .cells("#address-cells", &[2])
                .cells("#size-cells", &[2]);
            b.begin("memory@0")
                .prop("device_type", b"memory\0")
                .cells("reg", reg)
                .end();
            b.end();
            let blob = b.finish();
            assert_eq!(
                Fdt::parse(&blob).unwrap().memory(),
                Err(Error::MalformedReg),
                "{reg:x?}"
            );
        }
    }

    #[test]
    fn nodes_below_another_bus_are_not_used() {
        let mut b = Builder::new();
        b.begin("")
            .cells("#address-cells", &[2])
            .cells("#size-cells", &[2]);
        b.begin("soc");
        b.begin("serial@1000");
        b.prop("compatible", b"arm,pl011\0")
            .cells("reg", &[0, 0x1000, 0, 0x1000]);
        b.end().end().end();
        let blob = b.finish();
        assert_eq!(
            Fdt::parse(&blob).unwrap().device("arm,pl011"),
            Err(Error::NotFound)
        );
    }

    #[test]
    fn compatible_list_must_be_terminated() {
        let mut b = Builder::new();
        b.begin("")
            .cells("#address-cells", &[2])
            .cells("#size-cells", &[2]);
        b.begin("uart");
        b.prop("compatible", b"arm,pl011")
            .cells("reg", &[0, 0x1000, 0, 0x1000]);
        b.end().end();
        let blob = b.finish();
        assert_eq!(
            Fdt::parse(&blob).unwrap().device("arm,pl011"),
            Err(Error::NotFound)
        );
    }

    fn patch(blob: &mut [u8], offset: usize, value: u32) {
        blob[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
    }

    #[test]
    fn header_is_validated() {
        let good = qemu_like(0x1000_0000, None);
        let cases: [(usize, u32, Error); 9] = [
            (0, 0, Error::BadMagic),
            (4, 8, Error::BadTotalSize),
            (4, 0x20_0001, Error::BadTotalSize),
            (4, good.len() as u32 + 1, Error::BadTotalSize),
            (20, 16, Error::UnsupportedVersion),
            (24, 18, Error::UnsupportedVersion),
            (8, 0xffff_fffc, Error::BadHeader),
            (12, good.len() as u32, Error::BadHeader),
            (36, good.len() as u32, Error::BadHeader),
        ];
        for (offset, value, expected) in cases {
            let mut blob = good.clone();
            patch(&mut blob, offset, value);
            assert_eq!(Fdt::parse(&blob).err(), Some(expected), "offset {offset}");
        }
        let mut blob = good.clone();
        patch(&mut blob, 8, 0x2e);
        assert_eq!(Fdt::parse(&blob).err(), Some(Error::BadHeader));
        assert_eq!(Fdt::parse(&good[..20]).err(), Some(Error::BadTotalSize));
    }

    #[test]
    fn structure_errors_are_reported() {
        let mut b = Builder::new();
        b.begin("").end();
        b.structure.truncate(b.structure.len() - 4);
        let blob = b.finish();
        assert_eq!(Fdt::parse(&blob).unwrap().memory(), Err(Error::Truncated));

        let mut b = Builder::new();
        b.end();
        let blob = b.finish();
        assert_eq!(Fdt::parse(&blob).unwrap().memory(), Err(Error::BadToken));

        let mut b = Builder::new();
        b.word(0x77);
        let blob = b.finish();
        assert_eq!(Fdt::parse(&blob).unwrap().memory(), Err(Error::BadToken));

        let mut b = Builder::new();
        b.begin("");
        let mut blob = b.finish();
        // Cut the structure block short of its FDT_END without fixing the header.
        let size = blob.len();
        patch(&mut blob, 36, 4 + 4);
        patch(&mut blob, 4, size as u32);
        assert_eq!(Fdt::parse(&blob).unwrap().memory(), Err(Error::Truncated));
    }

    #[test]
    fn property_bounds_are_checked() {
        let mut b = Builder::new();
        b.begin("");
        b.prop("x", b"abcd");
        b.end();
        let good = b.finish();
        // The property length sits 4 bytes after its token: header (56) + root begin (8) + token (4).
        let length_offset = 56 + 8 + 4;
        for length in [0xffff_ffff_u32, 0x1000, 0x8000_0000] {
            let mut blob = good.clone();
            patch(&mut blob, length_offset, length);
            assert!(Fdt::parse(&blob).unwrap().memory().is_err());
        }
        let mut blob = good.clone();
        patch(&mut blob, length_offset + 4, 0xffff_ffff);
        assert_eq!(Fdt::parse(&blob).unwrap().memory(), Err(Error::BadString));
    }

    #[test]
    fn unterminated_name_is_rejected() {
        let mut b = Builder::new();
        b.begin("");
        let mut blob = b.finish();
        // Overwrite the node's padded name word (0) with non-zero bytes.
        let name_offset = 56 + 4;
        blob[name_offset..name_offset + 4].copy_from_slice(b"abcd");
        // The following word is the END_NODE/END token area; it is non-zero too.
        assert!(Fdt::parse(&blob).unwrap().memory().is_err());
    }

    #[test]
    fn address_checks() {
        // SAFETY: a null address is rejected before any read.
        let null = unsafe { Fdt::from_address(0) };
        assert_eq!(null.err(), Some(Error::NullPointer));
        // SAFETY: a misaligned address is rejected before any read.
        let misaligned = unsafe { Fdt::from_address(0x1004) };
        assert_eq!(misaligned.err(), Some(Error::Misaligned));
        let words: Vec<u64> = qemu_like(0x1000_0000, None)
            .chunks(8)
            .map(|c| {
                let mut a = [0u8; 8];
                a[..c.len()].copy_from_slice(c);
                u64::from_ne_bytes(a)
            })
            .collect();
        // SAFETY: `words` holds the whole blob, 8-byte aligned, and outlives `fdt`.
        let fdt = unsafe { Fdt::from_address(words.as_ptr() as usize) }.unwrap();
        assert_eq!(fdt.memory().unwrap().size, 0x1000_0000);
    }

    #[test]
    fn corrupted_blobs_never_panic() {
        let good = qemu_like(0x1000_0000, None);
        for length in 0..good.len() {
            if let Ok(fdt) = Fdt::parse(&good[..length]) {
                let _ = (fdt.memory(), fdt.device("arm,pl011"));
            }
        }
        for index in 0..good.len() {
            for flip in [0x01, 0x80, 0xff] {
                let mut blob = good.clone();
                blob[index] ^= flip;
                if let Ok(fdt) = Fdt::parse(&blob) {
                    let _ = (fdt.memory(), fdt.device("arm,pl011"));
                }
            }
        }
    }

    fn memory_node(b: &mut Builder, name: &str, reg: &[u32]) {
        b.begin(name)
            .prop("device_type", b"memory\0")
            .cells("reg", reg)
            .end();
    }

    fn root(b: &mut Builder) {
        b.begin("")
            .cells("#address-cells", &[2])
            .cells("#size-cells", &[2]);
    }

    fn memory_of(blob: &[u8]) -> Result<Vec<Region>, Error> {
        let mut regions = Vec::new();
        Fdt::parse(blob)?.for_each_memory_region(|r| regions.push(r))?;
        Ok(regions)
    }

    fn reserved_of(blob: &[u8]) -> Result<Vec<Region>, Error> {
        let mut regions = Vec::new();
        Fdt::parse(blob)?.for_each_reserved_region(|r| regions.push(r))?;
        Ok(regions)
    }

    fn region(base: u64, size: u64) -> Region {
        Region { base, size }
    }

    #[test]
    fn size_is_the_header_total() {
        let blob = qemu_like(0x1000_0000, None);
        assert_eq!(Fdt::parse(&blob).unwrap().size(), blob.len());
    }

    #[test]
    fn every_memory_entry_of_every_node_is_visited() {
        let mut b = Builder::new();
        root(&mut b);
        memory_node(
            &mut b,
            "memory@40000000",
            &[0, 0x4000_0000, 0, 0x1000, 0, 0x8000_0000, 0, 0x2000],
        );
        memory_node(&mut b, "memory@c0000000", &[0, 0xc000_0000, 0, 0x3000]);
        b.begin("memory@disabled")
            .prop("device_type", b"memory\0")
            .prop("status", b"disabled\0")
            .cells("reg", &[0, 0x1_0000, 0, 0x1000])
            .end();
        b.begin("not-memory")
            .cells("reg", &[0, 0x2_0000, 0, 0x1000])
            .end();
        b.end();
        assert_eq!(
            memory_of(&b.finish()),
            Ok(vec![
                region(0x4000_0000, 0x1000),
                region(0x8000_0000, 0x2000),
                region(0xc000_0000, 0x3000),
            ])
        );
    }

    #[test]
    fn hostile_memory_entries_are_passed_on_as_found() {
        let mut b = Builder::new();
        root(&mut b);
        memory_node(
            &mut b,
            "memory@0",
            &[0, 0x1000, 0, 0, 0xffff_ffff, 0xffff_f000, 0, 0x2000],
        );
        b.end();
        assert_eq!(
            memory_of(&b.finish()),
            Ok(vec![
                region(0x1000, 0),
                region(0xffff_ffff_ffff_f000, 0x2000)
            ])
        );
    }

    #[test]
    fn memory_reg_with_a_partial_entry_is_rejected() {
        let mut b = Builder::new();
        root(&mut b);
        memory_node(&mut b, "memory@0", &[0, 0x1000, 0, 0x1000, 0, 0x2000]);
        b.end();
        assert_eq!(memory_of(&b.finish()), Err(Error::MalformedReg));
    }

    #[test]
    fn memory_node_without_reg_is_skipped_by_the_walker_but_not_by_memory() {
        let mut b = Builder::new();
        root(&mut b);
        b.begin("memory").prop("device_type", b"memory\0").end();
        memory_node(&mut b, "memory@1000", &[0, 0x1000, 0, 0x1000]);
        b.end();
        let blob = b.finish();
        assert_eq!(memory_of(&blob), Ok(vec![region(0x1000, 0x1000)]));
        assert_eq!(
            Fdt::parse(&blob).unwrap().memory(),
            Err(Error::MalformedReg)
        );
    }

    #[test]
    fn no_memory_node_visits_nothing() {
        let mut b = Builder::new();
        root(&mut b);
        b.end();
        assert_eq!(memory_of(&b.finish()), Ok(vec![]));
    }

    #[test]
    fn memreserve_entries_are_reported() {
        let mut b = Builder::new();
        b.memreserve = vec![(0x4000_0000, 0x1000), (0x5000_0000, 0), (u64::MAX, 2)];
        root(&mut b);
        b.end();
        assert_eq!(
            reserved_of(&b.finish()),
            Ok(vec![
                region(0x4000_0000, 0x1000),
                region(0x5000_0000, 0),
                region(u64::MAX, 2)
            ])
        );
    }

    #[test]
    fn memreserve_without_terminator_is_rejected() {
        let mut b = Builder::new();
        root(&mut b);
        b.end();
        let mut blob = b.finish();
        // Point the reservation map at 8 trailing bytes: a lone half entry.
        let at = blob.len().next_multiple_of(8);
        blob.resize(at + 8, 0xff);
        let total = blob.len() as u32;
        patch(&mut blob, 4, total);
        patch(&mut blob, 16, at as u32);
        assert_eq!(reserved_of(&blob), Err(Error::Truncated));
        // And past the end of the blob altogether.
        let past = blob.len() as u32 + 8;
        patch(&mut blob, 16, past);
        assert_eq!(Fdt::parse(&blob).err(), Some(Error::BadHeader));
        // Misaligned maps are refused.
        patch(&mut blob, 16, 44);
        assert_eq!(Fdt::parse(&blob).err(), Some(Error::BadHeader));
    }

    fn reserved_memory_tree(extra: impl FnOnce(&mut Builder)) -> Vec<u8> {
        let mut b = Builder::new();
        root(&mut b);
        memory_node(&mut b, "memory@40000000", &[0, 0x4000_0000, 0, 0x1000_0000]);
        b.begin("reserved-memory")
            .cells("#address-cells", &[2])
            .cells("#size-cells", &[2])
            .prop("ranges", b"");
        b.begin("fw@45000000")
            .cells("reg", &[0, 0x4500_0000, 0, 0x1_0000])
            .prop("no-map", b"")
            .end();
        b.begin("pool@46000000")
            .cells(
                "reg",
                &[0, 0x4600_0000, 0, 0x2000, 0, 0x4700_0000, 0, 0x1000],
            )
            .end();
        b.begin("dynamic").cells("size", &[0, 0x10_0000]).end();
        b.begin("off")
            .cells("reg", &[0, 0x4800_0000, 0, 0x1000])
            .prop("status", b"disabled\0")
            .end();
        b.end();
        extra(&mut b);
        b.end();
        b.finish()
    }

    #[test]
    fn reserved_memory_children_are_reported() {
        let blob = reserved_memory_tree(|_| {});
        assert_eq!(
            reserved_of(&blob),
            Ok(vec![
                region(0x4500_0000, 0x1_0000),
                region(0x4600_0000, 0x2000),
                region(0x4700_0000, 0x1000),
            ])
        );
    }

    #[test]
    fn reserved_memory_uses_its_own_cell_counts() {
        let mut b = Builder::new();
        root(&mut b);
        b.begin("reserved-memory")
            .cells("#address-cells", &[1])
            .cells("#size-cells", &[1]);
        b.begin("fw").cells("reg", &[0x4500_0000, 0x1000]).end();
        b.end().end();
        assert_eq!(
            reserved_of(&b.finish()),
            Ok(vec![region(0x4500_0000, 0x1000)])
        );
    }

    #[test]
    fn reserved_memory_translation_is_refused() {
        let mut b = Builder::new();
        root(&mut b);
        b.begin("reserved-memory")
            .cells("ranges", &[0, 0, 0, 0, 0x1000]);
        b.begin("fw").cells("reg", &[0, 0x100, 0, 0x1000]).end();
        b.end().end();
        assert_eq!(reserved_of(&b.finish()), Err(Error::UnsupportedRanges));
    }

    #[test]
    fn reserved_memory_with_bad_reg_is_rejected() {
        let mut b = Builder::new();
        root(&mut b);
        b.begin("reserved-memory");
        b.begin("fw").cells("reg", &[0, 0x100, 0]).end();
        b.end().end();
        assert_eq!(reserved_of(&b.finish()), Err(Error::MalformedReg));
    }

    #[test]
    fn nested_nodes_in_reserved_memory_do_not_confuse_the_walk() {
        let blob = reserved_memory_tree(|b| {
            // A second root child with a `reg`, and a node that merely has
            // the same name below another parent.
            b.begin("soc");
            b.begin("reserved-memory");
            b.begin("fw")
                .cells("reg", &[0, 0x9000_0000, 0, 0x1000])
                .end();
            b.end().end();
        });
        assert_eq!(reserved_of(&blob).unwrap().len(), 3);
    }

    #[test]
    fn reservation_walkers_never_panic_on_corrupted_blobs() {
        let good = reserved_memory_tree(|_| {});
        for index in 0..good.len() {
            for flip in [0x01, 0x80, 0xff] {
                let mut blob = good.clone();
                blob[index] ^= flip;
                if let Ok(fdt) = Fdt::parse(&blob) {
                    let _ = fdt.for_each_memory_region(|_| {});
                    let _ = fdt.for_each_reserved_region(|_| {});
                }
            }
        }
    }
}
