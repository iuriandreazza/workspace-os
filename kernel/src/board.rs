//! What the kernel knows about the machine it runs on, read from the device
//! tree (direct load) or from the bootloader's boot info (ADR 0013) instead of
//! being fixed in the source.

use core::fmt;
use core::sync::atomic::{AtomicUsize, Ordering};

use bootinfo::{BootInfo, MemoryKind, MemoryMap};
use devicetree::{Error, Fdt, Region};

use crate::drivers::pl011::Pl011;

/// QEMU `virt` puts the device tree blob at the start of RAM when the kernel
/// image does not overlap it (see `link.ld`). It is the only place to look when
/// the loader does not pass the address, as with `-kernel` on an ELF or
/// `-device loader`.
const DEVICE_TREE_AT_RAM_START: usize = 0x4000_0000;

const UART_COMPATIBLE: &str = "arm,pl011";

/// Base address of the console UART, or 0 until the device tree has been read.
static CONSOLE_BASE: AtomicUsize = AtomicUsize::new(0);

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
                .filter(|entry| is_ram(entry.kind()))
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
                .filter(|entry| is_ram(entry.kind()) && entry.kind() != MemoryKind::Usable)
                .for_each(|entry| {
                    visit(Region {
                        base: entry.base,
                        size: entry.length,
                    })
                });
        }
        match &self.fdt {
            Some(fdt) => fdt.for_each_reserved_region(visit),
            None => Ok(()),
        }
    }
}

/// RAM that something owns or may own later. Firmware regions that are not RAM
/// (runtime services, MMIO, reserved) stay out of the allocator's map.
fn is_ram(kind: MemoryKind) -> bool {
    matches!(
        kind,
        MemoryKind::Usable
            | MemoryKind::BootloaderReclaimable
            | MemoryKind::AcpiReclaimable
            | MemoryKind::Kernel
    )
}

/// Reads the device tree and makes the console usable.
///
/// `boot_address` is what the loader passed in `x0`, zero when it passed
/// nothing. If that blob is absent or invalid, the start of RAM is tried before
/// giving up, and the error of the first attempt is the one reported.
pub fn discover(boot_address: usize) -> Result<Board, Error> {
    let board = read_device_tree(boot_address)
        .or_else(|first| read_device_tree(DEVICE_TREE_AT_RAM_START).map_err(|_| first))?;
    // A UART base that does not fit the address space cannot be used.
    let base = usize::try_from(board.uart.base).map_err(|_| Error::MalformedReg)?;
    CONSOLE_BASE.store(base, Ordering::Relaxed);
    Ok(board)
}

/// What stops the kernel when the bootloader's handoff is unusable.
pub enum HandoffError {
    Info(bootinfo::Error),
    UnsupportedArch(u32),
    UnsupportedConsole(u32),
    NoMemory,
    BadDeviceTree(devicetree::Error),
}

impl fmt::Display for HandoffError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Info(error) => write!(f, "{error}"),
            Self::UnsupportedArch(arch) => write!(f, "boot info is for architecture {arch}"),
            Self::UnsupportedConsole(kind) => write!(f, "unsupported console kind {kind}"),
            Self::NoMemory => f.write_str("boot info holds no memory"),
            Self::BadDeviceTree(error) => write!(f, "device tree copy: {error}"),
        }
    }
}

const PL011_SIZE: u64 = 0x1000;

/// Reads the boot info the bootloader left at `address` and makes the console
/// usable, with the console the frozen header names. On failure, the error
/// comes with the console to report it on, when the header was valid enough to
/// name one.
pub fn from_boot_info(address: usize) -> Result<Board, (HandoffError, bool)> {
    // SAFETY: the boot code only takes this path when `x1` held the entry
    // magic, so `address` is the structure the bootloader built (ADR 0013).
    let info = unsafe { BootInfo::from_address(address) };
    let info = match info {
        Ok(info) => info,
        Err(
            error @ bootinfo::Error::UnsupportedVersion {
                console_kind,
                console_base,
                ..
            },
        ) => {
            let usable = use_console(console_kind, console_base);
            return Err((HandoffError::Info(error), usable));
        }
        Err(error) => return Err((HandoffError::Info(error), false)),
    };
    if !use_console(info.console_kind, info.console_base) {
        return Err((HandoffError::UnsupportedConsole(info.console_kind), false));
    }
    if info.arch != bootinfo::ARCH_AARCH64 {
        return Err((HandoffError::UnsupportedArch(info.arch), true));
    }
    // SAFETY: the structure comes from the bootloader, which keeps the map
    // unchanged and in memory the allocator never hands out.
    let map = unsafe { info.memory_map() }.map_err(|e| (HandoffError::Info(e), true))?;
    map.validate(info.kernel_base, info.kernel_size)
        .map_err(|e| (HandoffError::Info(bootinfo::Error::BadMemoryMap(e)), true))?;

    let mut lowest = u64::MAX;
    let mut total = 0u64;
    for entry in map.iter().filter(|entry| is_ram(entry.kind())) {
        lowest = lowest.min(entry.base);
        total = total.saturating_add(entry.length);
    }
    if total == 0 {
        return Err((HandoffError::NoMemory, true));
    }

    // The copy of the device tree is optional, and only its reservations are
    // used: the memory map is the authority for RAM.
    let fdt = if info.device_tree == 0 {
        None
    } else {
        // SAFETY: the bootloader copied the blob to `device_tree` and reserves
        // that memory (BootloaderReclaimable) until the kernel is done with it.
        let blob = unsafe { Fdt::from_address(info.device_tree as usize) }
            .map_err(|e| (HandoffError::BadDeviceTree(e), true))?;
        Some(blob)
    };

    Ok(Board {
        memory: Region {
            base: lowest,
            size: total,
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
            map_entries: map.len(),
        },
        fdt,
        map: Some(map),
    })
}

/// Points the console at the UART the frozen header names. Only a PL011 is
/// driven for now, and a base that does not fit the address space is not used.
fn use_console(kind: u32, base: u64) -> bool {
    match (kind, usize::try_from(base)) {
        (bootinfo::CONSOLE_PL011, Ok(base)) if base != 0 => {
            CONSOLE_BASE.store(base, Ordering::Relaxed);
            true
        }
        _ => false,
    }
}

fn read_device_tree(address: usize) -> Result<Board, Error> {
    // SAFETY: the address is either the loader's device tree pointer or the
    // start of RAM on QEMU `virt`, where the blob lives. Nothing writes there:
    // the kernel never does, and `memory::init` reserves the blob so that the
    // frame allocator never hands it out. The contents are validated by the
    // reader; that the claimed size stays inside RAM is trusted from the
    // loader, as the boot protocol requires.
    let fdt = unsafe { Fdt::from_address(address) }?;
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
