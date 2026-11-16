//! What the kernel knows about the machine it runs on, read from the device
//! tree instead of being fixed in the source.

use core::fmt;
use core::sync::atomic::{AtomicUsize, Ordering};

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

pub struct Board {
    /// The first region of the first memory node, for the banner.
    pub memory: Region,
    pub uart: Region,
    /// Where the blob lives: the allocator must not hand it out.
    pub device_tree: Region,
    pub fdt: Fdt<'static>,
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
        fdt,
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
