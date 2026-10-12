// SPDX-License-Identifier: AGPL-3.0-or-later

//! Reading the view from the CRDT state: the whole document on open, and
//! single parts after an incremental change.

use std::collections::BTreeMap;
use std::sync::Arc;

use loro::{
    Container, ContainerTrait, LoroDoc, LoroMap, LoroTree, LoroValue, TreeID, TreeParentId,
    ValueOrContainer,
};

use super::{DocumentView, MetaView, NodeView, StyleView, TextView};
use crate::ids::{BlobId, NodeId, Parent, StyleId};
use crate::schema::{DocumentKind, NodeKind, meta, node, root};
use crate::value::{Geometry, Props, Value};

/// Builds the complete view of `doc`.
pub(crate) fn build(doc: &LoroDoc) -> DocumentView {
    let mut view = DocumentView {
        meta: Arc::new(read_meta(doc)),
        pages: read_props(&doc.get_map(root::PAGES)),
        ext: read_props(&doc.get_map(root::EXT)),
        ..DocumentView::default()
    };
    let tree = doc.get_tree(root::NODES);
    // Every live node with its tree parent, parents first.
    let mut order: Vec<(TreeID, Option<TreeID>)> = Vec::new();
    let mut stack: Vec<Option<TreeID>> = vec![None];
    while let Some(parent) = stack.pop() {
        let tree_parent = parent.map_or(TreeParentId::Root, TreeParentId::Node);
        for child in tree.children(tree_parent).unwrap_or_default() {
            order.push((child, parent));
            stack.push(Some(child));
        }
    }
    // Identities: a stored id belongs to the smallest tree id that claims it.
    let stored: Vec<Option<u64>> = order
        .iter()
        .map(|(tid, _)| stored_id(&tree, *tid))
        .collect();
    let mut winners: std::collections::HashMap<u64, TreeID> = std::collections::HashMap::new();
    for ((tid, _), stored) in order.iter().zip(&stored) {
        if let Some(id) = stored {
            let winner = winners.entry(*id).or_insert(*tid);
            if tree_key(*tid) < tree_key(*winner) {
                *winner = *tid;
            }
        }
    }
    for ((tid, _), stored) in order.iter().zip(&stored) {
        let id = match stored {
            Some(id) if winners.get(id) == Some(tid) => NodeId::Stored(*id),
            Some(id) => {
                view.shadowed.insert(*tid, *id);
                NodeId::Tree(*tid)
            }
            None => NodeId::Tree(*tid),
        };
        view.handles.insert(id, *tid);
        view.owners.insert(*tid, id);
    }
    for (tid, parent) in &order {
        let id = view.owners[tid];
        let parent = parent.map_or(Parent::Root, |p| Parent::Node(view.owners[&p]));
        view.children.entry(parent).or_default().push_back(id);
        view.nodes
            .insert(id, Arc::new(read_node(&tree, *tid, id, parent)));
    }
    let styles = doc.get_map(root::STYLES);
    for key in styles.keys() {
        if let (Ok(id), Some(style)) = (StyleId::new(key.as_str()), read_style(&styles, &key)) {
            view.styles.insert(id, Arc::new(style));
        }
    }
    let assets = doc.get_map(root::ASSETS);
    for key in assets.keys() {
        if let (Some(id), Some(asset)) = (BlobId::parse(&key), map_at(&assets, &key)) {
            view.assets.insert(id, read_props(&asset));
        }
    }
    view
}

/// Tree ids in a total order, for picking duplicate winners.
pub(crate) fn tree_key(tid: TreeID) -> (u64, i32) {
    (tid.peer, tid.counter)
}

/// The stored `id` of a tree node, if it has a valid one.
pub(crate) fn stored_id(tree: &LoroTree, tid: TreeID) -> Option<u64> {
    let meta = tree.get_meta(tid).ok()?;
    match meta.get(node::ID)? {
        ValueOrContainer::Value(LoroValue::String(s)) => match NodeId::parse_stored(&s)? {
            NodeId::Stored(id) => Some(id),
            NodeId::Tree(_) => None,
        },
        _ => None,
    }
}

