// SPDX-License-Identifier: AGPL-3.0-or-later

//! Tests of the tools and inserting (M3.3) through the widget.

use imaginarium_model::{Actor, Geometry, NodeId, NodeKind, Parent, UndoScope, Value};
use imaginarium_render::kurbo::Point;
use imaginarium_ui::{Input, Key, Modifiers};

use crate::tests_select::{Harness, sample};
use crate::{BoardCanvas, Preset, Tool};

fn press_key(board: &mut Harness, character: &str, shift: bool) {
    board.ui.input(
        &mut board.fonts,
        &Input::Key {
            key: Key::Character(character.into()),
            text: Some(character.into()),
            pressed: true,
            repeat: false,
            modifiers: Modifiers {
                shift,
                ..Modifiers::default()
            },
        },
    );
    board.frame();
}

fn drag(board: &mut Harness, from: (f64, f64), to: (f64, f64)) {
    board.button(from, true, 1, false);
    let middle = ((from.0 + to.0) / 2.0, (from.1 + to.1) / 2.0);
    for at in [middle, to] {
        board
            .ui
            .input(&mut board.fonts, &Input::PointerMoved(Point::from(at)));
        board.frame();
    }
    board.button(to, false, 1, false);
    board.frame();
}

fn only_selected(board: &Harness) -> NodeId {
    let [node] = board.selection()[..] else {
        panic!("one node selected: {:?}", board.selection())
    };
    node
}

fn geometry(board: &Harness, node: NodeId) -> Geometry {
    board
        .board()
        .document()
        .view()
        .node(node)
        .unwrap()
        .geometry()
        .unwrap()
}

/// The sample board, focused (a click on empty board).
fn focused() -> Harness {
    let mut board = sample();
    board.click((20.0, 800.0));
    board
}

#[test]
fn a_shape_tool_draws_one_shape_and_hands_back_to_select() {
    let mut board = focused();
    press_key(&mut board, "r", false);
    assert_eq!(
        board.board().tool().get_untracked(),
        Tool::Shape(Preset::Rectangle)
    );
    drag(&mut board, (800.0, 700.0), (900.0, 780.0));
    let node = only_selected(&board);
    let view = board.board().document().view();
    assert_eq!(
        view.node(node).unwrap().prop("shape"),
        Some(&Value::String("rect".into()))
    );
    assert_eq!(
        geometry(&board, node),
        Geometry::new(800.0, 700.0, 100.0, 80.0)
    );
    assert_eq!(board.board().tool().get_untracked(), Tool::Select);
    // The drawing is one undo step.
    let id = board.board;
    assert!(
        board
            .ui
            .widget_mut::<BoardCanvas>(id)
            .unwrap()
            .undo(UndoScope::User, &Actor::User)
            .unwrap()
    );
    assert!(board.board().document().view().node(node).is_none());
}

#[test]
fn a_click_places_the_tools_size() {
    let mut board = focused();
    press_key(&mut board, "o", false);
    board.click((900.0, 750.0));
    let node = only_selected(&board);
    assert_eq!(
        geometry(&board, node),
        Geometry::new(820.0, 702.0, 160.0, 96.0)
    );
}

#[test]
fn shapes_drawn_in_a_frame_go_into_it() {
    let mut board = focused();
    let research = board.node("Research");
    press_key(&mut board, "d", false);
    drag(&mut board, (260.0, 600.0), (300.0, 640.0));
    let node = only_selected(&board);
    let view = board.board().document().view();
    assert_eq!(view.node(node).unwrap().parent(), Parent::Node(research));
    // In the frame's space (it sits at 240, 200).
    assert_eq!(
        geometry(&board, node),
        Geometry::new(20.0, 400.0, 40.0, 40.0)
    );
}

#[test]
fn a_line_runs_from_the_press_to_the_release() {
    let mut board = focused();
    press_key(&mut board, "l", false);
    drag(&mut board, (900.0, 700.0), (800.0, 760.0));
    let node = only_selected(&board);
    let view = board.board().document().view();
    assert_eq!(
        view.node(node).unwrap().prop("flip"),
        Some(&Value::String("h".into()))
    );
    assert_eq!(
        geometry(&board, node),
        Geometry::new(800.0, 700.0, 100.0, 60.0)
    );
}

#[test]
fn the_text_tool_places_a_box_and_types_into_it() {
    let mut board = focused();
    press_key(&mut board, "t", false);
    board.click((800.0, 700.0));
    let node = only_selected(&board);
    assert!(board.board().is_editing(node));
    for character in "Hi".chars() {
        press_key(&mut board, &character.to_string(), false);
    }
    let view = board.board().document().view();
    assert!(matches!(view.node(node).unwrap().kind(), NodeKind::Text));
    assert_eq!(
        view.node(node).unwrap().text().unwrap().to_plain_string(),
        "Hi\n"
    );
    // Shift+T is the triangle, once editing ends.
    board.key(Key::Escape, false);
    press_key(&mut board, "T", true);
    assert_eq!(
        board.board().tool().get_untracked(),
        Tool::Shape(Preset::Triangle)
    );
}

#[test]
fn the_hand_pans() {
    let mut board = focused();
    press_key(&mut board, "h", false);
    let before = board.board().origin();
    drag(&mut board, (700.0, 700.0), (650.0, 680.0));
    let after = board.board().origin();
    assert_eq!((after.x - before.x, after.y - before.y), (50.0, 20.0));
    assert_eq!(
        board.board().tool().get_untracked(),
        Tool::Hand,
        "the hand stays"
    );
}

