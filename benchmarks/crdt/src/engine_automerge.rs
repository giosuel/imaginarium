// SPDX-License-Identifier: AGPL-3.0-or-later

//! Automerge: a map of nodes with parent and order fields, since Automerge
//! has no tree type. Each operation group is committed as its own change.

use std::collections::HashMap;

use automerge::marks::{ExpandMark, Mark};
use automerge::transaction::Transactable;
use automerge::{
    ActorId, AutoCommit, ChangeHash, ObjId, ObjType, ROOT, ReadDoc, ScalarValue, Value,
};
use imaginarium_test_utils::fixtures::{ParagraphRole, ParagraphSpec, ShapeSpec};

use crate::engine::{Board, Features, TextDoc, kind_name, links_have_cycle, node_key, order_key};

fn doc(peer: u64) -> AutoCommit {
    AutoCommit::new().with_actor(ActorId::from(peer.to_be_bytes().as_slice()))
}

fn object(doc: &AutoCommit, parent: &ObjId, key: &str) -> ObjId {
    match doc.get(parent, key).unwrap() {
        Some((Value::Object(_), id)) => id,
        other => panic!("expected an object at {key}, got {other:?}"),
    }
}

fn number(doc: &AutoCommit, obj: &ObjId, key: &str) -> f64 {
    match doc.get(obj, key).unwrap() {
        Some((Value::Scalar(value), _)) => match value.as_ref() {
            ScalarValue::F64(v) => *v,
            ScalarValue::Int(v) => *v as f64,
            other => panic!("expected a number, got {other:?}"),
        },
        other => panic!("expected a number, got {other:?}"),
    }
}

pub struct AutomergeBoard {
    doc: AutoCommit,
    nodes: ObjId,
    keys: Vec<String>,
    node_objs: Vec<ObjId>,
    geometry: Vec<ObjId>,
}

impl AutomergeBoard {
    fn insert(&mut self, shape: &ShapeSpec) {
        let index = self.keys.len();
        let key = node_key(index);
        let doc = &mut self.doc;
        let node = doc
            .put_object(&self.nodes, key.as_str(), ObjType::Map)
            .unwrap();
        doc.put(&node, "k", kind_name(shape.kind)).unwrap();
        doc.put(&node, "p", "root").unwrap();
        doc.put(&node, "z", order_key(index)).unwrap();
        let g = doc.put_object(&node, "g", ObjType::Map).unwrap();
        doc.put(&g, "x", shape.x).unwrap();
        doc.put(&g, "y", shape.y).unwrap();
        doc.put(&g, "w", shape.width).unwrap();
        doc.put(&g, "h", shape.height).unwrap();
        doc.put(&g, "r", shape.rotation_deg).unwrap();
        doc.put(&node, "f", i64::from(shape.fill_rgb)).unwrap();
        if !shape.label.is_empty() {
            let text = doc.put_object(&node, "t", ObjType::Text).unwrap();
            doc.splice_text(&text, 0, 0, &shape.label).unwrap();
        }
        self.keys.push(key);
        self.node_objs.push(node);
        self.geometry.push(g);
    }
}

impl Board for AutomergeBoard {
    type Version = Vec<ChangeHash>;
    type State = automerge::hydrate::Value;

    fn create(peer: u64, shapes: &[ShapeSpec]) -> Self {
        let mut doc = doc(peer);
        let nodes = doc.put_object(ROOT, "nodes", ObjType::Map).unwrap();
        let mut board = Self {
            doc,
            nodes,
            keys: Vec::with_capacity(shapes.len()),
            node_objs: Vec::with_capacity(shapes.len()),
            geometry: Vec::with_capacity(shapes.len()),
        };
        for shape in shapes {
            board.insert(shape);
        }
        board.doc.commit();
        board
    }

    fn load(peer: u64, bytes: &[u8]) -> Self {
        let doc = AutoCommit::load(bytes)
            .unwrap()
            .with_actor(ActorId::from(peer.to_be_bytes().as_slice()));
        let nodes = object(&doc, &ROOT, "nodes");
        let mut keys: Vec<String> = doc.keys(&nodes).collect();
        keys.sort_unstable();
        let node_objs: Vec<ObjId> = keys.iter().map(|key| object(&doc, &nodes, key)).collect();
        let geometry = node_objs
            .iter()
            .map(|node| object(&doc, node, "g"))
            .collect();
        Self {
            doc,
            nodes,
            keys,
            node_objs,
            geometry,
        }
    }

    fn save(&mut self) -> Vec<u8> {
        self.doc.save()
    }

    fn save_trimmed(&mut self) -> Option<Vec<u8>> {
        // Automerge always keeps the complete history.
        None
    }

    fn len(&self) -> usize {
        self.keys.len()
    }

