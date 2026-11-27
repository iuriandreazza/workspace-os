//! Kernel threads and the scheduler's mechanism (ADR 0037).
//!
//! The decisions are `sched`'s (the thread table, the ready set, the slice, the
//! checks on a policy's answer). This module owns what the architecture
//! provides: each thread's saved context, which is always a `TrapFrame`, its
//! stack, and the entry points the exception handlers call. A switch returns
//! from an exception into another thread's frame.
//!
//! One core: every access to the scheduler's state is with IRQs masked, by a
//! guard in a thread and by exception entry in a handler.

mod selftest;

use alloc::boxed::Box;
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

use kernelvm::{PAGE_SIZE, THREAD_STACK_SIZE, thread_stack};
use sched::{Core, MAX_THREADS, Policy, Switch, ThreadId, ThreadIndex};

use crate::arch::{self, IrqGuard, SVC_EXIT, SVC_SAMPLE_PSTATE, SVC_YIELD, TrapFrame, mmu};
use crate::heap;

pub use sched::{Counters, IDLE};
#[cfg(feature = "selftest-thread-overflow")]
pub use selftest::canary_intact;
pub use selftest::selftest;

const STACK_PAGES: usize = (THREAD_STACK_SIZE / PAGE_SIZE) as usize;

/// A thread's entry: it receives the argument it was spawned with and ends the
/// thread by returning.
pub type Entry = extern "C" fn(usize);

#[derive(Debug, PartialEq, Eq)]
pub enum SpawnError {
    /// Every entry of the thread table is in use.
    TableFull,
    /// No frame left for the thread's stack.
    NoStack,
}

impl core::fmt::Display for SpawnError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            Self::TableFull => "the thread table is full",
            Self::NoStack => "no frame is left for the thread's stack",
        })
    }
}

struct Scheduler {
    core: Core,
    frames: [TrapFrame; MAX_THREADS],
    /// `None` until `init`: a tick before then is not the scheduler's.
    policy: Option<Box<dyn Policy>>,
    /// How many pages of each slot's stack are mapped, counted from the bottom.
    /// They are never unmapped: a slot is reused by the next thread that gets its
    /// index, and a slot that was only partly mapped when frames ran out is
    /// finished by the next spawn.
    stack_pages: [u8; MAX_THREADS],
}

/// The scheduler's state, reached with IRQs masked.
struct Exclusive {
    busy: AtomicBool,
    value: UnsafeCell<Scheduler>,
}

// SAFETY: access goes through `with`, which masks IRQs and refuses a second
// entry. With one core that is exclusion; K11 replaces it with a lock and
// per-core queues (ADR 0037).
unsafe impl Sync for Exclusive {}

impl Exclusive {
    fn with<R>(&self, use_scheduler: impl FnOnce(&mut Scheduler) -> R) -> R {
        let _irqs_masked = IrqGuard::new();
        assert!(
            !self.busy.load(Ordering::Relaxed),
            "the scheduler was entered while it was in use"
        );
        self.busy.store(true, Ordering::Relaxed);
        core::sync::atomic::compiler_fence(Ordering::SeqCst);
        // SAFETY: IRQs are masked and `busy` was clear, so this is the only
        // reference to the state until it is cleared below.
        let result = use_scheduler(unsafe { &mut *self.value.get() });
        core::sync::atomic::compiler_fence(Ordering::SeqCst);
        self.busy.store(false, Ordering::Relaxed);
        result
    }
}

static SCHEDULER: Exclusive = Exclusive {
    busy: AtomicBool::new(false),
    value: UnsafeCell::new(Scheduler {
        core: Core::new(),
        frames: [TrapFrame::ZERO; MAX_THREADS],
        policy: None,
        stack_pages: [0; MAX_THREADS],
    }),
};

/// The index of the running thread, for the panic report, which cannot take the
/// scheduler.
static CURRENT: AtomicUsize = AtomicUsize::new(0);
/// What a running kernel thread's `SPSR` looks like, taken once at `init`.
static RUNNING_PSTATE: AtomicU64 = AtomicU64::new(0);
static PSTATE_SAMPLED: AtomicBool = AtomicBool::new(false);

/// The running thread's index.
pub fn current_index() -> ThreadIndex {
    CURRENT.load(Ordering::Relaxed)
}

/// Whether IRQs are masked while the scheduler's state is held (ADR 0037).
pub fn state_masks_interrupts() -> bool {
    SCHEDULER.with(|_| arch::irqs_masked())
}

pub fn counters() -> Counters {
    SCHEDULER.with(|scheduler| scheduler.core.counters())
}

/// Starts the scheduler with `policy` and the idle thread. The running code
/// becomes thread 0.
pub fn init(policy: Box<dyn Policy>) {
    assert!(
        arch::current_el() == 1,
        "threads: the kernel runs at EL1 once the boot code has dropped there"
    );
    // The boot thread's own PSTATE is what a new thread starts from, flags,
    // DAIF and mode aside.
    // SAFETY: the sample SVC is handled by `handle_svc`.
    unsafe { core::arch::asm!("svc #{imm}", imm = const SVC_SAMPLE_PSTATE, options(nostack)) };
    SCHEDULER.with(|scheduler| {
        assert!(scheduler.policy.is_none(), "the scheduler started twice");
        scheduler.policy = Some(policy);
    });
    let idle = prepare_frame(IDLE, idle_main, 0).expect("a stack for the idle thread");
    SCHEDULER.with(|scheduler| scheduler.frames[IDLE] = idle);
}

