---
title: "ADR 0037: Kernel threads, one saved-state format for every switch, and a narrow interface for the scheduling policy"
type: adr
status: accepted
created: 2026-11-27
updated: 2026-11-27
tags: [kernel, scheduler, threads, policy, interrupts, safety]
---

# ADR 0037: Kernel threads, one saved-state format for every switch, and a narrow interface for the scheduling policy

## Context

K7 gives the kernel threads, a context switch and a round-robin scheduler. It
is the first item where ADR 0002's split is built and not only written: the
kernel holds mechanism (create a thread, switch, keep the ready set, preempt on
the tick); a policy chooses. The policy plane (track P) will replace the first
policy with a trained model that has a fixed heuristic as fallback, so the
interface must be one a model's output can be checked against.

What the code already fixes (read, not assumed):

- Exception entry (ADR 0026, `exceptions.s`) saves a 304-byte `TrapFrame` on a
  separate exception stack, never on the interrupted stack, and restores it on
  return. The frame holds `x0`-`x30`, `SP_EL0`, `ELR`, `SPSR`, `ESR`, `FAR` and
  the interrupted `SP`. At the `bl` to the Rust handler `x0` is the frame
  pointer, so handlers can already take `&mut TrapFrame`.
- Entry masks IRQs and no handler unmasks them, so exceptions never nest on the
  exception stack, and a nested one is reported (ADR 0026, 0035).
- `interrupts::handle_irq` is also called outside an exception, by
  `check_spurious_is_counted`. It has no frame to switch.
- `TPIDR_EL1` is scratch for the entry code (it parks `x0` there for a few
  instructions); nothing else uses it.
- The heap's lock (`Locked::with`) only detects re-entry, and ADR 0029 lists
  what K7 must change in it. The console's `SAFETY` argument says nothing else
  can drive the UART because interrupts are masked.
- The tick (ADR 0035) is a counter, stopped at the end of the boot's interrupt
  test; ADR 0035 leaves what it means to K7.
- The kernel is built soft-float (no FP or SIMD state exists to save), runs at
  EL1 whatever the entry level, and the other three cores are parked with IRQs
  masked until K11.
- `aarch64-unknown-none-softfloat` has `"stack-probes": {"kind": "inline"}`
  (`rustc --print target-spec-json`, toolchain 1.100.0): a frame larger than a
  page is probed one page at a time, so a single unmapped page below a stack is
  hit before a large frame can jump over it. Variable-size stack use that the
  compiler does not probe (inline assembly, `alloca`-style code) is outside
  that, and the guard below is deliberately larger than a page.

## Decision

### Where the line is (ADR 0002)

**The kernel** creates threads, owns the thread table and the ready set, saves
and restores contexts, counts a thread's slice down on the tick and preempts
when it reaches zero, runs the idle thread when nothing is ready, and **checks
everything a policy says**.

**A policy** answers one question: of the threads that are ready, which runs
next and for how many ticks. It is a value behind this trait, in a host-tested
crate with no `unsafe` and no architecture code (`sched`, like `physmem` and
`kernelheap`):

```rust
pub trait Policy: Send {
    /// `ready` is never empty. `previous` is the thread that ran until now (it
    /// is in `ready` unless it exited or was the idle thread). `now` is `interrupts::ticks()`.
    fn pick(&mut self, ready: &ReadySet, previous: Option<ThreadIndex>, now: u64) -> Choice;
}
pub struct Choice { pub thread: ThreadIndex, pub slice_ticks: u32 }
```

What the interface does and does not allow, so that "the model proposes,
deterministic code decides" holds:

- The policy gets a read-only set of ids and returns a value. It cannot create,
  kill, block or edit a thread, and cannot reach the table.
- The kernel **validates** the choice: the thread must be in the ready set, and
  `slice_ticks` is clamped to `1..=MAX_SLICE_TICKS` (100). A choice that names a
  thread that is not ready is refused: the kernel runs the first ready thread
  after `previous` in id order (round-robin) and counts the refusal. The count
  is exported with the other scheduler counters so a watchdog (P3) can fall
  back to the fixed heuristic after repeated refusals.
- `pick` runs in IRQ context with IRQs masked. It must be bounded, must not
  allocate, free, print or block. Nothing in the trait lets it do those things
  except by breaking the rule. The rule is about bounded time: the heap's lock is
  held only with IRQs masked, so a handler never finds it held and an allocation
  there would work, unreported. Nothing enforces the rule; P3's watchdog does.
