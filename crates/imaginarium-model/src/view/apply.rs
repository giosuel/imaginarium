// SPDX-License-Identifier: AGPL-3.0-or-later

//! Incremental view updates from the CRDT's events.
//!
//! Loro reports every change as diffs of containers, each with its path
//! from a root container (`nodes` tree → node meta map → `g`, `t`, …). The
//! updater:
//! - applies tree diffs (create, move, delete) to the child lists, mapping
//!   CRDT tree ids to node identities;
//! - applies text deltas to the node's [`TextView`](super::TextView);
//! - re-reads the other parts it marked dirty (a few map keys each) once
//!   the batch is through.
//!
//! Two Loro behaviours shape it:
//! - When concurrent creations land at the same position, Loro reports a
//!   node's reordering as a delete followed by a create of the same tree
//!   id. A node that exists before and after a batch is therefore never
//!   "created": it's reported as moved, with its content re-read.
//! - Undoing a deletion re-creates the subtree under new tree ids. The
//!   stored `id` keys come back with it, so the nodes keep their identity.
//!
//! Anything unexpected (an unknown parent, a duplicate identity) falls back
//! to rebuilding the whole view and diffing it, which is always correct.
//! The property tests check that the result equals a view built from
//! scratch.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;

use loro::event::{Diff, DiffEvent};
use loro::{
    ContainerID, EventTriggerKind, Index, LoroDoc, LoroTree, TextDelta, TreeDiffItem,
    TreeExternalDiff, TreeID, TreeParentId,
};

use super::build::{
    Parts, build, map_at, read_meta, read_node, read_node_parts, read_props, read_style, stored_id,
    tree_key,
};
use super::{DocumentView, NodeView};
use crate::changes::{ChangeOrigin, ChangeSet, NodeChanges};
use crate::ids::{Actor, BlobId, NodeId, Parent, StyleId, UndoScope};
use crate::schema::{node, root};

/// An event, copied out of the subscription callback.
pub(crate) struct RawEvent {
    origin: String,
    import: bool,
    diffs: Vec<RawDiff>,
}

struct RawDiff {
    target: ContainerID,
    path: Vec<(ContainerID, Index)>,
    kind: RawKind,
}

enum RawKind {
    Tree(Vec<TreeDiffItem>),
    /// The keys that changed (values are re-read from the state).
    Map(Vec<String>),
    Text(Vec<TextDelta>),
    Other,
}

impl RawEvent {
    pub(crate) fn from_event(event: &DiffEvent<'_>) -> Self {
        Self {
            origin: event.origin.to_owned(),
            import: event.triggered_by == EventTriggerKind::Import,
            diffs: event
                .events
                .iter()
                .map(|diff| RawDiff {
                    target: diff.target.clone(),
                    path: diff.path.to_vec(),
                    kind: match &diff.diff {
                        Diff::Tree(tree) => RawKind::Tree(tree.diff.clone()),
                        Diff::Map(map) => {
                            RawKind::Map(map.updated.keys().map(|k| k.to_string()).collect())
                        }
                        Diff::Text(delta) => RawKind::Text(delta.clone()),
                        _ => RawKind::Other,
                    },
                })
                .collect(),
        }
    }

    pub(crate) fn origin(&self) -> ChangeOrigin {
        if self.import {
            return ChangeOrigin::Remote;
        }
        match self.origin.as_str() {
            "undo:user" => ChangeOrigin::Undo(UndoScope::User),
            "undo:ai" => ChangeOrigin::Undo(UndoScope::Assistant),
            origin => Actor::from_message(origin).map_or(ChangeOrigin::Remote, ChangeOrigin::Local),
        }
    }
}

/// The incremental path can't handle this batch; rebuild instead.
struct Rebuild;

