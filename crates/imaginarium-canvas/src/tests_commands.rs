// SPDX-License-Identifier: AGPL-3.0-or-later

//! Tests of the board's commands and their shortcuts (M3.3), through the
//! widget's keys.

use imaginarium_commands::Registry;
use imaginarium_model::{NodeId, NodeKind, Parent};
use imaginarium_render::kurbo::Rect;
use imaginarium_ui::{Input, Key, Modifiers};

use crate::BoardCanvas;
use crate::tests_select::{Harness, sample};

/// A key with the platform's Mod (Ctrl, ⌘ on macOS), and Shift or Alt
/// if asked.
fn chord(board: &mut Harness, key: Key, shift: bool, alt: bool) {
    let mac = cfg!(target_os = "macos");
    let modifiers = Modifiers {
        ctrl: !mac,
        meta: mac,
        shift,
        alt,
        ..Modifiers::default()
    };
    board.ui.input(
        &mut board.fonts,
        &Input::Key {
            key,
            text: None,
            pressed: true,
            repeat: false,
            modifiers,
        },
    );
    board.frame();
}

fn ctrl(board: &mut Harness, character: &str) {
    chord(board, Key::Character(character.into()), false, false);
}

fn node_box(board: &Harness, node: NodeId) -> Rect {
    let board = board.board();
    let items = board.items_for_tests();
    crate::hit::node_box(items, board.document().view(), node).unwrap()
}

fn close(a: Rect, b: Rect) -> bool {
    (a.x0 - b.x0).abs() < 1e-6
        && (a.y0 - b.y0).abs() < 1e-6
        && (a.x1 - b.x1).abs() < 1e-6
        && (a.y1 - b.y1).abs() < 1e-6
}

#[test]
fn every_board_command_registers() {
    let mut registry: Registry<BoardCanvas> = Registry::new();
    for command in crate::commands::board_commands() {
        registry.register(*command).unwrap();
    }
    assert_eq!(registry.len(), crate::commands::board_commands().len());
}

#[test]
fn delete_and_undo_through_the_keys() {
    let mut board = sample();
    let interview = board.node("Interview 5 users");
    board.click((300.0, 300.0));
    board.key(Key::Delete, false);
    assert!(board.board().document().view().node(interview).is_none());
    assert!(board.selection().is_empty());
    ctrl(&mut board, "z");
    assert!(board.board().document().view().node(interview).is_some());
    chord(&mut board, Key::Character("Z".into()), true, false);
    assert!(
        board.board().document().view().node(interview).is_none(),
        "redo"
    );
}

#[test]
fn duplicate_copies_down_and_right_with_text_and_children() {
    let mut board = sample();
    let research = board.node("Research");
    let before = board.board().document().view().node_count();
    // The frame by its name: it and its five children.
    board.click((250.0, 185.0));
    ctrl(&mut board, "d");
    let view = board.board().document().view();
    assert_eq!(view.node_count(), before + 6);
    let [copy] = board.selection()[..] else {
        panic!("one copy selected")
    };
    assert_ne!(copy, research);
    let (a, b) = (node_box(&board, research), node_box(&board, copy));
    assert!(
        close(b, a + imaginarium_render::kurbo::Vec2::new(16.0, 16.0)),
        "{a:?} {b:?}"
    );
    let texts: Vec<String> = view
        .children(Parent::Node(copy))
        .filter_map(|child| view.node(child)?.text().map(|text| text.to_plain_string()))
        .collect();
    assert!(
        texts.contains(&"Interview 5 users\n".to_owned()),
        "{texts:?}"
    );
}

#[test]
fn select_all_takes_every_unit() {
    let mut board = sample();
    board.click((20.0, 800.0));
    ctrl(&mut board, "a");
    // Every one of the 15 nodes: a frame's children are selectable on
    // their own (only groups take theirs along).
    assert_eq!(board.selection().len(), 15);
}

