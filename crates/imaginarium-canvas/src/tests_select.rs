// SPDX-License-Identifier: AGPL-3.0-or-later

//! Tests of hit testing and the selection (`canvas.md` §4, §5), through
//! the widget as a window drives it.

use imaginarium_model::{Actor, Document, DocumentKind, Geometry, NodeId, NodeKind, Parent, Value};
use imaginarium_render::Scene;
use imaginarium_render::kurbo::{Point, Rect};
use imaginarium_text::Fonts;
use imaginarium_ui::{AccessTree, Flex, Input, Key, Modifiers, Ui, WidgetId};

use crate::BoardCanvas;

const AREA: Rect = Rect::new(0.0, 0.0, 1440.0, 866.0);

pub(crate) struct Harness {
    pub(crate) ui: Ui,
    pub(crate) fonts: Fonts,
    pub(crate) board: WidgetId,
}

impl Harness {
    pub(crate) fn new(document: Document) -> Self {
        let mut ui = Ui::new();
        let board = ui.add(BoardCanvas::new(document));
        let root = ui.add_with_children(Flex::column().fill(), &[board]);
        ui.set_root(root);
        let mut harness = Self {
            ui,
            fonts: Fonts::bundled_only(),
            board,
        };
        harness.frame();
        harness
    }

    pub(crate) fn frame(&mut self) {
        let mut scene = Scene::new();
        self.ui.layout(&mut self.fonts, AREA);
        self.ui.paint(&mut self.fonts, &mut scene);
    }

    pub(crate) fn board(&self) -> &BoardCanvas {
        self.ui.widget::<BoardCanvas>(self.board).unwrap()
    }

    pub(crate) fn selection(&self) -> Vec<NodeId> {
        self.board().selection().get_untracked()
    }

    pub(crate) fn button(&mut self, at: (f64, f64), pressed: bool, clicks: u8, shift: bool) {
        let modifiers = Modifiers {
            shift,
            ..Modifiers::default()
        };
        self.ui.input(
            &mut self.fonts,
            &Input::PointerButton {
                primary: true,
                pressed,
                position: Point::from(at),
                clicks,
                modifiers,
            },
        );
    }

    pub(crate) fn click(&mut self, at: (f64, f64)) {
        self.button(at, true, 1, false);
        self.button(at, false, 1, false);
        self.frame();
    }

    pub(crate) fn shift_click(&mut self, at: (f64, f64)) {
        self.button(at, true, 1, true);
        self.button(at, false, 1, true);
        self.frame();
    }

    pub(crate) fn double_click(&mut self, at: (f64, f64)) {
        self.click(at);
        self.button(at, true, 2, false);
        self.button(at, false, 2, false);
        self.frame();
    }

    pub(crate) fn drag(&mut self, from: (f64, f64), to: (f64, f64), shift: bool) {
        self.button(from, true, 1, shift);
        self.ui
            .input(&mut self.fonts, &Input::PointerMoved(Point::from(to)));
        self.frame();
        self.button(to, false, 1, shift);
        self.frame();
    }

    pub(crate) fn key(&mut self, key: Key, shift: bool) {
        let modifiers = Modifiers {
            shift,
            ..Modifiers::default()
        };
        self.ui.input(
            &mut self.fonts,
            &Input::Key {
                key,
                text: None,
                pressed: true,
                repeat: false,
                modifiers,
            },
        );
        self.frame();
    }

    /// The node whose text (or name) is `label`.
    pub(crate) fn node(&self, label: &str) -> NodeId {
        let view = self.board().document().view();
        view.walk()
            .into_iter()
            .find(|id| {
                let node = view.node(*id).unwrap();
                node.name() == Some(label)
                    || node
                        .text()
                        .is_some_and(|text| text.to_plain_string().trim() == label)
            })
            .unwrap_or_else(|| panic!("no node {label}"))
    }
}