/// What the batch touched, collected before re-reading.
#[derive(Default)]
struct Dirty {
    /// Nodes that didn't exist before the batch.
    created: BTreeSet<NodeId>,
    /// Nodes that existed before the batch and were removed at some point.
    removed: BTreeSet<NodeId>,
    /// Removed nodes that came back (reordering, undo).
    recreated: BTreeSet<NodeId>,
    moved: BTreeSet<NodeId>,
    parts: std::collections::BTreeMap<NodeId, Parts>,
    /// Nodes whose text was updated in place from deltas.
    text_touched: BTreeSet<NodeId>,
    /// Parents whose child list must be re-read (an index didn't match).
    resync: HashSet<Parent>,
    meta: bool,
    styles: BTreeSet<StyleId>,
    assets: BTreeSet<BlobId>,
    pages: bool,
    ext: bool,
}

/// Applies a batch of events to `view` and reports what changed.
pub(crate) fn apply_events(
    view: &mut DocumentView,
    doc: &LoroDoc,
    events: Vec<RawEvent>,
) -> ChangeSet {
    let origin = events.first().map(RawEvent::origin).unwrap_or_default();
    let before = view.clone();
    let tree = doc.get_tree(root::NODES);
    let mut dirty = Dirty::default();
    let incremental = (|| {
        for event in &events {
            for diff in &event.diffs {
                apply_diff(view, &tree, &mut dirty, diff)?;
            }
        }
        finish(view, doc, &tree, dirty, origin.clone())
    })();
    match incremental {
        Ok(changes) => changes,
        Err(Rebuild) => {
            *view = build(doc);
            view.version = before.version + 1;
            diff_views(&before, view, origin)
        }
    }
}

fn root_name(id: &ContainerID) -> Option<&str> {
    match id {
        ContainerID::Root { name, .. } => Some(name.as_str()),
        ContainerID::Normal { .. } => None,
    }
}

fn apply_diff(
    view: &mut DocumentView,
    tree: &LoroTree,
    dirty: &mut Dirty,
    diff: &RawDiff,
) -> Result<(), Rebuild> {
    let Some((root_id, _)) = diff.path.first() else {
        return Ok(());
    };
    match root_name(root_id) {
        Some(root::NODES) => return apply_node_diff(view, tree, dirty, diff),
        Some(root::META) => dirty.meta = true,
        Some(root::PAGES) => dirty.pages = true,
        Some(root::EXT) => dirty.ext = true,
        Some(root::STYLES) => {
            for key in changed_entries(diff) {
                if let Ok(id) = StyleId::new(key) {
                    dirty.styles.insert(id);
                }
            }
        }
        Some(root::ASSETS) => {
            for key in changed_entries(diff) {
                if let Some(id) = BlobId::parse(&key) {
                    dirty.assets.insert(id);
                }
            }
        }
        // Other root containers are preserved by the CRDT but not viewed.
        _ => {}
    }
    Ok(())
}

/// The entries of a root map that a diff touches: the changed keys of the
/// root map itself, or the entry a nested diff lies under.
fn changed_entries(diff: &RawDiff) -> Vec<String> {
    match (diff.path.len(), &diff.kind) {
        (1, RawKind::Map(keys)) => keys.clone(),
        (len, _) if len >= 2 => match &diff.path[1].1 {
            Index::Key(key) => vec![key.to_string()],
            _ => Vec::new(),
        },
        _ => Vec::new(),
    }
}

