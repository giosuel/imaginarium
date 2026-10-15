// SPDX-License-Identifier: AGPL-3.0-or-later

//! Behaviour of signals, memos, effects, batches and scopes.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use imaginarium_reactive::{
    Scope, batch, effect, live_nodes_for_tests, memo, on_cleanup, signal, trigger, untrack,
};

/// A counter shared with closures.
#[derive(Clone, Default)]
struct Count(Rc<Cell<usize>>);

impl Count {
    fn bump(&self) {
        self.0.set(self.0.get() + 1);
    }
    fn get(&self) -> usize {
        self.0.get()
    }
}

#[test]
fn effects_run_once_now_and_once_per_change() {
    let value = signal(1);
    let runs = Count::default();
    let seen = Rc::new(Cell::new(0));
    let (r, s) = (runs.clone(), seen.clone());
    effect(move || {
        r.bump();
        s.set(value.get());
    });
    assert_eq!((runs.get(), seen.get()), (1, 1));
    value.set(2);
    assert_eq!((runs.get(), seen.get()), (2, 2));
    value.update(|v| *v += 1);
    assert_eq!((runs.get(), seen.get()), (3, 3));
    // Every write counts, even of an equal value...
    value.set(3);
    assert_eq!(runs.get(), 4);
    // ...unless asked otherwise.
    assert!(!value.set_if_changed(3));
    assert_eq!(runs.get(), 4);
    assert!(value.set_if_changed(4));
    assert_eq!(runs.get(), 5);
}

#[test]
fn memos_are_lazy_cached_and_stop_unchanged_updates() {
    let value = signal(3);
    let computed = Count::default();
    let c = computed.clone();
    let parity = memo(move |_| {
        c.bump();
        value.get() % 2
    });
    assert_eq!(computed.get(), 0, "not computed until read");
    assert_eq!(parity.get(), 1);
    assert_eq!(parity.get(), 1);
    assert_eq!(computed.get(), 1, "cached");

    let runs = Count::default();
    let r = runs.clone();
    effect(move || {
        let _ = parity.get();
        r.bump();
    });
    assert_eq!(runs.get(), 1);
    value.set(5); // still odd
    assert_eq!(computed.get(), 2, "the memo re-ran");
    assert_eq!(
        runs.get(),
        1,
        "but its value didn't change, so the effect didn't"
    );
    value.set(6);
    assert_eq!(runs.get(), 2);
}

#[test]
fn memos_get_their_previous_value() {
    let value = signal(1);
    let history = memo(move |previous: Option<&Vec<i32>>| {
        let mut list = previous.cloned().unwrap_or_default();
        list.push(value.get());
        list
    });
    assert_eq!(history.get(), vec![1]);
    value.set(2);
    assert_eq!(history.get(), vec![1, 2]);
}

#[test]
fn updates_are_glitch_free() {
    // The diamond: a → (b, c) → d. When a changes, d must never see the
    // new b with the old c, and runs once.
    let a = signal(1);
    let b = memo(move |_| a.get() * 2);
    let c = memo(move |_| a.get() * 3);
    let seen = Rc::new(RefCell::new(Vec::new()));
    let s = seen.clone();
    effect(move || s.borrow_mut().push((a.get(), b.get(), c.get())));
    a.set(2);
    a.set(3);
    assert_eq!(*seen.borrow(), vec![(1, 2, 3), (2, 4, 6), (3, 6, 9)]);
}

#[test]
fn batches_run_effects_once_at_the_end() {
    let (a, b) = (signal(1), signal(1));
    let runs = Count::default();
    let r = runs.clone();
    effect(move || {
        let _ = a.get() + b.get();
        r.bump();
    });
    batch(|| {
        a.set(2);
        b.set(2);
        batch(|| a.set(3)); // nested: still deferred
        assert_eq!(runs.get(), 1, "nothing ran inside the batch");
    });
    assert_eq!(runs.get(), 2);
}

