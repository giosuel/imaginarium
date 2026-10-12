// SPDX-License-Identifier: AGPL-3.0-or-later

//! Loro: the movable tree holds the hierarchy, each node's meta map holds its
//! properties. Fractional indexes are on, because z-order needs sibling order.

use imaginarium_test_utils::fixtures::{ParagraphRole, ParagraphSpec, ShapeSpec};
use loro::{
    Container, ExpandType, ExportMode, LoroDoc, LoroMap, LoroText, LoroTree, LoroValue,
    StyleConfig, StyleConfigMap, TreeID, TreeParentId, UndoManager, ValueOrContainer,
    VersionVector,
};

use crate::engine::{Board, Features, TextDoc, kind_name};

fn doc(peer: u64) -> LoroDoc {
    let doc = LoroDoc::new();
    doc.set_peer_id(peer).unwrap();
    doc
}

fn number(value: Option<ValueOrContainer>) -> f64 {
    match value {
        Some(ValueOrContainer::Value(LoroValue::Double(v))) => v,
        Some(ValueOrContainer::Value(LoroValue::I64(v))) => v as f64,
        other => panic!("expected a number, got {other:?}"),
    }
}

fn map(value: Option<ValueOrContainer>) -> LoroMap {
    match value {
        Some(ValueOrContainer::Container(Container::Map(map))) => map,
        other => panic!("expected a map, got {other:?}"),
    }
}

pub struct LoroBoard {
    doc: LoroDoc,
    tree: LoroTree,
    nodes: Vec<TreeID>,
    geometry: Vec<LoroMap>,
}

impl LoroBoard {
    fn insert(&mut self, shape: &ShapeSpec) {
        let id = self.tree.create(TreeParentId::Root).unwrap();
        let meta = self.tree.get_meta(id).unwrap();
        meta.insert("k", kind_name(shape.kind)).unwrap();
        let g = meta.insert_container("g", LoroMap::new()).unwrap();
        g.insert("x", shape.x).unwrap();
        g.insert("y", shape.y).unwrap();
        g.insert("w", shape.width).unwrap();
        g.insert("h", shape.height).unwrap();
        g.insert("r", shape.rotation_deg).unwrap();
        meta.insert("f", i64::from(shape.fill_rgb)).unwrap();
        if !shape.label.is_empty() {
            let text = meta.insert_container("t", LoroText::new()).unwrap();
            text.insert(0, &shape.label).unwrap();
        }
        self.nodes.push(id);
        self.geometry.push(g);
    }
}

impl Board for LoroBoard {
    type Version = VersionVector;
    type State = LoroValue;

    fn create(peer: u64, shapes: &[ShapeSpec]) -> Self {
        let doc = doc(peer);
        let tree = doc.get_tree("nodes");
        tree.enable_fractional_index(0);
        let mut board = Self {
            doc,
            tree,
            nodes: Vec::with_capacity(shapes.len()),
            geometry: Vec::with_capacity(shapes.len()),
        };
        for shape in shapes {
            board.insert(shape);
        }
        board.doc.commit();
        board
    }

    fn load(peer: u64, bytes: &[u8]) -> Self {
        let doc = doc(peer);
        doc.import(bytes).unwrap();
        let tree = doc.get_tree("nodes");
        let nodes = tree.children(TreeParentId::Root).unwrap_or_default();
        let geometry = nodes
            .iter()
            .map(|id| map(tree.get_meta(*id).unwrap().get("g")))
            .collect();
        Self {
            doc,
            tree,
            nodes,
            geometry,
        }
    }

    fn save(&mut self) -> Vec<u8> {
        self.doc.export(ExportMode::Snapshot).unwrap()
    }

    fn save_trimmed(&mut self) -> Option<Vec<u8>> {
        let frontiers = self.doc.oplog_frontiers();
        Some(
            self.doc
                .export(ExportMode::shallow_snapshot(&frontiers))
                .unwrap(),
        )
    }

    fn len(&self) -> usize {
        self.nodes.len()
    }

    fn read_geometry(&self) -> f64 {
        self.geometry
            .iter()
            .map(|g| {
                number(g.get("x")) + number(g.get("y")) + number(g.get("w")) + number(g.get("h"))
            })
            .sum()
    }

    fn move_shape(&mut self, index: usize, x: f64, y: f64) {
        let g = &self.geometry[index];
        g.insert("x", x).unwrap();
        g.insert("y", y).unwrap();
        self.doc.commit();
    }

    fn add_shape(&mut self, shape: &ShapeSpec) {
        self.insert(shape);
        self.doc.commit();
    }

