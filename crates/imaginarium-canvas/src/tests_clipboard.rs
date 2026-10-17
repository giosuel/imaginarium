// SPDX-License-Identifier: AGPL-3.0-or-later

//! Tests of copy, cut and paste (`canvas.md` §8).

use imaginarium_model::{Actor, Document, DocumentKind, Geometry, NodeKind, Parent, Value};
use imaginarium_ui::Input;

use crate::BoardCanvas;
use crate::clipboard::{self, MARKER};
use crate::tests_select::{Harness, sample};

fn copy(board: &mut Harness) -> String {
    let id = board.board;
    let canvas = board.ui.widget_mut::<BoardCanvas>(id).unwrap();
    assert!(canvas.copy_to_clipboard());
    canvas.clipboard_work_for_tests().copy.clone().unwrap().0
}

fn paste(board: &mut Harness, pasted: &str) {
    board
        .ui
        .input(&mut board.fonts, &Input::Paste(pasted.to_owned()));
    board.frame();
}

fn board_box(board: &Harness, node: imaginarium_model::NodeId) -> imaginarium_render::kurbo::Rect {
    let canvas = board.board();
    crate::hit::node_box(canvas.items_for_tests(), canvas.document().view(), node).unwrap()
}

#[test]
fn a_copy_pastes_down_and_right_and_again_further() {
    let mut board = sample();
    let interview = board.node("Interview 5 users");
    board.click((300.0, 300.0));
    let html = copy(&mut board);
    assert!(html.contains(MARKER) && html.contains("<p>Interview 5 users</p>"));
    let original = board_box(&board, interview);
    paste(&mut board, &html);
    let [first] = board.selection()[..] else {
        panic!("one pasted node")
    };
    let view = board.board().document().view();
    // On the board, out of the frame it was copied from, 16 pt off.
    assert_eq!(view.node(first).unwrap().parent(), Parent::Root);
    assert_eq!(
        board_box(&board, first),
        original + imaginarium_render::kurbo::Vec2::new(16.0, 16.0)
    );
    assert_eq!(
        view.node(first).unwrap().text().unwrap().to_plain_string(),
        "Interview 5 users\n"
    );
    paste(&mut board, &html);
    let [second] = board.selection()[..] else {
        panic!("one pasted node")
    };
    assert_eq!(
        board_box(&board, second),
        original + imaginarium_render::kurbo::Vec2::new(32.0, 32.0)
    );
}

#[test]
fn cut_deletes_and_text_pastes_as_a_text_box() {
    let mut board = sample();
    let interview = board.node("Interview 5 users");
    board.click((300.0, 300.0));
    let id = board.board;
    let canvas = board.ui.widget_mut::<BoardCanvas>(id).unwrap();
    canvas
        .execute("board.cut", &imaginarium_commands::Args::new())
        .unwrap();
    board.frame();
    assert!(board.board().document().view().node(interview).is_none());
    paste(&mut board, "From another app\n");
    let [node] = board.selection()[..] else {
        panic!("a text box")
    };
    let view = board.board().document().view();
    assert!(matches!(view.node(node).unwrap().kind(), NodeKind::Text));
    assert_eq!(
        view.node(node).unwrap().text().unwrap().to_plain_string(),
        "From another app\n"
    );
}

#[test]
fn text_formatting_survives_and_missing_styles_come_by_value() {
    // A board with a style, and a bold text box using it.
    let mut document = Document::new(DocumentKind::Board).unwrap();
    let (node, _) = document
        .transact(&Actor::User, |txn| {
            let style = txn.create_style("Callout", None)?;
            txn.set_style_prop(&style, "color", Value::String("#6366F1".into()))?;
            let node = txn.create_node(Parent::Root, None, &NodeKind::Text)?;
            txn.set_geometry(node, Geometry::new(100.0, 100.0, 200.0, 40.0))?;
            txn.insert_text(node, 0, "Bold and plain")?;
            txn.mark_text(node, 0..4, "b", Value::Bool(true))?;
            txn.set_style(node, Some(&style))?;
            Ok(node)
        })
        .unwrap();
    let mut board = Harness::new(document);
    board.click((150.0, 110.0));
    assert_eq!(board.selection(), vec![node]);
    let html = copy(&mut board);
    // Into a board that has no such style.
    let mut other = Harness::new(Document::new(DocumentKind::Board).unwrap());
    other.click((20.0, 20.0));
    paste(&mut other, &html);
    let [pasted] = other.selection()[..] else {
        panic!("one pasted node")
    };
    let view = other.board().document().view();
    let pasted = view.node(pasted).unwrap();
    assert!(pasted.style().is_none());
    assert_eq!(
        pasted.style_overrides().get("color"),
        Some(&Value::String("#6366F1".into()))
    );
    let runs: Vec<(String, bool)> = pasted
        .text()
        .unwrap()
        .paragraphs()
        .flat_map(|paragraph| paragraph.runs().to_vec())
        .map(|run| (run.text().to_owned(), run.marks().get("b").is_some()))
        .collect();
    assert_eq!(runs[0], ("Bold".to_owned(), true));
    assert!(!runs[1].1);
}