#[test]
fn dependencies_follow_the_latest_run() {
    let flag = signal(true);
    let (left, right) = (signal("left"), signal("right"));
    let runs = Count::default();
    let r = runs.clone();
    effect(move || {
        r.bump();
        let _ = if flag.get() { left.get() } else { right.get() };
    });
    right.set("RIGHT");
    assert_eq!(runs.get(), 1, "right wasn't read");
    flag.set(false);
    assert_eq!(runs.get(), 2);
    left.set("LEFT");
    assert_eq!(runs.get(), 2, "left isn't read any more");
    right.set("Right");
    assert_eq!(runs.get(), 3);
}

#[test]
fn untracked_reads_do_not_subscribe() {
    let (tracked, quiet) = (signal(0), signal(0));
    let runs = Count::default();
    let r = runs.clone();
    effect(move || {
        let _ = tracked.get();
        let _ = quiet.get_untracked();
        let _ = untrack(|| quiet.get());
        r.bump();
    });
    quiet.set(1);
    assert_eq!(runs.get(), 1);
    tracked.set(1);
    assert_eq!(runs.get(), 2);
}

#[test]
fn effects_may_write_signals_that_other_effects_read() {
    let input = signal(1);
    let doubled = signal(0);
    let seen = Rc::new(Cell::new(0));
    effect(move || doubled.set(input.get() * 2));
    let s = seen.clone();
    effect(move || s.set(doubled.get()));
    assert_eq!(seen.get(), 2);
    input.set(5);
    assert_eq!(seen.get(), 10);
}

#[test]
fn triggers_notify_without_a_value() {
    let changed = trigger();
    let runs = Count::default();
    let r = runs.clone();
    effect(move || {
        changed.track();
        r.bump();
    });
    changed.notify();
    changed.notify();
    assert_eq!(runs.get(), 3);
}

#[test]
fn scopes_dispose_everything_they_own() {
    let baseline = live_nodes_for_tests();
    let outside = signal(0);
    let cleaned = Count::default();
    let runs = Count::default();
    let scope = Scope::new();
    let (c, r) = (cleaned.clone(), runs.clone());
    let inner = scope.run(move || {
        let inner = signal(1);
        let derived = memo(move |_| inner.get() + outside.get());
        effect(move || {
            let _ = derived.get();
            r.bump();
        });
        on_cleanup(move || c.bump());
        inner
    });
    assert_eq!(runs.get(), 1);
    scope.dispose();
    assert_eq!(cleaned.get(), 1);
    assert!(!inner.is_alive());
    assert!(inner.try_with(|v| *v).is_none());
    outside.set(1);
    assert_eq!(runs.get(), 1, "the effect is gone");
    assert_eq!(
        live_nodes_for_tests(),
        baseline + 1,
        "only `outside` is left"
    );
}

#[test]
fn computations_dispose_what_they_created_before_running_again() {
    let baseline = live_nodes_for_tests();
    let rounds = signal(0);
    let cleanups = Count::default();
    let c = cleanups.clone();
    let outer = effect(move || {
        let _ = rounds.get();
        // Created anew in every run, like a list's rows.
        let row = signal(0);
        let c = c.clone();
        effect(move || {
            let _ = row.get();
        });
        on_cleanup(move || c.bump());
    });
    for round in 1..=50 {
        rounds.set(round);
    }
    assert_eq!(cleanups.get(), 50);
    // rounds, the outer effect, and the latest row and inner effect.
    assert_eq!(live_nodes_for_tests(), baseline + 4);
    outer.dispose();
    assert_eq!(cleanups.get(), 51);
    assert_eq!(live_nodes_for_tests(), baseline + 1);
}

#[test]
fn stale_handles_do_not_reach_reused_slots() {
    let old = signal(1);
    old.dispose();
    let new = signal(2); // likely reuses the slot
    assert!(!old.is_alive());
    assert!(old.try_with(|v| *v).is_none());
    old.set(3); // ignored
    assert_eq!(new.get(), 2);
}