    fn reparent(&mut self, child: usize, parent: usize) {
        self.tree
            .mov(self.nodes[child], TreeParentId::Node(self.nodes[parent]))
            .unwrap();
        self.doc.commit();
    }

    fn has_cycle(&mut self) -> bool {
        let limit = self.nodes.len();
        self.nodes.iter().any(|&start| {
            let mut node = start;
            for _ in 0..=limit {
                match self.tree.parent(node) {
                    Some(TreeParentId::Node(parent)) => node = parent,
                    _ => return false,
                }
            }
            true
        })
    }

    fn version(&mut self) -> VersionVector {
        self.doc.oplog_vv()
    }

    fn updates_since(&mut self, version: &VersionVector) -> Vec<u8> {
        self.doc.export(ExportMode::updates(version)).unwrap()
    }

    fn apply(&mut self, update: &[u8]) {
        self.doc.import(update).unwrap();
    }

    fn state(&mut self) -> LoroValue {
        self.doc.get_deep_value()
    }
}

pub struct LoroTextDoc {
    doc: LoroDoc,
    text: LoroText,
}

fn configure_styles(doc: &LoroDoc) {
    let mut styles = StyleConfigMap::new();
    for key in ["h", "li"] {
        styles.insert(
            key.into(),
            StyleConfig {
                expand: ExpandType::None,
            },
        );
    }
    doc.config_text_style(styles);
}

impl TextDoc for LoroTextDoc {
    type Version = VersionVector;
    type State = String;

    fn create(peer: u64, paragraphs: &[ParagraphSpec]) -> Self {
        let doc = doc(peer);
        configure_styles(&doc);
        let text = doc.get_text("body");
        let mut pos = 0;
        for paragraph in paragraphs {
            let len = paragraph.text.chars().count();
            text.insert(pos, &paragraph.text).unwrap();
            match paragraph.role {
                ParagraphRole::Heading1 => text.mark(pos..pos + len, "h", 1).unwrap(),
                ParagraphRole::Heading2 => text.mark(pos..pos + len, "h", 2).unwrap(),
                ParagraphRole::ListItem => text.mark(pos..pos + len, "li", true).unwrap(),
                ParagraphRole::Body => {}
            }
            pos += len;
            text.insert(pos, "\n").unwrap();
            pos += 1;
        }
        doc.commit();
        Self { doc, text }
    }

    fn load(peer: u64, bytes: &[u8]) -> Self {
        let doc = doc(peer);
        configure_styles(&doc);
        doc.import(bytes).unwrap();
        let text = doc.get_text("body");
        Self { doc, text }
    }

    fn save(&mut self) -> Vec<u8> {
        self.doc.export(ExportMode::Snapshot).unwrap()
    }

    fn save_trimmed(&mut self) -> Option<Vec<u8>> {
        let frontiers = self.doc.oplog_frontiers();
        Some(
            self.doc
                .export(ExportMode::shallow_snapshot(&frontiers))
                .unwrap(),
        )
    }

    fn len(&self) -> usize {
        self.text.len_unicode()
    }

    fn text(&self) -> String {
        self.text.to_string()
    }

    fn insert(&mut self, pos: usize, s: &str) {
        self.text.insert(pos, s).unwrap();
        self.doc.commit();
    }

    fn version(&mut self) -> VersionVector {
        self.doc.oplog_vv()
    }

    fn updates_since(&mut self, version: &VersionVector) -> Vec<u8> {
        self.doc.export(ExportMode::updates(version)).unwrap()
    }

    fn apply(&mut self, update: &[u8]) {
        self.doc.import(update).unwrap();
    }

    fn state(&mut self) -> String {
        self.text.to_string()
    }
}

/// Peer A types, peer B's edit arrives, A undoes: only A's text may go.
fn undo_check() -> bool {
    let a = doc(1);
    let b = doc(2);
    let mut undo = UndoManager::new(&a);
    a.get_text("t").insert(0, "local ").unwrap();
    a.commit();
    b.import(&a.export(ExportMode::Snapshot).unwrap()).unwrap();
    let b_text = b.get_text("t");
    b_text.insert(b_text.len_unicode(), "remote").unwrap();
    b.commit();
    a.import(&b.export(ExportMode::updates(&a.oplog_vv())).unwrap())
        .unwrap();
    undo.undo().unwrap();
    a.get_text("t").to_string() == "remote"
}

pub fn features() -> Features {
    Features {
        movable_tree: true,
        history_trimming: "shallow snapshot (`ExportMode::ShallowSnapshot`)",
        collaborative_undo: Some(undo_check()),
    }
}
