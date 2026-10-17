// SPDX-License-Identifier: AGPL-3.0-or-later

//! Tests of editing text in place (`canvas.md` §7) through the widget.

use imaginarium_model::{Actor, NodeId, UndoScope};
use imaginarium_ui::{Input, Key, Modifiers};

use crate::BoardCanvas;
use crate::tests_select::{Harness, sample};

fn text_of(board: &Harness, node: NodeId) -> String {
    board
        .board()
        .document()
        .view()
        .node(node)
        .and_then(|node| node.text())
        .map(|text| text.to_plain_string())
        .unwrap_or_default()
}

fn type_text(board: &mut Harness, text: &str) {
    for character in text.chars() {
        board.ui.input(
            &mut board.fonts,
            &Input::Key {
                key: Key::Character(character.to_string()),
                text: Some(character.to_string()),
                pressed: true,
                repeat: false,
                modifiers: Modifiers::default(),
            },
        );
    }
    board.frame();
}

fn editing(board: &Harness, node: NodeId) -> bool {
    board.board().is_editing(node)
}

#[test]
fn a_double_click_edits_a_label_and_every_key_is_in_the_model() {
    let mut board = sample();
    let interview = board.node("Interview 5 users");
    board.double_click((300.0, 300.0));
    assert!(editing(&board, interview));
    type_text(&mut board, " now");
    assert_eq!(text_of(&board, interview), "Interview 5 users now\n");
    board.key(Key::Backspace, false);
    assert_eq!(text_of(&board, interview), "Interview 5 users no\n");
    // Arrows move the caret, not the shape.
    let before = board
        .board()
        .document()
        .view()
        .node(interview)
        .unwrap()
        .geometry();
    board.key(Key::ArrowLeft, false);
    assert_eq!(
        board
            .board()
            .document()
            .view()
            .node(interview)
            .unwrap()
            .geometry(),
        before
    );
    // Esc ends editing, the node stays selected.
    board.key(Key::Escape, false);
    assert!(!editing(&board, interview));
    assert_eq!(board.selection(), vec![interview]);
}

#[test]
fn enter_edits_the_selected_text_box_and_a_click_elsewhere_ends_it() {
    let mut board = sample();
    let note = board.node("Ship the beta to 50 teams by December. Two interviews left this week.");
    board.click((1150.0, 280.0));
    assert_eq!(board.selection(), vec![note]);
    board.key(Key::Enter, false);
    assert!(editing(&board, note));
    type_text(&mut board, " Go!");
    assert!(text_of(&board, note).ends_with("this week. Go!\n"));
    // A click on another shape ends editing and selects it.
    let interview = board.node("Interview 5 users");
    board.click((300.0, 300.0));
    assert!(!editing(&board, note));
    assert_eq!(board.selection(), vec![interview]);
}

#[test]
fn a_shape_without_a_label_gets_one_and_undo_takes_the_typing_back() {
    let mut board = sample();
    // The lines have no label; the first one runs from (386, 378).
    let id = board.board;
    let line = {
        let view = board.board().document().view();
        view.walk()
            .into_iter()
            .find(|node| {
                view.node(*node)
                    .and_then(|node| node.prop("shape"))
                    .and_then(imaginarium_model::Value::as_str)
                    == Some("line")
            })
            .unwrap()
    };
    board
        .ui
        .widget_mut::<BoardCanvas>(id)
        .unwrap()
        .select(&[line]);
    board.frame();
    // A click on empty board would clear the selection: Tab gives the
    // board the keys instead, and the selection is put back.
    board.key(Key::Tab, false);
    board
        .ui
        .widget_mut::<BoardCanvas>(id)
        .unwrap()
        .select(&[line]);
    board.frame();
    board.key(Key::Enter, false);
    assert!(editing(&board, line));
    type_text(&mut board, "yes");
    assert_eq!(text_of(&board, line), "yes\n");
    // Deleting everything leaves no text at all.
    for _ in 0..3 {
        board.key(Key::Backspace, false);
    }
    assert_eq!(text_of(&board, line), "");
    type_text(&mut board, "ok");
    board.key(Key::Escape, false);
    // An undo of the typing shows in the model and on the board.
    while text_of(&board, line) == "ok\n" {
        assert!(
            board
                .ui
                .widget_mut::<BoardCanvas>(id)
                .unwrap()
                .undo(UndoScope::User, &Actor::User)
                .unwrap()
        );
        board.frame();
    }
    assert_ne!(text_of(&board, line), "ok\n");
}

#[test]
fn an_undo_while_editing_shows_in_the_field() {
    let mut board = sample();
    let interview = board.node("Interview 5 users");
    board.double_click((300.0, 300.0));
    type_text(&mut board, "!");
    let id = board.board;
    assert!(
        board
            .ui
            .widget_mut::<BoardCanvas>(id)
            .unwrap()
            .undo(UndoScope::User, &Actor::User)
            .unwrap()
    );
    board.frame();
    assert_eq!(text_of(&board, interview), "Interview 5 users\n");
    // The field shows the model's text: typing goes on from it.
    type_text(&mut board, "?");
    assert_eq!(text_of(&board, interview), "Interview 5 users?\n");
}

#[test]
fn frames_and_groups_have_no_text_to_edit() {
    let mut board = sample();
    let research = board.node("Research");
    board.click((250.0, 185.0));
    assert_eq!(board.selection(), vec![research]);
    board.key(Key::Enter, false);
    assert!(!editing(&board, research));
}

fn input(board: &mut Harness, input: Input) {
    board.ui.input(&mut board.fonts, &input);
    board.frame();
}

#[test]
fn line_breaks_typed_into_an_empty_text_are_kept() {
    let mut board = sample();
    let interview = board.node("Interview 5 users");
    board.double_click((300.0, 300.0));
    for _ in 0.."Interview 5 users".len() {
        board.key(Key::Backspace, false);
    }
    assert_eq!(text_of(&board, interview), "");
    board.key(Key::Enter, false);
    type_text(&mut board, "x");
    assert_eq!(text_of(&board, interview), "\nx\n");
    for _ in 0..2 {
        board.key(Key::Backspace, false);
    }
    assert_eq!(text_of(&board, interview), "");
    input(&mut board, Input::Paste("a\n".into()));
    type_text(&mut board, "b");
    assert_eq!(text_of(&board, interview), "a\nb\n");
}

#[test]
fn a_change_made_during_a_composition_is_kept() {
    use imaginarium_ui::Ime;
    let mut board = sample();
    let interview = board.node("Interview 5 users");
    board.double_click((300.0, 300.0));
    input(
        &mut board,
        Input::Ime(Ime::Preedit {
            text: "か".into(),
            cursor: None,
        }),
    );
    // Meanwhile the assistant puts a word in front.
    let id = board.board;
    board
        .ui
        .widget_mut::<BoardCanvas>(id)
        .unwrap()
        .transact(&Actor::Ai("session".into()), |txn| {
            txn.insert_text(interview, 0, "Z ")
        })
        .unwrap();
    board.frame();
    input(&mut board, Input::Ime(Ime::Commit("仮".into())));
    assert_eq!(text_of(&board, interview), "Z Interview 5 users仮\n");
}

#[test]
fn losing_the_focus_ends_editing() {
    let mut board = sample();
    let interview = board.node("Interview 5 users");
    board.double_click((300.0, 300.0));
    assert!(editing(&board, interview));
    board.ui.set_focus(&mut board.fonts, None);
    board.frame();
    assert!(!editing(&board, interview));
}