- A trained model cannot run in that path (microseconds, ADR 0002). It does not
  have to: it computes parameters asynchronously (a slice per thread, an order)
  that a small deterministic `Policy` reads in `pick`. How a model gets
  telemetry and writes those parameters is P1's ADR. This ADR leaves the room:
  `now` is passed, the kernel keeps decision counters (switches by cause,
  refusals), and `Policy` can be extended with a read-only per-thread view by a
  later ADR without touching the mechanism.
- The policy is held as a trait object by the scheduler core, installed at
  start. Replacing it at run time is not built here (P3 needs it, and decides
  how); nothing in the mechanism depends on which policy is installed.

A policy sees threads by their position in the table (`ThreadIndex`); the
generation-carrying `ThreadId` is the handle the rest of the kernel keeps.

`RoundRobin` is the first policy and has no state besides its slice: the next ready id after
`previous` in cyclic id order, or the lowest id; slice 2 ticks (20 ms).

### The tick, and who can change its period

The tick is the unit of time the scheduler counts in: a slice is a number of
ticks, `interrupts::ticks()` is the clock the policy sees. **The period is fixed
at 10 ms by the kernel** (ADR 0035's `TICK_HZ`); a policy chooses a slice in
ticks within the kernel's limits and cannot change the period. Changing it
(tickless idle, a finer tick) is a kernel change that needs its own ADR. The
tick runs under any thread while it is started; the boot test stops it at its
end (see "Sequencing the tick"), and with it stopped the idle thread masks IRQs
and parks, as `halt` did.

### A thread's saved state is always a `TrapFrame`

Every switch, cooperative or preemptive, is "return from an exception into a
different frame":

- **Preemption (IRQ).** `handle_irq_exception(frame)` calls
  `interrupts::handle_irq`, which now **returns whether a tick was taken**
  (it does not switch: `check_spurious_is_counted` calls it with no frame). The
  tick is fully ended (deadline moved, end of interrupt written) before the
  scheduler is consulted. If the slice is used up the scheduler asks the
  policy; if the answer is another thread, the handler copies `*frame` into the
  outgoing thread's slot and `*frame` from the incoming thread's slot. The
  assembly that follows restores whatever `*frame` holds.
- **Cooperative yield and exit.** `yield_now()` and the end of a thread execute
  `svc` with an immediate the kernel reserves (`0x100` yield, `0x101` exit);
  the synchronous handler runs the same switch. `ELR` of an SVC already points
  past it (no BRK-style `+4`). `svc #0` stays unhandled, because
  `selftest-unhandled-class` depends on it. An SVC from EL0 (K8) arrives on
  another vector, so there is no clash.
- **A new thread** is a synthesised frame: `ELR` = its entry, `x0` = its
  argument, `SP` = the top of its stack (16-byte aligned, asserted), `x30` = a return
  trampoline that executes the exit `svc`, `x29` = 0, `SP_EL0` = 0, and `SPSR`
  = `EL1h` with **only IRQs unmasked** (debug, SError and FIQ stay masked: no
  handler for them exists) and every other PSTATE bit clear (PAN, SSBS, DIT,
  BTYPE ...): the kernel depends on none of them, and the first exception taken
  in the thread sets what `SCTLR_EL1` says. A first version sampled the boot
  thread's `SPSR` with an SVC to inherit those bits; on every machine of the gate
  the sample was the same fixed value, so nothing could show the mechanism
  worked, and it was dropped. The scheduler starts only at EL1 (`init` asserts
  it). The first run is an `eret` into it.

Why one format: ADR 0026's entry was built so that no handler writes on the
interrupted stack (an overflowing stack is reported intact); a design that
saves context on each thread's own stack (a `switch(prev, next)` that pushes
callee-saved registers, plus a frame on the thread stack for preemption) would
need two saved-state formats, new assembly with no test yet, and would undo
that property. This design reuses the entry and restore code already tested by
`interrupt_selftest`. The price is two 304-byte copies per switch and an
exception on every yield; both are small against a 10 ms tick and are
revisited if P2's traces show the switch is the cost.

Registers the saved context could forget, one by one:

