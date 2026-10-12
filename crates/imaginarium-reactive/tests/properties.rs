// SPDX-License-Identifier: AGPL-3.0-or-later

//! Property tests of the guarantees in the crate docs, against a direct
//! evaluation of random graphs: values are always right, no computation
//! sees a glitch, each runs at most once per batch, and exactly when an
//! input really changed.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::cell::RefCell;
use std::rc::Rc;

use imaginarium_reactive::{
    Memo, Scope, Signal, batch, effect, live_nodes_for_tests, memo, signal,
};
use proptest::prelude::*;

/// A node a memo or effect can read: signal `i` or memo `i`.
#[derive(Clone, Copy, Debug)]
enum Input {
    Signal(usize),
    Memo(usize),
}

/// How a memo computes its value.
#[derive(Clone, Debug)]
enum MemoSpec {
    /// The sum of its inputs, modulo `modulus` (small moduli make
    /// unchanged results, which must stop the update).
    Sum { inputs: Vec<Input>, modulus: u32 },
    /// Reads `then` or `otherwise` depending on whether `condition` is
    /// even: dependencies that change from run to run.
    Branch {
        condition: Input,
        then: Vec<Input>,
        otherwise: Vec<Input>,
    },
}

#[derive(Clone, Debug)]
struct Graph {
    signals: usize,
    memos: Vec<MemoSpec>,
    effects: Vec<Vec<Input>>,
}

/// Inputs a node may read: signals, and memos created before it.
fn input(signals: usize, memos: usize) -> BoxedStrategy<Input> {
    let signal = (0..signals).prop_map(Input::Signal);
    if memos == 0 {
        signal.boxed()
    } else {
        prop_oneof![signal, (0..memos).prop_map(Input::Memo)].boxed()
    }
}

fn inputs(signals: usize, memos: usize) -> BoxedStrategy<Vec<Input>> {
    prop::collection::vec(input(signals, memos), 1..4).boxed()
}

fn graph() -> impl Strategy<Value = Graph> {
    (1usize..5, 0usize..8, 1usize..5).prop_flat_map(|(signals, memo_count, effect_count)| {
        let memos: Vec<BoxedStrategy<MemoSpec>> = (0..memo_count)
            .map(|i| {
                prop_oneof![
                    (inputs(signals, i), 2u32..6)
                        .prop_map(|(inputs, modulus)| MemoSpec::Sum { inputs, modulus }),
                    (input(signals, i), inputs(signals, i), inputs(signals, i)).prop_map(
                        |(condition, then, otherwise)| MemoSpec::Branch {
                            condition,
                            then,
                            otherwise
                        }
                    ),
                ]
                .boxed()
            })
            .collect();
        let effects = prop::collection::vec(inputs(signals, memo_count), effect_count);
        (Just(signals), memos, effects).prop_map(|(signals, memos, effects)| Graph {
            signals,
            memos,
            effects,
        })
    })
}

/// Batches of writes: (signal, value). Small values make equal writes and
/// equal results likely.
fn batches(signals: usize) -> impl Strategy<Value = Vec<Vec<(usize, u32)>>> {
    prop::collection::vec(prop::collection::vec((0..signals, 0u32..4), 1..4), 1..12)
}

/// The direct evaluation.
fn evaluate(graph: &Graph, signals: &[u32]) -> Vec<u32> {
    let mut memos: Vec<u32> = Vec::new();
    let value = |memos: &[u32], input: Input| match input {
        Input::Signal(i) => signals[i],
        Input::Memo(i) => memos[i],
    };
    for spec in &graph.memos {
        let result = match spec {
            MemoSpec::Sum { inputs, modulus } => {
                inputs.iter().map(|&i| value(&memos, i)).sum::<u32>() % modulus
            }
            MemoSpec::Branch {
                condition,
                then,
                otherwise,
            } => {
                let chosen = if value(&memos, *condition).is_multiple_of(2) {
                    then
                } else {
                    otherwise
                };
                chosen.iter().map(|&i| value(&memos, i)).sum()
            }
        };
        memos.push(result);
    }
    memos
}

/// The reactive version of a graph.
struct Live {
    signals: Vec<Signal<u32>>,
    memos: Vec<Memo<u32>>,
    /// Per effect: the values it saw, one entry per run.
    runs: Vec<Rc<RefCell<Vec<Vec<u32>>>>>,
}