fn apply_node_diff(
    view: &mut DocumentView,
    tree: &LoroTree,
    dirty: &mut Dirty,
    diff: &RawDiff,
) -> Result<(), Rebuild> {
    if diff.path.len() == 1 {
        if let RawKind::Tree(items) = &diff.kind {
            for item in items {
                apply_tree_item(view, tree, dirty, item)?;
            }
        }
        return Ok(());
    }
    let Index::Node(tid) = &diff.path[1].1 else {
        return Ok(());
    };
    let Some(&id) = view.owners.get(tid) else {
        return Ok(());
    };
    if dirty.created.contains(&id) || dirty.recreated.contains(&id) {
        return Ok(()); // read completely later
    }
    let parts = if diff.path.len() == 2 {
        // The node's meta map: these keys changed.
        match &diff.kind {
            RawKind::Map(keys) => {
                if keys.iter().any(|k| k == node::ID) {
                    return Err(Rebuild); // an identity changed
                }
                keys.iter()
                    .fold(Parts::default(), |p, k| p.union(Parts::of_key(k)))
            }
            _ => Parts::default(),
        }
    } else {
        let Index::Key(key) = &diff.path[2].1 else {
            return Ok(());
        };
        if key.as_str() == node::TEXT
            && diff.path.len() == 3
            && let RawKind::Text(delta) = &diff.kind
            && apply_text(view, dirty, id, &diff.target, delta)
        {
            return Ok(());
        }
        Parts::of_key(key)
    };
    let entry = dirty.parts.entry(id).or_default();
    *entry = entry.union(parts);
    Ok(())
}

/// Applies a text delta in place. `false` if the text must be re-read
/// instead (it was replaced, or the view has no text for it yet).
fn apply_text(
    view: &mut DocumentView,
    dirty: &mut Dirty,
    id: NodeId,
    target: &ContainerID,
    delta: &[TextDelta],
) -> bool {
    if dirty.parts.get(&id).is_some_and(|p| p.text) {
        return true; // re-read anyway; this delta is already in the state
    }
    let Some(node) = view.nodes.get_mut(&id) else {
        return false;
    };
    if node.text_container.as_ref() != Some(target) || node.text.is_none() {
        return false;
    }
    if let Some(text) = Arc::make_mut(node).text.as_mut() {
        text.apply_delta(delta);
    }
    dirty.text_touched.insert(id);
    true
}

/// The view parent of a tree parent. `Ok(None)` for "not in the document"
/// (deleted); `Err` for a parent the view doesn't know.
fn view_parent(view: &DocumentView, parent: TreeParentId) -> Result<Option<Parent>, Rebuild> {
    match parent {
        TreeParentId::Root => Ok(Some(Parent::Root)),
        TreeParentId::Node(tid) => view
            .owners
            .get(&tid)
            .map(|id| Some(Parent::Node(*id)))
            .ok_or(Rebuild),
        TreeParentId::Deleted | TreeParentId::Unexist => Ok(None),
    }
}

/// Gives a tree node that's new to the view its identity.
fn assign_identity(
    view: &mut DocumentView,
    tree: &LoroTree,
    tid: TreeID,
) -> Result<NodeId, Rebuild> {
    let id = match stored_id(tree, tid) {
        Some(stored) => match view.handles.get(&NodeId::Stored(stored)) {
            None => NodeId::Stored(stored),
            Some(&holder) if tree_key(tid) < tree_key(holder) => return Err(Rebuild), // it takes over
            Some(_) => {
                view.shadowed.insert(tid, stored);
                NodeId::Tree(tid)
            }
        },
        None => NodeId::Tree(tid),
    };
    view.handles.insert(id, tid);
    view.owners.insert(tid, id);
    Ok(id)
}

/// Inserts a node that's new to the view (placeholder, read later).
fn add_node(view: &mut DocumentView, dirty: &mut Dirty, id: NodeId, parent: Parent) {
    view.nodes
        .insert(id, Arc::new(NodeView::placeholder(id, parent)));
    if dirty.removed.contains(&id) {
        dirty.recreated.insert(id);
    } else {
        dirty.created.insert(id);
    }
}

