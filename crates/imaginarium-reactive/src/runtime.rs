// SPDX-License-Identifier: AGPL-3.0-or-later

//! The reactive graph: nodes in a generational arena, one per thread.
//!
//! The update algorithm is push-pull "coloring" (as in Reactively and
//! Solid 2): a write marks its direct subscribers `Dirty` and everything
//! further downstream `Check`. Reading a memo or running an effect first
//! brings its sources up to date and recomputes only if a source really
//! changed. That is glitch-free (no computation sees a mix of old and new
//! values) and runs each computation at most once per update.
//!
//! Rules that keep it correct when user code does unusual things:
//! - A computation is marked `Clean` *before* it runs, and subscribes to
//!   each source as it reads it. A write during the run marks it again,
//!   so it runs once more instead of keeping a stale result.
//! - Effects that writes trigger wait until no computation is running and
//!   no batch is open.
//! - An effect's owners run before it, so a child effect never runs on
//!   state its parent is about to replace.
//! - Disposal never edits other nodes' lists. Stale entries are skipped by
//!   their generation and compacted later.
//! - No user code runs while the arena is borrowed, and a panicking user
//!   closure leaves the node as it was before the run.
//! - Every effect that isn't `Clean` is pending (or running): writes stop
//!   marking at nodes that are marked already, so an effect that missed
//!   its turn would never be queued again. An effect whose run unwinds
//!   goes back into the queue.

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::rc::Rc;

/// More effect runs than this in one flush means an effect keeps
/// triggering itself (directly or through others).
const MAX_EFFECT_RUNS_PER_FLUSH: usize = 1_000_000;

/// More passes than this over one computation's sources means memos keep
/// writing each other's sources.
const MAX_CHECK_PASSES: usize = 10_000;

/// A caught panic, re-raised once the work it interrupted is finished.
type Panic = Box<dyn Any + Send>;

/// Subscriber and child lists are compacted (disposed entries dropped)
/// when they reach a power of two at least this long, so lists of
/// long-lived nodes stay proportional to their live entries.
const COMPACT_FROM: usize = 64;

/// A node handle: arena slot plus generation, so a handle to a disposed
/// node never reaches the slot's next tenant.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) struct NodeId {
    index: u32,
    generation: u32,
}

/// How up to date a computation is. Ordered: a higher state wins.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum State {
    /// Up to date.
    Clean,
    /// A source further upstream changed; a direct source may have.
    Check,
    /// A direct source changed: recompute.
    Dirty,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Kind {
    Signal,
    Memo,
    Effect,
    Scope,
}

/// A memo's or effect's function, type-erased.
pub(crate) trait Compute {
    /// Runs it. For memos: whether the value changed.
    fn run(&mut self) -> bool;

    /// Whether it has a value: false for a memo whose first run hasn't
    /// finished.
    fn has_value(&self) -> bool {
        true
    }
}

struct Node {
    generation: u32,
    alive: bool,
    kind: Kind,
    state: State,
    /// Signals: `RefCell<T>`. Memos: `RefCell<Option<T>>`.
    value: Option<Rc<dyn Any>>,
    /// Memos and effects. `None` while it runs.
    compute: Option<Box<dyn Compute>>,
    running: bool,
    /// What this computation read in its latest run. May contain disposed
    /// nodes, which are skipped.
    sources: Vec<NodeId>,
    /// While it runs: how many of the previous run's sources it has read
    /// again, in the same order. Those edges are kept as they are.
    cursor: usize,
    /// While it runs: sources read after the first difference, and whether
    /// reading them subscribed it (it didn't if it already was).
    new_sources: Vec<(NodeId, bool)>,
    /// Computations that read this node. May contain disposed ones.
    subscribers: Vec<NodeId>,
    owner: Option<NodeId>,
    /// May contain disposed ones.
    children: Vec<NodeId>,
    cleanups: Vec<Box<dyn FnOnce()>>,
    /// The run that last tracked this node, to skip repeated reads.
    tracked_by_run: u64,
    /// Scratch mark for `finish_tracking`.
    mark: u64,
}