pub(crate) fn sample() -> Harness {
    let no_merge = imaginarium_model::UndoConfig {
        merge_interval_ms: 0,
        max_steps: 1000,
    };
    Harness::new(crate::demo::launch_plan_with(no_merge).unwrap())
}

#[test]
fn a_click_selects_the_shape_under_it_and_empty_board_clears() {
    let mut board = sample();
    let interview = board.node("Interview 5 users");
    // Inside the frame, the rectangle at 276, 290 (220 × 88).
    board.click((300.0, 300.0));
    assert_eq!(board.selection(), vec![interview]);
    // Inside the frame but on no shape: nothing.
    board.click((260.0, 620.0));
    assert!(board.selection().is_empty());
    // A frame's name selects the frame.
    let research = board.node("Research");
    board.click((250.0, 185.0));
    assert_eq!(board.selection(), vec![research]);
}

#[test]
fn shift_click_adds_and_removes() {
    let mut board = sample();
    let (interview, scan) = (
        board.node("Interview 5 users"),
        board.node("Competitor scan"),
    );
    board.click((300.0, 300.0));
    board.shift_click((600.0, 300.0));
    assert_eq!(board.selection(), vec![interview, scan]);
    board.shift_click((300.0, 300.0));
    assert_eq!(board.selection(), vec![scan]);
}

#[test]
fn lines_are_hit_near_them_and_unfilled_shapes_on_their_outline() {
    let mut document = Document::new(DocumentKind::Board).unwrap();
    let ((line, outline), _) = document
        .transact(&Actor::User, |txn| {
            let line = txn.create_node(Parent::Root, None, &NodeKind::Shape)?;
            txn.set_geometry(line, Geometry::new(100.0, 100.0, 200.0, 0.0))?;
            txn.set_prop(line, "shape", Value::String("line".into()))?;
            let outline = txn.create_node(Parent::Root, None, &NodeKind::Shape)?;
            txn.set_geometry(outline, Geometry::new(400.0, 100.0, 100.0, 100.0))?;
            Ok((line, outline))
        })
        .unwrap();
    let mut board = Harness::new(document);
    // 3 px off a 1.5 pt line: within the 4 px tolerance.
    board.click((200.0, 103.0));
    assert_eq!(board.selection(), vec![line]);
    board.click((200.0, 110.0));
    assert!(board.selection().is_empty());
    // Not filled: the middle is empty, the edge isn't.
    board.click((450.0, 150.0));
    assert!(board.selection().is_empty());
    board.click((401.0, 150.0));
    assert_eq!(board.selection(), vec![outline]);
}

#[test]
fn groups_select_as_a_whole_until_a_double_click_enters_them() {
    let mut document = Document::new(DocumentKind::Board).unwrap();
    let ((group, first, locked), _) = document
        .transact(&Actor::User, |txn| {
            let group = txn.create_node(Parent::Root, None, &NodeKind::Group)?;
            txn.set_geometry(group, Geometry::new(100.0, 100.0, 300.0, 100.0))?;
            let mut children = Vec::new();
            for index in 0..2 {
                let id = txn.create_node(Parent::Node(group), None, &NodeKind::Shape)?;
                txn.set_geometry(
                    id,
                    Geometry::new(f64::from(index) * 150.0, 0.0, 100.0, 100.0),
                )?;
                txn.set_style_override(id, "fill", Value::String("#E0E7FF".into()))?;
                children.push(id);
            }
            let locked = txn.create_node(Parent::Root, None, &NodeKind::Shape)?;
            txn.set_geometry(locked, Geometry::new(600.0, 100.0, 100.0, 100.0))?;
            txn.set_style_override(locked, "fill", Value::String("#E0E7FF".into()))?;
            txn.set_prop(locked, "locked", Value::Bool(true))?;
            Ok((group, children[0], locked))
        })
        .unwrap();
    let mut board = Harness::new(document);
    board.click((150.0, 150.0));
    assert_eq!(board.selection(), vec![group]);
    board.double_click((150.0, 150.0));
    assert_eq!(board.selection(), vec![first]);
    // Esc leaves the group, with the group selected; Esc again clears.
    board.key(Key::Escape, false);
    assert_eq!(board.selection(), vec![group]);
    board.key(Key::Escape, false);
    assert!(board.selection().is_empty());
    // Locked nodes can't be hit.
    board.click((650.0, 150.0));
    assert!(board.selection().is_empty());
    let _ = locked;
}