fn apply_tree_item(
    view: &mut DocumentView,
    tree: &LoroTree,
    dirty: &mut Dirty,
    item: &TreeDiffItem,
) -> Result<(), Rebuild> {
    let tid = item.target;
    match &item.action {
        TreeExternalDiff::Create { parent, index, .. } => {
            if view.owners.contains_key(&tid) {
                return Err(Rebuild); // created twice without a delete
            }
            let Some(parent) = view_parent(view, *parent)? else {
                return Ok(());
            };
            let id = assign_identity(view, tree, tid)?;
            insert_child(view, dirty, parent, *index, id);
            add_node(view, dirty, id, parent);
        }
        TreeExternalDiff::Move {
            parent,
            index,
            old_parent,
            old_index,
            ..
        } => {
            let new_parent = view_parent(view, *parent)?;
            let Some(&id) = view.owners.get(&tid) else {
                // Moved out of a deleted subtree: new to the view.
                if let Some(parent) = new_parent {
                    let id = assign_identity(view, tree, tid)?;
                    insert_child(view, dirty, parent, *index, id);
                    add_node(view, dirty, id, parent);
                }
                return Ok(());
            };
            if let Some(old) = view_parent(view, *old_parent)? {
                remove_child(view, dirty, old, *old_index, id);
            }
            match new_parent {
                Some(parent) => {
                    insert_child(view, dirty, parent, *index, id);
                    if let Some(node) = view.nodes.get_mut(&id) {
                        Arc::make_mut(node).parent = parent;
                    }
                    if !dirty.created.contains(&id) {
                        dirty.moved.insert(id);
                    }
                }
                None => remove_subtree(view, dirty, id)?,
            }
        }
        TreeExternalDiff::Delete {
            old_parent,
            old_index,
        } => {
            let Some(&id) = view.owners.get(&tid) else {
                return Ok(());
            };
            if let Some(old) = view_parent(view, *old_parent)? {
                remove_child(view, dirty, old, *old_index, id);
            }
            remove_subtree(view, dirty, id)?;
        }
    }
    Ok(())
}

fn insert_child(
    view: &mut DocumentView,
    dirty: &mut Dirty,
    parent: Parent,
    index: usize,
    id: NodeId,
) {
    let list = view.children.entry(parent).or_default();
    if index <= list.len() {
        list.insert(index, id);
    } else {
        list.push_back(id);
        dirty.resync.insert(parent);
    }
}

fn remove_child(
    view: &mut DocumentView,
    dirty: &mut Dirty,
    parent: Parent,
    index: usize,
    id: NodeId,
) {
    let Some(list) = view.children.get_mut(&parent) else {
        dirty.resync.insert(parent);
        return;
    };
    if list.get(index) == Some(&id) {
        list.remove(index);
    } else if let Some(found) = list.index_of(&id) {
        list.remove(found);
        dirty.resync.insert(parent);
    }
    if list.is_empty() {
        view.children.remove(&parent);
    }
}

/// Removes a node and its descendants from the view. If one of them holds
/// a stored id that a shadowed duplicate is waiting for, the duplicate takes
/// over the id: that needs a rebuild.
fn remove_subtree(view: &mut DocumentView, dirty: &mut Dirty, id: NodeId) -> Result<(), Rebuild> {
    let mut stack = vec![id];
    while let Some(next) = stack.pop() {
        if let NodeId::Stored(stored) = next
            && !view.shadowed.is_empty()
            && view.shadowed.values().any(|s| *s == stored)
        {
            return Err(Rebuild);
        }
        if let Some(children) = view.children.remove(&Parent::Node(next)) {
            stack.extend(children.iter().copied());
        }
        if let Some(tid) = view.handles.remove(&next) {
            view.owners.remove(&tid);
            view.shadowed.remove(&tid);
        }
        if view.nodes.remove(&next).is_some() {
            if !dirty.created.remove(&next) {
                dirty.recreated.remove(&next);
                dirty.removed.insert(next);
            }
            dirty.moved.remove(&next);
            dirty.parts.remove(&next);
            dirty.text_touched.remove(&next);
        }
    }
    Ok(())
}

