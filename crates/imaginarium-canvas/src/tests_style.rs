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
        let fill =
            prop(&board, node, "fill").and_then(|fill| fill.as_str().map(str::to_ascii_lowercase));
        assert_eq!(fill.as_deref(), Some("#fef3c7"));
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

#[test]
fn opacity_keeps_each_nodes_own_color_and_old_uppercase_colors_count() {
    let mut board = sample();
    let interview = board.node("Interview 5 users");
    let competitor = board.node("Competitor scan");
    let widget = board.board;
    board
        .ui
        .widget_mut::<BoardCanvas>(widget)
        .unwrap()
        .select(&[interview, competitor]);
    let fills = |board: &Harness| {
        [interview, competitor].map(|node| match prop(board, node, "fill") {
            Some(Value::String(fill)) => fill.to_ascii_lowercase(),
            other => panic!("{other:?}"),
        })
    };
    let before = fills(&board);
    run(
        &mut board,
        "style.set_fill_opacity",
        Args::new().with("opacity", ArgValue::Float(0.5)),
    );
    let after = fills(&board);
    for (before, after) in before.iter().zip(&after) {
        assert_eq!(&after[..7], &before[..7], "its own color");
        assert_eq!(&after[7..], "80");
    }
    // The demo stores "#E0E7FF": picking "#e0e7ff" again changes nothing.
    let revision = board.board().revision();
    let canvas = board.ui.widget_mut::<BoardCanvas>(widget).unwrap();
    canvas.select(&[interview]);
    let stored = after[0].clone();
    canvas
        .execute(
            "style.set_fill",
            &Args::new().with("color", ArgValue::String(stored.to_ascii_uppercase())),
        )
        .unwrap();
    assert_eq!(
        board.board().revision(),
        revision,
        "no step for the same color"
    );
}

#[test]
fn the_inspectors_commands_change_the_selection_in_one_step_each() {
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
        "style.set_opacity",
        Args::new().with("opacity", ArgValue::Float(0.5)),
    );
    assert_eq!(prop(&board, interview, "opacity"), Some(Value::Number(0.5)));
    // Reset to style takes every own value away.
    run(&mut board, "style.reset", Args::new());
    let view = board.board().document().view();
    assert!(view.node(interview).unwrap().style_overrides().is_empty());
    // A name, a hidden node, another shape.
    run(
        &mut board,
        "board.rename",
        Args::new().with("name", ArgValue::String("First step".into())),
    );
    run(
        &mut board,
        "shape.set_preset",
        Args::new().with("preset", ArgValue::String("ellipse".into())),
    );
    run(
        &mut board,
        "board.set_hidden",
        Args::new().with("hidden", ArgValue::Bool(true)),
    );
    let view = board.board().document().view();
    let node = view.node(interview).unwrap();
    assert_eq!(node.name(), Some("First step"));
    assert!(node.hidden());
    assert_eq!(crate::commands_style::preset_of(node), "ellipse");
    // A hidden node leaves the selection; Show hidden brings it back.
    run(&mut board, "board.show_hidden", Args::new());
    assert!(
        !board
            .board()
            .document()
            .view()
            .node(interview)
            .unwrap()
            .hidden()
    );
}

#[test]
fn a_named_style_applies_and_hides_without_styles() {
    use imaginarium_model::{Actor, Document, DocumentKind, Geometry, NodeKind, Parent};
    let board = sample();
    let widget = board.board;
    let canvas = board.ui.widget::<BoardCanvas>(widget).unwrap();
    let apply = &crate::commands_style::APPLY_STYLE;
    assert_eq!(Command::availability(apply, canvas), Availability::Hidden);
    let mut document = Document::new(DocumentKind::Board).unwrap();
    let (node, _) = document
        .transact(&Actor::User, |txn| {
            let style = txn.create_style("Note", None)?;
            txn.set_style_prop(&style, "fill", Value::String("#fef3c7".into()))?;
            let node = txn.create_node(Parent::Root, None, &NodeKind::Shape)?;
            txn.set_geometry(node, Geometry::new(10.0, 10.0, 100.0, 60.0))?;
            Ok(node)
        })
        .unwrap();
    let mut board = Harness::new(document);
    let widget = board.board;
    board
        .ui
        .widget_mut::<BoardCanvas>(widget)
        .unwrap()
        .select(&[node]);
    let style = board
        .board()
        .document()
        .view()
        .style_ids()
        .next()
        .unwrap()
        .as_str()
        .to_owned();
    run(
        &mut board,
        "style.apply",
        Args::new().with("style", ArgValue::String(style)),
    );
    assert_eq!(
        prop(&board, node, "fill"),
        Some(Value::String("#fef3c7".into()))
    );
}
