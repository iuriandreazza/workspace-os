//! What the kernel knows about the machine it runs on, read from the device
//! tree (direct load) or from the bootloader's boot info (ADR 0013) instead of
//! being fixed in the source.

use alloc::boxed::Box;
use alloc::vec::Vec;
use core::fmt;
use core::sync::atomic::{AtomicUsize, Ordering};

use bootinfo::{BootInfo, MemoryKind, MemoryMap};
use devicetree::{Error, Fdt, Region};

use crate::arch::pointer_to;
use crate::drivers::pl011::Pl011;

/// QEMU `virt` puts the device tree blob at the start of RAM when the kernel
/// image does not overlap it (see `link.ld`). It is the only place to look when
/// the loader does not pass the address, as with `-kernel` on an ELF or
/// `-device loader`.
const DEVICE_TREE_AT_RAM_START: usize = 0x4000_0000;

const UART_COMPATIBLE: &str = "arm,pl011";

/// Where the console UART's registers are read and written: the physical
/// address from the moment the hand-over has been read, and an address in the
/// device window once the kernel's tables are live (ADR 0030). Zero until the
/// hand-over has been read: output is dropped.
static CONSOLE_BASE: AtomicUsize = AtomicUsize::new(0);

/// Points the console at `base`. The caller makes sure it is mapped: this is
/// called once for the address the hand-over names, and once, between the
/// switch to the kernel's tables and the drop of the lower half, for the
/// device window.
pub fn use_console_at(base: usize) {
    CONSOLE_BASE.store(base, Ordering::Relaxed);
}

/// Where the console is read and written now.
pub fn console_base() -> usize {
    CONSOLE_BASE.load(Ordering::Relaxed)
}

/// How the kernel was entered, for the log and for tests to read.
#[derive(Clone, Copy)]
pub enum Entry {
    /// Loaded straight into memory by QEMU: the device tree is the only source.
    Direct,
    Bootloader {
        version_major: u16,
        version_minor: u16,
        boot_mode: u32,
        map_entries: usize,
        boot_cpu: u64,
        rsdp: u64,
        efi_system_table: u64,
    },
}

pub struct Board {
    /// For the banner: the lowest RAM address and the amount of RAM.
    pub memory: Region,
    pub uart: Region,
    /// Where the device tree blob lives: the allocator must not hand it out.
    /// Empty when the bootloader passed none.
    pub device_tree: Region,
    pub entry: Entry,
    fdt: Option<Fdt<'static>>,
    /// Present only on the bootloader path, where it is the authority for RAM.
    map: Option<MemoryMap<'static>>,
}

impl Board {
    /// The bootloader's memory map, entry by entry, for the log. A direct load
    /// has none.
    pub fn map_entries(&self) -> Option<impl Iterator<Item = bootinfo::MemoryEntry>> {
        self.map.as_ref().map(MemoryMap::iter)
    }

    /// Calls `visit` with each region of RAM. From the bootloader's map that
    /// is everything backed by RAM, whether the kernel may use it or not: what
    /// it may not use comes back from `for_each_reserved_region`.
    pub fn for_each_memory_region(&self, mut visit: impl FnMut(Region)) -> Result<(), Error> {
        if let Some(map) = &self.map {
            map.iter()
                .filter(|entry| entry.kind().is_ram())
                .for_each(|entry| {
                    visit(Region {
                        base: entry.base,
                        size: entry.length,
                    })
                });
            return Ok(());
        }
        match &self.fdt {
            Some(fdt) => fdt.for_each_memory_region(visit),
            None => Ok(()),
        }
    }

    /// Calls `visit` with each region of RAM nothing has a claim on: Usable
    /// memory of the bootloader's map, or the device tree's memory nodes on a
    /// direct load. Reservations (the tree's own, the kernel image) are not
    /// subtracted.
    pub fn for_each_usable_region(&self, mut visit: impl FnMut(Region)) -> Result<(), Error> {
        if let Some(map) = &self.map {
            map.iter()
                .filter(|entry| entry.kind() == MemoryKind::Usable)
                .for_each(|entry| {
                    visit(Region {
                        base: entry.base,
                        size: entry.length,
                    })
                });
            return Ok(());
        }
        match &self.fdt {
            Some(fdt) => fdt.for_each_memory_region(visit),
            None => Ok(()),
        }
    }

    /// Calls `visit` with each region the frame allocator must not hand out
    /// besides the kernel's own: the device tree's reservations and, on the
    /// bootloader path, memory the bootloader still uses (ADR 0013).
    pub fn for_each_reserved_region(&self, mut visit: impl FnMut(Region)) -> Result<(), Error> {
        if let Some(map) = &self.map {
            map.iter()
                .filter(|entry| entry.kind().is_ram() && entry.kind() != MemoryKind::Usable)
                .for_each(|entry| {
                    visit(Region {
                        base: entry.base,
                        size: entry.length,
                    })
                });
        }
        self.for_each_tree_reservation(visit)
    }
}