impl Node {
    fn vacant(generation: u32) -> Self {
        Self {
            generation,
            alive: false,
            kind: Kind::Scope,
            state: State::Clean,
            value: None,
            compute: None,
            running: false,
            sources: Vec::new(),
            cursor: 0,
            new_sources: Vec::new(),
            subscribers: Vec::new(),
            owner: None,
            children: Vec::new(),
            cleanups: Vec::new(),
            tracked_by_run: 0,
            mark: 0,
        }
    }

    fn is(&self, id: NodeId) -> bool {
        self.alive && self.generation == id.generation
    }
}

/// Things taken out of disposed nodes, dropped once the arena is no longer
/// borrowed (their `Drop` may be user code).
#[derive(Default)]
struct Garbage {
    values: Vec<Rc<dyn Any>>,
    computes: Vec<Box<dyn Compute>>,
}

pub(crate) struct Runtime {
    nodes: RefCell<Vec<Node>>,
    free: RefCell<Vec<u32>>,
    /// The computation that is running, and its run id.
    observer: Cell<Option<(NodeId, u64)>>,
    /// Where new nodes are attached.
    owner: Cell<Option<NodeId>>,
    next_run: Cell<u64>,
    /// Open batches plus running computations: effects wait while it's > 0.
    batch_depth: Cell<u32>,
    pending: RefCell<VecDeque<NodeId>>,
    flushing: Cell<bool>,
    /// The work list of `notify`, kept to reuse its allocation.
    marking: RefCell<Vec<(NodeId, State)>>,
    /// Counts writes, so a check over a computation's sources notices
    /// writes made while it ran.
    writes: Cell<u64>,
}

thread_local! {
    static RUNTIME: Runtime = Runtime::new();
}

/// The current thread's runtime.
pub(crate) fn with_runtime<R>(f: impl FnOnce(&Runtime) -> R) -> R {
    RUNTIME.with(f)
}

/// Restores a `Cell` when dropped, so a panicking user closure leaves the
/// runtime consistent.
struct Restore<'a, T: Copy> {
    cell: &'a Cell<T>,
    value: T,
}

impl<T: Copy> Drop for Restore<'_, T> {
    fn drop(&mut self) {
        self.cell.set(self.value);
    }
}

fn replace<T: Copy>(cell: &Cell<T>, value: T) -> Restore<'_, T> {
    Restore {
        value: cell.replace(value),
        cell,
    }
}

/// Puts a computation back into its node after it ran, also when it
/// panicked.
struct Running<'a> {
    rt: &'a Runtime,
    id: NodeId,
    run: u64,
    compute: Option<Box<dyn Compute>>,
}

impl Drop for Running<'_> {
    fn drop(&mut self) {
        let compute = self.compute.take();
        if self.rt.is_alive(self.id) {
            self.rt.with_node_mut(self.id, |node| {
                node.running = false;
                if std::thread::panicking() && compute.as_ref().is_some_and(|c| !c.has_value()) {
                    // A memo's first run panicked: the next read runs it
                    // again (there is no previous value to keep).
                    node.state = State::Dirty;
                }
                node.compute = compute;
            });
            self.rt.finish_tracking(self.id, self.run);
        } else {
            drop(compute); // it disposed itself; drop it unborrowed
        }
    }
}

/// Puts an effect back into the queue if its update unwinds, so it isn't
/// left marked but not pending.
struct Requeue<'a> {
    rt: &'a Runtime,
    effect: NodeId,
    done: bool,
}

impl Drop for Requeue<'_> {
    fn drop(&mut self) {
        if !self.done
            && self
                .rt
                .state(self.effect)
                .is_some_and(|state| state != State::Clean)
        {
            self.rt.pending.borrow_mut().push_front(self.effect);
        }
    }
}

