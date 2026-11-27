//! The scheduler's mechanism and the interface a policy uses (ADR 0037).
//!
//! No architecture code and no `unsafe`: the thread table, the ready set, the
//! slice accounting and the checks on a policy's answer are tested on the host.
//! The kernel owns the saved contexts and the stacks and acts on the `Switch`
//! this core returns.
//!
//! The policy answers one question, which ready thread runs next and for how
//! many ticks, and the core checks the answer (ADR 0002: the model proposes,
//! deterministic code decides).

#![no_std]

/// Thread table entries, the idle thread's included.
pub const MAX_THREADS: usize = 64;
// The ready set is one word.
const _: () = assert!(MAX_THREADS <= u64::BITS as usize);
/// The idle thread's index: the last entry, never in the ready set.
pub const IDLE: ThreadIndex = MAX_THREADS - 1;
/// The longest slice a policy may ask for, in ticks.
pub const MAX_SLICE_TICKS: u32 = 100;
/// The slice of the round-robin policy, in ticks.
pub const ROUND_ROBIN_SLICE_TICKS: u32 = 2;

/// A position in the thread table. A policy sees threads only by index.
pub type ThreadIndex = usize;

/// A handle that outlives the thread: the table entry is reused once the thread
/// ends, and the generation tells a stale handle from the new thread.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ThreadId {
    pub index: ThreadIndex,
    generation: u32,
}

/// The ready threads, by index. The idle thread is never in it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct ReadySet(u64);

impl ReadySet {
    pub const EMPTY: Self = Self(0);

    pub fn contains(&self, index: ThreadIndex) -> bool {
        index < MAX_THREADS && self.0 & (1 << index) != 0
    }

    pub fn is_empty(&self) -> bool {
        self.0 == 0
    }

    pub fn len(&self) -> usize {
        self.0.count_ones() as usize
    }

    /// The lowest index in the set.
    pub fn first(&self) -> Option<ThreadIndex> {
        (!self.is_empty()).then(|| self.0.trailing_zeros() as usize)
    }

    /// The first index above `after` in the set, wrapping to the lowest one.
    pub fn next_after(&self, after: ThreadIndex) -> Option<ThreadIndex> {
        let above = if after + 1 >= MAX_THREADS {
            0
        } else {
            self.0 & !((1u64 << (after + 1)) - 1)
        };
        if above != 0 {
            Some(above.trailing_zeros() as usize)
        } else {
            self.first()
        }
    }

    fn insert(&mut self, index: ThreadIndex) {
        self.0 |= 1 << index;
    }

    fn remove(&mut self, index: ThreadIndex) {
        self.0 &= !(1 << index);
    }
}

/// What a policy answers: which thread, and for how many ticks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Choice {
    pub thread: ThreadIndex,
    pub slice_ticks: u32,
}

/// The one question a scheduling policy answers. It runs in interrupt context
/// with interrupts masked: it must be bounded and must not allocate, free,
/// print or block.
pub trait Policy: Send {
    /// `ready` is never empty. `previous` is the thread that ran until now: in
    /// `ready` unless it exited or was the idle thread. `now` is the tick count.
    fn pick(&mut self, ready: &ReadySet, previous: Option<ThreadIndex>, now: u64) -> Choice;
}

/// The first policy: the next ready thread in cyclic index order, each for
/// `slice_ticks`.
pub struct RoundRobin {
    slice_ticks: u32,
}

impl RoundRobin {
    pub const fn new(slice_ticks: u32) -> Self {
        Self { slice_ticks }
    }

    /// The thread after `previous` in the set, or the lowest. `ready` is not empty.
    fn next(ready: &ReadySet, previous: Option<ThreadIndex>) -> ThreadIndex {
        previous
            .and_then(|previous| ready.next_after(previous))
            .or_else(|| ready.first())
            .expect("the policy is never asked with an empty ready set")
    }
}

impl Policy for RoundRobin {
    fn pick(&mut self, ready: &ReadySet, previous: Option<ThreadIndex>, _now: u64) -> Choice {
        Choice {
            thread: Self::next(ready, previous),
            slice_ticks: self.slice_ticks,
        }
    }
}

impl Default for RoundRobin {
    fn default() -> Self {
        Self::new(ROUND_ROBIN_SLICE_TICKS)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Free,
    /// Taken by `reserve`, not runnable until `start`.
    Reserved,
    Ready,
    Running,
}

#[derive(Clone, Copy)]
struct Entry {
    state: State,
    generation: u32,
}

/// Why the running thread is being replaced.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Cause {
    Preempted,
    Yielded,
    Exited,
}

/// Which context the kernel saves and which it loads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Switch {
    /// The context to save: `None` when the thread ended.
    pub from: Option<ThreadIndex>,
    pub to: ThreadIndex,
}

/// What the scheduler counted, for the log and for a watchdog.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counters {
    pub preemptions: u64,
    pub yields: u64,
    pub exits: u64,
    /// Answers of the policy the core did not accept.
    pub refused: u64,
}

pub struct Core {
    threads: [Entry; MAX_THREADS],
    ready: ReadySet,
    current: ThreadIndex,
    slice_left: u32,
    counters: Counters,
}

