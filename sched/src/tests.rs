use super::*;

/// Starts `count` threads and returns their handles.
fn spawn(core: &mut Core, count: usize) -> [Option<ThreadId>; 8] {
    let mut ids = [None; 8];
    for slot in ids.iter_mut().take(count) {
        let id = core.reserve().expect("a free entry");
        core.start(id);
        *slot = Some(id);
    }
    ids
}

/// Runs `yields` yields and returns the indices that ran, in order.
fn run_order(core: &mut Core, policy: &mut dyn Policy, yields: usize) -> [usize; 16] {
    let mut order = [usize::MAX; 16];
    for slot in order.iter_mut().take(yields) {
        if let Some(switch) = core.yield_now(policy, 0) {
            *slot = switch.to;
        } else {
            *slot = core.current().index;
        }
    }
    order
}

#[test]
fn round_robin_visits_every_ready_thread_in_index_order() {
    let mut core = Core::new();
    spawn(&mut core, 3);
    let order = run_order(&mut core, &mut RoundRobin::default(), 8);
    assert_eq!(&order[..8], &[1, 2, 3, 0, 1, 2, 3, 0]);
}

#[test]
fn a_single_thread_is_not_switched_away_from() {
    let mut core = Core::new();
    assert_eq!(core.yield_now(&mut RoundRobin::default(), 0), None);
    assert_eq!(core.current().index, 0);
    assert_eq!(core.counters().yields, 0);
}

#[test]
fn the_tick_preempts_only_when_the_slice_is_used_up() {
    let mut core = Core::new();
    spawn(&mut core, 1);
    assert_eq!(core.yield_now(&mut RoundRobin::default(), 0).unwrap().to, 1);
    for _ in 0..ROUND_ROBIN_SLICE_TICKS - 1 {
        assert_eq!(core.on_tick(&mut RoundRobin::default(), 0), None);
    }
    let switch = core
        .on_tick(&mut RoundRobin::default(), 0)
        .expect("the slice ended");
    assert_eq!(
        switch,
        Switch {
            from: Some(1),
            to: 0
        }
    );
    assert_eq!(core.counters().preemptions, 1);
}

#[test]
fn a_yield_starts_a_new_slice_for_the_next_thread() {
    let mut core = Core::new();
    spawn(&mut core, 1);
    core.on_tick(&mut RoundRobin::default(), 0);
    core.yield_now(&mut RoundRobin::default(), 0);
    // Thread 1 got a whole slice: one tick does not end it.
    assert_eq!(core.on_tick(&mut RoundRobin::default(), 0), None);
}

#[test]
fn an_exit_frees_the_entry_and_a_stale_handle_is_finished() {
    let mut core = Core::new();
    let ids = spawn(&mut core, 2);
    let (first, second) = (ids[0].unwrap(), ids[1].unwrap());
    core.yield_now(&mut RoundRobin::default(), 0);
    assert_eq!(core.current(), first);
    let switch = core.exit(&mut RoundRobin::default(), 0);
    assert_eq!(
        switch,
        Switch {
            from: None,
            to: second.index
        }
    );
    assert!(core.is_finished(first));
    assert!(!core.is_finished(second));
    // The entry is reused, and the old handle does not name the new thread.
    let again = core.reserve().unwrap();
    assert_eq!(again.index, first.index);
    assert_ne!(again, first);
    core.start(again);
    assert!(core.is_finished(first));
    assert!(!core.is_finished(again));
}

#[test]
fn when_nothing_is_ready_the_idle_thread_runs_without_asking_the_policy() {
    struct NeverAsked;
    impl Policy for NeverAsked {
        fn pick(&mut self, _: &ReadySet, _: Option<ThreadIndex>, _: u64) -> Choice {
            panic!("asked with an empty ready set");
        }
    }
    let mut core = Core::new();
    // The boot thread is the only one: when it ends nothing is ready.
    let switch = core.exit(&mut NeverAsked, 0);
    assert_eq!(
        switch,
        Switch {
            from: None,
            to: IDLE
        }
    );
    assert!(!core.ready().contains(IDLE));
    // Idle is left as soon as something is ready, at the next tick.
    let woken = core.reserve().unwrap();
    core.start(woken);
    assert_eq!(
        core.on_tick(&mut RoundRobin::default(), 0),
        Some(Switch {
            from: Some(IDLE),
            to: woken.index
        })
    );
    assert!(!core.ready().contains(IDLE));
}