#[test]
fn insert_without_a_box_lands_in_the_middle_of_the_view() {
    let mut board = focused();
    let id = board.board;
    let args = imaginarium_commands::Args::new().with(
        "preset",
        imaginarium_commands::ArgValue::String("ellipse".into()),
    );
    board
        .ui
        .widget_mut::<BoardCanvas>(id)
        .unwrap()
        .execute("board.insert_shape", &args)
        .unwrap();
    board.frame();
    let node = only_selected(&board);
    // The board is 1440 × 866 at 100 % from the origin: its middle is in
    // the Research frame (at 240, 200), so the shape goes in there.
    let research = board.node("Research");
    assert_eq!(
        board.board().document().view().node(node).unwrap().parent(),
        Parent::Node(research)
    );
    assert_eq!(
        geometry(&board, node),
        Geometry::new(400.0, 185.0, 160.0, 96.0)
    );
    let args = imaginarium_commands::Args::new().with(
        "text",
        imaginarium_commands::ArgValue::String("Note".into()),
    );
    board
        .ui
        .widget_mut::<BoardCanvas>(id)
        .unwrap()
        .execute("board.insert_text", &args)
        .unwrap();
    board.frame();
    let node = only_selected(&board);
    assert!(!board.board().is_editing(node), "with its text, no editing");
    let text = board
        .board()
        .document()
        .view()
        .node(node)
        .unwrap()
        .text()
        .unwrap()
        .to_plain_string();
    assert_eq!(text, "Note\n");
}

#[test]
fn a_quick_drag_released_before_a_paint_keeps_its_box() {
    let mut board = focused();
    press_key(&mut board, "r", false);
    board.button((800.0, 700.0), true, 1, false);
    board.ui.input(
        &mut board.fonts,
        &Input::PointerMoved(Point::new(900.0, 780.0)),
    );
    board.button((900.0, 780.0), false, 1, false);
    board.frame();
    let node = only_selected(&board);
    assert_eq!(
        geometry(&board, node),
        Geometry::new(800.0, 700.0, 100.0, 80.0)
    );
}

#[test]
fn a_line_in_a_turned_frame_runs_where_it_was_dragged() {
    let (mut board, frame) = {
        let mut document =
            imaginarium_model::Document::new(imaginarium_model::DocumentKind::Board).unwrap();
        let (frame, _) = document
            .transact(&Actor::User, |txn| {
                let frame = txn.create_node(Parent::Root, None, &NodeKind::Frame)?;
                txn.set_geometry(
                    frame,
                    Geometry {
                        x: 200.0,
                        y: 200.0,
                        w: 600.0,
                        h: 600.0,
                        r: 30.0,
                    },
                )?;
                Ok(frame)
            })
            .unwrap();
        (Harness::new(document), frame)
    };
    board.click((1400.0, 850.0));
    press_key(&mut board, "l", false);
    drag(&mut board, (400.0, 500.0), (600.0, 500.0));
    let node = only_selected(&board);
    let view = board.board().document().view();
    assert_eq!(view.node(node).unwrap().parent(), Parent::Node(frame));
    // Its ends, on the board, are where the drag began and ended.
    let canvas = board.board();
    let item = &canvas.items_for_tests().by_node[&node];
    let flip = view
        .node(node)
        .unwrap()
        .prop("flip")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    let size = item.size;
    let x = |at_start: bool| {
        if at_start != flip.contains('h') {
            0.0
        } else {
            size.width
        }
    };
    let y = |at_start: bool| {
        if at_start != flip.contains('v') {
            0.0
        } else {
            size.height
        }
    };
    let start = item.world * Point::new(x(true), y(true));
    let end = item.world * Point::new(x(false), y(false));
    assert!(
        (start - Point::new(400.0, 500.0)).hypot() < 1e-6,
        "{start:?}"
    );
    assert!((end - Point::new(600.0, 500.0)).hypot() < 1e-6, "{end:?}");
}

#[test]
fn esc_puts_the_tool_down_and_a_shape_dragged_to_nothing_goes() {
    let mut board = focused();
    press_key(&mut board, "r", false);
    board.key(Key::Escape, false);
    assert_eq!(board.board().tool().get_untracked(), Tool::Select);
    let before = board.board().document().view().node_count();
    press_key(&mut board, "r", false);
    drag(&mut board, (800.0, 700.0), (800.0, 700.0));
    // A press that travels and comes back: nothing left behind.
    board.button((800.0, 700.0), true, 1, false);
    for at in [(840.0, 740.0), (800.0, 700.0)] {
        board
            .ui
            .input(&mut board.fonts, &Input::PointerMoved(Point::from(at)));
        board.frame();
    }
    board.button((800.0, 700.0), false, 1, false);
    board.frame();
    // The click placed one shape; the drag back to nothing none.
    assert_eq!(board.board().document().view().node_count(), before + 1);
}

#[test]
fn the_text_tool_on_a_text_edits_it() {
    let mut board = focused();
    let note = board.node("Ship the beta to 50 teams by December. Two interviews left this week.");
    let before = board.board().document().view().node_count();
    press_key(&mut board, "t", false);
    board.click((1150.0, 280.0));
    assert!(board.board().is_editing(note));
    assert_eq!(board.board().document().view().node_count(), before);
}
