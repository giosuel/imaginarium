// SPDX-License-Identifier: AGPL-3.0-or-later

//! Tests of the updates from change sets (`canvas.md` §3): the items an
//! update leaves are the items a build makes, and only touched tiles are
//! drawn again.

use std::time::Instant;

use imaginarium_model::{
    Actor, Document, DocumentKind, Geometry, NodeId, NodeKind, Parent, StyleId, UndoConfig,
    UndoScope, Value,
};
use imaginarium_render::Scene;
use imaginarium_render::kurbo::Rect;
use imaginarium_text::Fonts;
use imaginarium_ui::theme;
use proptest::prelude::*;

use crate::BoardCanvas;
use crate::items::{BoardColors, Items, Look};

const NO_MERGE: UndoConfig = UndoConfig {
    merge_interval_ms: 0,
    max_steps: 1000,
};

#[derive(Clone, Debug)]
enum Op {
    Create {
        kind: u8,
        parent: usize,
        x: i16,
        y: i16,
        label: bool,
    },
    Delete(usize),
    Move {
        node: usize,
        parent: usize,
        front: bool,
    },
    Geometry {
        node: usize,
        x: i16,
        y: i16,
        w: u8,
        h: u8,
        r: i16,
    },
    Hide {
        node: usize,
        hidden: bool,
    },
    Fill {
        node: usize,
        color: u8,
    },
    UseStyle {
        node: usize,
        style: bool,
    },
    StyleFill {
        style: bool,
        color: u8,
    },
    Text {
        node: usize,
        text: u8,
    },
    Undo,
    Redo,
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        3 => (0u8..5, any::<usize>(), -300i16..300, -300i16..300, any::<bool>())
            .prop_map(|(kind, parent, x, y, label)| Op::Create { kind, parent, x, y, label }),
        1 => any::<usize>().prop_map(Op::Delete),
        1 => (any::<usize>(), any::<usize>(), any::<bool>()).prop_map(|(node, parent, front)| Op::Move { node, parent, front }),
        2 => (any::<usize>(), -300i16..300, -300i16..300, 0u8..200, 0u8..200, -180i16..180)
            .prop_map(|(node, x, y, w, h, r)| Op::Geometry { node, x, y, w, h, r }),
        1 => (any::<usize>(), any::<bool>()).prop_map(|(node, hidden)| Op::Hide { node, hidden }),
        1 => (any::<usize>(), 0u8..4).prop_map(|(node, color)| Op::Fill { node, color }),
        1 => (any::<usize>(), any::<bool>()).prop_map(|(node, style)| Op::UseStyle { node, style }),
        1 => (any::<bool>(), 0u8..4).prop_map(|(style, color)| Op::StyleFill { style, color }),
        1 => (any::<usize>(), 0u8..4).prop_map(|(node, text)| Op::Text { node, text }),
        1 => Just(Op::Undo),
        1 => Just(Op::Redo),
    ]
}

const COLORS: [&str; 4] = ["#E0E7FF", "#FEF3C7", "#6366F1", "#00000000"];
const TEXTS: [&str; 4] = ["", "Plan", "A longer label that wraps", "Ship it"];

fn pick(document: &Document, index: usize) -> Option<NodeId> {
    let nodes = document.view().walk();
    (!nodes.is_empty()).then(|| nodes[index % nodes.len()])
}

