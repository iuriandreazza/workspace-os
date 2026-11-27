//! Arm GICv3 interrupt controller: the distributor and the boot core's
//! redistributor, enough to take one private interrupt (ADR 0035).
//!
//! The CPU interface of a GICv3 is system registers and lives in `arch`
//! (`gic_cpu`). Only the boot core is set up, with affinity routing on, and only
//! interrupts private to a core (SGIs and PPIs, IDs 0 to 31) can be enabled:
//! those are configured in the core's redistributor. Shared peripheral
//! interrupts need routing and come with the first device that raises one.

use core::fmt;
use core::ptr::{read_volatile, write_volatile};

use super::Acknowledged;

const DISTRIBUTOR_CONTROL: usize = 0x0000;
const DISTRIBUTOR_TYPE: usize = 0x0004;
const CLEAR_ENABLE: usize = 0x0180;
const CLEAR_PENDING: usize = 0x0280;
const CLEAR_ACTIVE: usize = 0x0380;
const PERIPHERAL_ID_2: usize = 0xffe8;

/// `GICD_CTLR`, in the single security state form (`DS` set) and in the
/// non-secure view of a controller with two states: the bit that turns Group 1
/// on moves from 1 to 0.
const CONTROL_ENABLE_GROUP_1_SINGLE_STATE: u32 = 1 << 1;
const CONTROL_ENABLE_GROUP_1_NON_SECURE: u32 = 1 << 0;
const CONTROL_AFFINITY_ROUTING: u32 = 1 << 4;
const CONTROL_DISABLE_SECURITY: u32 = 1 << 6;
const CONTROL_WRITE_PENDING: u32 = 1 << 31;

const REDISTRIBUTOR_WAKER: usize = 0x14;
const WAKER_PROCESSOR_SLEEP: u32 = 1 << 1;
const WAKER_CHILDREN_ASLEEP: u32 = 1 << 2;

/// The redistributor's second page: where the registers of SGIs and PPIs are.
const SGI_FRAME: usize = 0x10000;
const GROUP: usize = 0x080;
const SET_ENABLE: usize = 0x100;
const PRIORITY: usize = 0x400;

const ARCHITECTURE_GICV3: u32 = 3;
const ARCHITECTURE_GICV4: u32 = 4;
const PRIVATE_INTERRUPTS: u32 = 32;
/// The ID is the low 24 bits of what an acknowledge returns.
const INTERRUPT_ID_MASK: u32 = 0x00ff_ffff;
/// Polls of a register that a bit change must reach: far more than any
/// controller needs, few enough to end in a report.
const POLLS: u32 = 1_000_000;