/// Re-reads a (re-)created node and adopts descendants that Loro didn't
/// report separately.
fn adopt(
    view: &mut DocumentView,
    tree: &LoroTree,
    dirty: &mut Dirty,
    id: NodeId,
) -> Result<(), Rebuild> {
    let mut stack = vec![id];
    while let Some(next) = stack.pop() {
        let tid = *view.handles.get(&next).ok_or(Rebuild)?;
        let parent = view.nodes.get(&next).map(|n| n.parent).ok_or(Rebuild)?;
        view.nodes
            .insert(next, Arc::new(read_node(tree, tid, next, parent)));
        let mut list = imbl::Vector::new();
        for child in tree.children(TreeParentId::Node(tid)).unwrap_or_default() {
            let child_id = match view.owners.get(&child) {
                Some(&known) => known,
                None => {
                    let child_id = assign_identity(view, tree, child)?;
                    add_node(view, dirty, child_id, Parent::Node(next));
                    stack.push(child_id);
                    child_id
                }
            };
            list.push_back(child_id);
        }
        if list.is_empty() {
            view.children.remove(&Parent::Node(next));
        } else {
            view.children.insert(Parent::Node(next), list);
        }
    }
    Ok(())
}

/// Re-reads everything marked dirty and assembles the change set.
fn finish(
    view: &mut DocumentView,
    doc: &LoroDoc,
    tree: &LoroTree,
    mut dirty: Dirty,
    origin: ChangeOrigin,
) -> Result<ChangeSet, Rebuild> {
    for parent in std::mem::take(&mut dirty.resync) {
        let tree_parent = match parent {
            Parent::Root => TreeParentId::Root,
            Parent::Node(id) => match view.handles.get(&id) {
                Some(tid) => TreeParentId::Node(*tid),
                None => continue, // removed meanwhile
            },
        };
        let mut list = imbl::Vector::new();
        for child in tree.children(tree_parent).unwrap_or_default() {
            list.push_back(*view.owners.get(&child).ok_or(Rebuild)?);
        }
        if list.is_empty() {
            view.children.remove(&parent);
        } else {
            view.children.insert(parent, list);
        }
    }
    let fresh: Vec<NodeId> = dirty
        .created
        .iter()
        .chain(&dirty.recreated)
        .copied()
        .collect();
    for id in fresh {
        if view.nodes.contains_key(&id) {
            adopt(view, tree, &mut dirty, id)?;
        }
    }
    let mut changed: HashMap<NodeId, NodeChanges> = HashMap::new();
    for (id, parts) in &dirty.parts {
        if dirty.created.contains(id) || dirty.recreated.contains(id) {
            continue;
        }
        if parts != &Parts::default() {
            let tid = *view.handles.get(id).ok_or(Rebuild)?;
            if let (Some(node), Ok(meta)) = (view.nodes.get_mut(id), tree.get_meta(tid)) {
                read_node_parts(&meta, Arc::make_mut(node), parts);
            }
        }
        changed.insert(*id, to_changes(*parts, dirty.text_touched.contains(id)));
    }
    for id in &dirty.text_touched {
        if !dirty.parts.contains_key(id) && view.nodes.contains_key(id) {
            changed.insert(*id, to_changes(Parts::default(), true));
        }
    }
    let mut moved = dirty.moved;
    for id in &dirty.recreated {
        // It existed before and exists after: report it as moved and changed.
        moved.insert(*id);
        changed.insert(*id, to_changes(Parts::ALL, true));
    }
    let deleted: Vec<NodeId> = dirty
        .removed
        .iter()
        .filter(|id| !view.nodes.contains_key(*id))
        .copied()
        .collect();
    let mut changed: Vec<(NodeId, NodeChanges)> = changed.into_iter().collect();
    changed.sort_by_key(|(id, _)| *id);
    if dirty.meta {
        view.meta = Arc::new(read_meta(doc));
    }
    if dirty.pages {
        view.pages = read_props(&doc.get_map(root::PAGES));
    }
    if dirty.ext {
        view.ext = read_props(&doc.get_map(root::EXT));
    }
    let styles = doc.get_map(root::STYLES);
    for id in &dirty.styles {
        match read_style(&styles, id.as_str()) {
            Some(style) => {
                view.styles.insert(id.clone(), Arc::new(style));
            }
            None => {
                view.styles.remove(id);
            }
        }
    }
    let assets = doc.get_map(root::ASSETS);
    for id in &dirty.assets {
        match map_at(&assets, &id.to_string()) {
            Some(map) => {
                view.assets.insert(*id, read_props(&map));
            }
            None => {
                view.assets.remove(id);
            }
        }
    }
    view.version += 1;
    Ok(ChangeSet {
        origin,
        created: dirty.created.into_iter().collect(),
        deleted,
        moved: moved.into_iter().collect(),
        changed,
        meta: dirty.meta,
        styles: dirty.styles.into_iter().collect(),
        assets: dirty.assets.into_iter().collect(),
        pages: dirty.pages,
        ext: dirty.ext,
    })
}