/// Runs `op` on the board. Edits the model refuses (a move into a
/// descendant) change nothing.
fn run(board: &mut BoardCanvas, styles: &[StyleId; 2], op: &Op) {
    let user = Actor::User;
    let kinds = [
        NodeKind::Shape,
        NodeKind::Text,
        NodeKind::Frame,
        NodeKind::Group,
        NodeKind::Unknown("table".into()),
    ];
    let target = |index: usize| pick(board.document(), index);
    let _ = match op.clone() {
        Op::Create {
            kind,
            parent,
            x,
            y,
            label,
        } => {
            let parent = if parent % 3 == 0 {
                None
            } else {
                target(parent)
            };
            board.transact(&user, |txn| {
                let parent = parent.map_or(Parent::Root, Parent::Node);
                let id = txn.create_node(parent, None, &kinds[usize::from(kind)])?;
                txn.set_geometry(id, Geometry::new(f64::from(x), f64::from(y), 80.0, 40.0))?;
                if label {
                    txn.insert_text(id, 0, "Label")?;
                }
                Ok(())
            })
        }
        Op::Delete(node) => match target(node) {
            Some(node) => board.transact(&user, |txn| txn.delete_node(node)),
            None => Ok(()),
        },
        Op::Move {
            node,
            parent,
            front,
        } => match (target(node), target(parent)) {
            (Some(node), parent) => {
                let parent = if front {
                    Parent::Root
                } else {
                    parent.map_or(Parent::Root, Parent::Node)
                };
                board.transact(&user, |txn| txn.move_node(node, parent, Some(0)))
            }
            _ => Ok(()),
        },
        Op::Geometry {
            node,
            x,
            y,
            w,
            h,
            r,
        } => match target(node) {
            Some(node) => board.transact(&user, |txn| {
                txn.set_geometry(
                    node,
                    Geometry {
                        x: f64::from(x),
                        y: f64::from(y),
                        w: f64::from(w),
                        h: f64::from(h),
                        r: f64::from(r),
                    },
                )
            }),
            None => Ok(()),
        },
        Op::Hide { node, hidden } => match target(node) {
            Some(node) => board.transact(&user, |txn| {
                txn.set_prop(node, "hidden", Value::Bool(hidden))
            }),
            None => Ok(()),
        },
        Op::Fill { node, color } => match target(node) {
            Some(node) => board.transact(&user, |txn| {
                txn.set_style_override(
                    node,
                    "fill",
                    Value::String(COLORS[usize::from(color)].into()),
                )
            }),
            None => Ok(()),
        },
        Op::UseStyle { node, style } => match target(node) {
            Some(node) => board.transact(&user, |txn| {
                txn.set_style(node, Some(&styles[usize::from(style)]))
            }),
            None => Ok(()),
        },
        Op::StyleFill { style, color } => board.transact(&user, |txn| {
            txn.set_style_prop(
                &styles[usize::from(style)],
                "fill",
                Value::String(COLORS[usize::from(color)].into()),
            )
        }),
        Op::Text { node, text } => match target(node) {
            Some(node) => board.transact(&user, |txn| {
                let len = txn.text_len(node)?;
                if len > 0 {
                    txn.delete_text(node, 0, len)?;
                }
                txn.insert_text(node, 0, TEXTS[usize::from(text)])
            }),
            None => Ok(()),
        },
        Op::Undo => board.undo(UndoScope::User, &user).map(|_| ()),
        Op::Redo => board.redo(UndoScope::User, &user).map(|_| ()),
    };
}

/// What an item is, to compare two sets of items: everything but the
/// item id and the exact key.
fn summary(items: &Items) -> Vec<(NodeId, String)> {
    let mut nodes: Vec<(&NodeId, u64)> = items
        .by_node
        .iter()
        .map(|(node, item)| (node, item.z))
        .collect();
    nodes.sort_by_key(|(_, z)| *z);
    nodes
        .into_iter()
        .map(|(node, _)| {
            let item = &items.by_node[node];
            let round = |rect: Rect| {
                format!(
                    "{:.2},{:.2},{:.2},{:.2}",
                    rect.x0, rect.y0, rect.x1, rect.y1
                )
            };
            let look = match &item.look {
                Look::Shape { style, label, .. } => format!(
                    "shape {style:?} {}",
                    label
                        .as_ref()
                        .map_or(String::new(), |label| format!("{:?}", label.block.size()))
                ),
                Look::Text { text, opacity } => format!(
                    "text {opacity} {}",
                    text.as_ref()
                        .map_or(String::new(), |text| format!("{:?}", text.block.size()))
                ),
                Look::Frame { style } => format!("frame {style:?}"),
                Look::Placeholder => "placeholder".into(),
            };
            let clips: Vec<String> = item
                .clips
                .iter()
                .map(|clip| round(clip.world.transform_rect_bbox(clip.rect)))
                .collect();
            (
                *node,
                format!(
                    "{} {look} {:?} {clips:?} {:?}",
                    round(item.bounds),
                    item.size,
                    item.name
                ),
            )
        })
        .collect()
}