| State | Saved how |
|---|---|
| `x0`-`x30`, `SP`, `ELR`, `SPSR` (flags, DAIF, `M`) | in the frame |
| `SP_EL0` | in the frame; every kernel thread has 0 until K8 gives processes their own |
| `TPIDR_EL1` | **not saved**: it is scratch inside the entry code. A later per-core pointer there (K11) must move the scratch to another register or to the stack |
| FP/SIMD | none exists: the kernel is soft-float. K8's user programs need `CPACR_EL1` trapping and per-process state, in K8's ADR |
| `ESR`, `FAR` | in the frame, meaningless once copied; a thread never reads them |

### Exclusion: one core, so masking

All scheduler state (thread table, ready set, slice, counters) is touched only
with IRQs masked: by the handler because exception entry masks, by threads
through an `IrqGuard` that saves `DAIF`, masks, and restores the saved value
on drop (so it nests and never unmasks a caller that had IRQs masked, such as
a thread that called `yield_now` with them off). There is no lock: on one core
a masked section cannot be interleaved. The static is a `Sync` wrapper whose
`SAFETY` says exactly that. K11 replaces it with per-core queues and a spin
lock; this ADR does not pretend the masking scales.

Obligations ADR 0029 put on K7, one by one:

- `Locked::with` (the heap) takes an `IrqGuard` for the whole critical
  section: a tick must not switch away from a thread that holds the heap. The
  re-entry assertion stays, and now only reports a lock taken twice by a thread.
- Ordering: `msr daifset`/`daifclr` are compiler barriers and the existing
  compiler fences stay; one core observes its own accesses in program order and
  an exception entry or `eret` is a context synchronisation event, so no
  hardware barrier is needed. This is not true on K11 and says so.
- Exclusives on uncached memory: the scheduler uses plain loads and stores under
  the mask, no `ldxr`/`stxr`. (The existing atomic counters are not touched.)
- Panics: the panic handler already masks IRQs and does not allocate; it now
  also names the current thread.

**The console.** A `klog!` line is several `write_str` calls, so masking only
inside `write_str` would let two threads interleave inside a line and defeat a
check on the order of lines. `log::line` masks around the whole line and
`Console::write_str` does not mask: output that is not a `klog!` line comes from
the boot thread before the scheduler starts or with IRQs masked (the panic
report), and a thread that wrote outside `klog!` could only garble the text. The
`SAFETY` comment in `board.rs` says so. Cost, stated: at 115200 baud a
100-character line is about 9 ms with IRQs masked, nearly a whole tick. A tick
that arrives late is handled correctly (ADR 0035 skips missed ticks), so
scheduling is coarser while a thread logs, not wrong; QEMU's UART is instant.
A sleeping or ring-buffered console is the real answer and belongs with K10
(UART server).

### Thread stacks