#[test]
#[should_panic(expected = "depends on itself")]
fn a_memo_that_reads_itself_is_a_cycle() {
    let slot: Rc<Cell<Option<imaginarium_reactive::Memo<i32>>>> = Rc::new(Cell::new(None));
    let s = slot.clone();
    let looping = memo(move |_| s.get().map_or(0, |m| m.get() + 1));
    slot.set(Some(looping));
    let _ = looping.get();
}

#[test]
#[should_panic(expected = "disposed signal")]
fn reading_a_disposed_signal_panics() {
    let value = signal(1);
    value.dispose();
    let _ = value.get();
}

#[test]
fn deep_chains_and_wide_fan_outs_update_correctly() {
    let head = signal(0);
    let mut tail = memo(move |_| head.get());
    for _ in 0..1_000 {
        let previous = tail;
        tail = memo(move |_| previous.get() + 1);
    }
    assert_eq!(tail.get(), 1_000);
    head.set(5);
    assert_eq!(tail.get(), 1_005);

    let source = signal(0);
    let total = Rc::new(Cell::new(0));
    for _ in 0..10_000 {
        let t = total.clone();
        effect(move || t.set(t.get() + source.get()));
    }
    total.set(0);
    source.set(1);
    assert_eq!(total.get(), 10_000);
}

// ---- found in the M2.1 review ----------------------------------------------

#[test]
fn a_write_while_a_computation_runs_is_not_lost() {
    // The write lands while the effect (and the memo it reads) run: both
    // must run again, and later writes must still reach the effect.
    let s = signal(0);
    let m = memo(move |_| s.get());
    let log = Rc::new(RefCell::new(Vec::new()));
    let l = log.clone();
    effect(move || {
        let v = m.get();
        l.borrow_mut().push(v);
        if v == 0 {
            s.set(1);
        }
    });
    s.set(2);
    s.set(3);
    assert_eq!(log.borrow().last(), Some(&3), "{:?}", log.borrow());
}

#[test]
fn an_effect_that_writes_its_own_source_settles() {
    let s = signal(0);
    let seen = Rc::new(Cell::new(-1));
    let k = seen.clone();
    effect(move || {
        let v = s.get();
        k.set(v);
        if v < 3 {
            s.set(v + 1);
        }
    });
    assert_eq!((s.get_untracked(), seen.get()), (3, 3));
}

#[test]
fn disposing_an_earlier_source_during_an_update_skips_nothing() {
    // m1 creates a signal that the effect reads first. When m1 re-runs, it
    // disposes that signal: the effect's later sources must still be
    // checked.
    let s = signal(0);
    let inner: Rc<Cell<Option<imaginarium_reactive::Signal<i32>>>> = Rc::new(Cell::new(None));
    let i = inner.clone();
    let m1 = memo(move |_| {
        i.set(Some(signal(0)));
        s.get() / 2
    });
    let m2 = memo(move |_| s.get());
    let _ = m1.get();
    let seen = Rc::new(Cell::new(-1));
    let k = seen.clone();
    effect(move || {
        if let Some(x) = inner.get() {
            let _ = x.try_with(|v| *v);
        }
        let _ = m1.get();
        k.set(m2.get());
    });
    s.set(1);
    assert_eq!(seen.get(), 1);
}

#[test]
fn disposing_a_source_during_a_run_is_safe() {
    let (a, b) = (signal(1), signal(2));
    let runs = Count::default();
    let r = runs.clone();
    let trigger = signal(0);
    effect(move || {
        r.bump();
        let _ = trigger.get();
        let _ = a.try_with(|v| *v);
        let _ = b.get();
        if a.is_alive() && trigger.get_untracked() == 1 {
            a.dispose();
        }
    });
    trigger.set(1);
    b.set(3);
    assert_eq!(runs.get(), 3);
}

