//! Interrupts: the controller, the generic timer and the tick it raises
//! (ADR 0035).
//!
//! Mechanism only (ADR 0002): the kernel counts ticks and nothing more. What a
//! tick is for (time slices, timeouts) is for whatever the scheduler turns out
//! to be, and reads `ticks`.

use core::fmt::{self, Write};
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use crate::arch;
use crate::board::{self, InterruptWiring};
use crate::drivers::gicv2::{self, Gicv2};
use crate::log::klog;

/// Ticks per second.
const TICK_HZ: u64 = 100;
/// Where the tick sits among priorities; any value the priority mask passes.
const TIMER_PRIORITY: u8 = 0x80;
/// How long the wait for ticks lasts before it gives up, in seconds of the
/// counter: ten times what the ticks it waits for take.
const WAIT_LIMIT_SECONDS: u64 = 2;
/// The number of ticks the boot waits for.
pub const TICKS_TO_WAIT: u64 = 20;

static TICKS: AtomicU64 = AtomicU64::new(0);
static SPURIOUS: AtomicU64 = AtomicU64::new(0);
/// Interrupts that were not the tick: none is enabled, so one is a fault.
static UNEXPECTED: AtomicU64 = AtomicU64::new(0);
static TIMER_INTERRUPT: AtomicU32 = AtomicU32::new(0);
/// The counter values the timer is next due at and the distance between ticks.
static NEXT_DEADLINE: AtomicU64 = AtomicU64::new(0);
static PERIOD: AtomicU64 = AtomicU64::new(0);
static FREQUENCY: AtomicU64 = AtomicU64::new(0);

/// Why the interrupts could not be started.
#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    /// `CNTFRQ_EL0` is zero and the device tree gives no frequency either: a
    /// period cannot be computed.
    UnknownFrequency,
    /// The monitor of the boot code answered something other than success.
    Monitor(i64),
    Controller(gicv2::Error),
    /// The controller said none was pending and the handler counted that.
    SpuriousNotCounted,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownFrequency => {
                f.write_str("the counter frequency register is zero and the device tree gives none")
            }
            Self::Monitor(answer) => write!(
                f,
                "the EL3 monitor refused to give the interrupt controller to the non-secure world (answer {answer})"
            ),
            Self::Controller(error) => write!(f, "{error}"),
            Self::SpuriousNotCounted => f.write_str(
                "an interrupt the controller did not have pending was not counted as spurious",
            ),
        }
    }
}

impl From<gicv2::Error> for Error {
    fn from(error: gicv2::Error) -> Self {
        Self::Controller(error)
    }
}

/// What starting the interrupts settled, for the log.
pub struct Started {
    pub frequency: u64,
    pub frequency_from_device_tree: bool,
    pub period_microseconds: u64,
}

fn controller() -> Gicv2 {
    let (distributor, cpu_interface) = board::interrupt_controller_bases();
    // SAFETY: the two bases are the controller's registers in the device
    // window, set once the kernel's tables map them, and the kernel is the
    // only code that drives the controller.
    unsafe { Gicv2::new(distributor, cpu_interface) }
}

/// The tick count since `start`.
pub fn ticks() -> u64 {
    // SAFETY: `as_ptr` is the address of a live atomic. The read is volatile
    // because the interrupt handler, which the compiler cannot see being
    // called, changes the value between two reads.
    unsafe { TICKS.as_ptr().read_volatile() }
}

/// Sets the controller and the timer up and unmasks IRQs on this core, from
/// whatever state firmware left them in. `entered_at_el3` is true when the
/// boot code went through EL3 and stands in for firmware there.
///
/// The controller's registers must be mapped (`board::use_interrupt_controller_at`).
pub fn start(wiring: &InterruptWiring, entered_at_el3: bool) -> Result<Started, Error> {
    // Firmware may have left the timer running; nothing fires from here on
    // until the controller is ready.
    arch::timer::stop();
    let (frequency, frequency_from_device_tree) = match arch::timer::frequency() {
        0 => (
            u64::from(wiring.timer_frequency.unwrap_or(0)),
            wiring.timer_frequency.is_some(),
        ),
        hertz => (hertz, false),
    };
    if frequency < TICK_HZ {
        return Err(Error::UnknownFrequency);
    }
    if entered_at_el3 {
        arch::give_interrupt_controller_to_non_secure(
            wiring.distributor.base,
            wiring.cpu_interface.base,
        )
        .map_err(Error::Monitor)?;
    }
    let controller = controller();
    controller.initialise()?;
    controller.enable_private_interrupt(wiring.timer_interrupt, TIMER_PRIORITY)?;
    TIMER_INTERRUPT.store(wiring.timer_interrupt, Ordering::Relaxed);
    check_spurious_is_counted()?;

    FREQUENCY.store(frequency, Ordering::Relaxed);
    let period = frequency / TICK_HZ;
    PERIOD.store(period, Ordering::Relaxed);
    let deadline = arch::timer::count() + period;
    NEXT_DEADLINE.store(deadline, Ordering::Relaxed);
    arch::timer::set_deadline(deadline);
    arch::timer::start();
    arch::enable_interrupts();
    Ok(Started {
        frequency,
        frequency_from_device_tree,
        period_microseconds: period * 1_000_000 / frequency,
    })
}

/// Masks IRQs and stops the timer: no tick arrives until `start` again.
pub fn stop() {
    arch::disable_interrupts();
    arch::timer::stop();
}

