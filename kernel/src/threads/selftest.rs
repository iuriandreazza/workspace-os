//! The boot test of the scheduler (ADR 0037). None of the order it checks
//! depends on the host's speed: with the tick stopped a yield decides the order,
//! and under the tick a baton does. Time only bounds how long a hang is waited on.

use alloc::boxed::Box;
use core::fmt::Write;
use core::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, AtomicUsize, Ordering};

use kernelvm::{PAGE_SIZE, Permissions, THREAD_SLOT_SIZE, THREAD_STACK_SIZE, thread_stack};
use sched::{RoundRobin, ThreadId};

use super::{Counters, IDLE, counters, is_finished, spawn, yield_now};
use crate::arch::{self, Handover, Side, mmu};
use crate::board;
use crate::interrupts;
use crate::log::klog;

/// One tick: a test needs few of them, and the production slice is the
/// policy's business.
const TEST_SLICE_TICKS: u32 = 1;
/// How long a phase may take before it is a hang, in seconds of the counter.
const PHASE_LIMIT_SECONDS: u64 = 10;
const ROUNDS: usize = 3;

static ORDER: [AtomicU8; 16] = [const { AtomicU8::new(0) }; 16];
static ORDER_LEN: AtomicUsize = AtomicUsize::new(0);
static DEADLINE: AtomicU64 = AtomicU64::new(0);
static BATON: AtomicUsize = AtomicUsize::new(0);
static TIMED_OUT: AtomicBool = AtomicBool::new(false);
static STOP: AtomicBool = AtomicBool::new(false);
static OTHER_FAILED: AtomicBool = AtomicBool::new(false);
static OTHER_ROUNDS: AtomicU64 = AtomicU64::new(0);
static IDLE_REACHED: AtomicBool = AtomicBool::new(false);

fn record(name: u8) {
    let at = ORDER_LEN.fetch_add(1, Ordering::Relaxed);
    ORDER[at].store(name, Ordering::Relaxed);
}

fn recorded() -> alloc::vec::Vec<u8> {
    (0..ORDER_LEN.load(Ordering::Relaxed))
        .map(|at| ORDER[at].load(Ordering::Relaxed))
        .collect()
}

/// Switches so far, read in one go.
fn switch_count() -> u64 {
    let counters = counters();
    counters.yields + counters.preemptions
}

fn start_phase() {
    ORDER_LEN.store(0, Ordering::Relaxed);
    DEADLINE.store(
        interrupts::deadline_in_seconds(PHASE_LIMIT_SECONDS),
        Ordering::Relaxed,
    );
    TIMED_OUT.store(false, Ordering::Relaxed);
}

fn phase_expired() -> bool {
    interrupts::reached(DEADLINE.load(Ordering::Relaxed))
}

/// Yields until every thread has ended, or the phase's time is up.
fn wait_for(ids: &[ThreadId]) {
    while !ids.iter().all(|&id| is_finished(id)) {
        assert!(
            !phase_expired(),
            "threads: a phase did not finish within {PHASE_LIMIT_SECONDS} s"
        );
        yield_now();
    }
}

fn spawn_named(entry: super::Entry, name: u8) -> ThreadId {
    spawn(entry, usize::from(name)).unwrap_or_else(|error| panic!("threads: {error}"))
}

/// What a thread starts with: IRQs unmasked, debug, SError and FIQ masked (ADR 0037).
fn assert_start_state() {
    const DAIF_IRQS_ONLY_UNMASKED: u64 = 0b1101 << 6;
    let daif = arch::daif();
    assert!(
        daif == DAIF_IRQS_ONLY_UNMASKED,
        "threads: a thread starts with DAIF {daif:#x}, expected {DAIF_IRQS_ONLY_UNMASKED:#x}"
    );
}

/// Phase 1 thread: prints its name and yields, `ROUNDS` times.
extern "C" fn cooperative(name: usize) {
    assert_start_state();
    for round in 1..=ROUNDS {
        record(name as u8);
        let mut console = board::console();
        klog!(
            Ok,
            console,
            "threads: cooperative {} round {round}",
            name as u8 as char
        );
        yield_now();
    }
}

