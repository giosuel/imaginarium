// SPDX-License-Identifier: AGPL-3.0-or-later

//! Yrs (the Rust port of Yjs): a map of nodes with parent and order fields,
//! since Yrs has no tree type. Updates use the more compact v2 encoding.

use std::collections::HashMap;
use std::sync::Arc;

use imaginarium_test_utils::fixtures::{ParagraphRole, ParagraphSpec, ShapeSpec};
use yrs::types::{Attrs, ToJson};
use yrs::undo::UndoManager;
use yrs::updates::decoder::Decode;
use yrs::{
    Any, Doc, GetString, Map, MapPrelim, MapRef, Number, Out, ReadTxn, StateVector, Text,
    TextPrelim, TextRef, Transact, Update,
};

use crate::engine::{Board, Features, TextDoc, kind_name, links_have_cycle, node_key, order_key};

fn number(value: Option<Out>) -> f64 {
    match value {
        Some(Out::Any(Any::Number(v))) => v.as_f64().unwrap(),
        other => panic!("expected a number, got {other:?}"),
    }
}

fn float(value: f64) -> Any {
    Any::Number(Number::Float(value))
}

fn map(value: Option<Out>) -> MapRef {
    match value {
        Some(Out::YMap(map)) => map,
        other => panic!("expected a map, got {other:?}"),
    }
}

fn apply_v2(doc: &Doc, update: &[u8]) {
    let mut txn = doc.transact_mut_with("remote");
    txn.apply_update(Update::decode_v2(update).unwrap())
        .unwrap();
}

pub struct YrsBoard {
    doc: Doc,
    last_update: Option<Vec<u8>>,
    nodes: MapRef,
    keys: Vec<String>,
    node_maps: Vec<MapRef>,
    geometry: Vec<MapRef>,
}

impl YrsBoard {
    fn insert(&mut self, txn: &mut yrs::TransactionMut, shape: &ShapeSpec) {
        let index = self.keys.len();
        let key = node_key(index);
        let node = self.nodes.insert(
            txn,
            key.as_str(),
            MapPrelim::from_iter(Vec::<(&str, Any)>::new()),
        );
        node.insert(txn, "k", kind_name(shape.kind));
        node.insert(txn, "p", "root");
        node.insert(txn, "z", order_key(index));
        let g = node.insert(
            txn,
            "g",
            MapPrelim::from_iter([
                ("x", float(shape.x)),
                ("y", float(shape.y)),
                ("w", float(shape.width)),
                ("h", float(shape.height)),
                ("r", float(shape.rotation_deg)),
            ]),
        );
        node.insert(txn, "f", float(f64::from(shape.fill_rgb)));
        if !shape.label.is_empty() {
            node.insert(txn, "t", TextPrelim::new(shape.label.as_str()));
        }
        self.keys.push(key);
        self.node_maps.push(node);
        self.geometry.push(g);
    }
}

impl Board for YrsBoard {
    type Version = StateVector;
    type State = Any;

    fn create(peer: u64, shapes: &[ShapeSpec]) -> Self {
        let doc = Doc::with_client_id(peer);
        let nodes = doc.get_or_insert_map("nodes");
        let mut board = Self {
            doc,
            last_update: None,
            nodes,
            keys: Vec::with_capacity(shapes.len()),
            node_maps: Vec::with_capacity(shapes.len()),
            geometry: Vec::with_capacity(shapes.len()),
        };
        let doc = board.doc.clone();
        let mut txn = doc.transact_mut();
        for shape in shapes {
            board.insert(&mut txn, shape);
        }
        drop(txn);
        board
    }

    fn load(peer: u64, bytes: &[u8]) -> Self {
        let doc = Doc::with_client_id(peer);
        let nodes = doc.get_or_insert_map("nodes");
        apply_v2(&doc, bytes);
        let txn = doc.transact();
        let mut keys: Vec<String> = nodes.keys(&txn).map(str::to_owned).collect();
        keys.sort_unstable();
        let node_maps: Vec<MapRef> = keys.iter().map(|key| map(nodes.get(&txn, key))).collect();
        let geometry = node_maps
            .iter()
            .map(|node| map(node.get(&txn, "g")))
            .collect();
        drop(txn);
        Self {
            doc,
            last_update: None,
            nodes,
            keys,
            node_maps,
            geometry,
        }
    }

    fn save(&mut self) -> Vec<u8> {
        self.doc
            .transact()
            .encode_state_as_update_v2(&StateVector::default())
    }

    fn save_trimmed(&mut self) -> Option<Vec<u8>> {
        // Yrs garbage-collects deleted content by default, but it has no way
        // to drop the history of the remaining items.
        None
    }

    fn len(&self) -> usize {
        self.keys.len()
    }

    fn read_geometry(&self) -> f64 {
        let txn = self.doc.transact();
        self.geometry
            .iter()
            .map(|g| {
                number(g.get(&txn, "x"))
                    + number(g.get(&txn, "y"))
                    + number(g.get(&txn, "w"))
                    + number(g.get(&txn, "h"))
            })
            .sum()
    }

    fn move_shape(&mut self, index: usize, x: f64, y: f64) {
        let mut txn = self.doc.transact_mut_with("local");
        let g = &self.geometry[index];
        g.insert(&mut txn, "x", float(x));
        g.insert(&mut txn, "y", float(y));
        self.last_update = Some(txn.encode_update_v2());
    }

    fn add_shape(&mut self, shape: &ShapeSpec) {
        let doc = self.doc.clone();
        let mut txn = doc.transact_mut_with("local");
        self.insert(&mut txn, shape);
        self.last_update = Some(txn.encode_update_v2());
    }