#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    /// The device tree said GICv3 and the registers say another architecture
    /// revision.
    NotGicV3 {
        architecture: u32,
    },
    /// No redistributor in the region has this core's affinity.
    NoRedistributor {
        affinity: u32,
    },
    /// The redistributor did not wake, or a write to the distributor did not
    /// complete, in `POLLS` reads.
    Timeout(&'static str),
    NotPrivate {
        id: u32,
    },
    /// The enable bit did not stay set: the interrupt is in a group this world
    /// cannot enable.
    EnableRefused {
        id: u32,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotGicV3 { architecture } => write!(
                f,
                "the device tree names a GICv3 but its architecture revision register reads {architecture}"
            ),
            Self::NoRedistributor { affinity } => write!(
                f,
                "no redistributor has the affinity {affinity:#010x} of this core"
            ),
            Self::Timeout(what) => write!(f, "the controller did not finish: {what}"),
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

/// One core's redistributor, as `Gicv3::initialise` found it: the address of
/// the page that holds the registers of its private interrupts.
pub struct Redistributor {
    sgi_frame: usize,
}

pub struct Gicv3 {
    distributor: usize,
    redistributors: usize,
    redistributors_size: usize,
}

impl Gicv3 {
    /// # Safety
    ///
    /// `distributor` must be the mapped register block of one GICv3 and
    /// `redistributors` the mapped region of `size` bytes that holds its
    /// redistributors; no other code may drive them.
    pub const unsafe fn new(distributor: usize, redistributors: usize, size: usize) -> Self {
        Self {
            distributor,
            redistributors,
            redistributors_size: size,
        }
    }

    fn read32(&self, address: usize) -> u32 {
        // SAFETY: `new` guarantees the blocks are mapped, and every caller
        // passes an address inside one.
        unsafe { read_volatile(address as *const u32) }
    }

    fn write32(&self, address: usize, value: u32) {
        // SAFETY: as in `read32`.
        unsafe { write_volatile(address as *mut u32, value) };
    }

    fn read64(&self, address: usize) -> u64 {
        // SAFETY: as in `read32`; the address is 8-byte aligned.
        unsafe { read_volatile(address as *const u64) }
    }

    fn wait_until(
        &self,
        address: usize,
        done: impl Fn(u32) -> bool,
        what: &'static str,
    ) -> Result<(), Error> {
        for _ in 0..POLLS {
            if done(self.read32(address)) {
                return Ok(());
            }
            core::hint::spin_loop();
        }
        Err(Error::Timeout(what))
    }

    fn wait_for_distributor(&self) -> Result<(), Error> {
        self.wait_until(
            self.distributor + DISTRIBUTOR_CONTROL,
            |control| control & CONTROL_WRITE_PENDING == 0,
            "a write to the distributor",
        )
    }

    /// Brings the distributor and this core's redistributor from whatever
    /// state firmware left them in to one where nothing is enabled, pending or
    /// active, affinity routing and Group 1 are on and the redistributor of the
    /// core with `affinity` (`arch::gic_cpu::affinity`) is awake. Returns that
    /// redistributor.
    ///
    /// The CPU interface is the caller's: it turns the system registers on
    /// before this, and Group 1 on after.
    pub fn initialise(&self, affinity: u32) -> Result<Redistributor, Error> {
        let architecture = (self.read32(self.distributor + PERIPHERAL_ID_2) >> 4) & 0xf;
        if architecture != ARCHITECTURE_GICV3 && architecture != ARCHITECTURE_GICV4 {
            return Err(Error::NotGicV3 { architecture });
        }
        // Whether the controller has one security state or two decides which
        // bit turns Group 1 on, and is read before anything is written.
        let single_state =
            self.read32(self.distributor + DISTRIBUTOR_CONTROL) & CONTROL_DISABLE_SECURITY != 0;
        // Only the group enables go: a controller whose affinity routing firmware
        // turned on keeps it, since turning it off under a core that uses the
        // system registers is not something the architecture promises to do.
        let kept = self.read32(self.distributor + DISTRIBUTOR_CONTROL)
            & (CONTROL_AFFINITY_ROUTING | CONTROL_DISABLE_SECURITY);
        self.write32(self.distributor + DISTRIBUTOR_CONTROL, kept);
        self.wait_for_distributor()?;
        let words = (self.read32(self.distributor + DISTRIBUTOR_TYPE) & 0x1f) as usize + 1;
        // Word 0 is the SGIs and PPIs, which live in the redistributor.
        for word in 1..words {
            let offset = 4 * word;
            self.write32(self.distributor + CLEAR_ENABLE + offset, u32::MAX);
            self.write32(self.distributor + CLEAR_PENDING + offset, u32::MAX);
            self.write32(self.distributor + CLEAR_ACTIVE + offset, u32::MAX);
        }
        let group_1 = if single_state {
            CONTROL_ENABLE_GROUP_1_SINGLE_STATE | CONTROL_DISABLE_SECURITY
        } else {
            CONTROL_ENABLE_GROUP_1_NON_SECURE
        };
        self.write32(
            self.distributor + DISTRIBUTOR_CONTROL,
            CONTROL_AFFINITY_ROUTING | group_1,
        );
        self.wait_for_distributor()?;

        let frame = self.find_redistributor(affinity)?;
        self.wake(frame)?;
        let sgi_frame = frame + SGI_FRAME;
        self.write32(sgi_frame + CLEAR_ENABLE, u32::MAX);
        self.write32(sgi_frame + CLEAR_PENDING, u32::MAX);
        self.write32(sgi_frame + CLEAR_ACTIVE, u32::MAX);
        // Group 1 for every SGI and PPI; where a second security state keeps
        // the groups to itself the write is ignored, and the enable below says so.
        self.write32(sgi_frame + GROUP, u32::MAX);
        Ok(Redistributor { sgi_frame })
    }

    /// The frame of the redistributor whose affinity is `affinity`, found by
    /// walking the region (`giclayout`, tested on the host).
    fn find_redistributor(&self, affinity: u32) -> Result<usize, Error> {
        giclayout::find_redistributor(
            self.redistributors_size,
            affinity,
            |offset| self.read64(self.redistributors + offset),
            |offset| self.read32(self.redistributors + offset),
        )
        .map(|offset| self.redistributors + offset)
        .ok_or(Error::NoRedistributor { affinity })
    }

    fn wake(&self, frame: usize) -> Result<(), Error> {
        let waker = frame + REDISTRIBUTOR_WAKER;
        self.write32(waker, self.read32(waker) & !WAKER_PROCESSOR_SLEEP);
        self.wait_until(
            waker,
            |waker| waker & WAKER_CHILDREN_ASLEEP == 0,
            "the redistributor did not wake",
        )
    }

    /// Enables interrupt `id`, private to this core, at `priority` (lower is
    /// more urgent) and checks that the controller took it.
    pub fn enable_private_interrupt(
        &self,
        redistributor: &Redistributor,
        id: u32,
        priority: u8,
    ) -> Result<(), Error> {
        if id >= PRIVATE_INTERRUPTS {
            return Err(Error::NotPrivate { id });
        }
        // SAFETY: `redistributor` came from `initialise`, so its SGI frame is
        // mapped, and each interrupt has one priority byte.
        unsafe {
            write_volatile(
                (redistributor.sgi_frame + PRIORITY + id as usize) as *mut u8,
                priority,
            )
        };
        self.write32(redistributor.sgi_frame + SET_ENABLE, 1 << id);
        if self.read32(redistributor.sgi_frame + SET_ENABLE) & (1 << id) == 0 {
            return Err(Error::EnableRefused { id });
        }
        Ok(())
    }
}

/// Reads what `arch::gic_cpu::acknowledge` returned.
pub fn acknowledged(raw: u32) -> Option<Acknowledged> {
    Acknowledged::from_register(raw, INTERRUPT_ID_MASK)
}