/// Phase 2 thread: `name` is the letter, and bit 8 says which side of the baton
/// it is. It prints only when the baton is its own, passes it, and spins until
/// it comes back: it never yields, so every pass is a switch by the tick.
extern "C" fn baton(argument: usize) {
    assert_start_state();
    let me = argument >> 8;
    let name = argument as u8;
    for round in 1..=ROUNDS {
        while BATON.load(Ordering::Acquire) != me {
            if phase_expired() {
                TIMED_OUT.store(true, Ordering::Relaxed);
                return;
            }
            core::hint::spin_loop();
        }
        record(name);
        let mut console = board::console();
        klog!(Ok, console, "threads: baton {} round {round}", name as char);
        BATON.store(1 - me, Ordering::Release);
    }
}

/// The other thread of the context test: it runs the test with values of its
/// own over and over, until told to stop.
extern "C" fn context_other(handover: usize) {
    let handover = if handover == 0 {
        Handover::Yield
    } else {
        Handover::Tick
    };
    while !STOP.load(Ordering::Relaxed) {
        if arch::switch_selftest(handover, Side::Other) {
            OTHER_FAILED.store(true, Ordering::Relaxed);
        }
        OTHER_ROUNDS.fetch_add(1, Ordering::Relaxed);
    }
}

pub fn idle_reached() {
    assert!(
        arch::timer::is_running() || arch::irqs_masked(),
        "threads: the idle thread runs with IRQs unmasked and no tick"
    );
    if !IDLE_REACHED.swap(true, Ordering::Relaxed) {
        let mut console = board::console();
        klog!(
            Ok,
            console,
            "threads: idle thread running, nothing else ready"
        );
    }
}

fn expect_order(what: &str, wanted: &[u8]) {
    let seen = recorded();
    assert!(
        seen == wanted,
        "threads: {what}: ran {:?}, expected {:?}",
        core::str::from_utf8(&seen),
        core::str::from_utf8(wanted)
    );
}

fn cooperative_phase(mut out: impl Write) {
    start_phase();
    let ids = (*b"ABC").map(|name| spawn_named(cooperative, name));
    wait_for(&ids);
    expect_order("cooperative order", b"ABCABCABC");
    klog!(
        Ok,
        out,
        "threads: cooperative order ABCABCABC with the tick stopped, {} yields",
        counters().yields
    );
}

/// The context test across a switch to a thread that clobbers everything with
/// values of its own, then the same from the other side.
fn context_phase(mut out: impl Write, handover: Handover) {
    STOP.store(false, Ordering::Relaxed);
    OTHER_FAILED.store(false, Ordering::Relaxed);
    OTHER_ROUNDS.store(0, Ordering::Relaxed);
    start_phase();
    let kind = match handover {
        Handover::Yield => 0,
        Handover::Tick => 1,
    };
    let other = spawn(context_other, kind).unwrap_or_else(|error| panic!("threads: {error}"));
    // A tick handover can return without a switch (a `wfi` may wake early):
    // try again until the other thread has run.
    let mut attempts = 0;
    loop {
        let switches = switch_count();
        let failed = arch::switch_selftest(handover, Side::Own);
        assert!(
            !failed,
            "threads: context changed across a switch to another thread"
        );
        if switch_count() > switches {
            break;
        }
        attempts += 1;
        assert!(
            attempts < 50,
            "threads: the context test never switched to the other thread"
        );
    }
    STOP.store(true, Ordering::Relaxed);
    wait_for(&[other]);
    assert!(
        !OTHER_FAILED.load(Ordering::Relaxed),
        "threads: the other thread's context changed across a switch"
    );
    assert!(
        OTHER_ROUNDS.load(Ordering::Relaxed) >= 1,
        "threads: the other thread never completed the context test"
    );
    klog!(
        Ok,
        out,
        "threads: context intact across a switch to another thread, {} on both sides: registers, flags, SP, SP_EL0 and the stack below SP",
        match handover {
            Handover::Yield => "by a yield",
            Handover::Tick => "by the tick",
        }
    );
}

