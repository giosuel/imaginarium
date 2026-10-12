// SPDX-License-Identifier: AGPL-3.0-or-later

//! The materialized view model (ADR-0009): persistent `imbl` structures
//! that renderers, layout and other threads read without locks.
//!
//! A [`DocumentView`] is a value. Cloning it is O(1), and versions share
//! everything that didn't change. The [`Document`](crate::Document) updates
//! its view incrementally from the CRDT's events and reports what changed
//! as a [`ChangeSet`](crate::ChangeSet).

mod apply;
mod build;
mod text;

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

use loro::{ContainerID, TreeID};

pub(crate) use apply::{RawEvent, apply_events};
pub(crate) use build::build;
pub use text::{Marks, Paragraph, Run, TextView};

use crate::ids::{BlobId, NodeId, Parent, StyleId};
use crate::schema::{DocumentKind, NodeKind};
use crate::value::{Geometry, Props, Value};

/// The document's `meta` (schema-v1.md §4).
#[derive(Clone, Debug, PartialEq, Default)]
pub struct MetaView {
    pub(crate) schema_version: Option<i64>,
    pub(crate) kind: DocumentKind,
    pub(crate) title: Option<Arc<str>>,
    pub(crate) language: Option<Arc<str>>,
    pub(crate) created: Option<i64>,
    pub(crate) required_features: Vec<String>,
    pub(crate) other: Props,
}

impl MetaView {
    /// `meta.schema_version`.
    pub fn schema_version(&self) -> Option<i64> {
        self.schema_version
    }

    /// The default view.
    pub fn kind(&self) -> DocumentKind {
        self.kind
    }

    /// The title.
    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    /// The default language (BCP 47).
    pub fn language(&self) -> Option<&str> {
        self.language.as_deref()
    }

    /// Creation time (Unix milliseconds).
    pub fn created(&self) -> Option<i64> {
        self.created
    }

    /// Features needed to edit (schema-v1.md §10.2).
    pub fn required_features(&self) -> &[String] {
        &self.required_features
    }

    /// Keys this version doesn't interpret.
    pub fn other(&self) -> &Props {
        &self.other
    }
}

/// One node (schema-v1.md §5).
#[derive(Clone, Debug, PartialEq)]
pub struct NodeView {
    pub(crate) id: NodeId,
    pub(crate) parent: Parent,
    pub(crate) kind: NodeKind,
    pub(crate) geometry: Option<Geometry>,
    pub(crate) style: Option<StyleId>,
    pub(crate) style_overrides: Props,
    pub(crate) text: Option<TextView>,
    /// The Loro container the text came from, to detect a replaced text.
    pub(crate) text_container: Option<ContainerID>,
    pub(crate) assets: Arc<BTreeMap<String, BlobId>>,
    pub(crate) ext: Props,
    pub(crate) generation: Props,
    pub(crate) props: Props,
}

impl NodeView {
    pub(crate) fn placeholder(id: NodeId, parent: Parent) -> Self {
        Self {
            id,
            parent,
            kind: NodeKind::Unknown("".into()),
            geometry: None,
            style: None,
            style_overrides: Props::default(),
            text: None,
            text_container: None,
            assets: Arc::default(),
            ext: Props::default(),
            generation: Props::default(),
            props: Props::default(),
        }
    }

    /// The node's id.
    pub fn id(&self) -> NodeId {
        self.id
    }

    /// The parent.
    pub fn parent(&self) -> Parent {
        self.parent
    }

    /// The kind (`k`).
    pub fn kind(&self) -> &NodeKind {
        &self.kind
    }

    /// The box (`g`), if the node has one.
    pub fn geometry(&self) -> Option<Geometry> {
        self.geometry
    }

    /// The style (`s.id`).
    pub fn style(&self) -> Option<&StyleId> {
        self.style.as_ref()
    }

    /// Local style overrides (`s` without `id`).
    pub fn style_overrides(&self) -> &Props {
        &self.style_overrides
    }

    /// The text (`t`).
    pub fn text(&self) -> Option<&TextView> {
        self.text.as_ref()
    }

    /// Asset references (`a`): role → blob.
    pub fn assets(&self) -> &BTreeMap<String, BlobId> {
        &self.assets
    }

    /// Node-level add-on data (`x`).
    pub fn ext(&self) -> &Props {
        &self.ext
    }

    /// AI provenance (`gen`).
    pub fn generation(&self) -> &Props {
        &self.generation
    }

    /// Other scalar keys: `name`, `hidden`, `locked`, `shape`, `from`,
    /// `to`, and keys of newer versions.
    pub fn props(&self) -> &Props {
        &self.props
    }

    /// A scalar key.
    pub fn prop(&self, key: &str) -> Option<&Value> {
        self.props.get(key)
    }

    /// The user-facing name.
    pub fn name(&self) -> Option<&str> {
        self.prop("name")?.as_str()
    }

    /// Whether the node is hidden.
    pub fn hidden(&self) -> bool {
        self.prop("hidden")
            .and_then(Value::as_bool)
            .unwrap_or(false)
    }

    /// Whether the node is locked.
    pub fn locked(&self) -> bool {
        self.prop("locked")
            .and_then(Value::as_bool)
            .unwrap_or(false)
    }
}