#[test]
fn html_from_other_apps_isnt_nodes() {
    assert!(clipboard::from_html("<b>bold</b>").is_none());
    assert!(clipboard::from_html(&format!("{MARKER}%%%-->")).is_none());
}

#[test]
fn a_drawn_shape_pastes_unturned() {
    let mut board = sample();
    board.click((1435.0, 860.0));
    let press = |board: &mut Harness, character: &str| {
        board.ui.input(
            &mut board.fonts,
            &Input::Key {
                key: imaginarium_ui::Key::Character(character.into()),
                text: Some(character.into()),
                pressed: true,
                repeat: false,
                modifiers: imaginarium_ui::Modifiers::default(),
            },
        );
        board.frame();
    };
    press(&mut board, "d");
    board.button((1180.0, 640.0), true, 1, false);
    for at in [(1240.0, 700.0), (1300.0, 760.0)] {
        board.ui.input(
            &mut board.fonts,
            &Input::PointerMoved(imaginarium_render::kurbo::Point::from(at)),
        );
        board.frame();
    }
    board.button((1300.0, 760.0), false, 1, false);
    board.frame();
    let [drawn] = board.selection()[..] else {
        panic!()
    };
    let html = copy(&mut board);
    paste(&mut board, &html);
    let [pasted] = board.selection()[..] else {
        panic!()
    };
    let view = board.board().document().view();
    let (a, b) = (
        view.node(drawn).unwrap().geometry().unwrap(),
        view.node(pasted).unwrap().geometry().unwrap(),
    );
    assert_eq!(
        (b.x - a.x, b.y - a.y, b.w, b.h, b.r),
        (16.0, 16.0, a.w, a.h, 0.0)
    );
}

#[test]
fn copy_and_paste_from_the_palette_reach_the_clipboard() {
    let mut board = sample();
    board.click((300.0, 300.0));
    let id = board.board;
    board
        .ui
        .widget_mut::<BoardCanvas>(id)
        .unwrap()
        .execute("board.copy", &imaginarium_commands::Args::new())
        .unwrap();
    // The next frame hands the work over, without a key.
    board.frame();
    assert!(board.board().clipboard_work_for_tests().copy.is_none());
}

#[test]
fn a_wrapped_copy_still_pastes_down_and_right_and_a_cut_pastes_in_place() {
    let mut board = sample();
    let interview = board.node("Interview 5 users");
    board.click((300.0, 300.0));
    let html = copy(&mut board);
    let original = board_box(&board, interview);
    // macOS hands the HTML back wrapped in a document.
    paste(&mut board, &format!("<html><body>{html}</body></html>"));
    let [first] = board.selection()[..] else {
        panic!()
    };
    assert_eq!(
        board_box(&board, first),
        original + imaginarium_render::kurbo::Vec2::new(16.0, 16.0)
    );
    // A cut, then a paste: back where it was.
    let id = board.board;
    board
        .ui
        .widget_mut::<BoardCanvas>(id)
        .unwrap()
        .select(&[first]);
    let cut_html = {
        let canvas = board.ui.widget_mut::<BoardCanvas>(id).unwrap();
        canvas
            .execute("board.cut", &imaginarium_commands::Args::new())
            .unwrap();
        canvas.clipboard_work_for_tests().copy.clone().unwrap().0
    };
    board.frame();
    paste(&mut board, &cut_html);
    let [back] = board.selection()[..] else {
        panic!()
    };
    assert_eq!(
        board_box(&board, back),
        original + imaginarium_render::kurbo::Vec2::new(16.0, 16.0)
    );
}

#[test]
fn damaged_copies_and_refused_keys_dont_paste_garbage() {
    let mut board = sample();
    let before = board.board().document().view().node_count();
    board.click((20.0, 800.0));
    paste(&mut board, &format!("<p>hi</p>{MARKER}%%%-->"));
    assert_eq!(
        board.board().document().view().node_count(),
        before,
        "nothing, not the raw HTML"
    );
    // A shape with keys the model refuses: the shape comes, the keys don't.
    let copied = clipboard::Copied {
        nodes: vec![crate::copy::NodeCopy {
            kind: "shape".into(),
            geometry: Some(Geometry::new(10.0, 10.0, 50.0, 50.0)),
            style: None,
            overrides: Vec::new(),
            text: Vec::new(),
            props: vec![
                ("id".into(), Value::Int(5)),
                ("name".into(), Value::String("ok".into())),
            ],
            assets: Vec::new(),
            ext: vec![("not an add-on id".into(), Value::Bool(true))],
            generation: Vec::new(),
            children: Vec::new(),
        }],
        styles: Vec::new(),
    };
    paste(&mut board, &clipboard::to_html(&copied));
    let [node] = board.selection()[..] else {
        panic!("pasted")
    };
    assert_eq!(
        board.board().document().view().node(node).unwrap().name(),
        Some("ok")
    );
    // Windows line breaks in text.
    paste(&mut board, "a\r\nb\r\n");
    let [text] = board.selection()[..] else {
        panic!()
    };
    let content = board
        .board()
        .document()
        .view()
        .node(text)
        .unwrap()
        .text()
        .unwrap()
        .to_plain_string();
    assert_eq!(content, "a\nb\n");
}