impl Runtime {
    fn new() -> Self {
        Self {
            nodes: RefCell::new(Vec::new()),
            free: RefCell::new(Vec::new()),
            observer: Cell::new(None),
            owner: Cell::new(None),
            next_run: Cell::new(1),
            batch_depth: Cell::new(0),
            pending: RefCell::new(VecDeque::new()),
            flushing: Cell::new(false),
            marking: RefCell::new(Vec::new()),
            writes: Cell::new(0),
        }
    }

    // ---- nodes ------------------------------------------------------------

    /// Creates a node owned by the current owner.
    ///
    /// # Panics
    /// With more than `u32::MAX` nodes alive.
    pub(crate) fn create(
        &self,
        kind: Kind,
        value: Option<Rc<dyn Any>>,
        compute: Option<Box<dyn Compute>>,
    ) -> NodeId {
        let owner = self.owner.get().filter(|&owner| self.is_alive(owner));
        let state = match kind {
            Kind::Memo | Kind::Effect => State::Dirty,
            Kind::Signal | Kind::Scope => State::Clean,
        };
        let mut nodes = self.nodes.borrow_mut();
        let index = match self.free.borrow_mut().pop() {
            Some(index) => index,
            None => {
                let Ok(index) = u32::try_from(nodes.len()) else {
                    panic!("more than u32::MAX reactive nodes");
                };
                nodes.push(Node::vacant(0));
                index
            }
        };
        let slot = &mut nodes[index as usize];
        let id = NodeId {
            index,
            generation: slot.generation,
        };
        *slot = Node {
            alive: true,
            kind,
            state,
            value,
            compute,
            owner,
            ..Node::vacant(slot.generation)
        };
        if let Some(owner) = owner {
            push_compacting(&mut nodes, owner, id, |node| &mut node.children);
        }
        id
    }

    pub(crate) fn is_alive(&self, id: NodeId) -> bool {
        self.nodes
            .borrow()
            .get(id.index as usize)
            .is_some_and(|node| node.is(id))
    }

    /// The node's value cell, if the node is alive.
    pub(crate) fn value(&self, id: NodeId) -> Option<Rc<dyn Any>> {
        self.with_node(id, |node| node.value.clone()).flatten()
    }

    /// Entries in a node's subscriber list, live or not, for leak tests.
    pub(crate) fn subscriber_entries(&self, id: NodeId) -> usize {
        self.with_node(id, |node| node.subscribers.len())
            .unwrap_or(0)
    }

    /// Live nodes, for tests of disposal.
    pub(crate) fn live_nodes(&self) -> usize {
        self.nodes.borrow().iter().filter(|node| node.alive).count()
    }

    // ---- reading ----------------------------------------------------------

    /// Records that the running computation read `id`, and subscribes it.
    pub(crate) fn track(&self, id: NodeId) {
        let Some((observer, run)) = self.observer.get() else {
            return;
        };
        if observer == id {
            return;
        }
        let mut nodes = self.nodes.borrow_mut();
        let alive =
            |nodes: &[Node], id: NodeId| nodes.get(id.index as usize).is_some_and(|n| n.is(id));
        if !alive(&nodes, observer) || !alive(&nodes, id) {
            return;
        }
        let source = &mut nodes[id.index as usize];
        if source.tracked_by_run == run {
            return; // read before in this run
        }
        source.tracked_by_run = run;
        let reader = &mut nodes[observer.index as usize];
        if reader.new_sources.is_empty() && reader.sources.get(reader.cursor) == Some(&id) {
            reader.cursor += 1; // the same as last time: keep the edge
            return;
        }
        // Subscribe now, not after the run, so a write later in this same
        // run reaches it. A source of the previous run is subscribed
        // already.
        let subscribed_already = reader
            .sources
            .get(reader.cursor..)
            .is_some_and(|rest| rest.contains(&id));
        reader.new_sources.push((id, !subscribed_already));
        if !subscribed_already {
            push_compacting(&mut nodes, id, observer, |node| &mut node.subscribers);
        }
    }

