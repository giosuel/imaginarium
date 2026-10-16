// SPDX-License-Identifier: AGPL-3.0-or-later

//! Tests of the gestures (`canvas.md` §5, §6): moving, resizing, rotating
//! and rounding through the handles, nudging with the keys, each one undo
//! step through the commands.

use imaginarium_model::{Actor, Geometry, NodeId, UndoScope, Value};
use imaginarium_render::kurbo::Point;
use imaginarium_ui::{Input, Key};

use crate::BoardCanvas;
use crate::tests_select::{Harness, sample};

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

fn undo(board: &mut Harness) -> bool {
    let id = board.board;
    let undone = board
        .ui
        .widget_mut::<BoardCanvas>(id)
        .unwrap()
        .undo(UndoScope::User, &Actor::User)
        .unwrap();
    board.frame();
    undone
}

/// A press at `from`, pointer moves through `path`, a frame after each,
/// and the release at the last point.
fn gesture(board: &mut Harness, from: (f64, f64), path: &[(f64, f64)], shift: bool) {
    board.button(from, true, 1, shift);
    for at in path {
        board
            .ui
            .input(&mut board.fonts, &Input::PointerMoved(Point::from(*at)));
        board.frame();
    }
    let last = path.last().copied().unwrap_or(from);
    board.button(last, false, 1, shift);
    board.frame();
}

#[test]
fn dragging_moves_the_selection_and_undoes_as_one_step() {
    let mut board = sample();
    let interview = board.node("Interview 5 users");
    let before = geometry(&board, interview);
    // Fifteen pointer moves, one gesture.
    let path: Vec<(f64, f64)> = (1..=15)
        .map(|step| (300.0 + f64::from(step) * 4.0, 300.0 + f64::from(step) * 2.0))
        .collect();
    gesture(&mut board, (300.0, 300.0), &path, false);
    let after = geometry(&board, interview);
    assert_eq!((after.x - before.x, after.y - before.y), (60.0, 30.0));
    assert_eq!(board.selection(), vec![interview]);
    assert!(undo(&mut board));
    assert_eq!(geometry(&board, interview), before);
    // The step before the drag was the board's creation: one undo, back.
    let created = undo(&mut board);
    assert!(!created || board.board().document().view().node(interview).is_none());
}

#[test]
fn a_press_that_doesnt_travel_doesnt_move() {
    let mut board = sample();
    let interview = board.node("Interview 5 users");
    let before = geometry(&board, interview);
    gesture(&mut board, (300.0, 300.0), &[(301.0, 301.0)], false);
    assert_eq!(geometry(&board, interview), before);
}

#[test]
fn the_corner_handle_resizes_and_shift_keeps_the_shape() {
    let mut board = sample();
    let interview = board.node("Interview 5 users");
    board.click((300.0, 300.0));
    // The bottom right handle sits on the corner (276 + 220, 290 + 88).
    gesture(
        &mut board,
        (496.0, 378.0),
        &[(520.0, 400.0), (546.0, 418.0)],
        false,
    );
    assert_eq!(
        geometry(&board, interview),
        Geometry::new(36.0, 90.0, 270.0, 128.0)
    );
    assert!(undo(&mut board));
    assert_eq!(
        geometry(&board, interview),
        Geometry::new(36.0, 90.0, 220.0, 88.0)
    );
    // With Shift, double the width doubles the height.
    gesture(&mut board, (496.0, 378.0), &[(716.0, 380.0)], true);
    let g = geometry(&board, interview);
    assert!(
        (g.w - 440.0).abs() < 1e-9 && (g.h - 176.0).abs() < 1e-9,
        "{g:?}"
    );
}

#[test]
fn the_rotation_handle_turns_around_the_center_and_snaps_with_shift() {
    let mut board = sample();
    let interview = board.node("Interview 5 users");
    board.click((300.0, 300.0));
    // 20 px above the top edge's middle; the center is (386, 334).
    gesture(
        &mut board,
        (386.0, 270.0),
        &[(450.0, 300.0), (500.0, 334.0)],
        false,
    );
    let g = geometry(&board, interview);
    assert!((g.r - 90.0).abs() < 1e-6, "{g:?}");
    assert_eq!(
        (g.x, g.y, g.w, g.h),
        (36.0, 90.0, 220.0, 88.0),
        "it turns in place"
    );
    assert!(undo(&mut board));
    gesture(&mut board, (386.0, 270.0), &[(480.0, 300.0)], true);
    let r = geometry(&board, interview).r;
    assert_eq!(r % 15.0, 0.0, "{r}");
}

#[test]
fn the_radius_handle_rounds_the_corners() {
    let mut board = sample();
    let interview = board.node("Interview 5 users");
    board.click((300.0, 300.0));
    // Radius 12: the handle is 12 pt in from the top left corner.
    gesture(
        &mut board,
        (288.0, 302.0),
        &[(300.0, 314.0), (306.0, 320.0)],
        false,
    );
    let radius = board
        .board()
        .document()
        .view()
        .style_prop(interview, "radius")
        .and_then(Value::as_number);
    assert_eq!(radius, Some(30.0));
}

#[test]
fn arrows_nudge_by_one_point_and_shift_by_ten() {
    let mut board = sample();
    let interview = board.node("Interview 5 users");
    board.click((300.0, 300.0));
    board.key(Key::ArrowRight, false);
    board.key(Key::ArrowDown, true);
    let g = geometry(&board, interview);
    assert_eq!((g.x, g.y), (37.0, 100.0), "in the frame's space");
}

#[test]
fn several_nodes_move_and_scale_together() {
    let mut board = sample();
    let (interview, scan) = (
        board.node("Interview 5 users"),
        board.node("Competitor scan"),
    );
    board.click((300.0, 300.0));
    board.shift_click((600.0, 300.0));
    // Their box: 276..744 × 290..378. Drag the right edge's middle 468 pt
    // further: twice as wide.
    gesture(&mut board, (744.0, 334.0), &[(1212.0, 334.0)], false);
    let (a, b) = (geometry(&board, interview), geometry(&board, scan));
    assert!(
        (a.x - 36.0).abs() < 1e-9 && (a.w - 440.0).abs() < 1e-9,
        "{a:?}"
    );
    assert!(
        (b.x - 540.0).abs() < 1e-9 && (b.w - 432.0).abs() < 1e-9,
        "{b:?}"
    );
    assert!(undo(&mut board));
    // Dragging one of them moves both.
    gesture(
        &mut board,
        (600.0, 300.0),
        &[(610.0, 300.0), (620.0, 310.0)],
        false,
    );
    assert_eq!(geometry(&board, interview).x, 56.0);
    assert_eq!(geometry(&board, scan).x, 308.0);
}
