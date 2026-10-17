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

#[test]
fn a_click_on_a_handle_that_doesnt_move_changes_nothing() {
    let mut board = sample();
    let interview = board.node("Interview 5 users");
    board.click((300.0, 300.0));
    let before = geometry(&board, interview);
    // 4 px off the bottom right handle, pressed and released in place.
    gesture(&mut board, (500.0, 382.0), &[(501.0, 382.0)], false);
    assert_eq!(geometry(&board, interview), before);
    // The radius handle too.
    let radius = |board: &Harness| {
        board
            .board()
            .document()
            .view()
            .style_prop(interview, "radius")
            .and_then(Value::as_number)
    };
    gesture(&mut board, (292.0, 306.0), &[(292.0, 307.0)], false);
    assert_eq!(radius(&board), Some(12.0));
    // A drag from 4 px off the corner moves the corner by the drag.
    gesture(&mut board, (500.0, 382.0), &[(520.0, 402.0)], false);
    let after = geometry(&board, interview);
    assert!(
        (after.w - 240.0).abs() < 1e-9 && (after.h - 108.0).abs() < 1e-9,
        "{after:?}"
    );
}

fn board_with(
    build: impl FnOnce(
        &mut imaginarium_model::Transaction<'_>,
    ) -> imaginarium_model::Result<Vec<NodeId>>,
) -> (Harness, Vec<NodeId>) {
    let mut document = imaginarium_model::Document::new_with(
        imaginarium_model::DocumentKind::Board,
        imaginarium_model::UndoConfig {
            merge_interval_ms: 0,
            max_steps: 100,
        },
    )
    .unwrap();
    let (nodes, _) = document.transact(&Actor::User, build).unwrap();
    (Harness::new(document), nodes)
}

#[test]
fn small_shapes_and_flat_lines_still_drag() {
    use imaginarium_model::{NodeKind, Parent};
    let (mut board, nodes) = board_with(|txn| {
        let small = txn.create_node(Parent::Root, None, &NodeKind::Shape)?;
        txn.set_geometry(small, Geometry::new(100.0, 100.0, 30.0, 30.0))?;
        txn.set_style_override(small, "fill", Value::String("#E0E7FF".into()))?;
        let line = txn.create_node(Parent::Root, None, &NodeKind::Shape)?;
        txn.set_geometry(line, Geometry::new(300.0, 200.0, 100.0, 0.0))?;
        txn.set_prop(line, "shape", Value::String("line".into()))?;
        Ok(vec![small, line])
    });
    let (small, line) = (nodes[0], nodes[1]);
    board.click((115.0, 115.0));
    gesture(&mut board, (115.0, 115.0), &[(135.0, 135.0)], false);
    assert_eq!(
        geometry(&board, small),
        Geometry::new(120.0, 120.0, 30.0, 30.0)
    );
    // A flat line: its middle drags it, its end lengthens it.
    board.click((350.0, 200.0));
    gesture(&mut board, (350.0, 200.0), &[(350.0, 240.0)], false);
    assert_eq!(
        geometry(&board, line),
        Geometry::new(300.0, 240.0, 100.0, 0.0)
    );
    gesture(&mut board, (400.0, 240.0), &[(450.0, 240.0)], false);
    assert_eq!(
        geometry(&board, line),
        Geometry::new(300.0, 240.0, 150.0, 0.0)
    );
    // Past its other end, it flips instead of going negative.
    gesture(&mut board, (450.0, 240.0), &[(250.0, 280.0)], false);
    let g = geometry(&board, line);
    assert_eq!((g.x, g.y, g.w, g.h), (250.0, 240.0, 50.0, 40.0));
    let flip = board
        .board()
        .document()
        .view()
        .node(line)
        .unwrap()
        .prop("flip")
        .cloned();
    assert_eq!(flip, Some(Value::String("h".into())));
}

#[test]
fn resizing_a_group_scales_its_children() {
    use imaginarium_model::{NodeKind, Parent};
    let (mut board, nodes) = board_with(|txn| {
        let group = txn.create_node(Parent::Root, None, &NodeKind::Group)?;
        txn.set_geometry(group, Geometry::new(100.0, 100.0, 250.0, 100.0))?;
        let mut children = vec![group];
        for index in 0..2 {
            let id = txn.create_node(Parent::Node(group), None, &NodeKind::Shape)?;
            txn.set_geometry(
                id,
                Geometry::new(f64::from(index) * 150.0, 0.0, 100.0, 100.0),
            )?;
            txn.set_style_override(id, "fill", Value::String("#E0E7FF".into()))?;
            children.push(id);
        }
        Ok(children)
    });
    board.click((150.0, 150.0));
    assert_eq!(board.selection(), vec![nodes[0]]);
    // The right edge's middle, 250 pt further: twice as wide.
    gesture(&mut board, (350.0, 150.0), &[(600.0, 150.0)], false);
    assert_eq!(
        geometry(&board, nodes[2]),
        Geometry::new(300.0, 0.0, 200.0, 100.0)
    );
}

#[test]
fn commands_refuse_locked_nodes() {
    let mut board = sample();
    let interview = board.node("Interview 5 users");
    let id = board.board;
    let canvas = board.ui.widget_mut::<BoardCanvas>(id).unwrap();
    canvas
        .transact(&Actor::User, |txn| {
            txn.set_prop(interview, "locked", Value::Bool(true))
        })
        .unwrap();
    let args = imaginarium_commands::Args::new()
        .with(
            "node",
            imaginarium_commands::ArgValue::String(interview.to_string()),
        )
        .with("x", imaginarium_commands::ArgValue::Float(0.0));
    assert!(canvas.execute("board.set_box", &args).is_err());
    let args = imaginarium_commands::Args::new()
        .with(
            "node",
            imaginarium_commands::ArgValue::String(interview.to_string()),
        )
        .with("radius", imaginarium_commands::ArgValue::Float(4.0));
    assert!(canvas.execute("shape.set_radius", &args).is_err());
}
