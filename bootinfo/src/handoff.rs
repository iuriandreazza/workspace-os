//! The kernel's decision on what the bootloader handed over: accept it, or say
//! why not. It is plain computation over a parsed structure, so the host tests
//! every refusal; the kernel only supplies the unsafe reads.

use crate::{ARCH_AARCH64, BootInfo, CONSOLE_PL011, Error, MemoryMap};

/// Why the handoff is unusable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandoffError {
    Info(Error),
    UnsupportedArch(u32),
    UnsupportedConsole(u32),
}

impl core::fmt::Display for HandoffError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Info(error) => write!(f, "{error}"),
            Self::UnsupportedArch(arch) => write!(f, "boot info is for architecture {arch}"),
            Self::UnsupportedConsole(kind) => write!(f, "unsupported console kind {kind}"),
        }
    }
}

/// A refusal, with the console to report it on when the header named a usable one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rejected {
    pub error: HandoffError,
    pub console: Option<usize>,
}

/// A handoff that passed every check.
pub struct Handoff<'a> {
    pub info: BootInfo,
    pub map: MemoryMap<'a>,
    pub console: usize,
    /// The lowest address of RAM and the amount of it, from the memory map.
    pub ram_lowest: u64,
    pub ram_total: u64,
}

/// The base of the console the header names, if the kernel can drive it: only a
/// PL011 for now, at a nonzero base that fits the address space.
pub fn usable_console(kind: u32, base: u64) -> Option<usize> {
    match (kind, usize::try_from(base)) {
        (CONSOLE_PL011, Ok(base)) if base != 0 => Some(base),
        _ => None,
    }
}

/// Checks a parsed structure and the memory map it points at. `memory_map` is
/// only called once the console and the architecture have passed, as the map
/// is read from memory the structure names.
pub fn check<'a>(
    parsed: Result<BootInfo, Error>,
    memory_map: impl FnOnce(&BootInfo) -> Result<MemoryMap<'a>, Error>,
) -> Result<Handoff<'a>, Rejected> {
    let reject = |error, console| Rejected { error, console };
    let info = match parsed {
        Ok(info) => info,
        // The header was valid, so the console it names can report the version.
        Err(
            error @ Error::UnsupportedVersion {
                console_kind,
                console_base,
                ..
            },
        ) => {
            return Err(reject(
                HandoffError::Info(error),
                usable_console(console_kind, console_base),
            ));
        }
        Err(error) => return Err(reject(HandoffError::Info(error), None)),
    };
    let Some(console) = usable_console(info.console_kind, info.console_base) else {
        return Err(reject(
            HandoffError::UnsupportedConsole(info.console_kind),
            None,
        ));
    };
    let rejected = |error| reject(error, Some(console));
    if info.arch != ARCH_AARCH64 {
        return Err(rejected(HandoffError::UnsupportedArch(info.arch)));
    }
    let map = memory_map(&info).map_err(|e| rejected(HandoffError::Info(e)))?;
    map.validate(info.kernel_base, info.kernel_size)
        .map_err(|e| rejected(HandoffError::Info(Error::BadMemoryMap(e))))?;

    // `validate` guarantees a Kernel entry, which is RAM, so there is some.
    let mut ram_lowest = u64::MAX;
    let mut ram_total = 0u64;
    for entry in map.iter().filter(|entry| entry.kind().is_ram()) {
        ram_lowest = ram_lowest.min(entry.base);
        ram_total = ram_total.saturating_add(entry.length);
    }
    Ok(Handoff {
        info,
        map,
        console,
        ram_lowest,
        ram_total,
    })
}

/// The ACPI root pointer the bootloader hands over: the ACPI 2.0 one when the
/// firmware has both, zero when it has neither.
pub fn select_rsdp(acpi2: Option<u64>, acpi1: Option<u64>) -> u64 {
    acpi2.or(acpi1).unwrap_or(0)
}