#[test]
fn a_marquee_selects_what_lies_wholly_inside() {
    let mut board = sample();
    let (interview, scan, synthesis) = (
        board.node("Interview 5 users"),
        board.node("Competitor scan"),
        board.node("Synthesis"),
    );
    // From empty board inside the frame around the two rectangles (and
    // through the lines, which aren't wholly inside).
    board.drag((262.0, 280.0), (760.0, 400.0), false);
    assert_eq!(board.selection(), vec![interview, scan]);
    // Shift keeps what was selected.
    board.drag((390.0, 470.0), (630.0, 600.0), true);
    assert_eq!(board.selection(), vec![interview, scan, synthesis]);
    // Around the whole frame: the frame, not its children.
    let research = board.node("Research");
    board.drag((200.0, 190.0), (800.0, 700.0), false);
    assert_eq!(board.selection(), vec![research]);
}

#[test]
fn tab_steps_through_the_nodes_and_deleted_ones_leave_the_selection() {
    let mut board = sample();
    board.click((300.0, 300.0));
    let interview = board.node("Interview 5 users");
    board.key(Key::Tab, false);
    let next = board.selection();
    assert_eq!(next.len(), 1);
    assert_ne!(next, vec![interview]);
    board.key(Key::Tab, true);
    assert_eq!(board.selection(), vec![interview]);
    // Deleting the selected node empties the selection.
    let id = board.board;
    board
        .ui
        .widget_mut::<BoardCanvas>(id)
        .unwrap()
        .transact(&Actor::User, |txn| txn.delete_node(interview))
        .unwrap();
    board.frame();
    assert!(board.selection().is_empty());
}

#[test]
fn screen_readers_get_the_visible_items_and_follow_the_selection() {
    let mut board = sample();
    board.click((300.0, 300.0));
    let mut tree = AccessTree::default();
    board.ui.access(&mut board.fonts, &mut tree);
    let labels: Vec<String> = tree
        .nodes()
        .iter()
        .filter_map(|(_, node)| node.label().map(str::to_owned))
        .collect();
    for label in [
        "Board",
        "Research",
        "Interview 5 users",
        "Go / no-go",
        "Line",
    ] {
        assert!(
            labels.iter().any(|seen| seen == label),
            "{label} in {labels:?}"
        );
    }
    let focus = tree.focus().expect("the board has the focus");
    let focused = tree.nodes().iter().find(|(id, _)| *id == focus).unwrap();
    assert_eq!(focused.1.label(), Some("Interview 5 users"));
    assert!(focused.1.is_selected() == Some(true));
}

#[test]
fn a_screen_reader_focusing_an_item_selects_it_and_ids_stay_put() {
    let mut board = sample();
    let interview = board.node("Interview 5 users");
    let id_of = |board: &mut Harness, label: &str| {
        let mut tree = AccessTree::default();
        board.ui.access(&mut board.fonts, &mut tree);
        tree.nodes()
            .iter()
            .find(|(_, node)| node.label() == Some(label))
            .map(|(id, _)| *id)
            .unwrap()
    };
    let before = id_of(&mut board, "Interview 5 users");
    // A node added behind it doesn't renumber it.
    let id = board.board;
    board
        .ui
        .widget_mut::<BoardCanvas>(id)
        .unwrap()
        .transact(&Actor::User, |txn| {
            let shape = txn.create_node(Parent::Root, Some(0), &NodeKind::Shape)?;
            txn.set_geometry(shape, Geometry::new(0.0, 0.0, 50.0, 50.0))
        })
        .unwrap();
    board.frame();
    assert_eq!(id_of(&mut board, "Interview 5 users"), before);
    // Focus through the tree, as AccessKit sends it.
    let request = accesskit::ActionRequest {
        action: accesskit::Action::Focus,
        target_tree: accesskit::TreeId::ROOT,
        target_node: before,
        data: None,
    };
    assert!(board.ui.access_action(&mut board.fonts, &request));
    assert_eq!(board.selection(), vec![interview]);
}

