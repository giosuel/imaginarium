// SPDX-License-Identifier: AGPL-3.0-or-later

//! The accessibility tree a view describes itself in (AccessKit), for
//! screen readers (Orca, NVDA, VoiceOver).

use accesskit::{Affine, Node, NodeId, Role, TreeId, TreeInfo, TreeUpdate};
use imaginarium_render::kurbo;

/// The window's node. Views use ids from 1.
pub const ROOT: NodeId = NodeId(0);

/// The nodes a view adds in [`View::accessibility`](crate::View::accessibility).
/// Bounds are in logical pixels relative to the window: the window's node
/// carries the scale factor.
#[derive(Debug, Default)]
pub struct AccessTree {
    nodes: Vec<(NodeId, Node)>,
    children: Vec<NodeId>,
    focus: Option<NodeId>,
}

impl AccessTree {
    /// Adds (or replaces) a node. Give it a parent with
    /// [`Node::set_children`], or make it top level with
    /// [`AccessTree::add_top_level`].
    pub fn push(&mut self, id: NodeId, node: Node) {
        self.nodes.push((id, node));
    }

    /// Makes `id` a child of the window, after the ones before it.
    pub fn add_top_level(&mut self, id: NodeId) {
        self.children.push(id);
    }

    /// Marks the node with the keyboard focus.
    pub fn set_focus(&mut self, id: NodeId) {
        self.focus = Some(id);
    }

    /// The focused node, if any.
    pub fn focus(&self) -> Option<NodeId> {
        self.focus
    }

    /// The nodes added so far.
    pub fn nodes(&self) -> &[(NodeId, Node)] {
        &self.nodes
    }

    /// The whole tree as an AccessKit update, under a window node named
    /// `title` scaled by `scale` (AccessKit wants physical pixels).
    pub(crate) fn into_update(self, title: &str, scale: f64, initial: bool) -> TreeUpdate {
        let mut root = Node::new(Role::Window);
        root.set_label(title);
        root.set_children(self.children);
        root.set_transform(Affine::scale(scale));
        let mut nodes = self.nodes;
        nodes.push((ROOT, root));
        TreeUpdate {
            nodes,
            tree: initial.then(|| {
                let mut tree = TreeInfo::new(ROOT);
                tree.toolkit_name = Some("Imaginarium".into());
                tree.toolkit_version = Some(env!("CARGO_PKG_VERSION").into());
                tree
            }),
            tree_id: TreeId::ROOT,
            focus: self.focus.unwrap_or(ROOT),
        }
    }
}

/// A kurbo rectangle as AccessKit's.
pub fn rect(rect: kurbo::Rect) -> accesskit::Rect {
    accesskit::Rect::new(rect.x0, rect.y0, rect.x1, rect.y1)
}