#[test]
fn order_commands_move_in_the_stack() {
    let mut board = sample();
    let interview = board.node("Interview 5 users");
    board.click((300.0, 300.0));
    let order = |board: &Harness| {
        let view = board.board().document().view();
        let frame = view.node(interview).unwrap().parent();
        view.children(frame).collect::<Vec<_>>()
    };
    assert_eq!(order(&board).first(), Some(&interview));
    ctrl(&mut board, "]");
    assert_eq!(order(&board).last(), Some(&interview));
    ctrl(&mut board, "[");
    assert_eq!(order(&board).first(), Some(&interview));
    chord(&mut board, Key::ArrowUp, false, false);
    assert_eq!(order(&board)[1], interview, "one step forward");
}

#[test]
fn group_and_ungroup_keep_everything_in_place() {
    let mut board = sample();
    let (interview, scan) = (
        board.node("Interview 5 users"),
        board.node("Competitor scan"),
    );
    let (a, b) = (node_box(&board, interview), node_box(&board, scan));
    board.click((300.0, 300.0));
    board.shift_click((600.0, 300.0));
    ctrl(&mut board, "g");
    let [group] = board.selection()[..] else {
        panic!("the group selected")
    };
    let view = board.board().document().view();
    assert!(matches!(view.node(group).unwrap().kind(), NodeKind::Group));
    assert_eq!(view.node(interview).unwrap().parent(), Parent::Node(group));
    assert!(close(node_box(&board, interview), a) && close(node_box(&board, scan), b));
    // A click selects the group as a whole now.
    board.click((20.0, 800.0));
    board.click((300.0, 300.0));
    assert_eq!(board.selection(), vec![group]);
    chord(&mut board, Key::Character("G".into()), true, false);
    let view = board.board().document().view();
    assert!(view.node(group).is_none());
    assert_eq!(board.selection().len(), 2);
    assert!(close(node_box(&board, interview), a) && close(node_box(&board, scan), b));
}

#[test]
fn locked_nodes_cant_be_clicked_until_unlocked() {
    let mut board = sample();
    let interview = board.node("Interview 5 users");
    board.click((300.0, 300.0));
    chord(&mut board, Key::Character("L".into()), true, false);
    assert!(
        board
            .board()
            .document()
            .view()
            .node(interview)
            .unwrap()
            .locked()
    );
    board.click((20.0, 800.0));
    board.click((300.0, 300.0));
    assert!(board.selection().is_empty());
}

#[test]
fn zoom_commands_change_the_view() {
    let mut board = sample();
    board.click((20.0, 800.0));
    ctrl(&mut board, "=");
    assert!((board.board().zoom() - 1.25).abs() < 1e-9);
    ctrl(&mut board, "-");
    assert!((board.board().zoom() - 1.0).abs() < 1e-9);
    ctrl(&mut board, "=");
    ctrl(&mut board, "0");
    assert!((board.board().zoom() - 1.0).abs() < 1e-9);
    // Fit: everything in view, at most 100 %.
    ctrl(&mut board, "1");
    let zoom = board.board().zoom();
    assert!(zoom <= 1.0 && zoom > 0.5, "{zoom}");
}

/// A board with shapes A, B, C, D side by side, back to front.
fn four() -> (Harness, Vec<NodeId>) {
    let mut document =
        imaginarium_model::Document::new(imaginarium_model::DocumentKind::Board).unwrap();
    let (nodes, _) = document
        .transact(&imaginarium_model::Actor::User, |txn| {
            (0..4)
                .map(|index| {
                    let id = txn.create_node(Parent::Root, None, &NodeKind::Shape)?;
                    txn.set_geometry(
                        id,
                        imaginarium_model::Geometry::new(
                            100.0 + f64::from(index) * 150.0,
                            100.0,
                            100.0,
                            100.0,
                        ),
                    )?;
                    txn.set_style_override(
                        id,
                        "fill",
                        imaginarium_model::Value::String("#E0E7FF".into()),
                    )?;
                    Ok(id)
                })
                .collect::<imaginarium_model::Result<Vec<_>>>()
        })
        .unwrap();
    (Harness::new(document), nodes)
}

fn root_order(board: &Harness) -> Vec<NodeId> {
    board
        .board()
        .document()
        .view()
        .children(Parent::Root)
        .collect()
}