#[test]
fn child_effects_never_outlive_their_parent() {
    // A conditional subtree: when the list empties, the parent re-runs
    // and drops the child; the child must not run on the empty list first.
    let list = signal(vec![1]);
    effect(move || {
        if !list.with(Vec::is_empty) {
            effect(move || {
                let _ = list.with(|l| l[0]);
            });
        }
    });
    list.set(Vec::new());
    list.set(vec![2]);
}

#[test]
fn a_panicking_computation_recovers() {
    let s = signal(0);
    let m = memo(move |_| {
        let v = s.get();
        assert!(v != 1, "boom");
        v
    });
    assert_eq!(m.get(), 0);
    let seen = Rc::new(Cell::new(-1));
    let k = seen.clone();
    effect(move || {
        let v = s.get();
        assert!(v != 2, "boom");
        k.set(v);
    });
    let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        s.set(1);
        m.get()
    }));
    assert!(panicked.is_err());
    // The memo kept its value and runs again on the next change.
    s.set(3);
    assert_eq!(m.get(), 3);
    assert_eq!(seen.get(), 3);
    let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| s.set(2)));
    assert!(panicked.is_err());
    s.set(4);
    assert_eq!(seen.get(), 4, "the effect runs again after it panicked");
}

#[test]
fn a_memo_that_writes_a_signal_does_not_trip_the_cycle_check() {
    let (s, t) = (signal(0), signal(0));
    let m = memo(move |_| {
        t.set(s.get());
        s.get()
    });
    effect(move || {
        if t.get() > 0 {
            let _ = m.get();
        }
    });
    s.set(1);
    assert_eq!(m.get(), 1);
}

#[test]
fn readers_that_come_and_go_do_not_grow_a_quiet_signal() {
    // A theme signal that is never written, read by short-lived views.
    let theme = signal(0);
    for _ in 0..10_000 {
        let scope = Scope::new();
        scope.run(|| {
            effect(move || {
                let _ = theme.get();
            });
        });
        scope.dispose();
    }
    assert!(theme.subscriber_entries_for_tests() < 200);
}

#[test]
fn wide_computations_and_scopes_stay_linear() {
    // Quadratic bookkeeping would take seconds here even in release.
    let start = std::time::Instant::now();
    let signals: Vec<_> = (0..20_000).map(signal).collect();
    let reads = signals.clone();
    let total = memo(move |_| reads.iter().map(|s| s.get()).sum::<i64>());
    assert_eq!(total.get(), (0..20_000).sum::<i64>());
    for s in signals {
        s.dispose();
    }
    assert!(start.elapsed().as_secs() < 2, "{:?}", start.elapsed());
}

// ---- found in the M2.9 review ------------------------------------------------

#[test]
fn a_memo_that_writes_an_already_checked_source_is_not_lost() {
    // `m2` runs during the effect's check and writes what `m1` reads,
    // after `m1` was checked.
    let (s, t) = (signal(0), signal(0));
    let m1 = memo(move |_| t.get());
    let m2 = memo(move |_| {
        t.set_if_changed(s.get());
        0
    });
    let seen = Rc::new(Cell::new(-1));
    let k = seen.clone();
    effect(move || k.set(m1.get() + m2.get()));
    assert_eq!(seen.get(), 0);
    s.set(1);
    assert_eq!(seen.get(), 1);
    t.set(5);
    assert_eq!(seen.get(), 5, "later writes still reach the effect");
}

#[test]
fn an_effect_whose_memo_panicked_runs_on_the_next_change() {
    let s = signal(0);
    let m = memo(move |_| {
        let v = s.get();
        assert!(v != 1, "boom");
        v
    });
    let seen = Rc::new(Cell::new(-1));
    let k = seen.clone();
    effect(move || k.set(m.get()));
    let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| s.set(1)));
    assert!(panicked.is_err());
    s.set(3);
    assert_eq!(seen.get(), 3);
}