fn baton_phase(mut out: impl Write) {
    start_phase();
    BATON.store(0, Ordering::Release);
    let before: Counters = counters();
    let ids = [(b'A', 0usize), (b'B', 1)].map(|(name, side)| {
        spawn(baton, usize::from(name) | side << 8)
            .unwrap_or_else(|error| panic!("threads: {error}"))
    });
    wait_for(&ids);
    assert!(
        !TIMED_OUT.load(Ordering::Relaxed),
        "threads: the baton never came back"
    );
    expect_order("baton order", b"ABABAB");
    let preemptions = counters().preemptions - before.preemptions;
    // A, B, A, B each waited for the baton: four passes that needed the tick.
    assert!(
        preemptions >= 4,
        "threads: only {preemptions} switches by the tick for 4 baton passes"
    );
    klog!(
        Ok,
        out,
        "threads: baton order ABABAB without a yield, {preemptions} switches by the tick"
    );
}

/// Each thread's stack must be read and write and never executed, with nothing
/// mapped below it: the guard that stops an overflow (ADR 0037).
fn check_stacks(mut out: impl Write) {
    let mut checked = 0;
    for index in [1, 2, 3, IDLE] {
        let stack = thread_stack(index);
        let mut page = stack.start;
        while page < stack.end {
            let mapped = mmu::upper_page(page);
            assert!(
                matches!(mapped, Some((_, Some(Permissions::ReadWrite)))),
                "threads: stack page {page:#x} of thread {index} is not read and write"
            );
            page += PAGE_SIZE;
        }
        let slot = stack.end - THREAD_SLOT_SIZE;
        let mut guard = slot;
        while guard < stack.start {
            assert!(
                mmu::upper_page(guard).is_none(),
                "threads: guard page {guard:#x} below the stack of thread {index} is mapped"
            );
            guard += PAGE_SIZE;
        }
        checked += 1;
    }
    klog!(
        Ok,
        out,
        "threads: {checked} stacks of {} KiB read and write and never executed, the {} KiB below each unmapped, window at {:#x}",
        THREAD_STACK_SIZE >> 10,
        (THREAD_SLOT_SIZE - THREAD_STACK_SIZE) >> 10,
        kernelvm::THREAD_STACK_BASE
    );
}

/// A writer that notes whether IRQs were unmasked at any write.
struct MaskProbe {
    unmasked_write: bool,
}

impl Write for MaskProbe {
    fn write_str(&mut self, _: &str) -> core::fmt::Result {
        self.unmasked_write |= !arch::irqs_masked();
        Ok(())
    }
}

/// With IRQs unmasked, nothing a tick could switch away from may be held with
/// them unmasked: the heap, a log line of several writes, the scheduler's state.
/// Run where the tick is live, so that unmasked means a tick can arrive.
fn check_exclusion(mut out: impl Write) {
    assert!(
        !arch::irqs_masked(),
        "threads: the exclusion check needs IRQs unmasked"
    );
    // A guard puts the mask back as it found it: held with IRQs already masked
    // it must not unmask them.
    arch::disable_interrupts();
    let _ = crate::heap::lock_masks_interrupts();
    let _ = super::state_masks_interrupts();
    klog!(
        Ok,
        MaskProbe {
            unmasked_write: false
        },
        "a line"
    );
    assert!(
        arch::irqs_masked(),
        "threads: a guard unmasked IRQs that were masked before it"
    );
    arch::enable_interrupts();
    assert!(
        crate::heap::lock_masks_interrupts(),
        "threads: the heap's lock does not mask IRQs"
    );
    assert!(
        super::state_masks_interrupts(),
        "threads: the scheduler's state is held with IRQs unmasked"
    );
    let mut probe = MaskProbe {
        unmasked_write: false,
    };
    klog!(Ok, probe, "a line {} {}", 1, 2);
    assert!(
        !probe.unmasked_write,
        "threads: a log line is written with IRQs unmasked"
    );
    assert!(!arch::irqs_masked(), "threads: a guard left IRQs masked");
    klog!(
        Ok,
        out,
        "threads: IRQs are masked while the heap, a log line and the scheduler's state are held, and unmasked after"
    );
}