impl Core {
    /// A table with the boot thread (index 0) running and the idle thread
    /// created and not ready.
    pub const fn new() -> Self {
        let free = Entry {
            state: State::Free,
            generation: 0,
        };
        let mut threads = [free; MAX_THREADS];
        threads[0].state = State::Running;
        threads[IDLE].state = State::Ready;
        Self {
            threads,
            ready: ReadySet::EMPTY,
            current: 0,
            slice_left: ROUND_ROBIN_SLICE_TICKS,
            counters: Counters {
                preemptions: 0,
                yields: 0,
                exits: 0,
                refused: 0,
            },
        }
    }

    pub fn counters(&self) -> Counters {
        self.counters
    }

    pub fn current(&self) -> ThreadId {
        self.id(self.current)
    }

    pub fn ready(&self) -> ReadySet {
        self.ready
    }

    fn id(&self, index: ThreadIndex) -> ThreadId {
        ThreadId {
            index,
            generation: self.threads[index].generation,
        }
    }

    /// Takes the lowest free entry (never the boot or idle entries) without
    /// making it runnable: the kernel prepares its stack and context first.
    pub fn reserve(&mut self) -> Option<ThreadId> {
        let index = (1..IDLE).find(|&i| self.threads[i].state == State::Free)?;
        self.threads[index].state = State::Reserved;
        Some(self.id(index))
    }

    /// Gives a reserved entry back unused.
    pub fn release(&mut self, id: ThreadId) {
        assert!(
            self.is_reserved(id),
            "released a thread that is not reserved"
        );
        self.threads[id.index].state = State::Free;
    }

    /// Makes a reserved thread ready.
    pub fn start(&mut self, id: ThreadId) {
        assert!(
            self.is_reserved(id),
            "started a thread that is not reserved"
        );
        self.threads[id.index].state = State::Ready;
        self.ready.insert(id.index);
    }

    fn is_reserved(&self, id: ThreadId) -> bool {
        let entry = &self.threads[id.index];
        entry.generation == id.generation && entry.state == State::Reserved
    }

    /// Whether the thread ended (or never existed): a handle kept after the
    /// entry was reused is finished too.
    pub fn is_finished(&self, id: ThreadId) -> bool {
        let entry = &self.threads[id.index];
        entry.generation != id.generation || entry.state == State::Free
    }

    /// One tick of the running thread's slice. A switch when the slice is used
    /// up and the policy names another thread.
    pub fn on_tick(&mut self, policy: &mut dyn Policy, now: u64) -> Option<Switch> {
        self.slice_left = self.slice_left.saturating_sub(1);
        if self.slice_left > 0 {
            return None;
        }
        self.reschedule(policy, now, Cause::Preempted)
    }

    /// The running thread gives up the rest of its slice.
    pub fn yield_now(&mut self, policy: &mut dyn Policy, now: u64) -> Option<Switch> {
        self.reschedule(policy, now, Cause::Yielded)
    }

    /// The running thread ended: its entry is freed and another one runs.
    pub fn exit(&mut self, policy: &mut dyn Policy, now: u64) -> Switch {
        assert!(self.current != IDLE, "the idle thread does not exit");
        self.reschedule(policy, now, Cause::Exited)
            .expect("an exiting thread is always replaced")
    }

    fn reschedule(&mut self, policy: &mut dyn Policy, now: u64, cause: Cause) -> Option<Switch> {
        let leaving = self.current;
        let previous = Some(leaving);
        match cause {
            Cause::Exited => {
                let entry = &mut self.threads[leaving];
                entry.state = State::Free;
                entry.generation = entry.generation.wrapping_add(1);
                self.counters.exits += 1;
            }
            _ if leaving != IDLE => {
                self.threads[leaving].state = State::Ready;
                self.ready.insert(leaving);
            }
            // Idle is ready whenever it is not running, and never in the set.
            _ => self.threads[IDLE].state = State::Ready,
        }
        let (next, slice) = if self.ready.is_empty() {
            // Nothing to ask the policy about: the idle thread, for one tick.
            (IDLE, 1)
        } else {
            self.checked_choice(policy, previous, now)
        };
        self.slice_left = slice;
        if next != IDLE {
            self.ready.remove(next);
        }
        self.threads[next].state = State::Running;
        self.current = next;
        if next == leaving {
            return None;
        }
        match cause {
            Cause::Preempted => self.counters.preemptions += 1,
            Cause::Yielded => self.counters.yields += 1,
            Cause::Exited => {}
        }
        Some(Switch {
            from: (cause != Cause::Exited).then_some(leaving),
            to: next,
        })
    }

    /// The policy's answer if it names a ready thread, clamped; otherwise
    /// round-robin, and the refusal is counted.
    fn checked_choice(
        &mut self,
        policy: &mut dyn Policy,
        previous: Option<ThreadIndex>,
        now: u64,
    ) -> (ThreadIndex, u32) {
        let choice = policy.pick(&self.ready, previous, now);
        if self.ready.contains(choice.thread) && (1..=MAX_SLICE_TICKS).contains(&choice.slice_ticks)
        {
            return (choice.thread, choice.slice_ticks);
        }
        self.counters.refused += 1;
        if self.ready.contains(choice.thread) {
            // A valid thread with a slice outside the limits is clamped, not replaced.
            return (choice.thread, choice.slice_ticks.clamp(1, MAX_SLICE_TICKS));
        }
        (
            RoundRobin::next(&self.ready, previous),
            ROUND_ROBIN_SLICE_TICKS,
        )
    }
}

impl Default for Core {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;
