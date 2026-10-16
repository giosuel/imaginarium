// SPDX-License-Identifier: AGPL-3.0-or-later

//! The public handles: signals, memos, effects, triggers and scopes.

use std::any::Any;
use std::cell::RefCell;
use std::marker::PhantomData;
use std::rc::Rc;

use crate::runtime::{Compute, Kind, NodeId, with_runtime};

/// Makes a handle `!Send` and `!Sync`: the graph belongs to one thread.
type ThreadBound = PhantomData<*const ()>;

/// The value cell behind a handle. The handle's type guarantees the cell's
/// type, and generations keep a stale handle from reaching a new node, so
/// a failed downcast can't happen.
fn cell<T: 'static>(value: &Rc<dyn Any>) -> &T {
    match value.downcast_ref::<T>() {
        Some(cell) => cell,
        None => unreachable!("a reactive handle points to a node of another type"),
    }
}

fn disposed(what: &str) -> ! {
    panic!("read of a disposed {what} (use the try_ methods where that can happen)")
}

// ---- signals ----------------------------------------------------------------

/// A value that computations depend on. Reading it inside a memo or effect
/// subscribes them; writing it updates them.
///
/// Handles are `Copy` and belong to the thread that created them. A signal
/// lives until its owner (the scope, memo or effect that was running when
/// it was created) is disposed, or until [`Signal::dispose`].
///
/// ```
/// use imaginarium_reactive::{effect, signal};
/// use std::{cell::Cell, rc::Rc};
///
/// let count = signal(1);
/// let seen = Rc::new(Cell::new(0));
/// let sink = seen.clone();
/// effect(move || sink.set(count.get() * 10));
/// assert_eq!(seen.get(), 10);
/// count.set(2);
/// assert_eq!(seen.get(), 20);
/// ```
pub struct Signal<T: 'static> {
    id: NodeId,
    _types: PhantomData<fn() -> T>,
    _thread: ThreadBound,
}

impl<T> Clone for Signal<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for Signal<T> {}

impl<T> std::fmt::Debug for Signal<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Signal({:?})", self.id)
    }
}

/// Creates a signal holding `value`, owned by the current owner.
pub fn signal<T: 'static>(value: T) -> Signal<T> {
    let value: Rc<dyn Any> = Rc::new(RefCell::new(value));
    let id = with_runtime(|rt| rt.create(Kind::Signal, Some(value), None));
    Signal {
        id,
        _types: PhantomData,
        _thread: PhantomData,
    }
}

impl<T: 'static> Signal<T> {
    /// The value (cloned). Subscribes the running computation.
    ///
    /// # Panics
    /// If the signal was disposed. See [`Signal::try_with`].
    pub fn get(&self) -> T
    where
        T: Clone,
    {
        self.with(T::clone)
    }

    /// The value without subscribing.
    pub fn get_untracked(&self) -> T
    where
        T: Clone,
    {
        self.with_untracked(T::clone)
    }

    /// Calls `f` with a reference to the value. Subscribes the running
    /// computation.
    ///
    /// # Panics
    /// If the signal was disposed, or if `f` writes this same signal, also
    /// indirectly (an effect that a write in `f` triggers writes it).
    pub fn with<R>(&self, f: impl FnOnce(&T) -> R) -> R {
        self.try_with(f).unwrap_or_else(|| disposed("signal"))
    }

    /// Like [`Signal::with`], without subscribing.
    pub fn with_untracked<R>(&self, f: impl FnOnce(&T) -> R) -> R {
        let value = with_runtime(|rt| rt.value(self.id)).unwrap_or_else(|| disposed("signal"));
        f(&cell::<RefCell<T>>(&value).borrow())
    }

    /// Like [`Signal::with`], but `None` if the signal was disposed.
    pub fn try_with<R>(&self, f: impl FnOnce(&T) -> R) -> Option<R> {
        let value = with_runtime(|rt| {
            rt.track(self.id);
            rt.value(self.id)
        })?;
        Some(f(&cell::<RefCell<T>>(&value).borrow()))
    }

    /// Replaces the value and updates everything that depends on it, even
    /// if the new value equals the old one (see [`Signal::set_if_changed`]).
    /// Writes to a disposed signal are ignored.
    pub fn set(&self, value: T) {
        self.update(|current| *current = value);
    }

    /// Replaces the value only if it differs. Returns whether it did.
    pub fn set_if_changed(&self, value: T) -> bool
    where
        T: PartialEq,
    {
        let Some(cell_value) = with_runtime(|rt| rt.value(self.id)) else {
            return false;
        };
        {
            let mut current = cell::<RefCell<T>>(&cell_value).borrow_mut();
            if *current == value {
                return false;
            }
            *current = value;
        }
        with_runtime(|rt| rt.notify(self.id));
        true
    }

    /// Changes the value in place and updates everything that depends on
    /// it. Ignored if the signal was disposed.
    ///
    /// # Panics
    /// If called while the value is borrowed by [`Signal::with`].
    pub fn update(&self, f: impl FnOnce(&mut T)) {
        let Some(value) = with_runtime(|rt| rt.value(self.id)) else {
            return;
        };
        f(&mut cell::<RefCell<T>>(&value).borrow_mut());
        with_runtime(|rt| rt.notify(self.id));
    }

    /// Whether the signal still exists.
    pub fn is_alive(&self) -> bool {
        with_runtime(|rt| rt.is_alive(self.id))
    }

    /// Disposes the signal now, instead of with its owner.
    pub fn dispose(self) {
        with_runtime(|rt| rt.dispose(self.id));
    }

    /// Entries in its subscriber list (live or not), for leak tests.
    #[doc(hidden)]
    pub fn subscriber_entries_for_tests(&self) -> usize {
        with_runtime(|rt| rt.subscriber_entries(self.id))
    }
}