#[test]
fn an_effect_whose_memo_cleanup_panicked_runs_again() {
    let s = signal(0);
    let armed = Rc::new(Cell::new(true));
    let m = memo(move |_| {
        let a = armed.clone();
        on_cleanup(move || assert!(!a.replace(false), "boom"));
        s.get()
    });
    let seen = Rc::new(Cell::new(-1));
    let k = seen.clone();
    effect(move || k.set(m.get()));
    let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| s.set(1)));
    assert!(panicked.is_err());
    s.set(2);
    assert_eq!(seen.get(), 2);
}

#[test]
fn a_runaway_effect_does_not_strand_the_others() {
    let (s, ping, other) = (signal(0u64), trigger(), signal(0));
    // Keeps triggering itself, and marks the other effect on every run, so
    // one of the two is always pending when the guard stops the flush.
    effect(move || {
        let v = s.get();
        if v > 0 {
            s.set(v + 1);
            ping.notify();
        }
    });
    let seen = Rc::new(Cell::new(-1));
    let k = seen.clone();
    effect(move || {
        ping.track();
        k.set(other.get());
    });
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| s.set(1)));
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| other.set(2)));
    assert_eq!(seen.get(), 2, "the other effect still runs");
}

#[test]
fn a_memo_read_at_top_level_runs_once_although_its_cleanup_writes() {
    let (s, t) = (signal(0), signal(0));
    let runs = Count::default();
    let r = runs.clone();
    let children = Count::default();
    let c = children.clone();
    let m = memo(move |_| {
        r.bump();
        on_cleanup(move || t.set(t.get_untracked() + 1));
        let c = c.clone();
        effect(move || c.bump());
        s.get()
    });
    // Reads `m` only once its cleanup has written `t`.
    effect(move || {
        if t.get() > 0 {
            let _ = m.get();
        }
    });
    assert_eq!(m.get(), 0);
    assert_eq!(runs.get(), 1);
    let live = live_nodes_for_tests();
    s.set(1);
    assert_eq!(m.get(), 1);
    assert_eq!(runs.get(), 2, "one run per change");
    assert_eq!(live_nodes_for_tests(), live, "one child effect, not two");
}

#[test]
fn a_panicking_cleanup_still_disposes_the_rest() {
    let baseline = live_nodes_for_tests();
    let scope = Scope::new();
    let ran = Count::default();
    let (a, b) = scope.run(|| {
        let a = effect(|| on_cleanup(|| panic!("boom")));
        let r = ran.clone();
        let b = effect(move || {
            let r = r.clone();
            on_cleanup(move || r.bump());
        });
        let r = ran.clone();
        on_cleanup(move || r.bump());
        (a, b)
    });
    let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| scope.dispose()));
    assert!(panicked.is_err(), "the panic still reaches the caller");
    assert!(!a.is_alive() && !b.is_alive() && !scope.is_alive());
    assert_eq!(ran.get(), 2, "the other cleanups ran");
    assert_eq!(live_nodes_for_tests(), baseline);
}

#[test]
fn a_memo_whose_first_run_panicked_runs_again_when_read() {
    let s = signal(1);
    let m = memo(move |_| {
        let v = s.get();
        assert!(v != 1, "boom");
        v
    });
    let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| m.get()));
    assert!(panicked.is_err());
    assert!(m.is_alive());
    // Not "disposed": reading it runs it again.
    let again = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| m.get()));
    let payload = again.expect_err("it ran again");
    let message = payload
        .downcast_ref::<&str>()
        .map(|m| m.to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_default();
    assert!(message.contains("boom"), "{message}");
    s.set(2);
    assert_eq!(m.get(), 2);
}

#[test]
fn a_cleanup_for_a_disposed_owner_is_dropped_outside_the_graph() {
    struct WritesOnDrop(imaginarium_reactive::Signal<i32>);
    impl Drop for WritesOnDrop {
        fn drop(&mut self) {
            self.0.set(1);
        }
    }
    let s = signal(0);
    let scope = Scope::new();
    scope.run(|| {
        scope.dispose();
        let guard = WritesOnDrop(s);
        on_cleanup(move || drop(guard));
    });
    assert_eq!(s.get(), 1);
}
