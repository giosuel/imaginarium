// SPDX-License-Identifier: AGPL-3.0-or-later

//! Tests of the style commands on the selection (M3.4).

use imaginarium_commands::{ArgValue, Args, Availability, Command};
use imaginarium_model::{UndoScope, Value};

use crate::BoardCanvas;
use crate::commands_style::{SET_FILL, SET_SELECTION_RADIUS, SET_STROKE_WIDTH};
use crate::tests_select::{Harness, sample};

fn run(board: &mut Harness, id: &str, args: Args) {
    let widget = board.board;
    board
        .ui
        .widget_mut::<BoardCanvas>(widget)
        .unwrap()
        .execute(id, &args)
        .unwrap();
    board.frame();
}

fn prop(board: &Harness, node: imaginarium_model::NodeId, name: &str) -> Option<Value> {
    board
        .board()
        .document()
        .view()
        .style_prop(node, name)
        .cloned()
}

#[test]
fn a_fill_colors_every_selected_shape_in_one_step() {
    let mut board = sample();
    let interview = board.node("Interview 5 users");
    let competitor = board.node("Competitor scan");
    let widget = board.board;
    board
        .ui
        .widget_mut::<BoardCanvas>(widget)
        .unwrap()
        .select(&[interview, competitor]);
    let before = prop(&board, interview, "fill");
    run(
        &mut board,
        "style.set_fill",
        Args::new().with("color", ArgValue::String("#FEF3C7".into())),
    );
    for node in [interview, competitor] {
        assert_eq!(
            prop(&board, node, "fill"),
            Some(Value::String("#fef3c7".into()))
        );
    }
    // One undo takes both back.
    board
        .ui
        .widget_mut::<BoardCanvas>(widget)
        .unwrap()
        .undo(UndoScope::User, &imaginarium_model::Actor::User)
        .unwrap();
    board.frame();
    assert_eq!(prop(&board, interview, "fill"), before);
    assert_ne!(
        prop(&board, competitor, "fill"),
        Some(Value::String("#fef3c7".into()))
    );
}

#[test]
fn style_commands_hide_where_nothing_takes_them() {
    let mut board = sample();
    let widget = board.board;
    // Nothing selected.
    let canvas = board.ui.widget::<BoardCanvas>(widget).unwrap();
    assert_eq!(
        Command::availability(&SET_FILL, canvas),
        Availability::Hidden
    );
    // A text box takes no fill, outline or radius.
    let text = board.node("Ship the beta to 50 teams by December. Two interviews left this week.");
    board
        .ui
        .widget_mut::<BoardCanvas>(widget)
        .unwrap()
        .select(&[text]);
    let canvas = board.ui.widget::<BoardCanvas>(widget).unwrap();
    assert_eq!(
        Command::availability(&SET_FILL, canvas),
        Availability::Hidden
    );
    assert_eq!(
        Command::availability(&SET_STROKE_WIDTH, canvas),
        Availability::Hidden
    );
    assert_eq!(
        Command::availability(&SET_SELECTION_RADIUS, canvas),
        Availability::Hidden
    );
}

#[test]
fn a_radius_stops_at_half_the_shorter_side_of_each_rectangle() {
    let mut board = sample();
    let interview = board.node("Interview 5 users");
    let widget = board.board;
    board
        .ui
        .widget_mut::<BoardCanvas>(widget)
        .unwrap()
        .select(&[interview]);
    run(
        &mut board,
        "style.set_radius",
        Args::new().with("radius", ArgValue::Float(1000.0)),
    );
    let size = board
        .board()
        .document()
        .view()
        .node(interview)
        .unwrap()
        .geometry()
        .unwrap();
    assert_eq!(
        prop(&board, interview, "radius"),
        Some(Value::Number(size.w.min(size.h) / 2.0))
    );
}

#[test]
fn the_selection_box_on_screen_follows_the_view() {
    let mut board = sample();
    let interview = board.node("Interview 5 users");
    let widget = board.board;
    let canvas = board.ui.widget_mut::<BoardCanvas>(widget).unwrap();
    assert_eq!(canvas.selection_on_screen(), None);
    canvas.select(&[interview]);
    board.frame();
    let at_100 = board.board().selection_on_screen().unwrap();
    let document = crate::hit::node_box(
        board.board().items_for_tests(),
        board.board().document().view(),
        interview,
    )
    .unwrap();
    assert!((at_100.width() - document.width()).abs() < 1e-6);
    // Twice as large at 200 %.
    let canvas = board.ui.widget_mut::<BoardCanvas>(widget).unwrap();
    let origin = canvas.origin();
    canvas.set_view(origin, 2.0);
    board.frame();
    let at_200 = board.board().selection_on_screen().unwrap();
    assert!((at_200.width() - 2.0 * at_100.width()).abs() < 1e-6);
}