/// A signal without a value: computations that [`track`](Trigger::track)
/// it re-run when it is [`notify`](Trigger::notify)'d. For "something
/// changed" events, such as a model `ChangeSet` touching a node.
#[derive(Clone, Copy, Debug)]
pub struct Trigger(Signal<()>);

/// Creates a trigger, owned by the current owner.
pub fn trigger() -> Trigger {
    Trigger(signal(()))
}

impl Trigger {
    /// Subscribes the running computation.
    pub fn track(&self) {
        let _ = self.0.try_with(|()| ());
    }

    /// Re-runs everything that tracks it.
    pub fn notify(&self) {
        self.0.set(());
    }

    /// Whether the trigger still exists.
    pub fn is_alive(&self) -> bool {
        self.0.is_alive()
    }

    /// Disposes it now, instead of with its owner.
    pub fn dispose(self) {
        self.0.dispose();
    }
}

// ---- memos ------------------------------------------------------------------

struct MemoCompute<T, F> {
    f: F,
    value: Rc<RefCell<Option<T>>>,
}

impl<T: PartialEq + 'static, F: FnMut(Option<&T>) -> T> Compute for MemoCompute<T, F> {
    fn run(&mut self) -> bool {
        // The old value stays in place (borrowed) while `f` runs, so a
        // panic in `f` keeps it. Nothing else reads it meanwhile: a read
        // of this memo during its own run is a cycle and panics first.
        let new = {
            let old = self.value.borrow();
            (self.f)(old.as_ref())
        };
        let mut value = self.value.borrow_mut();
        let changed = value.as_ref() != Some(&new);
        *value = Some(new);
        changed
    }

    fn has_value(&self) -> bool {
        self.value
            .try_borrow()
            .map_or(true, |value| value.is_some())
    }
}

/// A derived value, recomputed only when a value it read changed, and only
/// when something reads it (it is lazy). If the result equals the previous
/// one, nothing downstream re-runs.
///
/// ```
/// use imaginarium_reactive::{memo, signal};
///
/// let width = signal(3);
/// let height = signal(4);
/// let area = memo(move |_| width.get() * height.get());
/// assert_eq!(area.get(), 12);
/// width.set(5);
/// assert_eq!(area.get(), 20);
/// ```
pub struct Memo<T: 'static> {
    id: NodeId,
    _types: PhantomData<fn() -> T>,
    _thread: ThreadBound,
}

impl<T> Clone for Memo<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for Memo<T> {}

impl<T> std::fmt::Debug for Memo<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Memo({:?})", self.id)
    }
}