/// What reading the hand-over (the boot info, the memory map and the device
/// tree, or the device tree alone) gave: how many bytes went into the digest
/// and the digest itself, FNV-1a over 64 bits. Two reads that disagree are not
/// reading the same memory.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct HandoffDigest {
    pub bytes: usize,
    pub value: u64,
}

impl HandoffDigest {
    const OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;

    fn new() -> Self {
        Self {
            bytes: 0,
            value: Self::OFFSET_BASIS,
        }
    }

    fn bytes(&mut self, data: &[u8]) {
        for &byte in data {
            self.value = (self.value ^ u64::from(byte)).wrapping_mul(Self::PRIME);
        }
        self.bytes += data.len();
    }

    fn number(&mut self, value: u64) {
        self.bytes(&value.to_le_bytes());
    }

    fn region(&mut self, region: &Region) {
        self.number(region.base);
        self.number(region.size);
    }
}

impl Board {
    /// A digest of everything the kernel read from the hand-over: the fields of
    /// the boot info, every entry of the memory map, every region the memory
    /// and reservation walks give, and all the bytes of the device tree blob.
    /// It is taken before the kernel switches to its own tables and again,
    /// from a fresh read, after (ADR 0026, 0030): the second read goes through
    /// the linear map, so a map that lost or misplaced a region differs. It is
    /// taken a third time from the copy the kernel makes before it gives the
    /// boot memory away, and a fourth after that memory has been used: a board
    /// that still pointed into it would differ.
    pub fn handoff_digest(&self) -> HandoffDigest {
        let mut digest = HandoffDigest::new();
        digest.region(&self.memory);
        digest.region(&self.uart);
        digest.region(&self.device_tree);
        match self.entry {
            Entry::Direct => digest.number(0),
            Entry::Bootloader {
                version_major,
                version_minor,
                boot_mode,
                map_entries,
                boot_cpu,
                rsdp,
                efi_system_table,
            } => {
                digest.number(1);
                digest.number(u64::from(version_major));
                digest.number(u64::from(version_minor));
                digest.number(u64::from(boot_mode));
                digest.number(map_entries as u64);
                digest.number(boot_cpu);
                digest.number(rsdp);
                digest.number(efi_system_table);
            }
        }
        for entry in self.map_entries().into_iter().flatten() {
            digest.number(entry.base);
            digest.number(entry.length);
            digest.bytes(entry.kind().name().as_bytes());
        }
        // A tree that cannot be walked ends the digest early, which differs
        // from a read that could.
        let _ = self.for_each_memory_region(|region| digest.region(&region));
        let _ = self.for_each_reserved_region(|region| digest.region(&region));
        if let Some(fdt) = &self.fdt {
            digest.bytes(fdt.blob());
        }
        digest
    }

    /// Reads the hand-over again from where the loader left it, at the physical
    /// address `handoff_address`, through whatever map `pointer_to` says is
    /// live: the boot info on the bootloader path, the device tree where it was
    /// found on the direct one.
    pub fn read_again(&self, handoff_address: usize) -> Result<Board, HandoffError> {
        match self.entry {
            Entry::Direct => read_device_tree(self.device_tree.base as usize)
                .map_err(HandoffError::BadDeviceTree),
            Entry::Bootloader { .. } => from_boot_info(handoff_address).map_err(|(error, _)| error),
        }
    }

    /// The same board with the memory map and the device tree copied into the
    /// kernel's heap (ADR 0030), so that nothing points into memory the
    /// bootloader owned and the kernel is about to give to the frame allocator.
    /// The copies live as long as the kernel. On the direct path the tree is
    /// ordinary RAM the kernel reserved and keeps, and there is no map: the
    /// board is returned as it is.
    pub fn into_owned(self) -> Board {
        let Entry::Bootloader { .. } = self.entry else {
            return self;
        };
        let keep =
            |bytes: &[u8]| -> &'static [u8] { Box::leak(Vec::from(bytes).into_boxed_slice()) };
        let map = self.map.map(|map| {
            let (bytes, entry_size) = map.as_bytes();
            MemoryMap::from_bytes(keep(bytes), entry_size)
                .expect("a map that was valid a moment ago, copied")
        });
        let fdt = self.fdt.map(|fdt| {
            Fdt::parse(keep(fdt.blob())).expect("a device tree that was valid a moment ago, copied")
        });
        Board { map, fdt, ..self }
    }

    /// Calls `visit` with each region of RAM the bootloader says it owns and
    /// the kernel may take once it has copied what it needs (ADR 0013,
    /// BootloaderReclaimable). A direct load has none.
    pub fn for_each_reclaimable_region(&self, mut visit: impl FnMut(Region)) {
        for entry in self.map_entries().into_iter().flatten() {
            if entry.kind() == MemoryKind::BootloaderReclaimable {
                visit(Region {
                    base: entry.base,
                    size: entry.length,
                });
            }
        }
    }

    /// Calls `visit` with each region the device tree itself reserves
    /// (`/memreserve/` and `/reserved-memory`), and not the bootloader's map.
    pub fn for_each_tree_reservation(&self, visit: impl FnMut(Region)) -> Result<(), Error> {
        match &self.fdt {
            Some(fdt) => fdt.for_each_reserved_region(visit),
            None => Ok(()),
        }
    }
}

