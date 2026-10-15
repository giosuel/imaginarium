// SPDX-License-Identifier: AGPL-3.0-or-later

//! Reactive graph benchmarks: the cost of propagating changes.
//!
//! No PLAN §10 budget applies directly. It sits inside "input to pixels
//! < 1 frame (8 ms)": a typical model `ChangeSet` writes a handful of
//! signals per frame, which must cost microseconds, not milliseconds.
//! The graphs are fixed shapes, so no fixture data is needed.

use std::cell::Cell;
use std::hint::black_box;
use std::rc::Rc;

use criterion::{Criterion, criterion_group, criterion_main};
use imaginarium_reactive::{Scope, batch, effect, memo, signal};

fn writes(c: &mut Criterion) {
    let mut group = c.benchmark_group("reactive/write");

    // One signal, one effect: the smallest update.
    let scope = Scope::new();
    let (value, sink) = scope.run(|| {
        let value = signal(0u64);
        let sink = Rc::new(Cell::new(0));
        let s = sink.clone();
        effect(move || s.set(value.get()));
        (value, sink)
    });
    let mut n = 0;
    group.bench_function("signal_to_effect", |b| {
        b.iter(|| {
            n += 1;
            value.set(n);
        });
    });
    black_box(sink.get());
    scope.dispose();

    // One signal read by 10,000 effects (a theme or zoom change).
    let scope = Scope::new();
    let value = scope.run(|| {
        let value = signal(0u64);
        for _ in 0..10_000 {
            effect(move || {
                black_box(value.get());
            });
        }
        value
    });
    group.bench_function("fan_out_10k_effects", |b| {
        b.iter(|| {
            n += 1;
            value.set(n);
        });
    });
    scope.dispose();

    // 10,000 signals, each through a memo into an effect, all written in one
    // batch (a whole board's nodes changing).
    let scope = Scope::new();
    let signals = scope.run(|| {
        (0..10_000)
            .map(|_| {
                let value = signal(0u64);
                let doubled = memo(move |_| value.get() * 2);
                effect(move || {
                    black_box(doubled.get());
                });
                value
            })
            .collect::<Vec<_>>()
    });
    group.bench_function("batch_10k_signals_memo_effect", |b| {
        b.iter(|| {
            n += 1;
            batch(|| {
                for value in &signals {
                    value.set(n);
                }
            });
        });
    });
    scope.dispose();

    // 10,000 effects that read a hot signal only while a flag is set (items
    // that read the zoom only while visible): one toggle drops or adds
    // 10,000 edges of one signal.
    let scope = Scope::new();
    let flag = scope.run(|| {
        let flag = signal(true);
        let hot = signal(0u64);
        for _ in 0..10_000 {
            effect(move || {
                if flag.get() {
                    black_box(hot.get());
                }
            });
        }
        flag
    });
    group.bench_function("toggle_10k_dynamic_reads", |b| {
        b.iter(|| flag.update(|on| *on = !*on));
    });
    scope.dispose();

    // A chain of 1,000 memos, written at the head and read at the tail.
    let scope = Scope::new();
    let (head, tail) = scope.run(|| {
        let head = signal(0u64);
        let mut tail = memo(move |_| head.get());
        for _ in 0..1_000 {
            let previous = tail;
            tail = memo(move |_| previous.get() + 1);
        }
        (head, tail)
    });
    group.bench_function("chain_1000_memos", |b| {
        b.iter(|| {
            n += 1;
            head.set(n);
            black_box(tail.get());
        });
    });
    scope.dispose();
    group.finish();
}

fn scopes(c: &mut Criterion) {
    let mut group = c.benchmark_group("reactive/scope");
    // Mounting and unmounting a view of 1,000 rows, each a signal and an
    // effect reading it and a shared one.
    let shared = signal(0u64);
    group.bench_function("create_and_dispose_1000_rows", |b| {
        b.iter(|| {
            let scope = Scope::new();
            scope.run(|| {
                for row in 0..1_000u64 {
                    let value = signal(row);
                    effect(move || {
                        black_box(value.get() + shared.get());
                    });
                }
            });
            scope.dispose();
        });
    });
    group.finish();
}

criterion_group!(benches, writes, scopes);
criterion_main!(benches);