/// Creates a memo, owned by the current owner. `f` gets the previous value
/// (`None` the first time).
pub fn memo<T: PartialEq + 'static>(f: impl FnMut(Option<&T>) -> T + 'static) -> Memo<T> {
    let value = Rc::new(RefCell::new(None::<T>));
    let compute = MemoCompute {
        f,
        value: value.clone(),
    };
    let value: Rc<dyn Any> = value;
    let id = with_runtime(|rt| rt.create(Kind::Memo, Some(value), Some(Box::new(compute))));
    Memo {
        id,
        _types: PhantomData,
        _thread: PhantomData,
    }
}

impl<T: 'static> Memo<T> {
    /// The value (cloned), computed first if needed. Subscribes the running
    /// computation.
    ///
    /// # Panics
    /// If the memo was disposed, or depends on itself.
    pub fn get(&self) -> T
    where
        T: Clone,
    {
        self.with(T::clone)
    }

    /// The value without subscribing.
    pub fn get_untracked(&self) -> T
    where
        T: Clone,
    {
        with_runtime(|rt| rt.untrack(|| self.get()))
    }

    /// Calls `f` with a reference to the value. Subscribes the running
    /// computation.
    ///
    /// # Panics
    /// If the memo was disposed, or depends on itself, or if `f` writes a
    /// signal that makes this memo run again (the value is borrowed while
    /// `f` runs).
    pub fn with<R>(&self, f: impl FnOnce(&T) -> R) -> R {
        self.try_with(f).unwrap_or_else(|| disposed("memo"))
    }

    /// Like [`Memo::with`], but `None` if the memo was disposed.
    pub fn try_with<R>(&self, f: impl FnOnce(&T) -> R) -> Option<R> {
        let value = with_runtime(|rt| {
            rt.update(self.id);
            rt.track(self.id);
            rt.value(self.id)
        })?;
        let value = cell::<RefCell<Option<T>>>(&value).borrow();
        value.as_ref().map(f)
    }

    /// Whether the memo still exists.
    pub fn is_alive(&self) -> bool {
        with_runtime(|rt| rt.is_alive(self.id))
    }

    /// Disposes the memo now, instead of with its owner.
    pub fn dispose(self) {
        with_runtime(|rt| rt.dispose(self.id));
    }
}

// ---- effects ----------------------------------------------------------------

struct EffectCompute<F>(F);

impl<F: FnMut()> Compute for EffectCompute<F> {
    fn run(&mut self) -> bool {
        (self.0)();
        true
    }
}

/// A side effect that re-runs whenever a value it read changed. In the
/// toolkit, effects are where signals meet widgets: they mark widgets for
/// layout or paint.
///
/// It runs once right away (or when the current batch or computation
/// ends), then after every change, at most once per batch. It may write
/// signals, also ones it reads: it then runs again with the new values.
/// Effects it creates belong to it and are disposed before it runs again,
/// and they never run before it.
///
/// If it panics, the panic propagates out of the write that ran it, and it
/// runs again on its next change.
///
/// ```
/// use imaginarium_reactive::{effect, signal};
/// use std::{cell::RefCell, rc::Rc};
///
/// let name = signal(String::from("Ada"));
/// let shown = Rc::new(RefCell::new(String::new()));
/// let label = shown.clone();
/// effect(move || *label.borrow_mut() = format!("Hello, {}", name.get()));
/// name.set(String::from("Grace"));
/// assert_eq!(*shown.borrow(), "Hello, Grace");
/// ```
#[derive(Clone, Copy, Debug)]
pub struct Effect {
    id: NodeId,
    _thread: ThreadBound,
}

/// Creates an effect, owned by the current owner, and runs it.
pub fn effect(f: impl FnMut() + 'static) -> Effect {
    let id = with_runtime(|rt| {
        let id = rt.create(Kind::Effect, None, Some(Box::new(EffectCompute(f))));
        rt.schedule(id);
        id
    });
    Effect {
        id,
        _thread: PhantomData,
    }
}

impl Effect {
    /// Whether the effect still exists.
    pub fn is_alive(&self) -> bool {
        with_runtime(|rt| rt.is_alive(self.id))
    }

    /// Disposes the effect now (its cleanups run), instead of with its
    /// owner.
    pub fn dispose(self) {
        with_runtime(|rt| rt.dispose(self.id));
    }
}

// ---- scopes and the rest ----------------------------------------------------