fn read(signals: &[Signal<u32>], memos: &[Memo<u32>], input: Input) -> u32 {
    match input {
        Input::Signal(i) => signals[i].get(),
        Input::Memo(i) => memos[i].get(),
    }
}

fn build(graph: &Graph) -> Live {
    let signals: Vec<Signal<u32>> = (0..graph.signals).map(|_| signal(0)).collect();
    let mut memos: Vec<Memo<u32>> = Vec::new();
    for spec in &graph.memos {
        let (signals, earlier, spec) = (signals.clone(), memos.clone(), spec.clone());
        memos.push(memo(move |_| match &spec {
            MemoSpec::Sum { inputs, modulus } => {
                inputs
                    .iter()
                    .map(|&i| read(&signals, &earlier, i))
                    .sum::<u32>()
                    % modulus
            }
            MemoSpec::Branch {
                condition,
                then,
                otherwise,
            } => {
                let chosen = if read(&signals, &earlier, *condition).is_multiple_of(2) {
                    then
                } else {
                    otherwise
                };
                chosen.iter().map(|&i| read(&signals, &earlier, i)).sum()
            }
        }));
    }
    let runs = graph
        .effects
        .iter()
        .map(|reads| {
            let log = Rc::new(RefCell::new(Vec::new()));
            let (signals, memos, reads, sink) =
                (signals.clone(), memos.clone(), reads.clone(), log.clone());
            effect(move || {
                let seen = reads.iter().map(|&i| read(&signals, &memos, i)).collect();
                sink.borrow_mut().push(seen);
            });
            log
        })
        .collect();
    Live {
        signals,
        memos,
        runs,
    }
}

fn values(reads: &[Input], signals: &[u32], memos: &[u32]) -> Vec<u32> {
    reads
        .iter()
        .map(|&input| match input {
            Input::Signal(i) => signals[i],
            Input::Memo(i) => memos[i],
        })
        .collect()
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn graphs_stay_correct_glitch_free_and_minimal(
        (graph, writes) in graph().prop_flat_map(|g| {
            let signals = g.signals;
            (Just(g), batches(signals))
        })
    ) {
        let scope = Scope::new();
        let live = scope.run(|| build(&graph));
        let mut current = vec![0u32; graph.signals];
        let mut memos = evaluate(&graph, &current);
        for (effect, log) in graph.effects.iter().zip(&live.runs) {
            prop_assert_eq!(log.borrow().clone(), vec![values(effect, &current, &memos)]);
        }

        for batch_writes in writes {
            let before = live.runs.iter().map(|log| log.borrow().len()).collect::<Vec<_>>();
            let previous_memos = memos.clone();
            let mut written = vec![false; graph.signals];
            batch(|| {
                for &(i, value) in &batch_writes {
                    live.signals[i].set(value);
                    current[i] = value;
                    written[i] = true;
                }
            });
            memos = evaluate(&graph, &current);

            for (i, expected) in memos.iter().enumerate() {
                prop_assert_eq!(live.memos[i].get_untracked(), *expected, "memo {}", i);
            }
            for (e, reads) in graph.effects.iter().enumerate() {
                let log = live.runs[e].borrow();
                let new_runs = log.len() - before[e];
                prop_assert!(new_runs <= 1, "effect {} ran {} times in one batch", e, new_runs);
                let touched = reads.iter().any(|&input| match input {
                    Input::Signal(i) => written[i],
                    Input::Memo(i) => memos[i] != previous_memos[i],
                });
                prop_assert_eq!(new_runs == 1, touched, "effect {} ran: {}, expected: {}", e, new_runs == 1, touched);
                if new_runs == 1 {
                    // It saw the final values, never a mix (no glitch).
                    prop_assert_eq!(log.last().cloned().unwrap(), values(reads, &current, &memos));
                }
            }
        }
        scope.dispose();
    }

    #[test]
    fn disposing_a_scope_frees_everything_in_it(
        (graph, writes) in graph().prop_flat_map(|g| {
            let signals = g.signals;
            (Just(g), batches(signals))
        })
    ) {
        let baseline = live_nodes_for_tests();
        let scope = Scope::new();
        let live = scope.run(|| build(&graph));
        for batch_writes in writes {
            batch(|| {
                for &(i, value) in &batch_writes {
                    live.signals[i].set(value);
                }
            });
        }
        scope.dispose();
        prop_assert_eq!(live_nodes_for_tests(), baseline);
    }
}