Stacks come from a **window of their own** in the upper half, like the heap
(ADR 0033's table gains a row; its decision is unchanged): `KERNEL_OFFSET +
416 GiB`, 1 GiB, between the device window and the heap, nothing else mapped
in it.

- A slot is 64 KiB: the **top 32 KiB are the stack**, mapped read and write,
  never executable, from frames of the frame allocator (through
  `heap::with_frames`); the **bottom 32 KiB are never mapped**, the guard. The
  gap is larger than the one page that inline probes need because it costs only
  virtual space and also covers code the compiler does not probe.
- The window holds `MAX_THREADS` slots (64, including the idle thread). Slots
  are **recycled, never unmapped**: a thread's exit frees its table entry, which
  is its slot (the next spawn takes the lowest free one), still mapped. No page is ever removed, so no break-before-make and no
  TLB invalidation are needed (the property `grow_heap` already relies on); the
  cost is that the stacks of the most threads ever alive stay allocated. The
  contents are not cleared; a later ADR (K8) decides that for anything a user
  program could read.
- `mmu::grow_heap` is generalised to `mmu::map_pages(frames, at, pages)` and the
  heap calls it; no second copy.
- `mmu::verify` runs before the heap exists and never sees this window. The
  scheduler's boot test checks, through `mmu::upper_page`, that each stack page
  is read and write and not executable and that the page just below each
  stack, and the one below the lowest mapped page of the slot, is unmapped.
- **Overflow** of a thread stack faults on the guard. It is a synchronous
  abort, not one of the SVCs, so it ends in the existing report ("unhandled
  synchronous exception ... fault address"), now naming the thread. The report
  runs on the exception stack, so the neighbouring slot is untouched; a boot
  checks a canary written in the lower neighbour's stack (a page below its top
  page) from the panic path,
  as `stack_probe_intact` does for `.bss`. The boot thread keeps its own stack
  and guard (ADR 0026).
- Limit: a frame the compiler does not probe and that is larger than 32 KiB
  can still reach the lower neighbour's stack. Nothing in the kernel does that;
  it is a limit, not a guarantee.

### Threads, the table and the end of a thread

- **The table is static**, `MAX_THREADS` entries, each holding its state
  (`Free`, `Reserved` while a thread is being prepared, `Ready`, `Running`; a
  thread that ended is `Free` with a new generation) and a generation; the saved
  frames sit beside the table in the kernel and the remaining slice in the core. It is not on the heap because handlers must not allocate or
  free (the heap's lock is held by the code a tick may have interrupted).
  `ThreadId` carries the generation, so an id kept after the thread exited and
  the entry was reused is reported as finished and never names the new thread.
  `spawn` returns an error (`TableFull`, `NoStack`) when the table is full or a
  stack cannot be mapped: it reports, it does not wait. A slot is the lowest free
  table index (no separate free list).
- **The boot thread** is adopted as thread 0: it keeps the boot stack and
  becomes `Running`; its frame is filled the first time it is switched away
  from. `kernel_main` stays `-> !`.
- **Idle** is a thread created at start, outside the ready set, that loops
  `wfi`, with IRQs unmasked while the tick runs and masked while it is stopped. When the ready set is empty the kernel runs it
  without asking the policy (the policy is never asked with an empty set). While the tick runs it keeps the clock alive (the boot test stops the tick at
  its end, as ADR 0035 left it, so after it nothing is preempted or woken). With no blocking primitive in K7
  (that comes with IPC, K9) it only runs once every other thread has ended,
  the boot thread included: at the end of `kernel_main` the boot thread ends
  (it replaces `halt`) and the boot test sees the idle thread run.
- **A thread returns**: its entry returns into the trampoline, which executes
  the exit `svc`. The handler marks it `Exited`, releases its slot, and picks
  the next thread; the outgoing frame is not saved.
- **A thread panics**: kernel threads are trusted kernel code, so a panic is a
  kernel panic, as everywhere else, and the report names the thread. Killing
  only the faulting program is K8's criterion, for user programs.
- **A thread that never yields** is preempted when its slice ends. **A thread
  that masks IRQs and never unmasks them** stops the system: no mechanism
  here can preempt it, and a hardware watchdog is outside K7. P3's watchdog on
  the policy does not cover this.
- Only the boot core schedules. The three parked cores have no frame, no ready
  set and IRQs masked (ADR 0035); K11 gives each its own.

### Sequencing the tick

`interrupts::start` stays what ADR 0035 made it and is **not** called again:
it re-initialises the controller, calls the EL3 monitor and reruns the spurious
check. A new `interrupts::resume` only sets the next deadline, starts the timer
and unmasks IRQs, the inverse of `stop`. The interrupt test still ends in
`stop()`; the scheduler's boot test calls `resume` for the preemptive phase and
`stop` at its end, so the boots that follow run with the tick stopped as before.
A boot that reaches its end now ends the boot thread (it replaces `halt`) and
the idle thread parks the core with IRQs masked; a boot that ends in a fault
still ends in the panic handler, which now names the thread.

### The gate

The scheduler's boot test runs in **every boot that reaches the end**: direct,
UEFI, EL1, EL2, EL3, GICv2, GICv3, one and four cores. Its output is exact
lines that `scripts/boot-smoke-test.sh` compares in order; none of the order
depends on time:

1. **Round-robin order, tick stopped.** Three threads A, B, C each print their
   name and yield, three rounds. With no tick nothing can preempt, so the line
   order is exactly `A B C A B C A B C`, checked.
2. **Preemption, no yields.** Two threads pass a baton held in an atomic: a
   thread prints only when the baton is its own, hands it over, and spins until
   it is its own again. It never yields, so every handover is a tick-driven
   preemption. The order `A B A B A B` is fixed by the baton, not by the host's
   speed; a bound counted in the timer's counter turns a hang into a report. The
   gate also checks that the kernel counted at least four tick-driven
   switches: A, B, A and B each wait for the baton, which only a tick can end
   (the structural minimum; a slow host sees more).
3. **Context across a switch.** The `context_selftest` macro gets two triggers
   that switch to a thread which loads different values in every register,
   the flags and `SP_EL0`, and back: one cooperative, one by the tick. The
   original thread checks that all of them (and the pattern below its `SP`)
   came back. Output order cannot show a frame-copy bug; this does.
4. **The policy interface.** Host unit tests of `sched`: round-robin order,
   exit and slot reuse (a stale id is not the new thread), a refused choice
   (not ready, slice 0, slice above the maximum) falls back and is counted, an
   empty ready set never reaches the policy, the table and the slot pool fill
   and report.
5. **Stacks.** The page checks above, run in the boot test.
6. **Boots that end in a fault**: a thread that overflows its stack (report
   names the thread, the neighbour's canary intact), a thread that panics
   (report names it).

The boot test runs the scheduler with a slice of one tick, so that a test needs
few ticks (production default: `ROUND_ROBIN_SLICE_TICKS`, 2). A seventh check
asks, with IRQs unmasked, that they are masked while the heap's lock, a log
line and the scheduler's state are held and that a guard does not unmask what
was masked before it. The threads of the order phases assert at their start that
only IRQs are unmasked, the idle thread that IRQs are masked when the tick is
stopped, and the boot test asserts that no tick arrived during the phases that
run with the tick stopped.

Each check is confirmed by mutation before the pull request: break the
behaviour (skip the frame restore of one register, switch on the wrong
thread, drop the `IrqGuard` in the heap or in `log::line`, map the guard page,
remove the baton wait, make the policy return a thread that is not ready) and
see the gate fail.

## Consequences

- Every switch goes through the exception path; a yield costs an exception
  entry and two 304-byte copies. Acceptable, measured later by P2's traces.
- `Locked::with` and `log::line` now mask IRQs. A long
  line delays the tick by up to its transmit time on a real UART.
- The scheduler is single-core by construction (masking, one table, one ready
  set); K11 must replace the masking with a lock and the table with per-core
  queues, and the console with something that does not mask.
- At most 64 threads and 32 KiB per stack, both constants; the stacks of the
  most threads ever alive stay allocated.
- `TPIDR_EL1` cannot hold a per-core pointer until the entry code stops using it
  as scratch.
- No blocking, sleeping or wakeup yet: the scheduler's states are `Ready`,
  `Running`, with `Reserved` while a thread is prepared and an ended thread back
  to `Free`. Blocking comes with IPC (K9), and with it the policy may
  need to know why a thread is not ready.
- A policy cannot be swapped while running until P3 designs it, and a policy
  cannot see per-thread facts (priority, time run) until P1 does.
- ADR 0033's layout table has one more row (the stack window); its decision is
  unchanged. ADR 0035's "stopped again until K7 starts it" is answered here.
- A thread stuck with IRQs masked is not detected (see above). Neither is a
  policy whose `pick` does not return in time or panics: the kernel checks what
  it returns, not how long it takes. A bound on that is P3's watchdog.
- The scheduler's state, with its table of saved frames (about 19 KiB), is in
  `.data`, because the table starts with the boot and idle entries set.
- `spawn` reports `NoStack` when the frame allocator cannot give a thread's
  stack; pages mapped before that stay mapped and are counted, so the next spawn
  of the slot finishes the stack. Stack frames come from the allocator the heap
  also draws on and are not part of the heap's reserve of frames. No boot of the
  gate exhausts frames at a spawn, so that path is exercised by reading only.

## Alternatives considered

- **`switch(prev, next)` in assembly with the context on each thread's stack**,
  as most kernels do. Faster for a cooperative switch; rejected for the two
  formats and for undoing ADR 0026's guarantee that no handler writes on the
  stack it interrupted.
- **The policy owns the ready queue.** Rejected: the state would exist twice
  (policy and kernel) and a faulty policy could lose a thread. The kernel owns
  the set; the policy returns a choice from it.
- **A policy that may return "no thread" or set the tick period.** Rejected:
  "nothing to run" is the kernel's fact (the set is empty), and the period is
  mechanism.
- **Thread control blocks and stacks on the heap, freed on exit.** Rejected for
  the handler (it cannot allocate or free) and for unmapping (it needs
  break-before-make and a TLB invalidation that the QEMU TCG would not exercise).
- **A lock instead of masking.** A spin lock on one core can deadlock against
  the handler that interrupted its holder; masking is the correct primitive
  until there is a second core.
- **A test that sleeps and measures time.** Rejected: the CI is slow and
  irregular. Order is fixed by the program (yields with the tick stopped, a
  baton under preemption), and time only bounds how long a hang is waited on.
