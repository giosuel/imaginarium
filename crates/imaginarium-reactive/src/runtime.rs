// SPDX-License-Identifier: AGPL-3.0-or-later

//! The reactive graph: nodes in a generational arena, one per thread.
//!
//! The update algorithm is push-pull "coloring" (as in Reactively and
//! Solid 2): a write marks its direct subscribers `Dirty` and everything
//! further downstream `Check`. Reading a memo or running an effect first
//! brings its sources up to date, newest information first, and recomputes
//! only if a source really changed. That is glitch-free (no computation
//! sees a mix of old and new values) and runs each computation at most once
//! per update.
//!
//! No user code runs while the arena is borrowed: computations are taken
//! out of their node, run, and put back. User code can therefore read and
//! write other signals freely.

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;

/// More effect runs than this in one flush means an effect keeps
/// triggering itself (directly or through others).
const MAX_EFFECT_RUNS_PER_FLUSH: usize = 1_000_000;

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
    /// What this computation read in its latest run.
    sources: Vec<NodeId>,
    /// While it runs: how many of the previous run's sources it has read
    /// again, in the same order. Those edges are kept as they are.
    cursor: usize,
    /// While it runs: sources read after the first difference.
    new_sources: Vec<NodeId>,
    /// Computations that read this node. May contain disposed ones, which
    /// are dropped the next time the list is walked.
    subscribers: Vec<NodeId>,
    owner: Option<NodeId>,
    children: Vec<NodeId>,
    cleanups: Vec<Box<dyn FnOnce()>>,
    /// The run that last tracked this node, to skip duplicate edges.
    tracked_by_run: u64,
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
        }
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
    batch_depth: Cell<u32>,
    pending: RefCell<VecDeque<NodeId>>,
    flushing: Cell<bool>,
    /// The work list of `notify`, kept to reuse its allocation.
    marking: RefCell<Vec<(NodeId, State)>>,
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
        }
    }

    // ---- nodes ------------------------------------------------------------

    /// Creates a node owned by the current owner.
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
                nodes.push(Node::vacant(0));
                u32::try_from(nodes.len() - 1).unwrap_or(u32::MAX)
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
            nodes[owner.index as usize].children.push(id);
        }
        id
    }

    pub(crate) fn is_alive(&self, id: NodeId) -> bool {
        self.nodes
            .borrow()
            .get(id.index as usize)
            .is_some_and(|node| node.alive && node.generation == id.generation)
    }

    /// The node's value cell, if the node is alive.
    pub(crate) fn value(&self, id: NodeId) -> Option<Rc<dyn Any>> {
        let nodes = self.nodes.borrow();
        let node = nodes.get(id.index as usize)?;
        (node.alive && node.generation == id.generation)
            .then(|| node.value.clone())
            .flatten()
    }

    /// Live nodes, for tests of disposal.
    pub(crate) fn live_nodes(&self) -> usize {
        self.nodes.borrow().iter().filter(|node| node.alive).count()
    }

    // ---- reading ----------------------------------------------------------

    /// Records that the running computation read `id`.
    pub(crate) fn track(&self, id: NodeId) {
        let Some((observer, run)) = self.observer.get() else {
            return;
        };
        if observer == id || !self.is_alive(observer) || !self.is_alive(id) {
            return;
        }
        let mut nodes = self.nodes.borrow_mut();
        let source = &mut nodes[id.index as usize];
        if source.tracked_by_run == run {
            return; // read before in this run
        }
        source.tracked_by_run = run;
        let reader = &mut nodes[observer.index as usize];
        if reader.new_sources.is_empty() && reader.sources.get(reader.cursor) == Some(&id) {
            reader.cursor += 1; // the same as last time: keep the edge
        } else {
            reader.new_sources.push(id);
        }
    }

    /// After a run: keep the unchanged prefix of the sources, subscribe to
    /// the new ones, unsubscribe from the ones no longer read. A run that
    /// reads what it read last time touches no subscriber list.
    fn finish_tracking(&self, id: NodeId) {
        let mut nodes = self.nodes.borrow_mut();
        let Some(node) = nodes.get_mut(id.index as usize) else {
            return;
        };
        if !node.alive || node.generation != id.generation {
            return;
        }
        let cursor = node.cursor;
        if cursor == node.sources.len() && node.new_sources.is_empty() {
            return; // read exactly what it read last time
        }
        let dropped = node.sources.split_off(cursor);
        let added = std::mem::take(&mut node.new_sources);
        let mut subscribe = Vec::new();
        for source in added {
            if !node.sources.contains(&source) {
                node.sources.push(source);
                if !dropped.contains(&source) {
                    subscribe.push(source); // `dropped` ones are still subscribed
                }
            }
        }
        let unsubscribe: Vec<NodeId> = dropped
            .into_iter()
            .filter(|source| !node.sources.contains(source))
            .collect();
        for source in subscribe {
            if let Some(source_node) = live_mut(&mut nodes, source) {
                source_node.subscribers.push(id);
            }
        }
        for source in unsubscribe {
            if let Some(source_node) = live_mut(&mut nodes, source) {
                source_node.subscribers.retain(|&s| s != id);
            }
        }
    }

    /// Brings a memo or effect up to date: recomputes it if a source really
    /// changed. Panics on a cycle (a memo that reads itself).
    pub(crate) fn update(&self, id: NodeId) {
        let Some(state) = self.state(id) else {
            return;
        };
        if state == State::Check {
            // By index, not over a copy: this runs for every affected node.
            let mut i = 0;
            while let Some(source) = self
                .with_node(id, |node| node.sources.get(i).copied())
                .flatten()
            {
                i += 1;
                if self.kind(source) == Some(Kind::Memo) {
                    self.update(source);
                }
                if self.state(id) == Some(State::Dirty) {
                    break;
                }
            }
        }
        if self.state(id) == Some(State::Dirty) {
            self.recompute(id);
        }
        self.set_state(id, State::Clean);
    }

    fn recompute(&self, id: NodeId) {
        if self.with_node(id, |node| node.running).unwrap_or(false) {
            panic!("reactive cycle: a memo or effect depends on itself");
        }
        self.reset(id);
        let Some(mut compute) = self
            .with_node_mut(id, |node| {
                node.running = true;
                node.cursor = 0;
                node.new_sources.clear();
                node.compute.take()
            })
            .flatten()
        else {
            return;
        };
        let run = self.next_run.get();
        self.next_run.set(run + 1);
        let changed = {
            let _observer = replace(&self.observer, Some((id, run)));
            let _owner = replace(&self.owner, Some(id));
            compute.run()
        };
        let still_here = self
            .with_node_mut(id, |node| {
                node.running = false;
                node.compute = Some(compute);
            })
            .is_some();
        if !still_here {
            return; // it disposed itself
        }
        self.finish_tracking(id);
        if changed && self.kind(id) == Some(Kind::Memo) {
            // Its subscribers were marked `Check` when the change started.
            let mut nodes = self.nodes.borrow_mut();
            let subscribers = take_live_subscribers(&mut nodes, id);
            for &subscriber in &subscribers {
                if let Some(node) = live_mut(&mut nodes, subscriber) {
                    node.state = State::Dirty;
                }
            }
            put_subscribers(&mut nodes, id, subscribers);
        }
    }

    /// Before a computation runs again: dispose what it created last time
    /// and run its cleanups. (Its sources are updated as it runs.)
    fn reset(&self, id: NodeId) {
        let children = self
            .with_node_mut(id, |node| std::mem::take(&mut node.children))
            .unwrap_or_default();
        for child in children {
            self.dispose(child);
        }
        self.run_cleanups(id);
    }

    fn run_cleanups(&self, id: NodeId) {
        let cleanups = self
            .with_node_mut(id, |node| std::mem::take(&mut node.cleanups))
            .unwrap_or_default();
        let _observer = replace(&self.observer, None);
        for cleanup in cleanups {
            cleanup();
        }
    }

    // ---- writing ----------------------------------------------------------

    /// After a signal's value changed: mark everything downstream and run
    /// the effects (now, or at the end of the outermost batch).
    pub(crate) fn notify(&self, id: NodeId) {
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
                let Some(node) = nodes.get_mut(next.index as usize) else {
                    continue;
                };
                if !node.alive || node.generation != next.generation || node.state >= state {
                    continue;
                }
                let was_clean = node.state == State::Clean;
                node.state = state;
                if !was_clean {
                    continue; // already marked downstream
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

    /// Runs pending effects until none are left.
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
                self.pending.borrow_mut().clear();
                debug_assert!(false, "an effect keeps triggering itself");
                break;
            }
            if self
                .state(effect)
                .is_some_and(|state| state != State::Clean)
            {
                self.update(effect);
            }
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
        if let Some(owner) = self.owner.get() {
            self.with_node_mut(owner, |node| node.cleanups.push(cleanup));
        }
    }

    // ---- disposal ---------------------------------------------------------

    /// Disposes a node and everything it owns: cleanups run, edges go, the
    /// slot is freed.
    pub(crate) fn dispose(&self, id: NodeId) {
        if !self.is_alive(id) {
            return;
        }
        // Effects that cleanups trigger run once the whole subtree is gone.
        self.batch(|| {
            let mut garbage = Garbage::default();
            self.dispose_into(id, &mut garbage);
            drop(garbage);
        });
    }

    fn dispose_into(&self, id: NodeId, garbage: &mut Garbage) {
        let children = self
            .with_node_mut(id, |node| std::mem::take(&mut node.children))
            .unwrap_or_default();
        for child in children {
            if self.is_alive(child) {
                self.dispose_into(child, garbage);
            }
        }
        self.run_cleanups(id);
        let mut nodes = self.nodes.borrow_mut();
        let Some(node) = nodes.get_mut(id.index as usize) else {
            return;
        };
        if !node.alive || node.generation != id.generation {
            return;
        }
        let sources = std::mem::take(&mut node.sources);
        let subscribers = std::mem::take(&mut node.subscribers);
        let owner = node.owner.take();
        garbage.values.extend(node.value.take());
        garbage.computes.extend(node.compute.take());
        node.alive = false;
        node.generation = node.generation.wrapping_add(1);
        node.state = State::Clean;
        // Its sources' subscriber lists still name it: they drop it the
        // next time they are walked. Removing it from each now would make
        // disposing many readers of one signal quadratic.
        drop(sources);
        for subscriber in subscribers {
            if let Some(subscriber_node) = nodes.get_mut(subscriber.index as usize)
                && subscriber_node.generation == subscriber.generation
            {
                subscriber_node.sources.retain(|&s| s != id);
            }
        }
        if let Some(owner) = owner
            && let Some(owner_node) = nodes.get_mut(owner.index as usize)
            && owner_node.generation == owner.generation
        {
            owner_node.children.retain(|&c| c != id);
        }
        drop(nodes);
        self.free.borrow_mut().push(id.index);
    }

    // ---- helpers ----------------------------------------------------------

    fn with_node<R>(&self, id: NodeId, f: impl FnOnce(&Node) -> R) -> Option<R> {
        let nodes = self.nodes.borrow();
        let node = nodes.get(id.index as usize)?;
        (node.alive && node.generation == id.generation).then(|| f(node))
    }

    fn with_node_mut<R>(&self, id: NodeId, f: impl FnOnce(&mut Node) -> R) -> Option<R> {
        let mut nodes = self.nodes.borrow_mut();
        let node = nodes.get_mut(id.index as usize)?;
        (node.alive && node.generation == id.generation).then(|| f(node))
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
    nodes
        .get_mut(id.index as usize)
        .filter(|node| node.alive && node.generation == id.generation)
}

/// Takes `id`'s subscriber list out of its node, without the disposed
/// ones (filtered in place, no allocation). Put it back with
/// [`put_subscribers`].
fn take_live_subscribers(nodes: &mut [Node], id: NodeId) -> Vec<NodeId> {
    let Some(node) = live_mut(nodes, id) else {
        return Vec::new();
    };
    let mut subscribers = std::mem::take(&mut node.subscribers);
    subscribers.retain(|&s| {
        nodes
            .get(s.index as usize)
            .is_some_and(|n| n.alive && n.generation == s.generation)
    });
    subscribers
}

fn put_subscribers(nodes: &mut [Node], id: NodeId, subscribers: Vec<NodeId>) {
    if let Some(node) = live_mut(nodes, id) {
        node.subscribers = subscribers;
    }
}
