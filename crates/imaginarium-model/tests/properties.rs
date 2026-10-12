// SPDX-License-Identifier: AGPL-3.0-or-later

//! Property tests of the model's invariants:
//! - **Incremental = rebuild**: after any sequence of local edits, remote
//!   updates, undo and redo, every peer's view equals a view built from its
//!   CRDT state from scratch.
//! - **Convergence**: peers that have exchanged all updates have equal views.
//! - **Undo is an inverse**: undoing an edit that changed something restores
//!   the content from before it.

// Test helpers panic on failure by design.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use imaginarium_model::{
    Actor, Document, DocumentKind, DocumentView, Geometry, NodeId, NodeKind, OpenOptions, Parent,
    UndoConfig, UndoScope, Value,
};
use proptest::prelude::*;
use proptest::sample::Index;

const NO_MERGE: UndoConfig = UndoConfig {
    merge_interval_ms: 0,
    max_steps: 1000,
};

#[derive(Clone, Debug)]
enum Op {
    Create {
        parent: Option<Index>,
        kind: u8,
        ai: bool,
    },
    Delete(Index),
    Move {
        node: Index,
        parent: Option<Index>,
        index: Option<Index>,
    },
    Geometry {
        node: Index,
        x: i16,
        y: i16,
    },
    Prop {
        node: Index,
        key: u8,
        value: Option<i16>,
    },
    InsertText {
        node: Index,
        pos: Index,
        text: String,
    },
    DeleteText {
        node: Index,
        pos: Index,
        len: u8,
    },
    Mark {
        node: Index,
        start: Index,
        len: u8,
        key: u8,
    },
    ParagraphMark {
        node: Index,
        pos: Index,
        level: Option<u8>,
    },
    Style {
        node: Index,
        fill: Option<u8>,
    },
    Undo {
        ai: bool,
    },
    Redo {
        ai: bool,
    },
}

fn op() -> impl Strategy<Value = Op> {
    let index = any::<Index>();
    let text = prop::collection::vec(
        prop::sample::select(vec!["a", "b", "xy", " ", "\n", "é", "😀"]),
        1..4,
    )
    .prop_map(|parts| parts.concat());
    prop_oneof![
        3 => (prop::option::of(index.clone()), 0u8..4, any::<bool>())
            .prop_map(|(parent, kind, ai)| Op::Create { parent, kind, ai }),
        1 => index.clone().prop_map(Op::Delete),
        2 => (index.clone(), prop::option::of(index.clone()), prop::option::of(index.clone()))
            .prop_map(|(node, parent, index)| Op::Move { node, parent, index }),
        2 => (index.clone(), any::<i16>(), any::<i16>()).prop_map(|(node, x, y)| Op::Geometry { node, x, y }),
        1 => (index.clone(), 0u8..3, prop::option::of(any::<i16>())).prop_map(|(node, key, value)| Op::Prop { node, key, value }),
        3 => (index.clone(), index.clone(), text).prop_map(|(node, pos, text)| Op::InsertText { node, pos, text }),
        2 => (index.clone(), index.clone(), 1u8..8).prop_map(|(node, pos, len)| Op::DeleteText { node, pos, len }),
        2 => (index.clone(), index.clone(), 1u8..6, 0u8..3).prop_map(|(node, start, len, key)| Op::Mark { node, start, len, key }),
        1 => (index.clone(), index.clone(), prop::option::of(1u8..4))
            .prop_map(|(node, pos, level)| Op::ParagraphMark { node, pos, level }),
        1 => (index.clone(), prop::option::of(0u8..3)).prop_map(|(node, fill)| Op::Style { node, fill }),
        1 => any::<bool>().prop_map(|ai| Op::Undo { ai }),
        1 => any::<bool>().prop_map(|ai| Op::Redo { ai }),
    ]
}

#[derive(Clone, Debug)]
enum Step {
    Edit { peer: usize, op: Op },
    Sync { from: usize, to: usize },
}

fn step(peers: usize) -> impl Strategy<Value = Step> {
    prop_oneof![
        5 => (0..peers, op()).prop_map(|(peer, op)| Step::Edit { peer, op }),
        1 => (0..peers, 0..peers).prop_map(|(from, to)| Step::Sync { from, to }),
    ]
}

fn pick(nodes: &[NodeId], index: &Index) -> Option<NodeId> {
    (!nodes.is_empty()).then(|| nodes[index.index(nodes.len())])
}

fn text_nodes(view: &DocumentView) -> Vec<NodeId> {
    view.walk()
        .into_iter()
        .filter(|id| view.node(*id).is_some_and(|n| n.text().is_some()))
        .collect()
}