    fn read_geometry(&self) -> f64 {
        self.geometry
            .iter()
            .map(|g| {
                number(&self.doc, g, "x")
                    + number(&self.doc, g, "y")
                    + number(&self.doc, g, "w")
                    + number(&self.doc, g, "h")
            })
            .sum()
    }

    fn move_shape(&mut self, index: usize, x: f64, y: f64) {
        let g = &self.geometry[index];
        self.doc.put(g, "x", x).unwrap();
        self.doc.put(g, "y", y).unwrap();
        self.doc.commit();
    }

    fn add_shape(&mut self, shape: &ShapeSpec) {
        self.insert(shape);
        self.doc.commit();
    }

    fn reparent(&mut self, child: usize, parent: usize) {
        let parent_key = self.keys[parent].clone();
        self.doc
            .put(&self.node_objs[child], "p", parent_key)
            .unwrap();
        self.doc.commit();
    }

    fn has_cycle(&mut self) -> bool {
        let index: HashMap<&str, usize> = self
            .keys
            .iter()
            .enumerate()
            .map(|(i, k)| (k.as_str(), i))
            .collect();
        let parents: Vec<Option<usize>> = self
            .node_objs
            .iter()
            .map(|node| match self.doc.get(node, "p").unwrap() {
                Some((Value::Scalar(value), _)) => match value.as_ref() {
                    ScalarValue::Str(p) => index.get(p.as_str()).copied(),
                    _ => None,
                },
                _ => None,
            })
            .collect();
        links_have_cycle(&parents)
    }

    fn version(&mut self) -> Vec<ChangeHash> {
        self.doc.get_heads()
    }

    fn updates_since(&mut self, version: &Vec<ChangeHash>) -> Vec<u8> {
        self.doc.save_after(version)
    }

    fn apply(&mut self, update: &[u8]) {
        self.doc.load_incremental(update).unwrap();
    }

    fn state(&mut self) -> automerge::hydrate::Value {
        self.doc.hydrate(&ROOT, None).unwrap()
    }
}

pub struct AutomergeTextDoc {
    doc: AutoCommit,
    text: ObjId,
}

impl TextDoc for AutomergeTextDoc {
    type Version = Vec<ChangeHash>;
    type State = String;

    fn create(peer: u64, paragraphs: &[ParagraphSpec]) -> Self {
        let mut doc = doc(peer);
        let text = doc.put_object(ROOT, "body", ObjType::Text).unwrap();
        let mut pos = 0;
        for paragraph in paragraphs {
            let len = paragraph.text.chars().count();
            doc.splice_text(&text, pos, 0, &paragraph.text).unwrap();
            let mark = match paragraph.role {
                ParagraphRole::Heading1 => Some(Mark::new("h".into(), 1i64, pos, pos + len)),
                ParagraphRole::Heading2 => Some(Mark::new("h".into(), 2i64, pos, pos + len)),
                ParagraphRole::ListItem => Some(Mark::new("li".into(), true, pos, pos + len)),
                ParagraphRole::Body => None,
            };
            if let Some(mark) = mark {
                doc.mark(&text, mark, ExpandMark::None).unwrap();
            }
            pos += len;
            doc.splice_text(&text, pos, 0, "\n").unwrap();
            pos += 1;
        }
        doc.commit();
        Self { doc, text }
    }

    fn load(peer: u64, bytes: &[u8]) -> Self {
        let doc = AutoCommit::load(bytes)
            .unwrap()
            .with_actor(ActorId::from(peer.to_be_bytes().as_slice()));
        let text = object(&doc, &ROOT, "body");
        Self { doc, text }
    }

    fn save(&mut self) -> Vec<u8> {
        self.doc.save()
    }

    fn save_trimmed(&mut self) -> Option<Vec<u8>> {
        None
    }

    fn len(&self) -> usize {
        self.doc.length(&self.text)
    }

    fn text(&self) -> String {
        self.doc.text(&self.text).unwrap()
    }

    fn insert(&mut self, pos: usize, s: &str) {
        self.doc.splice_text(&self.text, pos, 0, s).unwrap();
        self.doc.commit();
    }

    fn version(&mut self) -> Vec<ChangeHash> {
        self.doc.get_heads()
    }

    fn updates_since(&mut self, version: &Vec<ChangeHash>) -> Vec<u8> {
        self.doc.save_after(version)
    }

    fn apply(&mut self, update: &[u8]) {
        self.doc.load_incremental(update).unwrap();
    }

    fn state(&mut self) -> String {
        self.doc.text(&self.text).unwrap()
    }
}

pub fn features() -> Features {
    Features {
        movable_tree: false,
        history_trimming: "none (the complete history is always kept)",
        // 0.12 can roll back an uncommitted transaction, but has no undo
        // manager for committed changes.
        collaborative_undo: None,
    }
}
