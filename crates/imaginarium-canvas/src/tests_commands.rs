// SPDX-License-Identifier: AGPL-3.0-or-later

//! Tests of the board's commands and their shortcuts (M3.3), through the
//! widget's keys.

use imaginarium_commands::Registry;
use imaginarium_model::{NodeId, NodeKind, Parent};
use imaginarium_render::kurbo::Rect;
use imaginarium_ui::{Input, Key, Modifiers};

use crate::BoardCanvas;
use crate::tests_select::{Harness, sample};

/// A key with Ctrl (Linux's Mod), and Shift or Alt if asked.
fn chord(board: &mut Harness, key: Key, shift: bool, alt: bool) {
    let modifiers = Modifiers {
        ctrl: true,
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
    chord(&mut board, Key::Character("]".into()), false, true);
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