fn to_changes(parts: Parts, text_touched: bool) -> NodeChanges {
    NodeChanges {
        kind: parts.kind,
        geometry: parts.geometry,
        style: parts.style,
        text: parts.text || text_touched,
        assets: parts.assets,
        ext: parts.ext,
        generation: parts.generation,
        props: parts.props,
    }
}

/// The changes between two views, computed by comparing them. Used after
/// a full rebuild. O(document size).
pub(crate) fn diff_views(
    old: &DocumentView,
    new: &DocumentView,
    origin: ChangeOrigin,
) -> ChangeSet {
    fn positions(view: &DocumentView) -> HashMap<NodeId, (Parent, usize)> {
        let mut out = HashMap::new();
        for (parent, list) in &view.children {
            for (index, id) in list.iter().enumerate() {
                out.insert(*id, (*parent, index));
            }
        }
        out
    }
    let (old_pos, new_pos) = (positions(old), positions(new));
    let mut changes = ChangeSet {
        origin,
        ..ChangeSet::default()
    };
    for (id, node) in &new.nodes {
        match old.nodes.get(id) {
            None => changes.created.push(*id),
            Some(before) => {
                if old_pos.get(id) != new_pos.get(id) {
                    changes.moved.push(*id);
                }
                let parts = NodeChanges {
                    kind: before.kind != node.kind,
                    geometry: before.geometry != node.geometry,
                    style: before.style != node.style
                        || before.style_overrides != node.style_overrides,
                    text: before.text != node.text,
                    assets: before.assets != node.assets,
                    ext: before.ext != node.ext,
                    generation: before.generation != node.generation,
                    props: before.props != node.props,
                };
                if parts != NodeChanges::default() {
                    changes.changed.push((*id, parts));
                }
            }
        }
    }
    changes.deleted = old
        .nodes
        .keys()
        .filter(|id| !new.nodes.contains_key(*id))
        .copied()
        .collect();
    changes.created.sort();
    changes.deleted.sort();
    changes.moved.sort();
    changes.changed.sort_by_key(|(id, _)| *id);
    changes.meta = old.meta != new.meta;
    changes.pages = old.pages != new.pages;
    changes.ext = old.ext != new.ext;
    let styles: BTreeSet<&StyleId> = old.styles.keys().chain(new.styles.keys()).collect();
    changes.styles = styles
        .into_iter()
        .filter(|id| old.styles.get(*id) != new.styles.get(*id))
        .cloned()
        .collect();
    let assets: BTreeSet<&BlobId> = old.assets.keys().chain(new.assets.keys()).collect();
    changes.assets = assets
        .into_iter()
        .filter(|id| old.assets.get(*id) != new.assets.get(*id))
        .copied()
        .collect();
    changes
}