fn items_of(board: &mut BoardCanvas, fonts: &mut Fonts) -> Vec<(NodeId, String)> {
    board.sync_for_tests(fonts);
    summary(board.items_for_tests())
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 64, ..ProptestConfig::default() })]

    #[test]
    fn updates_leave_the_items_a_build_makes(ops in prop::collection::vec(op(), 1..40)) {
        let mut fonts = Fonts::bundled_only();
        let mut document = Document::new_with(DocumentKind::Board, NO_MERGE).unwrap();
        let (styles, _) = document
            .transact(&Actor::User, |txn| {
                let base = txn.create_style("Base", None)?;
                let child = txn.create_style("Child", Some(&base))?;
                Ok([base, child])
            })
            .unwrap();
        let mut board = BoardCanvas::new(document);
        items_of(&mut board, &mut fonts);
        for op in &ops {
            run(&mut board, &styles, op);
            // Sometimes several changes wait for one sync.
            if ops.len() % 3 != 0 {
                items_of(&mut board, &mut fonts);
            }
        }
        let updated = items_of(&mut board, &mut fonts);
        let colors = BoardColors::of(&theme());
        let built = summary(&Items::build(board.document().view(), &mut fonts, &colors));
        prop_assert_eq!(updated, built);
    }
}

#[test]
fn a_moved_shape_redraws_only_the_tiles_it_touches() {
    let mut fonts = Fonts::bundled_only();
    let mut board = BoardCanvas::new(crate::demo::grid(400).unwrap());
    let bounds = Rect::new(0.0, 0.0, 1280.0, 1280.0);
    let frame = |board: &mut BoardCanvas, fonts: &mut Fonts| {
        let mut scene = Scene::new();
        board.paint_into(fonts, &mut scene, bounds, Instant::now());
        board.stats().clone()
    };
    frame(&mut board, &mut fonts);
    // Fill the cache (the budget allows a share of the tiles per frame).
    for _ in 0..20 {
        if frame(&mut board, &mut fonts).owed == 0 {
            break;
        }
    }
    let shape = board.document().view().walk()[0];
    board
        .transact(&Actor::User, |txn| {
            txn.set_geometry(shape, Geometry::new(8.0, 8.0, 48.0, 48.0))
        })
        .unwrap();
    let stats = frame(&mut board, &mut fonts);
    assert!(stats.visible >= 25, "{stats:?}");
    // The shape moved within the first tile.
    assert!(stats.rendered <= 1, "{stats:?}");
    // An undo redraws the same tile again, and nothing else.
    assert!(board.undo(UndoScope::User, &Actor::User).unwrap());
    let stats = frame(&mut board, &mut fonts);
    assert!(stats.rendered <= 1, "{stats:?}");
}

#[test]
fn a_style_change_rebuilds_the_nodes_that_use_it() {
    let mut fonts = Fonts::bundled_only();
    let mut document = Document::new(DocumentKind::Board).unwrap();
    let ((style, users, other), _) = document
        .transact(&Actor::User, |txn| {
            let style = txn.create_style("Note", None)?;
            let mut users = Vec::new();
            for index in 0..3 {
                let id = txn.create_node(Parent::Root, None, &NodeKind::Shape)?;
                txn.set_geometry(id, Geometry::new(f64::from(index) * 100.0, 0.0, 80.0, 40.0))?;
                txn.set_style(id, Some(&style))?;
                users.push(id);
            }
            let other = txn.create_node(Parent::Root, None, &NodeKind::Shape)?;
            txn.set_geometry(other, Geometry::new(0.0, 100.0, 80.0, 40.0))?;
            Ok((style, users, other))
        })
        .unwrap();
    let mut board = BoardCanvas::new(document);
    board.sync_for_tests(&mut fonts);
    let (_, changes) = board
        .document_mut_for_tests()
        .transact(&Actor::User, |txn| {
            txn.set_style_prop(&style, "fill", Value::String("#FEF3C7".into()))
        })
        .unwrap();
    let colors = BoardColors::of(&theme());
    let view = board.document().view().clone();
    let update = board
        .items_mut_for_tests()
        .apply(&view, &changes, &mut fonts, &colors);
    let mut upserted = update.upserted.clone();
    upserted.sort();
    let mut expected = users.clone();
    expected.sort();
    assert_eq!(upserted, expected);
    assert!(!update.upserted.contains(&other));
    assert!(update.removed.is_empty());
}