/// With nothing enabled and pending the controller answers every acknowledge
/// as spurious: the handler must count that and end nothing. Run before the
/// timer starts, so the answer is certain.
fn check_spurious_is_counted() -> Result<(), Error> {
    handle_irq();
    let counted = SPURIOUS.swap(0, Ordering::Relaxed) == 1
        && TICKS.load(Ordering::Relaxed) == 0
        && UNEXPECTED.load(Ordering::Relaxed) == 0;
    if counted {
        Ok(())
    } else {
        Err(Error::SpuriousNotCounted)
    }
}

/// Entered by the IRQ vector with the interrupted context saved, IRQs masked
/// and the exception stack in use. It prints nothing and unmasks nothing: the
/// console's SAFETY argument and the single exception stack both rest on that.
pub fn handle_irq() {
    let controller = controller();
    let Some(id) = controller.acknowledge() else {
        // Nothing to end: an acknowledge that found nothing took nothing.
        SPURIOUS.fetch_add(1, Ordering::Relaxed);
        return;
    };
    if id == TIMER_INTERRUPT.load(Ordering::Relaxed) {
        // The line is level: until the deadline moves past the count the timer
        // keeps it asserted, so this comes before the end of the interrupt.
        let now = arch::timer::count();
        let period = PERIOD.load(Ordering::Relaxed);
        let due = NEXT_DEADLINE.load(Ordering::Relaxed) + period;
        // A core that fell more than a period behind skips the ticks it
        // missed instead of firing them back to back.
        let next = if due > now { due } else { now + period };
        NEXT_DEADLINE.store(next, Ordering::Relaxed);
        arch::timer::set_deadline(next);
        TICKS.fetch_add(1, Ordering::Relaxed);
    } else {
        UNEXPECTED.fetch_add(1, Ordering::Relaxed);
    }
    controller.end_of_interrupt(id);
}

/// What a wait for ticks measured.
pub struct TickRun {
    pub ticks: u64,
    pub elapsed_microseconds: u64,
    pub period_microseconds: u64,
    pub spurious: u64,
    pub unexpected: u64,
}

/// Why a wait for ticks gave up.
#[derive(Debug)]
pub struct NoTick {
    pub ticks: u64,
    pub wanted: u64,
}

/// Waits, with IRQs unmasked, until `wanted` more ticks have arrived, and
/// measures them against the counter. The wait polls and never sleeps: a
/// timer that does not fire must end in a report and not in a hang.
pub fn wait_for_ticks(wanted: u64) -> Result<TickRun, NoTick> {
    let frequency = FREQUENCY.load(Ordering::Relaxed).max(1);
    let limit = WAIT_LIMIT_SECONDS * frequency;
    let first = ticks();
    let start = arch::timer::count();
    while ticks() - first < wanted {
        if arch::timer::count() - start > limit {
            return Err(NoTick {
                ticks: ticks() - first,
                wanted,
            });
        }
        core::hint::spin_loop();
    }
    // The ticks are read before the clock: any that arrive in between make the
    // elapsed time longer, never the count larger than it allows.
    let counted = ticks() - first;
    let elapsed = arch::timer::count() - start;
    let period = PERIOD.load(Ordering::Relaxed);
    Ok(TickRun {
        ticks: counted,
        elapsed_microseconds: elapsed * 1_000_000 / frequency,
        period_microseconds: period * 1_000_000 / frequency,
        spurious: SPURIOUS.load(Ordering::Relaxed),
        unexpected: UNEXPECTED.load(Ordering::Relaxed),
    })
}

/// The boot's interrupt test (ADR 0035): starts the controller and the timer,
/// waits for ticks and reports them, takes one with the whole context live and
/// checks it came back, then stops again. Any failure ends the boot.
pub fn selftest(mut out: impl Write, wiring: &InterruptWiring, entered_at_el3: bool) {
    let started =
        start(wiring, entered_at_el3).unwrap_or_else(|error| panic!("interrupts: {error}"));
    klog!(
        Ok,
        out,
        "interrupts: gicv2, distributor {:#x}, cpu interface {:#x}, timer interrupt {}{}",
        wiring.distributor.base,
        wiring.cpu_interface.base,
        wiring.timer_interrupt,
        if entered_at_el3 {
            ", handed to the non-secure world by the EL3 monitor"
        } else {
            ""
        }
    );
    klog!(
        Ok,
        out,
        "timer: virtual, {} Hz{}, a tick every {} us",
        started.frequency,
        if started.frequency_from_device_tree {
            " (from the device tree)"
        } else {
            ""
        },
        started.period_microseconds
    );
    let run = wait_for_ticks(TICKS_TO_WAIT).unwrap_or_else(|error| {
        panic!(
            "interrupts: no tick: {} of {} within {} s",
            error.ticks, error.wanted, WAIT_LIMIT_SECONDS
        )
    });
    klog!(
        Ok,
        out,
        "ticks: {} in {} us, a tick every {} us, spurious {}, unexpected {}",
        run.ticks,
        run.elapsed_microseconds,
        run.period_microseconds,
        run.spurious,
        run.unexpected
    );
    let before = ticks();
    arch::run_interrupt_selftest();
    assert!(
        ticks() > before,
        "interrupts: the context test returned without a tick"
    );
    klog!(
        Ok,
        out,
        "interrupts: context intact across a tick: registers, flags, SP, SP_EL0 and the stack below SP"
    );
    stop();
}