    /// After a run: the sources are the unchanged prefix plus what was read
    /// after it; unsubscribe from the rest. A run that reads what it read
    /// last time doesn't get here.
    fn finish_tracking(&self, id: NodeId, run: u64) {
        let mut nodes = self.nodes.borrow_mut();
        let Some(node) = live_mut(&mut nodes, id) else {
            return;
        };
        let cursor = node.cursor.min(node.sources.len());
        if cursor == node.sources.len() && node.new_sources.is_empty() {
            return;
        }
        let mut sources = std::mem::take(&mut node.sources);
        let added = std::mem::take(&mut node.new_sources);
        let dropped = sources.split_off(cursor);
        // Marks instead of `contains`: linear in the number of sources.
        for &source in &sources {
            if let Some(source_node) = live_mut(&mut nodes, source) {
                source_node.mark = run;
            }
        }
        for (source, subscribed) in added {
            match live_mut(&mut nodes, source) {
                Some(source_node) if source_node.mark != run => {
                    source_node.mark = run;
                    sources.push(source);
                }
                Some(source_node) if subscribed => {
                    // A repeated read that subscribed a second time.
                    if let Some(at) = source_node.subscribers.iter().rposition(|&s| s == id) {
                        source_node.subscribers.swap_remove(at);
                    }
                }
                _ => {}
            }
        }
        for source in dropped {
            if let Some(source_node) = live_mut(&mut nodes, source)
                && source_node.mark != run
            {
                source_node.subscribers.retain(|&s| s != id);
            }
        }
        if let Some(node) = live_mut(&mut nodes, id) {
            node.sources = sources;
        }
    }

    /// Brings a memo or effect up to date: recomputes it if a source really
    /// changed.
    ///
    /// # Panics
    /// On a cycle: a memo read while it computes, by itself or by a memo it
    /// reads.
    pub(crate) fn update(&self, id: NodeId) {
        let Some((state, running)) = self.with_node(id, |node| (node.state, node.running)) else {
            return;
        };
        if running {
            panic!("reactive cycle: a memo depends on itself");
        }
        if state == State::Check {
            let mut passes = 0;
            loop {
                let writes = self.writes.get();
                // By index, not over a copy: this runs for every affected
                // node. Nothing but this node's own runs changes its source
                // list.
                let mut i = 0;
                while let Some(source) = self
                    .with_node(id, |node| node.sources.get(i).copied())
                    .flatten()
                {
                    i += 1;
                    if self.kind(source) == Some(Kind::Memo) {
                        self.update(source);
                    }
                    if self.state(id) != Some(State::Check) {
                        break;
                    }
                }
                if self.state(id) != Some(State::Check) {
                    break;
                }
                if self.writes.get() == writes {
                    self.set_state(id, State::Clean); // nothing it read changed
                    return;
                }
                // A source's run wrote something, maybe what a source
                // checked before reads. That one is marked now, but this
                // node isn't again (it is marked already): check again.
                passes += 1;
                if passes > MAX_CHECK_PASSES {
                    panic!("reactive cycle: memos keep writing each other's sources");
                }
            }
        }
        if self.state(id) == Some(State::Dirty) {
            self.recompute(id);
        }
    }

