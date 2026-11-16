//! Minimal flattened device tree (FDT) reader.
//!
//! Written for the kernel, which has no allocator when it first needs the
//! tree, so nothing here allocates. The blob comes from outside the kernel and
//! is treated as untrusted: every offset, length and string is checked against
//! the bounds of the block it lives in before it is used.
//!
//! Only what the kernel asks for is supported: the first `reg` entry of the
//! first usable memory node and of the first usable node with a given
//! `compatible` string, among the direct children of the root node. `reg`
//! values are taken as CPU physical addresses, which holds when the parent bus
//! has no address translation; nodes below another bus are not looked at.

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

pub struct Fdt<'a> {
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

        let structure_offset = to_usize(read_u32(blob, 8)?)?;
        let strings_offset = to_usize(read_u32(blob, 12)?)?;
        let strings_size = to_usize(read_u32(blob, 32)?)?;
        let structure_size = to_usize(read_u32(blob, 36)?)?;
        if !structure_offset.is_multiple_of(4) || !structure_size.is_multiple_of(4) {
            return Err(Error::BadHeader);
        }
        Ok(Fdt {
            structure: sub_block(blob, structure_offset, structure_size)?,
            strings: sub_block(blob, strings_offset, strings_size)?,
        })
    }

    /// First region of the first enabled node whose `device_type` is `memory`.
    pub fn memory(&self) -> Result<Region, Error> {
        let node = self.find_root_child(|node| node.device_type == Some(b"memory\0"))?;
        node.region()
    }

    /// First region of the first enabled node whose `compatible` list holds `compatible`.
    pub fn device(&self, compatible: &str) -> Result<Region, Error> {
        let node = self.find_root_child(|node| node.is_compatible(compatible))?;
        node.region()
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
                Token::BeginNode if tokens.depth > 1 => break,
                Token::Property { name, value } if tokens.depth == 1 => match name {
                    b"#address-cells" => cells.address = single_cell(value)?,
                    b"#size-cells" => cells.size = single_cell(value)?,
                    _ => {}
                },
                Token::BeginNode | Token::Property { .. } | Token::EndNode => {}
            }
        }
        Ok(cells)
    }

    fn find_root_child(&self, wanted: impl Fn(&Node<'a>) -> bool) -> Result<Node<'a>, Error> {
        let cells = self.root_cells()?;
        let mut tokens = Tokens::new(self);
        let mut current = Node::empty(cells);
        while let Some(token) = tokens.next_token()? {
            match token {
                Token::BeginNode if tokens.depth == 2 => current = Node::empty(cells),
                Token::Property { name, value } if tokens.depth == 2 => current.record(name, value),
                // `depth` already counts the node this token closes as closed.
                Token::EndNode if tokens.depth == 1 && current.is_enabled() && wanted(&current) => {
                    return Ok(current);
                }
                Token::BeginNode | Token::Property { .. } | Token::EndNode => {}
            }
        }
        Err(Error::NotFound)
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

    fn region(&self) -> Result<Region, Error> {
        let reg = self.reg.ok_or(Error::MalformedReg)?;
        let (address_cells, size_cells) = (self.cells.address, self.cells.size);
        if !(1..=2).contains(&address_cells) || !(1..=2).contains(&size_cells) {
            return Err(Error::UnsupportedCells);
        }
        let address_bytes = to_usize(address_cells)? * 4;
        let entry_bytes = address_bytes + to_usize(size_cells)? * 4;
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
    BeginNode,
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
                    // The name is only validated: nothing here needs it.
                    let name = c_string(&structure[self.position..])?;
                    self.position = align4(self.position + name.len() + 1)?;
                    self.depth += 1;
                    return Ok(Some(Token::BeginNode));
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
    }

    impl Builder {
        fn new() -> Self {
            Self {
                structure: Vec::new(),
                strings: Vec::new(),
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
            let structure_offset = HEADER_SIZE + 16;
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
}