/// Starts the scheduler and runs the phases. It leaves the tick stopped, as the
/// interrupt test did.
pub fn selftest(mut out: impl Write) {
    super::init(Box::new(RoundRobin::new(TEST_SLICE_TICKS)));
    klog!(
        Ok,
        out,
        "threads: scheduler started, round robin with a slice of {TEST_SLICE_TICKS} tick, idle thread ready"
    );
    // The tick is stopped here (the interrupt test ended in `stop`): a yield
    // alone decides the order.
    assert!(
        !arch::timer::is_running(),
        "threads: the timer runs where the cooperative phases need the tick stopped"
    );
    let ticks = interrupts::ticks();
    cooperative_phase(&mut out);
    context_phase(&mut out, Handover::Yield);
    assert!(
        interrupts::ticks() == ticks,
        "threads: a tick arrived while the cooperative phases ran with the tick stopped"
    );
    interrupts::resume();
    check_exclusion(&mut out);
    baton_phase(&mut out);
    context_phase(&mut out, Handover::Tick);
    interrupts::stop();
    check_stacks(&mut out);
    let refused = counters().refused;
    assert!(
        refused == 0,
        "threads: the policy was refused {refused} times"
    );
    klog!(
        Ok,
        out,
        "threads: policy refused {refused} times, {} threads ended",
        counters().exits
    );
    #[cfg(feature = "selftest-thread-panic")]
    thread_that_panics();
    #[cfg(feature = "selftest-thread-overflow")]
    thread_that_overflows(&mut out);
}

#[cfg(feature = "selftest-thread-panic")]
fn thread_that_panics() {
    extern "C" fn panics(_: usize) {
        panic!("selftest: a thread panicked");
    }
    let id = spawn(panics, 0).unwrap_or_else(|error| panic!("threads: {error}"));
    // The scheduler is stopped: run it by hand, and the panic ends the boot.
    loop {
        yield_now();
        assert!(
            !is_finished(id),
            "selftest: the thread ended without panicking"
        );
    }
}

/// Spawns a thread that parks in slot 1, writes a canary in the page below its
/// stack's top page, and one that overflows its own stack, in slot 2: the fault
/// must be reported and leave the canary whole.
#[cfg(feature = "selftest-thread-overflow")]
fn thread_that_overflows(mut out: impl Write) {
    const CANARY: u64 = 0x57AC_C0DE_0000_0000;

    extern "C" fn parked(_: usize) {
        loop {
            yield_now();
        }
    }
    extern "C" fn overflows(_: usize) {
        recurse(0);
    }
    #[inline(never)]
    fn recurse(depth: usize) -> usize {
        let mut frame = [0u8; 256];
        core::hint::black_box(&mut frame)[0] = depth as u8;
        let below = if core::hint::black_box(true) {
            recurse(depth + 1)
        } else {
            0
        };
        below + core::hint::black_box(frame[0]) as usize
    }

    spawn(parked, 0).unwrap_or_else(|error| panic!("threads: {error}"));
    let neighbour = canary_page();
    for word in 0..(PAGE_SIZE / 8) {
        // SAFETY: the page is the stack of slot 1, mapped read and write; the
        // parked thread only uses the top of it.
        unsafe {
            (neighbour as *mut u64)
                .add(word as usize)
                .write_volatile(CANARY | word)
        };
    }
    klog!(Info, out, "selftest: a thread overflows its stack");
    spawn(overflows, 0).unwrap_or_else(|error| panic!("threads: {error}"));
    loop {
        yield_now();
    }
}

/// The page just below the top page of slot 1's stack.
#[cfg(feature = "selftest-thread-overflow")]
fn canary_page() -> u64 {
    thread_stack(1).end - 2 * PAGE_SIZE
}

/// Whether the canary the overflow test wrote is whole, for the panic report.
#[cfg(feature = "selftest-thread-overflow")]
pub fn canary_intact() -> bool {
    let page = canary_page() as *const u64;
    (0..PAGE_SIZE / 8).all(|word| {
        // SAFETY: as in `thread_that_overflows`.
        unsafe { page.add(word as usize).read_volatile() == 0x57AC_C0DE_0000_0000 | word }
    })
}
