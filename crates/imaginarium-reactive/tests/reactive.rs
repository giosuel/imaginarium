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