/// `meta`.
pub(crate) fn read_meta(doc: &LoroDoc) -> MetaView {
    let map = doc.get_map(root::META);
    let mut view = MetaView::default();
    let mut other = BTreeMap::new();
    for key in map.keys() {
        let Some(item) = map.get(&key) else { continue };
        match (key.as_str(), item) {
            (meta::SCHEMA_VERSION, ValueOrContainer::Value(LoroValue::I64(v))) => {
                view.schema_version = Some(v)
            }
            (meta::KIND, ValueOrContainer::Value(LoroValue::String(s))) => {
                view.kind = DocumentKind::parse(&s)
            }
            (meta::TITLE, ValueOrContainer::Value(LoroValue::String(s))) => {
                view.title = Some(Arc::from(s.as_str()))
            }
            (meta::LANGUAGE, ValueOrContainer::Value(LoroValue::String(s))) => {
                view.language = Some(Arc::from(s.as_str()))
            }
            (meta::CREATED, ValueOrContainer::Value(LoroValue::I64(v))) => view.created = Some(v),
            (meta::REQUIRED_FEATURES, ValueOrContainer::Container(Container::List(list))) => {
                view.required_features = list
                    .get_deep_value()
                    .into_list()
                    .map(|l| {
                        l.iter()
                            .filter_map(|v| v.as_string().map(|s| s.to_string()))
                            .collect()
                    })
                    .unwrap_or_default();
            }
            (_, item) => {
                other.insert(key.to_string(), deep(&item));
            }
        }
    }
    view.other = Arc::new(other);
    view
}

/// One node, with all its parts.
pub(crate) fn read_node(tree: &LoroTree, tid: TreeID, id: NodeId, parent: Parent) -> NodeView {
    let mut view = NodeView::placeholder(id, parent);
    if let Ok(meta) = tree.get_meta(tid) {
        read_node_parts(&meta, &mut view, &Parts::ALL);
    }
    view
}

/// Which parts of a node to (re-)read.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Parts {
    pub(crate) kind: bool,
    pub(crate) geometry: bool,
    pub(crate) style: bool,
    pub(crate) text: bool,
    pub(crate) assets: bool,
    pub(crate) ext: bool,
    pub(crate) generation: bool,
    pub(crate) props: bool,
}

impl Parts {
    pub(crate) const ALL: Parts = Parts {
        kind: true,
        geometry: true,
        style: true,
        text: true,
        assets: true,
        ext: true,
        generation: true,
        props: true,
    };

    /// The part a key of the node's meta map belongs to.
    pub(crate) fn of_key(key: &str) -> Parts {
        let mut parts = Parts::default();
        match key {
            node::KIND => parts.kind = true,
            node::GEOMETRY => parts.geometry = true,
            node::STYLE => parts.style = true,
            node::TEXT => parts.text = true,
            node::ASSETS => parts.assets = true,
            node::EXT => parts.ext = true,
            node::GEN => parts.generation = true,
            _ => parts.props = true,
        }
        parts
    }

    pub(crate) fn union(self, other: Parts) -> Parts {
        Parts {
            kind: self.kind || other.kind,
            geometry: self.geometry || other.geometry,
            style: self.style || other.style,
            text: self.text || other.text,
            assets: self.assets || other.assets,
            ext: self.ext || other.ext,
            generation: self.generation || other.generation,
            props: self.props || other.props,
        }
    }
}