/// What the idle thread does when nothing is ready.
extern "C" fn idle_main(_: usize) {
    selftest::idle_reached();
    loop {
        // SAFETY: `wfi` only waits for an interrupt.
        unsafe { core::arch::asm!("wfi", options(nomem, nostack, preserves_flags)) };
    }
}

/// Maps the stack of slot `index` if it is not yet, and builds the frame of a
/// thread that starts at `entry`.
fn prepare_frame(
    index: ThreadIndex,
    entry: Entry,
    argument: usize,
) -> Result<TrapFrame, SpawnError> {
    let stack = thread_stack(index);
    let have = SCHEDULER.with(|scheduler| usize::from(scheduler.stack_pages[index]));
    if have < STACK_PAGES {
        let next = stack.start + have as u64 * PAGE_SIZE;
        let added = heap::with_frames(|frames| mmu::map_pages(frames, next, STACK_PAGES - have));
        // Whatever was mapped stays mapped and is counted, so a later spawn
        // continues from there and does not run into its own pages.
        SCHEDULER.with(|scheduler| scheduler.stack_pages[index] = (have + added) as u8);
        if have + added < STACK_PAGES {
            return Err(SpawnError::NoStack);
        }
    }
    Ok(TrapFrame::new_thread(
        entry as usize,
        argument,
        stack.end,
        RUNNING_PSTATE.load(Ordering::Relaxed),
        arch::exit_current_thread as *const () as usize,
    ))
}

/// Starts a thread at `entry(argument)`. It runs when the scheduler picks it.
pub fn spawn(entry: Entry, argument: usize) -> Result<ThreadId, SpawnError> {
    let id = SCHEDULER
        .with(|scheduler| scheduler.core.reserve())
        .ok_or(SpawnError::TableFull)?;
    match prepare_frame(id.index, entry, argument) {
        Ok(frame) => {
            SCHEDULER.with(|scheduler| {
                scheduler.frames[id.index] = frame;
                scheduler.core.start(id);
            });
            Ok(id)
        }
        Err(error) => {
            SCHEDULER.with(|scheduler| scheduler.core.release(id));
            Err(error)
        }
    }
}

/// Gives up the rest of the running thread's slice.
pub fn yield_now() {
    arch::yield_now();
}

pub fn is_finished(id: ThreadId) -> bool {
    SCHEDULER.with(|scheduler| scheduler.core.is_finished(id))
}

/// Saves the outgoing thread's context in its slot and loads the incoming one
/// into `frame`, which the exception return then restores.
fn apply(scheduler: &mut Scheduler, switch: Switch, frame: &mut TrapFrame) {
    if let Some(from) = switch.from {
        scheduler.frames[from] = *frame;
    }
    *frame = scheduler.frames[switch.to];
    CURRENT.store(switch.to, Ordering::Relaxed);
}

/// The tick was taken and ended: counts it against the running thread's slice
/// and switches when the policy says so. Called by the IRQ handler, IRQs masked.
pub fn on_tick(frame: &mut TrapFrame) {
    SCHEDULER.with(|scheduler| {
        let Scheduler { core, policy, .. } = &mut *scheduler;
        let Some(policy) = policy.as_mut() else {
            return;
        };
        if let Some(switch) = core.on_tick(policy.as_mut(), crate::interrupts::ticks()) {
            apply(scheduler, switch, frame);
        }
    });
}

/// The SVCs the threads use. True when `immediate` was one of them; anything
/// else is not handled here.
pub fn handle_svc(immediate: u64, frame: &mut TrapFrame) -> bool {
    match immediate {
        SVC_SAMPLE_PSTATE => {
            // Once, at `init`: another sample would change every later thread's PSTATE.
            if PSTATE_SAMPLED.swap(true, Ordering::Relaxed) {
                return false;
            }
            RUNNING_PSTATE.store(frame.spsr(), Ordering::Relaxed);
        }
        SVC_YIELD => SCHEDULER.with(|scheduler| {
            let Scheduler { core, policy, .. } = &mut *scheduler;
            let policy = policy
                .as_mut()
                .expect("a thread yielded before the scheduler started");
            if let Some(switch) = core.yield_now(policy.as_mut(), crate::interrupts::ticks()) {
                apply(scheduler, switch, frame);
            }
        }),
        SVC_EXIT => SCHEDULER.with(|scheduler| {
            let Scheduler { core, policy, .. } = &mut *scheduler;
            let policy = policy
                .as_mut()
                .expect("a thread ended before the scheduler started");
            let switch = core.exit(policy.as_mut(), crate::interrupts::ticks());
            apply(scheduler, switch, frame);
        }),
        _ => return false,
    }
    true
}

/// The end of the boot thread's work: it ends, and the idle thread keeps the
/// core. Nothing returns here.
pub fn retire_boot_thread() -> ! {
    loop {
        // SAFETY: the handler frees the boot thread's entry and never returns to it.
        unsafe { core::arch::asm!("svc #{imm}", imm = const SVC_EXIT, options(nostack)) };
    }
}