/// Reads the device tree. The caller makes the console usable (`use_console_at`
/// with the board's UART), because reading the hand-over again later must not
/// change where the console is.
///
/// `boot_address` is what the loader passed in `x0`, zero when it passed
/// nothing. If that blob is absent or invalid, the start of RAM is tried before
/// giving up, and the error of the first attempt is the one reported.
pub fn discover(boot_address: usize) -> Result<Board, Error> {
    let board = read_device_tree(boot_address)
        .or_else(|first| read_device_tree(DEVICE_TREE_AT_RAM_START).map_err(|_| first))?;
    // A UART base that does not fit the address space cannot be used.
    usize::try_from(board.uart.base).map_err(|_| Error::MalformedReg)?;
    Ok(board)
}

/// What stops the kernel when the bootloader's handoff is unusable.
pub enum HandoffError {
    Rejected(bootinfo::HandoffError),
    BadDeviceTree(devicetree::Error),
}

impl fmt::Display for HandoffError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Rejected(error) => write!(f, "{error}"),
            Self::BadDeviceTree(error) => write!(f, "device tree copy: {error}"),
        }
    }
}

const PL011_SIZE: u64 = 0x1000;

/// Reads the boot info the bootloader left at the physical address `address`,
/// through `pointer_to`. On failure, the error comes with the base of the
/// console that can report it, which exists when the header was valid enough to
/// name a usable one. The checks themselves are `bootinfo::check`, tested on
/// the host. The console is the caller's to point at (`use_console_at`).
pub fn from_boot_info(address: usize) -> Result<Board, (HandoffError, Option<usize>)> {
    // SAFETY: the boot code only takes this path when `x1` held the entry
    // magic, so `address` is the structure the bootloader built (ADR 0013),
    // readable at `pointer_to(address)` through the map the kernel runs on.
    let parsed = unsafe { BootInfo::from_address(pointer_to(address as u64)) };
    let handoff = bootinfo::check(parsed, |info| {
        // SAFETY: the structure comes from the bootloader, which keeps the map
        // unchanged and in memory the allocator never hands out until the
        // kernel has copied it (ADR 0030); `pointer_to` is how that memory is
        // read.
        unsafe { info.memory_map(pointer_to) }
    })
    .map_err(|rejected| (HandoffError::Rejected(rejected.error), rejected.console))?;
    let info = handoff.info;

    // The copy of the device tree is optional, and only its reservations are
    // used: the memory map is the authority for RAM.
    let fdt = if info.device_tree == 0 {
        None
    } else {
        // SAFETY: the bootloader copied the blob to `device_tree` and reserves
        // that memory (BootloaderReclaimable) until the kernel has copied it
        // (ADR 0030); it is read at `pointer_to(device_tree)`.
        let blob = unsafe { Fdt::from_address(pointer_to(info.device_tree)) }
            .map_err(|e| (HandoffError::BadDeviceTree(e), Some(handoff.console)))?;
        Some(blob)
    };

    Ok(Board {
        memory: Region {
            base: handoff.ram_lowest,
            size: handoff.ram_total,
        },
        uart: Region {
            base: info.console_base,
            size: PL011_SIZE,
        },
        device_tree: Region {
            base: info.device_tree,
            size: if fdt.is_some() {
                info.device_tree_size
            } else {
                0
            },
        },
        entry: Entry::Bootloader {
            version_major: info.version_major,
            version_minor: info.version_minor,
            boot_mode: info.boot_mode,
            map_entries: handoff.map.len(),
            boot_cpu: info.boot_cpu,
            rsdp: info.rsdp,
            efi_system_table: info.efi_system_table,
        },
        fdt,
        map: Some(handoff.map),
    })
}

fn read_device_tree(address: usize) -> Result<Board, Error> {
    // SAFETY: the address is either the loader's device tree pointer or the
    // start of RAM on QEMU `virt`, where the blob lives, read at
    // `pointer_to(address)`. Nothing writes there: the kernel never does, and
    // `memory::init` reserves the blob so that the frame allocator never hands
    // it out. The contents are validated by the reader; that the claimed size
    // stays inside RAM is trusted from the loader, as the boot protocol
    // requires.
    let fdt = unsafe { Fdt::from_address(pointer_to(address as u64)) }?;
    Ok(Board {
        memory: fdt.memory()?,
        uart: fdt.device(UART_COMPATIBLE)?,
        device_tree: Region {
            base: address as u64,
            size: fdt.size() as u64,
        },
        entry: Entry::Direct,
        fdt: Some(fdt),
        map: None,
    })
}

/// The serial console; output is dropped until `discover` has found the UART.
pub struct Console;

pub fn console() -> Console {
    Console
}

impl fmt::Write for Console {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        let base = CONSOLE_BASE.load(Ordering::Relaxed);
        if base == 0 {
            return Ok(());
        }
        // SAFETY: `base` is the PL011 the device tree describes. The kernel is
        // its only user: the boot code parks every core but the boot core and
        // masks interrupts, so nothing else can drive the UART concurrently.
        unsafe { Pl011::new(base) }.write_str(text)
    }
}