/// Re-reads `parts` of a node from its meta map.
pub(crate) fn read_node_parts(meta: &LoroMap, view: &mut NodeView, parts: &Parts) {
    if parts.kind {
        view.kind = match meta.get(node::KIND) {
            Some(ValueOrContainer::Value(LoroValue::String(kind))) => NodeKind::parse(&kind),
            _ => NodeKind::Unknown("".into()),
        };
    }
    if parts.geometry {
        view.geometry = map_at(meta, node::GEOMETRY).map(|g| Geometry {
            x: number(&g, "x"),
            y: number(&g, "y"),
            w: number(&g, "w"),
            h: number(&g, "h"),
            r: number(&g, "r"),
        });
    }
    if parts.style {
        view.style = None;
        let mut overrides = BTreeMap::new();
        if let Some(style) = map_at(meta, node::STYLE) {
            for key in style.keys() {
                let Some(item) = style.get(&key) else {
                    continue;
                };
                if key.as_str() == node::STYLE_ID {
                    view.style = deep(&item).as_str().and_then(|s| StyleId::new(s).ok());
                } else {
                    overrides.insert(key.to_string(), deep(&item));
                }
            }
        }
        view.style_overrides = Arc::new(overrides);
    }
    if parts.text {
        match meta.get(node::TEXT) {
            Some(ValueOrContainer::Container(Container::Text(text))) => {
                view.text_container = Some(text.id());
                view.text = Some(TextView::from_delta(&text.to_delta()));
            }
            _ => {
                view.text_container = None;
                view.text = None;
            }
        }
    }
    if parts.assets {
        let mut assets = BTreeMap::new();
        if let Some(map) = map_at(meta, node::ASSETS) {
            for key in map.keys() {
                if let Some(blob) = map
                    .get(&key)
                    .and_then(|v| deep(&v).as_str().and_then(BlobId::parse))
                {
                    assets.insert(key.to_string(), blob);
                }
            }
        }
        view.assets = Arc::new(assets);
    }
    if parts.ext {
        view.ext = map_at(meta, node::EXT)
            .map(|m| read_props(&m))
            .unwrap_or_default();
    }
    if parts.generation {
        view.generation = map_at(meta, node::GEN)
            .map(|m| read_props(&m))
            .unwrap_or_default();
    }
    if parts.props {
        let mut props = BTreeMap::new();
        for key in meta.keys() {
            if key.as_str() == node::KIND
                || key.as_str() == node::ID
                || node::CONTAINERS.contains(&key.as_str())
            {
                continue;
            }
            if let Some(item) = meta.get(&key) {
                props.insert(key.to_string(), deep(&item));
            }
        }
        view.props = Arc::new(props);
    }
}

/// A style of the `styles` map.
pub(crate) fn read_style(styles: &LoroMap, key: &str) -> Option<StyleView> {
    let map = map_at(styles, key)?;
    let mut view = StyleView::default();
    let mut props = BTreeMap::new();
    for key in map.keys() {
        let Some(item) = map.get(&key) else { continue };
        let value = deep(&item);
        match key.as_str() {
            "name" => view.name = value.as_str().map(Arc::from),
            "parent" => view.parent = value.as_str().and_then(|s| StyleId::new(s).ok()),
            _ => {
                props.insert(key.to_string(), value);
            }
        }
    }
    view.props = Arc::new(props);
    Some(view)
}

/// A map's entries as values (nested containers as their deep values).
pub(crate) fn read_props(map: &LoroMap) -> Props {
    match Value::from_loro(&map.get_deep_value()) {
        Value::Map(map) => map,
        _ => Props::default(),
    }
}

pub(crate) fn map_at(map: &LoroMap, key: &str) -> Option<LoroMap> {
    match map.get(key)? {
        ValueOrContainer::Container(Container::Map(map)) => Some(map),
        _ => None,
    }
}

fn number(map: &LoroMap, key: &str) -> f64 {
    map.get(key)
        .and_then(|v| deep(&v).as_number())
        .unwrap_or(0.0)
}

fn deep(item: &ValueOrContainer) -> Value {
    match item {
        ValueOrContainer::Value(value) => Value::from_loro(value),
        container => Value::from_loro(&container.get_deep_value()),
    }
}