    fn reparent(&mut self, child: usize, parent: usize) {
        let mut txn = self.doc.transact_mut_with("local");
        let parent_key = self.keys[parent].clone();
        self.node_maps[child].insert(&mut txn, "p", parent_key);
        self.last_update = Some(txn.encode_update_v2());
    }

    fn has_cycle(&mut self) -> bool {
        let txn = self.doc.transact();
        let index: HashMap<&str, usize> = self
            .keys
            .iter()
            .enumerate()
            .map(|(i, k)| (k.as_str(), i))
            .collect();
        let parents: Vec<Option<usize>> = self
            .node_maps
            .iter()
            .map(|node| match node.get(&txn, "p") {
                Some(Out::Any(Any::String(p))) => index.get(&*p).copied(),
                _ => None,
            })
            .collect();
        links_have_cycle(&parents)
    }

    fn version(&mut self) -> StateVector {
        self.doc.transact().state_vector()
    }

    fn updates_since(&mut self, version: &StateVector) -> Vec<u8> {
        self.doc.transact().encode_state_as_update_v2(version)
    }

    fn edit_update(&mut self, version: &StateVector) -> Vec<u8> {
        self.last_update
            .take()
            .unwrap_or_else(|| self.updates_since(version))
    }

    fn apply(&mut self, update: &[u8]) {
        apply_v2(&self.doc, update);
    }

    fn state(&mut self) -> Any {
        self.nodes.to_json(&self.doc.transact())
    }
}

pub struct YrsTextDoc {
    doc: Doc,
    last_update: Option<Vec<u8>>,
    text: TextRef,
}

fn attrs(key: &str, value: Any) -> Attrs {
    HashMap::from([(Arc::<str>::from(key), value)])
}

impl TextDoc for YrsTextDoc {
    type Version = StateVector;
    type State = String;

    fn create(peer: u64, paragraphs: &[ParagraphSpec]) -> Self {
        let doc = Doc::with_client_id(peer);
        let text = doc.get_or_insert_text("body");
        {
            let mut txn = doc.transact_mut();
            let mut pos = 0u32;
            for paragraph in paragraphs {
                let len = paragraph.text.chars().count() as u32;
                let marks = match paragraph.role {
                    ParagraphRole::Heading1 => Some(attrs("h", float(1.0))),
                    ParagraphRole::Heading2 => Some(attrs("h", float(2.0))),
                    ParagraphRole::ListItem => Some(attrs("li", Any::Bool(true))),
                    ParagraphRole::Body => None,
                };
                match marks {
                    Some(marks) => {
                        text.insert_with_attributes(&mut txn, pos, &paragraph.text, marks)
                    }
                    None => text.insert(&mut txn, pos, &paragraph.text),
                }
                pos += len;
                text.insert(&mut txn, pos, "\n");
                pos += 1;
            }
        }
        Self {
            doc,
            last_update: None,
            text,
        }
    }

    fn load(peer: u64, bytes: &[u8]) -> Self {
        let doc = Doc::with_client_id(peer);
        let text = doc.get_or_insert_text("body");
        apply_v2(&doc, bytes);
        Self {
            doc,
            last_update: None,
            text,
        }
    }

    fn save(&mut self) -> Vec<u8> {
        self.doc
            .transact()
            .encode_state_as_update_v2(&StateVector::default())
    }

    fn save_trimmed(&mut self) -> Option<Vec<u8>> {
        None
    }

    fn len(&self) -> usize {
        self.text.len(&self.doc.transact()) as usize
    }

    fn text(&self) -> String {
        self.text.get_string(&self.doc.transact())
    }

    fn insert(&mut self, pos: usize, s: &str) {
        let mut txn = self.doc.transact_mut_with("local");
        self.text.insert(&mut txn, pos as u32, s);
        self.last_update = Some(txn.encode_update_v2());
    }

    fn version(&mut self) -> StateVector {
        self.doc.transact().state_vector()
    }

    fn updates_since(&mut self, version: &StateVector) -> Vec<u8> {
        self.doc.transact().encode_state_as_update_v2(version)
    }

    fn edit_update(&mut self, version: &StateVector) -> Vec<u8> {
        self.last_update
            .take()
            .unwrap_or_else(|| self.updates_since(version))
    }

    fn apply(&mut self, update: &[u8]) {
        apply_v2(&self.doc, update);
    }

    fn state(&mut self) -> String {
        self.text.get_string(&self.doc.transact())
    }
}

/// Peer A types, peer B's edit arrives, A undoes: only A's text may go.
fn undo_check() -> bool {
    let a = Doc::with_client_id(1);
    let b = Doc::with_client_id(2);
    let a_text = a.get_or_insert_text("t");
    let b_text = b.get_or_insert_text("t");
    let mut undo: UndoManager<()> = UndoManager::new();
    undo.expand_scope(&a, &a_text);
    undo.include_origin("local");
    {
        let mut txn = a.transact_mut_with("local");
        a_text.insert(&mut txn, 0, "local ");
    }
    apply_v2(
        &b,
        &a.transact()
            .encode_state_as_update_v2(&StateVector::default()),
    );
    {
        let mut txn = b.transact_mut_with("local");
        let len = b_text.len(&txn);
        b_text.insert(&mut txn, len, "remote");
    }
    let since = a.transact().state_vector();
    apply_v2(&a, &b.transact().encode_state_as_update_v2(&since));
    undo.undo_blocking();
    a_text.get_string(&a.transact()) == "remote"
}

pub fn features() -> Features {
    Features {
        movable_tree: false,
        history_trimming: "none (deleted content is garbage-collected, history of live items stays)",
        collaborative_undo: Some(undo_check()),
    }
}
