// SPDX-License-Identifier: AGPL-3.0-or-later

//! What one change did to the view: the input for fine-grained signals.

use crate::ids::{Actor, BlobId, NodeId, StyleId, UndoScope};

/// Where a change came from.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum ChangeOrigin {
    /// A local transaction of this actor.
    Local(Actor),
    /// An undo or redo on this stack.
    Undo(UndoScope),
    /// Changes received from other peers.
    #[default]
    Remote,
}

/// Which parts of a node changed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NodeChanges {
    /// The kind (`k`).
    pub kind: bool,
    /// The box (`g`).
    pub geometry: bool,
    /// The style reference or overrides (`s`).
    pub style: bool,
    /// The text (`t`).
    pub text: bool,
    /// Asset references (`a`).
    pub assets: bool,
    /// Add-on data (`x`).
    pub ext: bool,
    /// AI provenance (`gen`).
    pub generation: bool,
    /// Other scalar keys (`name`, `hidden`, …).
    pub props: bool,
}

/// The changes of one transaction, undo step or batch of remote updates.
///
/// Created nodes are listed only in `created` and deleted ones only in
/// `deleted` (with their whole subtrees). `changed` lists the other nodes,
/// sorted by id.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ChangeSet {
    /// Where the change came from.
    pub origin: ChangeOrigin,
    /// New nodes.
    pub created: Vec<NodeId>,
    /// Removed nodes, including the descendants of removed nodes.
    pub deleted: Vec<NodeId>,
    /// Nodes with a new parent or position among their siblings.
    pub moved: Vec<NodeId>,
    /// Nodes whose content changed.
    pub changed: Vec<(NodeId, NodeChanges)>,
    /// `meta` changed.
    pub meta: bool,
    /// Styles that were added, changed or removed.
    pub styles: Vec<StyleId>,
    /// Assets that were added, changed or removed.
    pub assets: Vec<BlobId>,
    /// `pages` changed.
    pub pages: bool,
    /// Document-level add-on data changed.
    pub ext: bool,
}

impl ChangeSet {
    /// Whether nothing changed.
    pub fn is_empty(&self) -> bool {
        self.created.is_empty()
            && self.deleted.is_empty()
            && self.moved.is_empty()
            && self.changed.is_empty()
            && !self.meta
            && self.styles.is_empty()
            && self.assets.is_empty()
            && !self.pages
            && !self.ext
    }

    /// Drops every mention of `nodes`, for nodes that came and went within
    /// one change.
    pub(crate) fn forget(&mut self, nodes: &[NodeId]) {
        self.created.retain(|id| !nodes.contains(id));
        self.deleted.retain(|id| !nodes.contains(id));
        self.moved.retain(|id| !nodes.contains(id));
        self.changed.retain(|(id, _)| !nodes.contains(id));
    }

    /// What changed about one node (nothing for created or deleted nodes).
    pub fn node(&self, id: NodeId) -> Option<NodeChanges> {
        self.changed
            .binary_search_by_key(&id, |(node, _)| *node)
            .ok()
            .map(|index| self.changed[index].1)
    }
}