/// A style (schema-v1.md §8).
#[derive(Clone, Debug, PartialEq, Default)]
pub struct StyleView {
    pub(crate) name: Option<Arc<str>>,
    pub(crate) parent: Option<StyleId>,
    pub(crate) props: Props,
}

impl StyleView {
    /// The user-facing name.
    pub fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    /// The parent style.
    pub fn parent(&self) -> Option<&StyleId> {
        self.parent.as_ref()
    }

    /// The style's own properties.
    pub fn props(&self) -> &Props {
        &self.props
    }
}

/// A snapshot of the whole document. Cheap to clone and to share.
#[derive(Clone, Debug, Default)]
pub struct DocumentView {
    pub(crate) meta: Arc<MetaView>,
    pub(crate) nodes: imbl::HashMap<NodeId, Arc<NodeView>>,
    pub(crate) children: imbl::HashMap<Parent, imbl::Vector<NodeId>>,
    pub(crate) styles: imbl::HashMap<StyleId, Arc<StyleView>>,
    pub(crate) assets: imbl::HashMap<BlobId, Props>,
    pub(crate) pages: Props,
    pub(crate) ext: Props,
    pub(crate) version: u64,
    /// Node identity → its current CRDT tree id (which undo can change).
    pub(crate) handles: imbl::HashMap<NodeId, TreeID>,
    /// CRDT tree id → node identity.
    pub(crate) owners: imbl::HashMap<TreeID, NodeId>,
    /// Live nodes whose stored id is held by another node, and that id.
    /// Almost always empty (schema-v1.md §5).
    pub(crate) shadowed: imbl::HashMap<TreeID, u64>,
}

/// Views are equal if their content is, whatever their version number.
impl PartialEq for DocumentView {
    fn eq(&self, other: &Self) -> bool {
        self.meta == other.meta
            && self.nodes == other.nodes
            && self.children_equal(other)
            && self.styles == other.styles
            && self.assets == other.assets
            && self.pages == other.pages
            && self.ext == other.ext
            && self.handles == other.handles
            && self.shadowed == other.shadowed
    }
}

impl DocumentView {
    /// Child lists compare equal if they hold the same children, ignoring
    /// empty lists that one view kept and the other dropped.
    fn children_equal(&self, other: &Self) -> bool {
        let keys: HashSet<&Parent> = self.children.keys().chain(other.children.keys()).collect();
        keys.into_iter().all(|parent| {
            let empty = imbl::Vector::new();
            self.children.get(parent).unwrap_or(&empty)
                == other.children.get(parent).unwrap_or(&empty)
        })
    }

    /// `meta`.
    pub fn meta(&self) -> &MetaView {
        &self.meta
    }

    /// A node.
    pub fn node(&self, id: NodeId) -> Option<&NodeView> {
        self.nodes.get(&id).map(|n| &**n)
    }

    /// Number of nodes.
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// The children of `parent`, in order (back to front, or reading order).
    pub fn children(&self, parent: Parent) -> impl Iterator<Item = NodeId> + '_ {
        self.children
            .get(&parent)
            .into_iter()
            .flat_map(|list| list.iter().copied())
    }

    /// Every node, depth first in document order.
    pub fn walk(&self) -> Vec<NodeId> {
        let mut out = Vec::with_capacity(self.nodes.len());
        let mut stack: Vec<NodeId> = self.children(Parent::Root).collect();
        stack.reverse();
        while let Some(id) = stack.pop() {
            out.push(id);
            let start = stack.len();
            stack.extend(self.children(Parent::Node(id)));
            stack[start..].reverse();
        }
        out
    }

    /// A style.
    pub fn style(&self, id: &StyleId) -> Option<&StyleView> {
        self.styles.get(id).map(|s| &**s)
    }

    /// Every style id.
    pub fn style_ids(&self) -> impl Iterator<Item = &StyleId> {
        self.styles.keys()
    }

    /// An asset's properties.
    pub fn asset(&self, id: &BlobId) -> Option<&Props> {
        self.assets.get(id)
    }

    /// Page and slide setup.
    pub fn pages(&self) -> &Props {
        &self.pages
    }

    /// Document-level add-on data.
    pub fn ext(&self) -> &Props {
        &self.ext
    }

    /// Counts the changes applied since the document was opened. Two views
    /// with the same version of the same document are identical.
    pub fn version(&self) -> u64 {
        self.version
    }

    /// The effective value of a style property for a node: its override,
    /// then its style and that style's ancestors (schema-v1.md §8). Stops
    /// at cycles and missing styles.
    pub fn style_prop(&self, node: NodeId, prop: &str) -> Option<&Value> {
        let node = self.node(node)?;
        if let Some(value) = node.style_overrides.get(prop) {
            return Some(value);
        }
        let mut seen = HashSet::new();
        let mut next = node.style.as_ref();
        while let Some(id) = next {
            if !seen.insert(id) {
                return None;
            }
            let style = self.style(id)?;
            if let Some(value) = style.props.get(prop) {
                return Some(value);
            }
            next = style.parent.as_ref();
        }
        None
    }
}