/// Always names the same thread and slice, whatever is ready.
struct Fixed(Choice);
impl Policy for Fixed {
    fn pick(&mut self, _: &ReadySet, _: Option<ThreadIndex>, _: u64) -> Choice {
        self.0
    }
}

#[test]
fn a_thread_that_is_not_ready_is_refused_and_counted() {
    let mut core = Core::new();
    spawn(&mut core, 2);
    // Index 5 is free: not ready.
    let mut policy = Fixed(Choice {
        thread: 5,
        slice_ticks: 3,
    });
    let switch = core.yield_now(&mut policy, 0).unwrap();
    assert_eq!(switch.to, 1, "the fallback is round-robin");
    assert_eq!(core.counters().refused, 1);
}

#[test]
fn a_thread_that_is_running_or_idle_is_not_a_valid_choice() {
    let mut core = Core::new();
    spawn(&mut core, 1);
    for thread in [IDLE, MAX_THREADS, usize::MAX] {
        let mut policy = Fixed(Choice {
            thread,
            slice_ticks: 1,
        });
        core.yield_now(&mut policy, 0);
    }
    assert_eq!(core.counters().refused, 3);
}

#[test]
fn a_slice_outside_the_limits_is_clamped_and_counted() {
    let mut core = Core::new();
    spawn(&mut core, 1);
    let mut policy = Fixed(Choice {
        thread: 1,
        slice_ticks: 0,
    });
    core.yield_now(&mut policy, 0);
    // Clamped to one tick: the next tick ends it.
    assert!(core.on_tick(&mut RoundRobin::default(), 0).is_some());
    let mut policy = Fixed(Choice {
        thread: 1,
        slice_ticks: u32::MAX,
    });
    core.yield_now(&mut policy, 0);
    for _ in 0..MAX_SLICE_TICKS - 1 {
        assert_eq!(core.on_tick(&mut RoundRobin::default(), 0), None);
    }
    assert!(core.on_tick(&mut RoundRobin::default(), 0).is_some());
    assert_eq!(core.counters().refused, 2);
}

#[test]
fn a_policy_that_chooses_a_ready_thread_is_obeyed() {
    let mut core = Core::new();
    spawn(&mut core, 3);
    let mut policy = Fixed(Choice {
        thread: 3,
        slice_ticks: 7,
    });
    assert_eq!(core.yield_now(&mut policy, 0).unwrap().to, 3);
    assert_eq!(core.counters().refused, 0);
}

#[test]
fn the_table_fills_and_reports() {
    let mut core = Core::new();
    let mut taken = 0;
    while let Some(id) = core.reserve() {
        core.start(id);
        taken += 1;
    }
    assert_eq!(
        taken,
        MAX_THREADS - 2,
        "the boot and idle entries are not for spawn"
    );
    assert_eq!(core.ready().len(), taken);
}

#[test]
fn a_released_reservation_never_ran() {
    let mut core = Core::new();
    let id = core.reserve().unwrap();
    core.release(id);
    assert!(core.ready().is_empty());
    assert_eq!(core.reserve().unwrap().index, id.index);
}

#[test]
fn a_reserved_thread_is_not_scheduled() {
    let mut core = Core::new();
    let id = core.reserve().unwrap();
    assert_eq!(core.yield_now(&mut RoundRobin::default(), 0), None);
    assert!(!core.is_finished(id));
}

#[test]
fn next_after_wraps_and_handles_the_last_index() {
    let mut set = ReadySet::EMPTY;
    set.insert(2);
    set.insert(62);
    assert_eq!(set.next_after(2), Some(62));
    assert_eq!(set.next_after(62), Some(2));
    assert_eq!(set.next_after(63), Some(2));
    assert_eq!(set.next_after(0), Some(2));
    assert_eq!(ReadySet::EMPTY.next_after(0), None);
}

#[test]
#[should_panic(expected = "idle thread does not exit")]
fn the_idle_thread_does_not_exit() {
    let mut core = Core::new();
    // The boot thread leaves, nothing else is ready: idle runs, and cannot exit.
    core.exit(&mut RoundRobin::default(), 0);
    core.exit(&mut RoundRobin::default(), 0);
}