    fn recompute(&self, id: NodeId) {
        let changed = {
            // Effects that this run's writes trigger wait for it to end, and
            // so do those of its cleanups: an effect that read this node
            // would run it again from inside this update.
            let _depth = replace(&self.batch_depth, self.batch_depth.get() + 1);
            self.reset(id);
            let run = self.next_run.get();
            self.next_run.set(run + 1);
            let compute = self
                .with_node_mut(id, |node| {
                    // Clean before the run: a write during it marks it again.
                    node.state = State::Clean;
                    node.running = true;
                    node.cursor = 0;
                    node.new_sources.clear();
                    node.compute.take()
                })
                .flatten();
            compute.is_some_and(|compute| {
                let mut running = Running {
                    rt: self,
                    id,
                    run,
                    compute: Some(compute),
                };
                let _observer = replace(&self.observer, Some((id, run)));
                let _owner = replace(&self.owner, Some(id));
                running.compute.as_mut().is_some_and(|c| c.run())
            })
        };
        if changed && self.kind(id) == Some(Kind::Memo) {
            // Subscribers that are checking whether to run (marked `Check`
            // when the change started) now must. A `Clean` one is the
            // computation reading this memo right now: it sees the new
            // value, and marking it would leave it `Dirty` without a run.
            let mut nodes = self.nodes.borrow_mut();
            let subscribers = take_live_subscribers(&mut nodes, id);
            for &subscriber in &subscribers {
                if let Some(node) = live_mut(&mut nodes, subscriber)
                    && node.state == State::Check
                {
                    node.state = State::Dirty;
                }
            }
            put_subscribers(&mut nodes, id, subscribers);
        }
        self.flush_unless_batching();
    }

    /// Before a computation runs again: dispose what it created last time
    /// and run its cleanups. (Its sources are updated as it runs.) If a
    /// cleanup panics, the rest still run, and then the panic goes on.
    fn reset(&self, id: NodeId) {
        let (children, cleanups) = self
            .with_node_mut(id, |node| {
                (
                    std::mem::take(&mut node.children),
                    !node.cleanups.is_empty(),
                )
            })
            .unwrap_or_default();
        let mut panic = None;
        for child in children {
            let caught = self.dispose_catching(child);
            panic = panic.or(caught);
        }
        if cleanups {
            let caught = self.run_cleanups(id);
            panic = panic.or(caught);
        }
        if let Some(panic) = panic {
            resume_unwind(panic);
        }
    }

    /// Runs all of a node's cleanups and returns the first panic.
    fn run_cleanups(&self, id: NodeId) -> Option<Panic> {
        let cleanups = self
            .with_node_mut(id, |node| std::mem::take(&mut node.cleanups))
            .unwrap_or_default();
        let _observer = replace(&self.observer, None);
        let mut panic = None;
        for cleanup in cleanups {
            if let Err(caught) = catch_unwind(AssertUnwindSafe(cleanup)) {
                panic = panic.or(Some(caught));
            }
        }
        panic
    }

    // ---- writing ----------------------------------------------------------

    /// After a signal's value changed: mark everything downstream and run
    /// the effects (now, or once no batch is open and nothing computes).
    pub(crate) fn notify(&self, id: NodeId) {
        self.writes.set(self.writes.get().wrapping_add(1));
        {
            let mut nodes = self.nodes.borrow_mut();
            let mut pending = self.pending.borrow_mut();
            // Reused between writes: a write allocates nothing in the
            // common case.
            let mut stack = self.marking.borrow_mut();
            stack.clear();
            let subscribers = take_live_subscribers(&mut nodes, id);
            stack.extend(subscribers.iter().map(|&s| (s, State::Dirty)));
            put_subscribers(&mut nodes, id, subscribers);
            while let Some((next, state)) = stack.pop() {
                let Some(node) = live_mut(&mut nodes, next) else {
                    continue;
                };
                if node.state >= state {
                    continue;
                }
                let was_clean = node.state == State::Clean;
                node.state = state;
                if !was_clean {
                    continue; // marked before, and everything downstream with it
                }
                if node.kind == Kind::Effect {
                    pending.push_back(next);
                }
                stack.extend(node.subscribers.iter().map(|&s| (s, State::Check)));
            }
        }
        self.flush_unless_batching();
    }

    pub(crate) fn schedule(&self, effect: NodeId) {
        self.pending.borrow_mut().push_back(effect);
        self.flush_unless_batching();
    }

    fn flush_unless_batching(&self) {
        if self.batch_depth.get() == 0 {
            self.flush();
        }
    }