#[test]
fn reordering_keeps_the_selected_nodes_in_their_order() {
    let (mut board, n) = four();
    let (a, b, c, d) = (n[0], n[1], n[2], n[3]);
    // A and C to the front: C stays above A.
    board.click((150.0, 150.0));
    board.shift_click((450.0, 150.0));
    ctrl(&mut board, "]");
    assert_eq!(root_order(&board), vec![b, d, a, c]);
    ctrl(&mut board, "[");
    assert_eq!(root_order(&board), vec![a, c, b, d]);
    // B and C one step forward: B doesn't pass C.
    let (mut board, n) = four();
    board.click((300.0, 150.0));
    board.shift_click((450.0, 150.0));
    chord(&mut board, Key::ArrowUp, false, false);
    assert_eq!(root_order(&board), vec![n[0], n[3], n[1], n[2]]);
}

#[test]
fn ungrouping_two_groups_keeps_the_order() {
    let (mut board, n) = four();
    let (a, b, c, d) = (n[0], n[1], n[2], n[3]);
    board.click((150.0, 150.0));
    board.shift_click((300.0, 150.0));
    ctrl(&mut board, "g");
    let [first] = board.selection()[..] else {
        panic!()
    };
    board.click((450.0, 150.0));
    board.shift_click((600.0, 150.0));
    ctrl(&mut board, "g");
    let [second] = board.selection()[..] else {
        panic!()
    };
    // Something between them.
    let id = board.board;
    let x = board
        .ui
        .widget_mut::<BoardCanvas>(id)
        .unwrap()
        .transact(&imaginarium_model::Actor::User, |txn| {
            let x = txn.create_node(Parent::Root, Some(1), &NodeKind::Shape)?;
            txn.set_geometry(
                x,
                imaginarium_model::Geometry::new(100.0, 400.0, 50.0, 50.0),
            )?;
            Ok(x)
        })
        .unwrap();
    board.frame();
    board
        .ui
        .widget_mut::<BoardCanvas>(id)
        .unwrap()
        .select(&[first, second]);
    board.frame();
    chord(&mut board, Key::Character("G".into()), true, false);
    assert_eq!(root_order(&board), vec![a, b, x, c, d]);
}

#[test]
fn unlock_without_a_selection_unlocks_every_locked_node() {
    let (mut board, n) = four();
    board.click((150.0, 150.0));
    chord(&mut board, Key::Character("L".into()), true, false);
    board.click((20.0, 800.0));
    let id = board.board;
    board
        .ui
        .widget_mut::<BoardCanvas>(id)
        .unwrap()
        .execute("board.unlock", &imaginarium_commands::Args::new())
        .unwrap();
    board.frame();
    assert!(!board.board().document().view().node(n[0]).unwrap().locked());
    assert_eq!(board.selection(), vec![n[0]]);
}

#[test]
fn copies_keep_marks_on_line_breaks_and_empty_paragraphs() {
    use crate::copy::NodeCopy;
    use imaginarium_model::{Actor, Document, DocumentKind, Value};
    let mut document = Document::new(DocumentKind::Board).unwrap();
    let ((bold, empty), _) = document
        .transact(&Actor::User, |txn| {
            let bold = txn.create_node(Parent::Root, None, &NodeKind::Text)?;
            txn.insert_text(bold, 0, "ab\n\ncd")?;
            txn.mark_text(bold, 0..7, "b", Value::Bool(true))?;
            let empty = txn.create_node(Parent::Root, None, &NodeKind::Text)?;
            txn.insert_text(empty, 0, "\n")?;
            Ok((bold, empty))
        })
        .unwrap();
    for node in [bold, empty] {
        let original = NodeCopy::read(document.view(), node).unwrap();
        let (copy, _) = document
            .transact(&Actor::User, |txn| {
                original.write(txn, Parent::Root, None, (0.0, 0.0))
            })
            .unwrap();
        assert_eq!(NodeCopy::read(document.view(), copy).unwrap(), original);
    }
}
