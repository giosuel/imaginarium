// SPDX-License-Identifier: AGPL-3.0-or-later

//! Fine-grained reactivity for the UI toolkit (ADR-0007): signals, memos
//! and effects in the style of SolidJS and Leptos.
//!
//! A changed [`Signal`] updates only the computations that read it, so a
//! widget re-lays out or repaints only when data it shows changed. There is
//! no virtual DOM and no diffing. Model `ChangeSet`s feed signals; effects
//! mark widgets dirty.
//!
//! # Pieces
//! - [`Signal`]: a value. Reading it inside a memo or effect subscribes
//!   them; [`Signal::set`] and [`Signal::update`] update them.
//!   [`Trigger`] is a signal without a value.
//! - [`Memo`]: a derived value, lazy, and recomputed only when something
//!   it read changed. An unchanged result (by `PartialEq`) stops the update
//!   there.
//! - [`Effect`]: a side effect that re-runs when something it read changed.
//! - [`Scope`]: an owner. Everything created inside [`Scope::run`] (and,
//!   automatically, inside a memo or effect) is disposed with it, after
//!   running its [`on_cleanup`] callbacks.
//! - [`batch`]: groups writes, so effects run once at the end. [`untrack`]:
//!   reads without subscribing.
//!
//! # Guarantees
//! - **Glitch-free:** no memo or effect ever sees a mix of old and new
//!   values. Before one runs, everything it depends on is brought up to
//!   date.
//! - **Minimal:** after a write (or a batch), each affected computation
//!   runs at most once, and only if a value it read really changed (memos
//!   compare with `PartialEq`; signals count every write).
//! - **Dynamic dependencies:** a computation depends on what it read in its
//!   latest run, not on everything it ever read.
//! - **No leaks:** disposing an owner frees everything below it. A memo or
//!   effect disposes what it created before it runs again.
//!
//! Writes are allowed anywhere. A computation that writes something it
//! read runs again with the new value. Effects that a write triggers wait
//! until no batch is open and no computation runs, so they never see a
//! half-finished computation. (Memos are meant to be pure; a memo that
//! writes works, but its effects run after it.)
//!
//! A computation whose closure panics keeps its previous value and runs
//! again on its next change (a memo without a previous value: on its next
//! read). Effects that were still pending, and the one whose update
//! panicked, wait for the next write. A panicking cleanup doesn't stop the
//! others: the whole subtree is still disposed, then the panic goes on.
//!
//! Programming errors panic: reading a disposed handle (the `try_` methods
//! return `None` instead), a memo that depends on itself, memos that keep
//! writing each other's sources, or writing a signal inside its own
//! [`Signal::with`]. An effect that keeps triggering itself is stopped
//! after a million runs in one flush: a panic in debug builds; in release
//! builds the flush stops, and its pending effects go on at the next
//! write.
//!
//! Limits: memos are brought up to date recursively, so a chain of memos
//! (each reading the previous one) deeper than a few thousand can overflow
//! a small thread stack. Generations are 32-bit; a slot whose generation
//! would wrap is retired.
//!
//! # Boundaries
//! - **One graph per thread.** Handles are `Copy` but not `Send`: the graph
//!   lives on the UI thread. Results from workers arrive through channels
//!   and are written to signals there (PLAN §4.3).
//! - **No scheduling policy.** Effects run synchronously when a write (or
//!   the outermost [`batch`]) ends. The UI's frame scheduler wraps each
//!   event in a batch.
//! - No UI types, no async runtime, no dependencies outside `std`.

mod handles;
mod runtime;

pub use handles::{
    Effect, Memo, Scope, Signal, Trigger, batch, effect, live_nodes_for_tests, memo, on_cleanup,
    signal, trigger, untrack,
};