/// Applies one edit. Errors (cycles, out-of-range positions) are part of
/// the test: they must leave the view consistent too.
fn apply(doc: &mut Document, op: &Op) {
    let view = doc.view().clone();
    let nodes = view.walk();
    let user = Actor::User;
    let ai = Actor::Ai("prop".into());
    let _ = match op {
        Op::Create {
            parent,
            kind,
            ai: by_ai,
        } => {
            let parent = parent
                .as_ref()
                .and_then(|i| pick(&nodes, i))
                .map_or(Parent::Root, Parent::Node);
            let kind = [
                NodeKind::Shape,
                NodeKind::Text,
                NodeKind::Group,
                NodeKind::Frame,
            ][usize::from(*kind)]
            .clone();
            doc.transact(if *by_ai { &ai } else { &user }, |t| {
                t.create_node(parent, None, &kind).map(|_| ())
            })
            .map(|_| ())
        }
        Op::Delete(node) => match pick(&nodes, node) {
            Some(id) => doc.transact(&user, |t| t.delete_node(id)).map(|_| ()),
            None => Ok(()),
        },
        Op::Move {
            node,
            parent,
            index,
        } => match pick(&nodes, node) {
            Some(id) => {
                let parent = parent
                    .as_ref()
                    .and_then(|i| pick(&nodes, i))
                    .map_or(Parent::Root, Parent::Node);
                let siblings = view.children(parent).count();
                let index = index.as_ref().map(|i| i.index(siblings + 1));
                doc.transact(&user, |t| t.move_node(id, parent, index))
                    .map(|_| ())
            }
            None => Ok(()),
        },
        Op::Geometry { node, x, y } => match pick(&nodes, node) {
            Some(id) => doc
                .transact(&user, |t| {
                    t.set_geometry(id, Geometry::new(f64::from(*x), f64::from(*y), 10.0, 10.0))
                })
                .map(|_| ()),
            None => Ok(()),
        },
        Op::Prop { node, key, value } => match pick(&nodes, node) {
            Some(id) => {
                let key = ["name", "hidden", "custom"][usize::from(*key)];
                let value = value.map_or(Value::Null, |v| Value::Int(i64::from(v)));
                doc.transact(&user, |t| t.set_prop(id, key, value))
                    .map(|_| ())
            }
            None => Ok(()),
        },
        Op::InsertText { node, pos, text } => match pick(&text_nodes(&view), node) {
            Some(id) => {
                let len = view.node(id).and_then(|n| n.text()).map_or(0, |t| t.len());
                let pos = pos.index(len + 1);
                doc.transact(&user, |t| t.insert_text(id, pos, text))
                    .map(|_| ())
            }
            None => Ok(()),
        },
        Op::DeleteText { node, pos, len } => match pick(&text_nodes(&view), node) {
            Some(id) => {
                let total = view.node(id).and_then(|n| n.text()).map_or(0, |t| t.len());
                if total == 0 {
                    Ok(())
                } else {
                    let pos = pos.index(total);
                    let len = usize::from(*len).min(total - pos);
                    doc.transact(&user, |t| t.delete_text(id, pos, len))
                        .map(|_| ())
                }
            }
            None => Ok(()),
        },
        Op::Mark {
            node,
            start,
            len,
            key,
        } => match pick(&text_nodes(&view), node) {
            Some(id) => {
                let total = view.node(id).and_then(|n| n.text()).map_or(0, |t| t.len());
                if total == 0 {
                    Ok(())
                } else {
                    let start = start.index(total);
                    let end = (start + usize::from(*len)).min(total);
                    let key = ["b", "i", "link"][usize::from(*key)];
                    let value = if key == "link" {
                        Value::from("https://example.org")
                    } else {
                        Value::Bool(true)
                    };
                    doc.transact(&user, |t| t.mark_text(id, start..end, key, value))
                        .map(|_| ())
                }
            }
            None => Ok(()),
        },
        Op::ParagraphMark { node, pos, level } => match pick(&text_nodes(&view), node) {
            Some(id) => {
                let total = view.node(id).and_then(|n| n.text()).map_or(0, |t| t.len());
                if total == 0 {
                    Ok(())
                } else {
                    let value = level.map_or(Value::Null, |l| Value::Int(i64::from(l)));
                    doc.transact(&user, |t| {
                        t.set_paragraph_mark(id, pos.index(total), "h", value)
                    })
                    .map(|_| ())
                }
            }
            None => Ok(()),
        },
        Op::Style { node, fill } => match pick(&nodes, node) {
            Some(id) => {
                let value = fill.map_or(Value::Null, |f| {
                    Value::from(["#ff0000", "#00ff00", "#0000ff"][usize::from(f)])
                });
                doc.transact(&user, |t| t.set_style_override(id, "fill", value))
                    .map(|_| ())
            }
            None => Ok(()),
        },
        Op::Undo { ai: by_ai } => {
            let scope = if *by_ai {
                UndoScope::Assistant
            } else {
                UndoScope::User
            };
            doc.undo(scope, &user).map(|_| ())
        }
        Op::Redo { ai: by_ai } => {
            let scope = if *by_ai {
                UndoScope::Assistant
            } else {
                UndoScope::User
            };
            doc.redo(scope, &user).map(|_| ())
        }
    };
}