#[test]
fn a_frames_inside_hides_what_is_behind_it() {
    let mut document = Document::new(DocumentKind::Board).unwrap();
    document
        .transact(&Actor::User, |txn| {
            let rect = txn.create_node(Parent::Root, None, &NodeKind::Shape)?;
            txn.set_geometry(rect, Geometry::new(100.0, 100.0, 400.0, 400.0))?;
            txn.set_style_override(rect, "fill", Value::String("#E0E7FF".into()))?;
            let frame = txn.create_node(Parent::Root, None, &NodeKind::Frame)?;
            txn.set_geometry(frame, Geometry::new(150.0, 150.0, 200.0, 200.0))
        })
        .unwrap();
    let mut board = Harness::new(document);
    board.click((250.0, 250.0));
    assert!(board.selection().is_empty());
}

#[test]
fn a_deleted_frames_name_selects_nothing() {
    let mut board = sample();
    let research = board.node("Research");
    let id = board.board;
    board
        .ui
        .widget_mut::<BoardCanvas>(id)
        .unwrap()
        .transact(&Actor::User, |txn| txn.delete_node(research))
        .unwrap();
    // No paint in between: the names are from before the delete.
    board.button((250.0, 185.0), true, 1, false);
    board.button((250.0, 185.0), false, 1, false);
    assert!(board.selection().is_empty());
}

#[test]
fn a_click_outside_an_entered_group_leaves_it() {
    let mut document = Document::new(DocumentKind::Board).unwrap();
    let (outside, _) = document
        .transact(&Actor::User, |txn| {
            let group = txn.create_node(Parent::Root, None, &NodeKind::Group)?;
            for index in 0..2 {
                let id = txn.create_node(Parent::Node(group), None, &NodeKind::Shape)?;
                txn.set_geometry(
                    id,
                    Geometry::new(100.0 + f64::from(index) * 150.0, 100.0, 100.0, 100.0),
                )?;
                txn.set_style_override(id, "fill", Value::String("#E0E7FF".into()))?;
            }
            let outside = txn.create_node(Parent::Root, None, &NodeKind::Shape)?;
            txn.set_geometry(outside, Geometry::new(600.0, 100.0, 100.0, 100.0))?;
            txn.set_style_override(outside, "fill", Value::String("#E0E7FF".into()))?;
            Ok(outside)
        })
        .unwrap();
    let mut board = Harness::new(document);
    board.double_click((150.0, 150.0));
    board.click((650.0, 150.0));
    assert_eq!(board.selection(), vec![outside]);
    // Esc clears, instead of going back to the group.
    board.key(Key::Escape, false);
    assert!(board.selection().is_empty());
}

#[test]
fn a_click_on_one_of_several_selected_selects_it_alone() {
    let mut board = sample();
    let (interview, scan) = (
        board.node("Interview 5 users"),
        board.node("Competitor scan"),
    );
    board.click((300.0, 300.0));
    board.shift_click((600.0, 300.0));
    assert_eq!(board.selection(), vec![interview, scan]);
    board.click((600.0, 300.0));
    assert_eq!(board.selection(), vec![scan]);
}

#[test]
fn tab_past_the_last_node_leaves_the_board() {
    let mut board = sample();
    let count = board.board().document().view().node_count();
    // A click on empty board gives it the keys.
    board.click((20.0, 800.0));
    for _ in 0..count {
        board.key(Key::Tab, false);
    }
    // Every node once, then out.
    assert!(!board.selection().is_empty());
    board.key(Key::Tab, false);
    assert!(board.selection().is_empty());
}