    /// Runs pending effects until none are left. If one panics, it and the
    /// rest wait for the next write.
    fn flush(&self) {
        if self.flushing.get() {
            return; // the running flush picks them up
        }
        let _flushing = replace(&self.flushing, true);
        let mut runs = 0;
        loop {
            let next = self.pending.borrow_mut().pop_front();
            let Some(effect) = next else {
                break;
            };
            runs += 1;
            if runs > MAX_EFFECT_RUNS_PER_FLUSH {
                // The rest stay pending: dropping them would leave them
                // marked but never queued again.
                self.pending.borrow_mut().push_front(effect);
                debug_assert!(false, "an effect keeps triggering itself");
                break;
            }
            let mut requeue = Requeue {
                rt: self,
                effect,
                done: false,
            };
            self.run_top(effect);
            requeue.done = true;
        }
    }

    /// Updates an effect, after its owners that are out of date: an owner's
    /// run disposes its old children, which then never run on state it is
    /// about to replace.
    fn run_top(&self, id: NodeId) {
        // Out-of-date owners, innermost first. Usually there are none, so
        // the list is only allocated when needed.
        let mut stale: Vec<NodeId> = Vec::new();
        let mut current = self.with_node(id, |node| node.owner).flatten();
        while let Some(owner) = current {
            let Some((kind, state, next)) =
                self.with_node(owner, |node| (node.kind, node.state, node.owner))
            else {
                break;
            };
            if matches!(kind, Kind::Effect | Kind::Memo) && state != State::Clean {
                stale.push(owner);
            }
            current = next;
        }
        for &owner in stale.iter().rev() {
            if self.state(owner).is_some_and(|state| state != State::Clean) {
                self.update(owner);
            }
        }
        if self.state(id).is_some_and(|state| state != State::Clean) {
            self.update(id);
        }
    }

    pub(crate) fn batch<R>(&self, f: impl FnOnce() -> R) -> R {
        let result = {
            let _depth = replace(&self.batch_depth, self.batch_depth.get() + 1);
            f()
        };
        self.flush_unless_batching();
        result
    }

    pub(crate) fn untrack<R>(&self, f: impl FnOnce() -> R) -> R {
        let _observer = replace(&self.observer, None);
        f()
    }

    pub(crate) fn with_owner<R>(&self, owner: NodeId, f: impl FnOnce() -> R) -> R {
        let _owner = replace(&self.owner, Some(owner));
        f()
    }

    /// Registers `cleanup` with the current owner. Without a live owner
    /// nothing is ever disposed, so it never runs and is dropped.
    pub(crate) fn on_cleanup(&self, cleanup: Box<dyn FnOnce()>) {
        let mut cleanup = Some(cleanup);
        if let Some(owner) = self.owner.get() {
            self.with_node_mut(owner, |node| node.cleanups.extend(cleanup.take()));
        }
        drop(cleanup); // a dead owner's: its `Drop` may be user code
    }

    // ---- disposal ---------------------------------------------------------

    /// Disposes a node and everything it owns: cleanups run, the slot is
    /// freed. If a cleanup panics, the whole subtree is still disposed, and
    /// then the panic goes on.
    pub(crate) fn dispose(&self, id: NodeId) {
        if let Some(panic) = self.dispose_catching(id) {
            resume_unwind(panic);
        }
    }

    /// Disposes a node and returns the first panic of its cleanups.
    fn dispose_catching(&self, id: NodeId) -> Option<Panic> {
        if !self.is_alive(id) {
            return None;
        }
        // Effects that cleanups trigger run once the whole subtree is gone.
        self.batch(|| {
            let mut garbage = Garbage::default();
            let mut panic = None;
            self.dispose_into(id, &mut garbage, &mut panic);
            drop(garbage);
            panic
        })
    }