/// An owner for signals, memos and effects, typically one per widget
/// subtree. Disposing it disposes everything created inside
/// [`Scope::run`], runs their cleanups, and frees them.
///
/// ```
/// use imaginarium_reactive::{Scope, effect, on_cleanup, signal};
/// use std::{cell::Cell, rc::Rc};
///
/// let cleaned = Rc::new(Cell::new(false));
/// let scope = Scope::new();
/// let flag = cleaned.clone();
/// let label = scope.run(|| {
///     on_cleanup(move || flag.set(true));
///     signal("hello")
/// });
/// assert!(label.is_alive());
/// scope.dispose();
/// assert!(!label.is_alive() && cleaned.get());
/// ```
#[derive(Clone, Copy, Debug)]
pub struct Scope {
    id: NodeId,
    _thread: ThreadBound,
}

impl Scope {
    /// A new scope, itself owned by the current owner (if any).
    #[allow(clippy::new_without_default)] // creating one has an effect
    pub fn new() -> Self {
        let id = with_runtime(|rt| rt.create(Kind::Scope, None, None));
        Self {
            id,
            _thread: PhantomData,
        }
    }

    /// Runs `f` with this scope as the owner of what it creates.
    pub fn run<R>(&self, f: impl FnOnce() -> R) -> R {
        with_runtime(|rt| {
            if rt.is_alive(self.id) {
                rt.with_owner(self.id, f)
            } else {
                f()
            }
        })
    }

    /// Whether the scope still exists.
    pub fn is_alive(&self) -> bool {
        with_runtime(|rt| rt.is_alive(self.id))
    }

    /// Disposes the scope and everything it owns.
    pub fn dispose(self) {
        with_runtime(|rt| rt.dispose(self.id));
    }
}

/// Runs `f` and the effects its writes trigger only afterwards, each at most
/// once. Batches nest; the outermost one runs the effects.
///
/// ```
/// use imaginarium_reactive::{batch, effect, signal};
/// use std::{cell::Cell, rc::Rc};
///
/// let (a, b) = (signal(1), signal(2));
/// let runs = Rc::new(Cell::new(0));
/// let count = runs.clone();
/// effect(move || {
///     let _ = a.get() + b.get();
///     count.set(count.get() + 1);
/// });
/// batch(|| {
///     a.set(10);
///     b.set(20);
/// });
/// assert_eq!(runs.get(), 2, "once on creation, once for the batch");
/// ```
pub fn batch<R>(f: impl FnOnce() -> R) -> R {
    with_runtime(|rt| rt.batch(f))
}

/// Runs `f` without subscribing the running computation to what it reads.
pub fn untrack<R>(f: impl FnOnce() -> R) -> R {
    with_runtime(|rt| rt.untrack(f))
}

/// Runs `f` outside any owner and computation: what it creates belongs to
/// nobody and lives until it is disposed explicitly (or the thread ends),
/// even when `f` runs inside an effect. For values that outlive the code
/// that first needs them, such as the toolkit's theme.
///
/// ```
/// use imaginarium_reactive::{Signal, detached, effect, signal};
///
/// let input = signal(0);
/// let made = std::rc::Rc::new(std::cell::Cell::new(None::<Signal<u8>>));
/// let effect = effect({
///     let made = made.clone();
///     move || {
///         input.get();
///         if made.get().is_none() {
///             made.set(Some(detached(|| signal(7))));
///         }
///     }
/// });
/// input.set(1); // the effect runs again; an owned signal would be gone
/// assert_eq!(made.get().map(|s| s.get()), Some(7));
/// effect.dispose();
/// ```
pub fn detached<R>(f: impl FnOnce() -> R) -> R {
    with_runtime(|rt| rt.detached(f))
}

/// Runs `cleanup` when the current owner (scope, memo or effect) is
/// disposed or, for memos and effects, before they run again. Outside any
/// owner it never runs.
pub fn on_cleanup(cleanup: impl FnOnce() + 'static) {
    with_runtime(|rt| rt.on_cleanup(Box::new(cleanup)));
}

/// Live reactive nodes on this thread, for leak tests.
#[doc(hidden)]
pub fn live_nodes_for_tests() -> usize {
    with_runtime(|rt| rt.live_nodes())
}