fn sync(docs: &mut [Document], from: usize, to: usize) {
    if from == to {
        return;
    }
    let update = docs[from]
        .export_updates_since(&docs[to].version())
        .unwrap();
    docs[to].apply_remote(&[&update]).unwrap();
}

fn consistent(doc: &Document) -> Result<(), TestCaseError> {
    let rebuilt = doc.rebuild_view();
    prop_assert!(
        doc.view() == &rebuilt,
        "incremental view diverged:\nincremental: {:#?}\nrebuilt: {:#?}",
        doc.view(),
        rebuilt
    );
    Ok(())
}

/// The content of a view, without CRDT handles (which undo may change).
fn content(view: &DocumentView) -> Vec<String> {
    let mut out = vec![format!("{:?}", view.meta())];
    for id in view.walk() {
        let node = view.node(id).unwrap();
        out.push(format!(
            "{id} parent={:?} kind={:?} g={:?} s={:?}/{:?} text={:?} a={:?} x={:?} gen={:?} props={:?} children={:?}",
            node.parent(),
            node.kind(),
            node.geometry(),
            node.style(),
            node.style_overrides(),
            node.text(),
            node.assets(),
            node.ext(),
            node.generation(),
            node.props(),
            view.children(Parent::Node(id)).collect::<Vec<_>>(),
        ));
    }
    out
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 96, max_global_rejects: 1_000_000, ..ProptestConfig::default() })]

    #[test]
    fn views_stay_consistent_and_peers_converge(
        peers in 2usize..4,
        steps in prop::collection::vec(step(3), 1..60),
    ) {
        let first = Document::new_with(DocumentKind::Board, NO_MERGE).unwrap();
        let snapshot = first.export_snapshot().unwrap();
        let mut docs = vec![first];
        for _ in 1..peers {
            docs.push(Document::open_with(&[&snapshot], &OpenOptions { undo: NO_MERGE, ..OpenOptions::default() }).unwrap());
        }
        for step in &steps {
            match step {
                Step::Edit { peer, op } => {
                    let peer = peer % peers;
                    apply(&mut docs[peer], op);
                    consistent(&docs[peer])?;
                }
                Step::Sync { from, to } => {
                    let (from, to) = (from % peers, to % peers);
                    sync(&mut docs, from, to);
                    consistent(&docs[to])?;
                }
            }
        }
        // Everyone exchanges everything (twice, so updates travel through).
        for _ in 0..2 {
            for from in 0..peers {
                for to in 0..peers {
                    sync(&mut docs, from, to);
                }
            }
        }
        for doc in &docs {
            consistent(doc)?;
            prop_assert_eq!(content(doc.view()), content(docs[0].view()));
        }
    }

    #[test]
    fn undo_restores_the_content_before_an_edit(
        setup in prop::collection::vec(op(), 0..25),
        edit in op().prop_filter("a plain user edit", |op| {
            !matches!(op, Op::Undo { .. } | Op::Redo { .. } | Op::Create { ai: true, .. })
        }),
    ) {
        let mut doc = Document::new_with(DocumentKind::Board, NO_MERGE).unwrap();
        // Something to edit: a text, and a frame with a shape.
        doc.transact(&Actor::User, |t| {
            let text = t.create_node(Parent::Root, None, &NodeKind::Text)?;
            t.insert_text(text, 0, "hello\nworld")?;
            let frame = t.create_node(Parent::Root, None, &NodeKind::Frame)?;
            t.create_node(Parent::Node(frame), None, &NodeKind::Shape)?;
            Ok(())
        })
        .unwrap();
        for op in &setup {
            if !matches!(op, Op::Undo { .. } | Op::Redo { .. }) {
                apply(&mut doc, op);
            }
        }
        let before = content(doc.view());
        let version = doc.view().version();
        apply(&mut doc, &edit);
        prop_assume!(content(doc.view()) != before && doc.view().version() != version);
        doc.undo(UndoScope::User, &Actor::User).unwrap();
        consistent(&doc)?;
        prop_assert_eq!(content(doc.view()), before);
    }
}