    fn dispose_into(&self, id: NodeId, garbage: &mut Garbage, panic: &mut Option<Panic>) {
        let children = self
            .with_node_mut(id, |node| std::mem::take(&mut node.children))
            .unwrap_or_default();
        for child in children {
            if self.is_alive(child) {
                self.dispose_into(child, garbage, panic);
            }
        }
        if let Some(caught) = self.run_cleanups(id) {
            panic.get_or_insert(caught);
        }
        let mut nodes = self.nodes.borrow_mut();
        let Some(node) = live_mut(&mut nodes, id) else {
            return;
        };
        // Other nodes' lists (sources, subscribers, children) still name
        // it. They skip it by its generation and drop it when compacted:
        // editing them here would shift lists that a running update walks,
        // and make disposing many readers of one signal quadratic.
        garbage.values.extend(node.value.take());
        garbage.computes.extend(node.compute.take());
        let generation = node.generation;
        *node = Node::vacant(generation.wrapping_add(1));
        drop(nodes);
        if generation < u32::MAX - 1 {
            self.free.borrow_mut().push(id.index);
        }
        // Otherwise the slot is retired: its generation would wrap, and a
        // stale handle could reach the next tenant.
    }

    // ---- helpers ----------------------------------------------------------

    fn with_node<R>(&self, id: NodeId, f: impl FnOnce(&Node) -> R) -> Option<R> {
        let nodes = self.nodes.borrow();
        let node = nodes.get(id.index as usize)?;
        node.is(id).then(|| f(node))
    }

    fn with_node_mut<R>(&self, id: NodeId, f: impl FnOnce(&mut Node) -> R) -> Option<R> {
        let mut nodes = self.nodes.borrow_mut();
        live_mut(&mut nodes, id).map(f)
    }

    fn state(&self, id: NodeId) -> Option<State> {
        self.with_node(id, |node| node.state)
    }

    fn set_state(&self, id: NodeId, state: State) {
        self.with_node_mut(id, |node| node.state = state);
    }

    fn kind(&self, id: NodeId) -> Option<Kind> {
        self.with_node(id, |node| node.kind)
    }
}

/// The node, if `id` is alive.
fn live_mut(nodes: &mut [Node], id: NodeId) -> Option<&mut Node> {
    nodes.get_mut(id.index as usize).filter(|node| node.is(id))
}

/// Pushes `entry` onto one of `owner`'s lists, dropping disposed entries
/// when the list reaches a power of two: amortized O(1), and the list stays
/// proportional to its live entries.
fn push_compacting(
    nodes: &mut [Node],
    owner: NodeId,
    entry: NodeId,
    list: impl Fn(&mut Node) -> &mut Vec<NodeId>,
) {
    let Some(node) = live_mut(nodes, owner) else {
        return;
    };
    let mut entries = std::mem::take(list(node));
    entries.push(entry);
    if entries.len() >= COMPACT_FROM && entries.len().is_power_of_two() {
        entries.retain(|&e| nodes.get(e.index as usize).is_some_and(|n| n.is(e)));
        if entries.capacity() > 4 * entries.len().max(COMPACT_FROM) {
            entries.shrink_to(2 * entries.len());
        }
    }
    if let Some(node) = live_mut(nodes, owner) {
        *list(node) = entries;
    }
}

/// Takes `id`'s subscriber list out of its node, without the disposed
/// ones (filtered in place, no allocation). Put it back with
/// [`put_subscribers`].
fn take_live_subscribers(nodes: &mut [Node], id: NodeId) -> Vec<NodeId> {
    let Some(node) = live_mut(nodes, id) else {
        return Vec::new();
    };
    let mut subscribers = std::mem::take(&mut node.subscribers);
    subscribers.retain(|&s| nodes.get(s.index as usize).is_some_and(|n| n.is(s)));
    subscribers
}

fn put_subscribers(nodes: &mut [Node], id: NodeId, subscribers: Vec<NodeId>) {
    if let Some(node) = live_mut(nodes, id) {
        node.subscribers = subscribers;
    }
}
