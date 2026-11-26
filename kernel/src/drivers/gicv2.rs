//! Arm GICv2 interrupt controller: the distributor and the CPU interface of
//! the boot core, enough to take one private interrupt (ADR 0035).
//!
//! Only the boot core's CPU interface is set up, and only interrupts private
//! to a core (SGIs and PPIs, IDs 0 to 31) can be enabled: shared peripheral
//! interrupts need targets and a group, which the first device that raises one
//! will bring. A GIC with the security extensions lets a non-secure kernel
//! enable only Group 1 interrupts, which only the secure side can assign; an
//! enable that does not take is reported, not ignored.

use core::fmt;
use core::ptr::{read_volatile, write_volatile};

const DISTRIBUTOR_CONTROL: usize = 0x000;
const DISTRIBUTOR_TYPE: usize = 0x004;
const SET_ENABLE: usize = 0x100;
const CLEAR_ENABLE: usize = 0x180;
const CLEAR_PENDING: usize = 0x280;
const CLEAR_ACTIVE: usize = 0x380;
const PRIORITY: usize = 0x400;
const PERIPHERAL_ID_2: usize = 0xfe8;

const CPU_CONTROL: usize = 0x00;
const CPU_PRIORITY_MASK: usize = 0x04;
const CPU_BINARY_POINT: usize = 0x08;
const CPU_ACKNOWLEDGE: usize = 0x0c;
const CPU_END_OF_INTERRUPT: usize = 0x10;

const ENABLE_GROUP: u32 = 1;
/// `ICFGR`, `ITARGETSR` and the rest of the distributor have 32 bits per
/// register: the number of interrupt lines is `32 * (ITLinesNumber + 1)`.
const LINES_PER_WORD: usize = 32;
const INTERRUPT_ID_MASK: u32 = 0x3ff;
/// What an acknowledge returns when nothing is pending.
const SPURIOUS: u32 = 1023;
/// Every priority passes the mask, the lowest (255) included.
const PRIORITY_MASK_ALL: u32 = 0xff;
const ARCHITECTURE_GICV2: u32 = 2;
/// IDs 0 to 15 are SGIs, 16 to 31 PPIs: the ones private to a core.
const PRIVATE_INTERRUPTS: u32 = 32;

#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    /// The device tree said GICv2 and the registers say another architecture
    /// revision.
    NotGicV2 { architecture: u32 },
    /// Only interrupts private to a core can be enabled.
    NotPrivate { id: u32 },
    /// The enable bit did not stay set: the interrupt is not one this world
    /// may take (a secure Group 0 interrupt seen from the non-secure side).
    EnableRefused { id: u32 },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotGicV2 { architecture } => write!(
                f,
                "the device tree names a GICv2 but its architecture revision register reads {architecture}"
            ),
            Self::NotPrivate { id } => {
                write!(
                    f,
                    "interrupt {id} is shared: only IDs below 32 are supported"
                )
            }
            Self::EnableRefused { id } => write!(
                f,
                "the controller did not enable interrupt {id}: it is in a group this world cannot enable"
            ),
        }
    }
}

pub struct Gicv2 {
    distributor: usize,
    cpu_interface: usize,
}

impl Gicv2 {
    /// # Safety
    ///
    /// `distributor` and `cpu_interface` must be the mapped register blocks of
    /// one GICv2, and no other code may drive them.
    pub const unsafe fn new(distributor: usize, cpu_interface: usize) -> Self {
        Self {
            distributor,
            cpu_interface,
        }
    }

    fn distributor_read(&self, offset: usize) -> u32 {
        // SAFETY: `new` guarantees the distributor is mapped; `offset` is one
        // of this file's register offsets.
        unsafe { read_volatile((self.distributor + offset) as *const u32) }
    }

    fn distributor_write(&self, offset: usize, value: u32) {
        // SAFETY: as in `distributor_read`.
        unsafe { write_volatile((self.distributor + offset) as *mut u32, value) };
    }

    fn cpu_read(&self, offset: usize) -> u32 {
        // SAFETY: `new` guarantees the CPU interface is mapped; `offset` is
        // one of this file's register offsets.
        unsafe { read_volatile((self.cpu_interface + offset) as *const u32) }
    }

    fn cpu_write(&self, offset: usize, value: u32) {
        // SAFETY: as in `cpu_read`.
        unsafe { write_volatile((self.cpu_interface + offset) as *mut u32, value) };
    }

    /// Brings the controller from whatever state firmware left it in to one
    /// where nothing is enabled, pending or active, every priority passes and
    /// the boot core's interface and the distributor are on. Reset values are
    /// not assumed: firmware that used interrupts leaves them set.
    pub fn initialise(&self) -> Result<(), Error> {
        let architecture = (self.distributor_read(PERIPHERAL_ID_2) >> 4) & 0xf;
        if architecture != ARCHITECTURE_GICV2 {
            return Err(Error::NotGicV2 { architecture });
        }
        self.distributor_write(DISTRIBUTOR_CONTROL, 0);
        let words = (self.distributor_read(DISTRIBUTOR_TYPE) & 0x1f) as usize + 1;
        for word in 0..words {
            let offset = 4 * word;
            self.distributor_write(CLEAR_ENABLE + offset, u32::MAX);
            self.distributor_write(CLEAR_PENDING + offset, u32::MAX);
            self.distributor_write(CLEAR_ACTIVE + offset, u32::MAX);
        }
        // Written whole: bits firmware set, such as the EOI mode that splits
        // the end of an interrupt in two, go back to their reset meaning.
        self.cpu_write(CPU_CONTROL, ENABLE_GROUP);
        self.cpu_write(CPU_PRIORITY_MASK, PRIORITY_MASK_ALL);
        self.cpu_write(CPU_BINARY_POINT, 0);
        // An interrupt the interface already signalled stays pending there.
        for _ in 0..words * LINES_PER_WORD {
            match self.acknowledge() {
                Some(id) => self.end_of_interrupt(id),
                None => break,
            }
        }
        self.distributor_write(DISTRIBUTOR_CONTROL, ENABLE_GROUP);
        Ok(())
    }

    /// Enables interrupt `id`, private to this core, at `priority` (lower is
    /// more urgent) and checks that the controller took it.
    pub fn enable_private_interrupt(&self, id: u32, priority: u8) -> Result<(), Error> {
        if id >= PRIVATE_INTERRUPTS {
            return Err(Error::NotPrivate { id });
        }
        // SAFETY: `new` guarantees the distributor is mapped, and each
        // interrupt has one priority byte.
        unsafe {
            write_volatile(
                (self.distributor + PRIORITY + id as usize) as *mut u8,
                priority,
            )
        };
        self.distributor_write(SET_ENABLE, 1 << id);
        if self.distributor_read(SET_ENABLE) & (1 << id) == 0 {
            return Err(Error::EnableRefused { id });
        }
        Ok(())
    }

    /// Takes the highest priority pending interrupt: its ID, or `None` when
    /// the controller says none is (spurious), in which case nothing is to be
    /// ended.
    pub fn acknowledge(&self) -> Option<u32> {
        let id = self.cpu_read(CPU_ACKNOWLEDGE) & INTERRUPT_ID_MASK;
        (id != SPURIOUS).then_some(id)
    }

    /// Ends interrupt `id`, which `acknowledge` returned: the controller may
    /// signal it again.
    pub fn end_of_interrupt(&self, id: u32) {
        self.cpu_write(CPU_END_OF_INTERRUPT, id);
    }
}
