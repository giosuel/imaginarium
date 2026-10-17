// SPDX-License-Identifier: AGPL-3.0-or-later

//! Tests of the board window: the panels drive the board through its
//! commands, the tool bar docks where it is dropped, placeholders say
//! what is coming.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::cell::RefCell;
use std::rc::Rc;

use imaginarium_canvas::{BoardCanvas, Preset, Tool};
use imaginarium_model::{Document, DocumentKind};
use imaginarium_render::Scene;
use imaginarium_render::kurbo::{Point, Rect, Size};
use imaginarium_settings::Registry;
use imaginarium_text::Fonts;
use imaginarium_ui::{AccessTree, Input, Modifiers, View, WidgetId};

use crate::{BoardWindow, Dock, TOOLBAR_POSITION};

const SIZE: Size = Size::new(1440.0, 900.0);

struct Harness {
    window: BoardWindow,
    settings: Rc<RefCell<Registry>>,
    size: Size,
}

impl Harness {
    fn new() -> Self {
        Self::with(Document::new(DocumentKind::Board).unwrap())
    }

    fn with(document: Document) -> Self {
        let mut registry = Registry::new();
        registry.register_all(crate::SETTINGS).unwrap();
        let settings = Rc::new(RefCell::new(registry));
        let board = BoardCanvas::new(document);
        let window = BoardWindow::new(
            board,
            "Untitled board",
            Fonts::bundled_only(),
            settings.clone(),
        );
        let mut harness = Self {
            window,
            settings,
            size: SIZE,
        };
        harness.frame();
        harness
    }

    fn resize(&mut self, size: Size) {
        self.size = size;
        self.frame();
        self.frame();
    }

    fn frame(&mut self) {
        let mut scene = Scene::new();
        self.window.draw(&mut scene, self.size);
    }

    fn input(&mut self, input: Input) {
        self.window.input(&input);
        self.frame();
    }

    fn press(&mut self, at: Point, pressed: bool, clicks: u8, shift: bool) {
        self.input(Input::PointerButton {
            primary: true,
            pressed,
            position: at,
            clicks,
            modifiers: Modifiers {
                shift,
                ..Modifiers::default()
            },
        });
    }

    fn click(&mut self, at: Point) {
        self.input(Input::PointerMoved(at));
        self.press(at, true, 1, false);
        self.press(at, false, 1, false);
    }

    fn drag(&mut self, from: Point, to: Point, shift: bool) {
        self.input(Input::PointerMoved(from));
        self.press(from, true, 1, false);
        for step in 1..=8 {
            let t = f64::from(step) / 8.0;
            self.input(Input::PointerMoved(from.lerp(to, t)));
        }
        self.press(to, false, 1, shift);
    }

    fn key(&mut self, character: &str) {
        self.input(Input::Key {
            key: imaginarium_ui::Key::Character(character.into()),
            text: Some(character.into()),
            pressed: true,
            repeat: false,
            modifiers: Modifiers::default(),
        });
    }

    fn bounds(&mut self, id: WidgetId) -> Rect {
        let (ui, _) = self.window.ui();
        ui.bounds(id).unwrap()
    }

    fn board(&self) -> &BoardCanvas {
        self.window.board().unwrap()
    }

    /// The widget a screen reader would call `label`.
    fn named(&mut self, label: &str) -> WidgetId {
        let (ui, fonts) = self.window.ui();
        ui.find_by_label(fonts, label)
            .unwrap_or_else(|| panic!("no widget named {label:?}"))
    }

    /// The middle of the node a screen reader knows as `label` with
    /// `role` (two controls can share a name: the bar's Delete and the
    /// menu's).
    fn item_center(&mut self, label: &str, role: accesskit::Role) -> Point {
        let (ui, fonts) = self.window.ui();
        let mut tree = AccessTree::default();
        ui.access(fonts, &mut tree);
        let (_, node) = tree
            .nodes()
            .iter()
            .find(|(_, node)| node.label() == Some(label) && node.role() == role)
            .unwrap_or_else(|| panic!("no {role:?} named {label:?}"));
        let bounds = node.bounds().unwrap();
        Point::new((bounds.x0 + bounds.x1) / 2.0, (bounds.y0 + bounds.y1) / 2.0)
    }

    /// Every label's text, as a screen reader reads them.
    fn texts(&mut self) -> Vec<String> {
        let (ui, fonts) = self.window.ui();
        let mut tree = AccessTree::default();
        ui.access(fonts, &mut tree);
        tree.nodes()
            .iter()
            .filter(|(_, node)| node.role() == accesskit::Role::Label)
            .filter_map(|(_, node)| node.value().map(str::to_owned))
            .collect()
    }
}

#[test]
fn a_tool_button_picks_the_tool_and_leaves_the_keys_to_the_board() {
    let mut window = Harness::new();
    let rectangle = window.named("Rectangle");
    let at = window.bounds(rectangle).center();
    window.click(at);
    assert_eq!(
        window.board().tool().get_untracked(),
        Tool::Shape(Preset::Rectangle)
    );
    // The board still has the keyboard: V picks Select again.
    window.key("v");
    assert_eq!(window.board().tool().get_untracked(), Tool::Select);
}

#[test]
fn undo_and_redo_buttons_follow_the_board() {
    let mut window = Harness::new();
    let rectangle = window.named("Rectangle");
    let at = window.bounds(rectangle).center();
    window.click(at);
    // Draw a shape on the board, between the panels.
    window.drag(Point::new(500.0, 300.0), Point::new(700.0, 420.0), false);
    assert_eq!(window.board().document().view().node_count(), 1);
    let undo = window.named("Undo");
    let redo = window.named("Redo");
    let (undo_at, redo_at) = (window.bounds(undo).center(), window.bounds(redo).center());
    // Redo can't run yet: a click does nothing.
    window.click(redo_at);
    assert_eq!(window.board().document().view().node_count(), 1);
    window.click(undo_at);
    assert_eq!(window.board().document().view().node_count(), 0);
    window.click(redo_at);
    assert_eq!(window.board().document().view().node_count(), 1);
}

#[test]
fn zoom_buttons_and_the_zoom_menu_change_the_view() {
    let mut window = Harness::new();
    let zoom_in = window.named("Zoom in");
    let at = window.bounds(zoom_in).center();
    window.click(at);
    let zoom = window.board().zoom();
    assert!(zoom > 1.0, "{zoom}");
    assert!(
        window.texts().contains(&format!("{:.0} %", zoom * 100.0)),
        "the percentage follows"
    );
    let percent = window.named("Zoom: fit, 100 % or the selection");
    let at = window.bounds(percent).center();
    window.click(at);
    let hundred = window.named("Zoom to 100 %");
    let at = window.bounds(hundred).center();
    window.click(at);
    assert_eq!(window.board().zoom(), 1.0);
    let (ui, fonts) = window.window.ui();
    assert!(
        ui.find_by_label(fonts, "Zoom to 100 %").is_none(),
        "the menu closed"
    );
}

#[test]
fn placeholders_say_what_is_coming() {
    let mut window = Harness::new();
    for (button, words) in [
        ("Share", "Hub in M8"),
        ("Save…", "M3.6"),
        ("Help and shortcuts", "M3.9"),
    ] {
        let id = window.named(button);
        let at = window.bounds(id).center();
        window.click(at);
        let texts = window.texts();
        assert!(
            texts.iter().any(|text| text.contains(words)),
            "{button}: {texts:?}"
        );
        window.input(Input::Key {
            key: imaginarium_ui::Key::Escape,
            text: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::default(),
        });
    }
    // Nothing changed on the board.
    assert_eq!(window.board().document().view().node_count(), 0);
}

#[test]
fn the_tool_bar_docks_where_it_is_dropped_and_floats_elsewhere() {
    let mut window = Harness::new();
    assert_eq!(window.window.dock(), Dock::Left);
    let grip = window.named("Move the tool bar: drag it to an edge");
    let bar = window.named("Tools");
    let left = window.bounds(bar);
    assert!(left.x0 <= 17.0 && left.height() > left.width(), "{left:?}");
    // Near the right edge: docked there, a column against it.
    let from = window.bounds(grip).center();
    window.drag(from, Point::new(1400.0, 450.0), false);
    assert_eq!(window.window.dock(), Dock::Right);
    let right = window.bounds(bar);
    assert!((right.x1 - (SIZE.width - 16.0)).abs() < 1.0, "{right:?}");
    assert_eq!(
        window.settings.borrow().get(&TOOLBAR_POSITION).unwrap(),
        Dock::Right
    );
    // Near the bottom: a row, centered.
    let from = window.bounds(grip).center();
    window.drag(from, Point::new(720.0, 880.0), false);
    assert_eq!(window.window.dock(), Dock::Bottom);
    let bottom = window.bounds(bar);
    assert!(bottom.width() > bottom.height());
    assert!((bottom.center().x - SIZE.width / 2.0).abs() < 1.0);
    assert!((bottom.y1 - (SIZE.height - 16.0)).abs() < 1.0);
    // In the middle: it floats where it was dropped.
    let from = window.bounds(grip).center();
    let corner = bottom.origin();
    window.drag(from, Point::new(700.0, 400.0), false);
    assert_eq!(window.window.dock(), Dock::Floating);
    let floating = window.bounds(bar);
    let moved = Point::new(700.0, 400.0) - from;
    assert!((floating.origin() - (corner + moved)).hypot() < 1.0);
    // Shift keeps it floating even near an edge.
    let from = window.bounds(grip).center();
    window.drag(from, Point::new(30.0, 450.0), true);
    assert_eq!(window.window.dock(), Dock::Floating);
    let shifted = window.bounds(bar);
    assert!(shifted.x0 >= 0.0, "kept inside the window: {shifted:?}");
    // A double-click on the grip puts it back on the left.
    let at = window.bounds(grip).center();
    window.input(Input::PointerMoved(at));
    window.press(at, true, 1, false);
    window.press(at, false, 1, false);
    window.press(at, true, 2, false);
    window.press(at, false, 2, false);
    assert_eq!(window.window.dock(), Dock::Left);
}

#[test]
fn a_dragged_tool_bar_stays_in_the_window() {
    let mut window = Harness::new();
    let grip = window.named("Move the tool bar: drag it to an edge");
    let bar = window.named("Tools");
    let from = window.bounds(grip).center();
    window.input(Input::PointerMoved(from));
    window.press(from, true, 1, false);
    window.input(Input::PointerMoved(Point::new(700.0, 2000.0)));
    let dragged = window.bounds(bar);
    assert!(dragged.y1 <= SIZE.height, "{dragged:?}");
    window.press(Point::new(700.0, 2000.0), false, 1, true);
}

#[test]
fn snapping_picks_the_nearest_edge_in_the_outer_quarter() {
    use crate::toolbar::snap;
    assert_eq!(snap(Point::new(30.0, 450.0), SIZE), Some(Dock::Left));
    assert_eq!(snap(Point::new(346.0, 396.0), SIZE), Some(Dock::Left));
    assert_eq!(snap(Point::new(1430.0, 10.0), SIZE), Some(Dock::Right));
    assert_eq!(snap(Point::new(700.0, 20.0), SIZE), Some(Dock::Top));
    assert_eq!(snap(Point::new(720.0, 450.0), SIZE), None);
}

#[test]
fn the_window_is_named_after_the_board() {
    let window = Harness::new();
    let options = window.window.options();
    assert_eq!(options.title, "Untitled board — Imaginarium");
    assert_eq!(options.app_id.as_deref(), Some("ch.giosuel.Imaginarium"));
}

#[test]
fn esc_closes_a_note_and_the_next_click_works() {
    let mut window = Harness::with(imaginarium_canvas::demo::launch_plan().unwrap());
    let share = window.named("Share");
    let at = window.bounds(share).center();
    window.click(at);
    assert!(window.texts().iter().any(|text| text.contains("Hub in M8")));
    // The pointer moves on, then Esc: the note goes, though the board has
    // the keys.
    window.input(Input::PointerMoved(Point::new(1300.0, 300.0)));
    window.input(Input::Key {
        key: imaginarium_ui::Key::Escape,
        text: None,
        pressed: true,
        repeat: false,
        modifiers: Modifiers::default(),
    });
    assert!(!window.texts().iter().any(|text| text.contains("Hub in M8")));
    // So the next click reaches its button instead of closing the note.
    let zoom_in = window.named("Zoom in");
    let at = window.bounds(zoom_in).center();
    window.click(at);
    assert!(window.board().zoom() > 1.0);
}

/// The tool bar's grip and the bar.
fn grip_and_bar(window: &mut Harness) -> (WidgetId, WidgetId) {
    (
        window.named("Move the tool bar: drag it to an edge"),
        window.named("Tools"),
    )
}

fn key_input(key: imaginarium_ui::Key, text: Option<&str>) -> Input {
    Input::Key {
        key,
        text: text.map(str::to_owned),
        pressed: true,
        repeat: false,
        modifiers: Modifiers::default(),
    }
}

#[test]
fn a_click_on_a_floating_bars_grip_leaves_it_and_a_double_click_docks_it_left() {
    let mut window = Harness::new();
    let (grip, bar) = grip_and_bar(&mut window);
    // Float it near the left edge, with Shift.
    let from = window.bounds(grip).center();
    window.drag(from, Point::new(60.0, 450.0), true);
    assert_eq!(window.window.dock(), Dock::Floating);
    let floating = window.bounds(bar);
    // A click (with a pixel of jitter) changes nothing, near the edge too.
    let at = window.bounds(grip).center();
    window.input(Input::PointerMoved(at));
    window.press(at, true, 1, false);
    window.input(Input::PointerMoved(at + (1.0, 0.5)));
    window.press(at + (1.0, 0.5), false, 1, false);
    assert_eq!(window.window.dock(), Dock::Floating);
    assert_eq!(window.bounds(bar), floating);
    // The second click of a double-click lands on the grip: docked left.
    window.press(at, true, 2, false);
    window.press(at, false, 2, false);
    assert_eq!(window.window.dock(), Dock::Left);
}

#[test]
fn a_drag_cut_short_by_losing_the_focus_puts_the_bar_back() {
    let mut window = Harness::new();
    let (grip, bar) = grip_and_bar(&mut window);
    let docked = window.bounds(bar);
    let from = window.bounds(grip).center();
    window.input(Input::PointerMoved(from));
    window.press(from, true, 1, false);
    window.input(Input::PointerMoved(Point::new(600.0, 400.0)));
    assert_eq!(window.window.dock(), Dock::Floating);
    window.input(Input::WindowFocus(false));
    window.input(Input::WindowFocus(true));
    assert_eq!(window.window.dock(), Dock::Left);
    assert_eq!(window.bounds(bar), docked);
    // Moving over the grip without a button drags nothing, and the hint
    // is gone.
    let at = window.bounds(grip).center();
    window.input(Input::PointerMoved(at));
    window.input(Input::PointerMoved(at + (40.0, 40.0)));
    assert_eq!(window.bounds(bar), docked);
    assert!(
        !window
            .texts()
            .iter()
            .any(|text| text.starts_with("Release"))
    );
}

#[test]
fn esc_closes_a_note_and_keeps_the_selection() {
    let mut window = Harness::with(imaginarium_canvas::demo::launch_plan().unwrap());
    let card = window.board().node_at(Point::new(1230.0, 518.0)).unwrap();
    window.click(Point::new(1230.0, 518.0));
    assert_eq!(window.board().selection().get_untracked(), vec![card]);
    let share = window.named("Share");
    let at = window.bounds(share).center();
    window.click(at);
    assert!(window.texts().iter().any(|text| text.contains("Hub in M8")));
    window.input(key_input(imaginarium_ui::Key::Escape, None));
    assert!(!window.texts().iter().any(|text| text.contains("Hub in M8")));
    assert_eq!(window.board().selection().get_untracked(), vec![card]);
}

#[test]
fn keys_on_a_focused_panel_button_reach_the_board() {
    let mut window = Harness::new();
    // Tab moves the keys from the board to the first panel button.
    window.input(key_input(imaginarium_ui::Key::Tab, None));
    let board = window.window.board_widget();
    let focused = window.window.ui().0.focused();
    assert!(focused.is_some() && focused != Some(board));
    // R is still the rectangle tool.
    window.input(key_input(
        imaginarium_ui::Key::Character("r".into()),
        Some("r"),
    ));
    assert_eq!(
        window.board().tool().get_untracked(),
        Tool::Shape(Preset::Rectangle)
    );
    // Esc on a panel goes back to the board.
    window.input(key_input(imaginarium_ui::Key::Tab, None));
    window.input(key_input(imaginarium_ui::Key::Escape, None));
    let board = window.window.board_widget();
    assert_eq!(window.window.ui().0.focused(), Some(board));
}

#[test]
fn a_docked_bar_keeps_clear_of_the_corner_panels() {
    let mut window = Harness::new();
    let (grip, bar) = grip_and_bar(&mut window);
    let undo = window.named("Undo");
    let help = window.named("Help and shortcuts");
    // Half of a 1920 screen, docked at the bottom.
    window.resize(Size::new(960.0, 1000.0));
    let from = window.bounds(grip).center();
    window.drag(from, Point::new(480.0, 990.0), false);
    assert_eq!(window.window.dock(), Dock::Bottom);
    window.resize(Size::new(960.0, 1000.0));
    let (bar_box, undo_box, help_box) =
        (window.bounds(bar), window.bounds(undo), window.bounds(help));
    let overlap = |a: Rect, b: Rect| a.intersect(b).area() > 0.0;
    assert!(
        !overlap(bar_box, undo_box) && !overlap(bar_box, help_box),
        "{bar_box:?}"
    );
    // A short window: docked left, the grip stays in reach.
    window
        .window
        .execute(
            "view.dock_toolbar",
            &imaginarium_commands::Args::new().with(
                "edge",
                imaginarium_commands::ArgValue::String("left".into()),
            ),
        )
        .unwrap();
    window.resize(Size::new(1000.0, 380.0));
    let grip_box = window.bounds(grip);
    assert!(grip_box.y0 >= 0.0 && grip_box.y1 <= 380.0, "{grip_box:?}");
}

#[test]
fn the_bar_follows_its_setting_and_a_command() {
    let mut window = Harness::new();
    let (_, bar) = grip_and_bar(&mut window);
    window
        .window
        .execute(
            "view.dock_toolbar",
            &imaginarium_commands::Args::new().with(
                "edge",
                imaginarium_commands::ArgValue::String("right".into()),
            ),
        )
        .unwrap();
    window.frame();
    assert_eq!(window.window.dock(), Dock::Right);
    assert!(window.bounds(bar).x1 > SIZE.width - 20.0);
    // Changed elsewhere (Settings, another window): the bar follows.
    window
        .settings
        .borrow_mut()
        .set(&TOOLBAR_POSITION, Dock::Top)
        .unwrap();
    window.frame();
    window.frame();
    assert_eq!(window.window.dock(), Dock::Top);
    assert!(window.bounds(bar).y0 < 20.0);
}

#[test]
fn a_floating_setting_survives_the_start() {
    let mut registry = Registry::new();
    registry.register_all(crate::SETTINGS).unwrap();
    registry.set(&TOOLBAR_POSITION, Dock::Floating).unwrap();
    let settings = Rc::new(RefCell::new(registry));
    let board = BoardCanvas::new(Document::new(DocumentKind::Board).unwrap());
    let window = BoardWindow::new(
        board,
        "Untitled board",
        Fonts::bundled_only(),
        settings.clone(),
    );
    assert_eq!(window.dock(), Dock::Floating);
    assert_eq!(
        settings.borrow().get(&TOOLBAR_POSITION).unwrap(),
        Dock::Floating
    );
}

/// Ctrl+K (⌘K on macOS).
fn mod_k() -> Input {
    let mac = cfg!(target_os = "macos");
    Input::Key {
        key: imaginarium_ui::Key::Character("k".into()),
        text: None,
        pressed: true,
        repeat: false,
        modifiers: Modifiers {
            ctrl: !mac,
            meta: mac,
            ..Modifiers::default()
        },
    }
}

fn type_text(window: &mut Harness, text: &str) {
    for c in text.chars() {
        let c = c.to_string();
        window.input(key_input(
            imaginarium_ui::Key::Character(c.clone()),
            Some(&c),
        ));
    }
}

fn palette_open(window: &mut Harness) -> bool {
    let (ui, fonts) = window.window.ui();
    ui.find_by_label(fonts, "Command palette").is_some()
}

#[test]
fn ctrl_k_finds_a_command_and_enter_runs_it() {
    let mut window = Harness::with(imaginarium_canvas::demo::launch_plan().unwrap());
    window.input(mod_k());
    assert!(palette_open(&mut window));
    type_text(&mut window, "select all");
    window.input(key_input(imaginarium_ui::Key::Enter, None));
    assert!(!palette_open(&mut window), "running closes it");
    assert!(
        window.board().selection().get_untracked().len() > 5,
        "Select all ran"
    );
    // Opened again without a query, it lists what was used first.
    window.input(mod_k());
    let texts = window.texts();
    let heading = |name: &str| texts.iter().position(|text| text == name);
    assert!(heading("Recent") < heading("Arrange"), "{texts:?}");
    let first = heading("Recent").unwrap() + 1;
    assert_eq!(texts.get(first).map(String::as_str), Some("Select all"));
}

#[test]
fn a_command_that_cant_run_says_why_and_stays() {
    let mut window = Harness::new();
    window.input(mod_k());
    type_text(&mut window, "redo");
    let (ui, fonts) = window.window.ui();
    assert!(
        ui.find_by_label(fonts, "Redo, unavailable: Nothing to redo")
            .is_some()
    );
    window.input(key_input(imaginarium_ui::Key::Enter, None));
    assert!(palette_open(&mut window));
    // Esc closes it and the keys go back to the board.
    window.input(key_input(imaginarium_ui::Key::Escape, None));
    assert!(!palette_open(&mut window));
    let board = window.window.board_widget();
    assert_eq!(window.window.ui().0.focused(), Some(board));
}

#[test]
fn a_setting_changes_in_place_and_the_bar_follows() {
    let mut window = Harness::new();
    // Ctrl+K, or ⌘K on macOS.
    let keys = crate::toolbar::shortcut(
        &crate::commands::shell_keymap(imaginarium_commands::Platform::current()),
        "palette.open",
    )
    .unwrap();
    let search = window.named(&format!("Search commands and settings ({keys})"));
    let at = window.bounds(search).center();
    window.click(at);
    assert!(palette_open(&mut window));
    type_text(&mut window, "tool bar position");
    assert!(window.texts().contains(&"Left".to_owned()));
    window.input(key_input(imaginarium_ui::Key::Tab, None));
    assert!(palette_open(&mut window), "a setting keeps it open");
    assert!(window.texts().contains(&"Top".to_owned()));
    window.frame();
    assert_eq!(window.window.dock(), Dock::Top);
    // A click beside it closes it.
    window.click(Point::new(40.0, 800.0));
    assert!(!palette_open(&mut window));
}

#[test]
fn arrows_pick_the_row_enter_runs() {
    let mut window = Harness::new();
    window.input(mod_k());
    type_text(&mut window, "zoom in");
    // Down and back up: the first row, "Zoom in", runs.
    window.input(key_input(imaginarium_ui::Key::ArrowDown, None));
    window.input(key_input(imaginarium_ui::Key::ArrowUp, None));
    window.input(key_input(imaginarium_ui::Key::Enter, None));
    assert!(!palette_open(&mut window));
    assert!(window.board().zoom() > 1.0);
}

/// The selected row (a screen reader hears it as selected) and the
/// results' box.
fn active_row(window: &mut Harness) -> (Rect, Rect) {
    let (ui, fonts) = window.window.ui();
    let mut tree = AccessTree::default();
    ui.access(fonts, &mut tree);
    let rect = |bounds: accesskit::Rect| Rect::new(bounds.x0, bounds.y0, bounds.x1, bounds.y1);
    let row = tree
        .nodes()
        .iter()
        .find(|(_, node)| node.is_selected() == Some(true))
        .and_then(|(_, node)| node.bounds())
        .map(rect)
        .unwrap();
    let results = tree
        .nodes()
        .iter()
        .find(|(_, node)| {
            node.label() == Some("Results") && node.role() != accesskit::Role::ListBox
        })
        .and_then(|(_, node)| node.bounds())
        .map(rect)
        .unwrap();
    (row, results)
}

#[test]
fn a_new_query_shows_its_first_row() {
    let mut window = Harness::new();
    window.input(mod_k());
    type_text(&mut window, "e");
    for _ in 0..25 {
        window.input(key_input(imaginarium_ui::Key::ArrowDown, None));
    }
    window.input(key_input(imaginarium_ui::Key::Backspace, None));
    let (row, results) = active_row(&mut window);
    assert!(results.contains(row.center()), "{row:?} in {results:?}");
}

#[test]
fn typing_reuses_rows_and_closing_frees_them() {
    let mut window = Harness::new();
    let baseline = imaginarium_reactive::live_nodes_for_tests();
    window.input(mod_k());
    type_text(&mut window, "tool");
    window.input(key_input(imaginarium_ui::Key::Backspace, None));
    let after_one = imaginarium_reactive::live_nodes_for_tests();
    for _ in 0..10 {
        type_text(&mut window, "l");
        window.input(key_input(imaginarium_ui::Key::Backspace, None));
    }
    assert_eq!(imaginarium_reactive::live_nodes_for_tests(), after_one);
    window.input(key_input(imaginarium_ui::Key::Escape, None));
    assert_eq!(imaginarium_reactive::live_nodes_for_tests(), baseline);
}

#[test]
fn the_palette_follows_the_window_size() {
    let mut window = Harness::new();
    window.input(mod_k());
    window.resize(Size::new(800.0, 500.0));
    let panel = window.named("Command palette");
    let panel = window.bounds(panel);
    assert!((panel.center().x - 400.0).abs() < 1.0, "{panel:?}");
    assert!(panel.y1 <= 500.0, "{panel:?}");
}

#[test]
fn a_matched_synonym_keeps_its_place_and_shift_tab_steps_back() {
    let mut window = Harness::new();
    window.input(mod_k());
    type_text(&mut window, "artboard");
    assert!(window.texts().iter().any(|text| text == " · also artboard"));
    // Shift+Tab steps the tool bar's position back: Left → Floating.
    for _ in 0..8 {
        window.input(key_input(imaginarium_ui::Key::Backspace, None));
    }
    type_text(&mut window, "tool bar position");
    window.input(Input::Key {
        key: imaginarium_ui::Key::Tab,
        text: None,
        pressed: true,
        repeat: false,
        modifiers: Modifiers {
            shift: true,
            ..Modifiers::default()
        },
    });
    assert!(window.texts().contains(&"Floating".to_owned()));
}

#[test]
#[ignore = "a timing: cargo test --release -p imaginarium-shell -- --ignored --nocapture"]
fn a_keystroke_costs_well_under_a_frame() {
    let mut window = Harness::new();
    window.input(mod_k());
    type_text(&mut window, "e");
    window.input(key_input(imaginarium_ui::Key::Backspace, None));
    let mut worst = std::time::Duration::ZERO;
    let mut total = std::time::Duration::ZERO;
    let letters = "selectzoomtool";
    for c in letters.chars() {
        let started = std::time::Instant::now();
        type_text(&mut window, &c.to_string());
        let spent = started.elapsed();
        worst = worst.max(spent);
        total += spent;
        window.input(key_input(imaginarium_ui::Key::Backspace, None));
    }
    eprintln!(
        "keystroke + frame: average {:?}, worst {worst:?}",
        total / letters.len() as u32
    );
}

fn demo() -> Harness {
    Harness::with(imaginarium_canvas::demo::launch_plan().unwrap())
}

fn shows(window: &mut Harness, label: &str) -> bool {
    let (ui, fonts) = window.window.ui();
    ui.find_by_label(fonts, label).is_some()
}

fn style(window: &Harness, node: imaginarium_model::NodeId, prop: &str) -> Option<String> {
    window
        .board()
        .document()
        .view()
        .style_prop(node, prop)
        .and_then(imaginarium_model::Value::as_str)
        .map(str::to_owned)
}

/// The "Interview 5 users" card of the demo board, and a point on it.
const CARD: Point = Point::new(330.0, 320.0);

#[test]
fn the_bar_comes_with_a_selection_above_it_and_fills_it() {
    let mut window = demo();
    assert!(!shows(&mut window, "Selection"), "no selection, no bar");
    window.click(CARD);
    let card = window.board().selection().get_untracked()[0];
    assert!(shows(&mut window, "Selection"));
    let bar = window.named("Selection");
    let (bar_box, selection) = (
        window.bounds(bar),
        window.board().selection_on_screen().unwrap(),
    );
    assert!(
        bar_box.y1 <= selection.y0,
        "above: {bar_box:?} {selection:?}"
    );
    assert!((bar_box.center().x - selection.center().x).abs() < 1.0);
    // A rectangle: fill, outline, width, corner radius; no text color.
    for control in [
        "Fill",
        "Outline",
        "Outline width",
        "Arrange",
        "Duplicate",
        "More",
    ] {
        assert!(shows(&mut window, control), "{control}");
    }
    assert!(!shows(&mut window, "Text color"));
    // Fill › Amber.
    let fill = window.named("Fill");
    let at = window.bounds(fill).center();
    window.click(at);
    let amber = window.named("Amber");
    let at = window.bounds(amber).center();
    window.click(at);
    assert_eq!(style(&window, card, "fill").as_deref(), Some("#fef3c7"));
    // A typed color, with its opacity.
    let fill = window.named("Fill");
    let at = window.bounds(fill).center();
    window.click(at);
    let hex = window.named("Hex color");
    let at = window.bounds(hex).center();
    window.click(at);
    type_text(&mut window, "16A34A");
    window.input(key_input(imaginarium_ui::Key::Enter, None));
    assert_eq!(style(&window, card, "fill").as_deref(), Some("#16a34a"));
}

#[test]
fn widths_and_radii_come_from_their_popovers() {
    let mut window = demo();
    window.click(CARD);
    let card = window.board().selection().get_untracked()[0];
    let width = window.named("Outline width");
    let at = window.bounds(width).center();
    window.click(at);
    let bold = window.named("Bold · 3");
    let at = window.bounds(bold).center();
    window.click(at);
    let view = window.board().document().view();
    assert_eq!(
        view.style_prop(card, "stroke_w"),
        Some(&imaginarium_model::Value::Number(3.0))
    );
    // Corner radius › Round ends: half the card's shorter side.
    let radius = window.named("Corner radius, 12");
    let at = window.bounds(radius).center();
    window.click(at);
    let round = window.named("Round ends");
    let at = window.bounds(round).center();
    window.click(at);
    let view = window.board().document().view();
    let size = view.node(card).unwrap().geometry().unwrap();
    assert_eq!(
        view.style_prop(card, "radius"),
        Some(&imaginarium_model::Value::Number(size.w.min(size.h) / 2.0))
    );
}

#[test]
fn the_bar_steps_aside_during_a_drag() {
    let mut window = demo();
    window.click(CARD);
    assert!(shows(&mut window, "Selection"));
    window.input(Input::PointerMoved(CARD));
    window.press(CARD, true, 1, false);
    for step in 1..=4 {
        window.input(Input::PointerMoved(CARD + (10.0 * f64::from(step), 0.0)));
    }
    assert!(!shows(&mut window, "Selection"), "hidden while moving");
    window.press(CARD + (40.0, 0.0), false, 1, false);
    assert!(shows(&mut window, "Selection"), "back after");
}

#[test]
fn a_text_box_gets_a_text_color_and_no_fill() {
    let mut window = demo();
    window.click(Point::new(1200.0, 300.0));
    assert_eq!(window.board().selection().get_untracked().len(), 1);
    assert!(shows(&mut window, "Text color"));
    assert!(!shows(&mut window, "Fill"));
}

#[test]
fn more_and_the_context_menu_run_the_selections_commands() {
    let mut window = demo();
    let before = window.board().document().view().node_count();
    // A right-click on a card selects it and opens its menu.
    window.input(Input::PointerMoved(CARD));
    window.input(Input::PointerButton {
        primary: false,
        pressed: true,
        position: CARD,
        clicks: 1,
        modifiers: Modifiers::default(),
    });
    window.frame();
    assert_eq!(window.board().selection().get_untracked().len(), 1);
    assert!(shows(&mut window, "Cut") && shows(&mut window, "Bring to front"));
    // ↓ moves through the rows; Delete runs from its row (the menu's, not
    // the bar's button of the same name).
    window.input(key_input(imaginarium_ui::Key::ArrowDown, None));
    let at = window.item_center("Delete", accesskit::Role::MenuItem);
    window.click(at);
    assert_eq!(window.board().document().view().node_count(), before - 1);
    // On the empty board: the board's menu.
    let empty = Point::new(900.0, 750.0);
    window.input(Input::PointerMoved(empty));
    window.input(Input::PointerButton {
        primary: false,
        pressed: true,
        position: empty,
        clicks: 1,
        modifiers: Modifiers::default(),
    });
    window.frame();
    assert!(shows(&mut window, "Select all"));
    let all = window.named("Select all");
    let at = window.bounds(all).center();
    window.click(at);
    assert!(window.board().selection().get_untracked().len() > 5);
    // More on the bar lists the same commands.
    let more = window.named("More");
    let at = window.bounds(more).center();
    window.click(at);
    assert!(shows(&mut window, "Copy"));
}

fn key_with(key: imaginarium_ui::Key, shift: bool) -> Input {
    Input::Key {
        key,
        text: None,
        pressed: true,
        repeat: false,
        modifiers: Modifiers {
            shift,
            ..Modifiers::default()
        },
    }
}

#[test]
fn a_short_window_with_a_selection_draws() {
    let mut window = demo();
    window.click(CARD);
    for height in [179.0, 120.0, 60.0, 10.0, 1.0] {
        window.resize(Size::new(1440.0, height));
    }
}

#[test]
fn opacity_keeps_each_cards_color_and_the_fill_is_the_first_filled_nodes() {
    let mut window = demo();
    // A text box first, then a card: the bar's fill is the card's.
    window.click(Point::new(1200.0, 300.0));
    window.input(Input::PointerMoved(CARD));
    window.press(CARD, true, 1, true);
    window.press(CARD, false, 1, true);
    assert_eq!(window.board().selection().get_untracked().len(), 2);
    assert_eq!(
        window.window.bar_fill_for_tests().as_deref(),
        Some("#e0e7ff")
    );
    // Add the amber card and make both fills half opaque.
    let amber_at = Point::new(600.0, 320.0);
    window.input(Input::PointerMoved(amber_at));
    window.press(amber_at, true, 1, true);
    window.press(amber_at, false, 1, true);
    let fill = window.named("Fill");
    let at = window.bounds(fill).center();
    window.click(at);
    let at = window.item_center("Opacity, percent", accesskit::Role::TextInput);
    window.click(at);
    type_text(&mut window, "50");
    window.input(key_input(imaginarium_ui::Key::Enter, None));
    let view = window.board().document().view();
    let fills: Vec<String> = window
        .board()
        .selection()
        .get_untracked()
        .iter()
        .filter_map(|node| view.style_prop(*node, "fill"))
        .filter_map(imaginarium_model::Value::as_str)
        .map(str::to_ascii_lowercase)
        .collect();
    assert_eq!(fills.len(), 2, "{fills:?}");
    assert!(fills.contains(&"#e0e7ff80".to_owned()), "{fills:?}");
    assert!(
        fills.iter().any(|fill| fill.starts_with("#fef3c7")),
        "{fills:?}"
    );
}

#[test]
fn the_keyboard_reaches_the_bar_and_the_context_menu() {
    let mut window = demo();
    window.click(CARD);
    // Shift+F10: the selection's menu.
    window.input(key_with(imaginarium_ui::Key::Other("F10".into()), true));
    assert!(shows(&mut window, "Cut"));
    window.input(key_input(imaginarium_ui::Key::Escape, None));
    // F6: from the board to the contextual toolbar's first control.
    window.input(key_with(imaginarium_ui::Key::Other("F6".into()), false));
    let fill = window.named("Fill");
    assert_eq!(window.window.ui().0.focused(), Some(fill));
    // Shift+F6 goes back to the board.
    window.input(key_with(imaginarium_ui::Key::Other("F6".into()), true));
    let board = window.window.board_widget();
    assert_eq!(window.window.ui().0.focused(), Some(board));
}

#[test]
fn right_clicks_keep_a_selection_and_offer_unlock_on_the_board() {
    let mut window = demo();
    let right = |window: &mut Harness, at: Point| {
        window.input(Input::PointerMoved(at));
        window.input(Input::PointerButton {
            primary: false,
            pressed: true,
            position: at,
            clicks: 1,
            modifiers: Modifiers::default(),
        });
        window.frame();
    };
    // Two cards, then a right-click on the empty space between them.
    window.click(CARD);
    let amber_at = Point::new(600.0, 320.0);
    window.input(Input::PointerMoved(amber_at));
    window.press(amber_at, true, 1, true);
    window.press(amber_at, false, 1, true);
    right(&mut window, Point::new(510.0, 300.0));
    assert_eq!(window.board().selection().get_untracked().len(), 2);
    window.input(key_input(imaginarium_ui::Key::Escape, None));
    // Locked: a right-click on it gives the board's menu, with Unlock.
    window
        .window
        .execute("board.lock", &imaginarium_commands::Args::new())
        .unwrap();
    window.frame();
    right(&mut window, CARD);
    let at = window.item_center("Unlock", accesskit::Role::MenuItem);
    window.click(at);
    let view = window.board().document().view();
    assert!(
        view.walk()
            .iter()
            .all(|node| !view.node(*node).unwrap().locked())
    );
}

#[test]
fn a_new_shape_has_its_bar_in_the_same_frame() {
    let mut window = Harness::new();
    let args = imaginarium_commands::Args::new().with(
        "preset",
        imaginarium_commands::ArgValue::String("rect".into()),
    );
    window.window.execute("board.insert_shape", &args).unwrap();
    window.frame();
    assert!(shows(&mut window, "Selection"));
}

#[test]
#[ignore = "a timing: cargo test --release -p imaginarium-shell -- --ignored --nocapture"]
fn a_frame_with_ten_thousand_selected_stays_cheap() {
    let mut window = Harness::with(imaginarium_canvas::demo::grid(10_000).unwrap());
    let time = |window: &mut Harness, what: &str| {
        for _ in 0..3 {
            window.frame();
        }
        let frames = 20;
        let started = std::time::Instant::now();
        for _ in 0..frames {
            window.frame();
        }
        eprintln!("{what}: {:?} a frame", started.elapsed() / frames);
    };
    time(&mut window, "nothing selected");
    window
        .window
        .execute("board.select_all", &imaginarium_commands::Args::new())
        .unwrap();
    time(&mut window, "10,000 selected");
    let started = std::time::Instant::now();
    let mut fonts = Fonts::bundled_only();
    for _ in 0..20 {
        let mut scene = Scene::new();
        let board = window.window.board_mut_for_tests().unwrap();
        board.paint_into(
            &mut fonts,
            &mut scene,
            Rect::new(0.0, 0.0, 1440.0, 900.0),
            std::time::Instant::now(),
        );
    }
    eprintln!(
        "the board alone, 10,000 selected: {:?} a frame",
        started.elapsed() / 20
    );
}

/// Ctrl+I (⌘I on macOS).
fn mod_i() -> Input {
    let mac = cfg!(target_os = "macos");
    Input::Key {
        key: imaginarium_ui::Key::Character("i".into()),
        text: None,
        pressed: true,
        repeat: false,
        modifiers: Modifiers {
            ctrl: !mac,
            meta: mac,
            ..Modifiers::default()
        },
    }
}

/// Types `text` into the field a screen reader calls `label`, then Enter.
fn enter_in(window: &mut Harness, label: &str, text: &str) {
    let at = window.item_center(label, accesskit::Role::TextInput);
    window.click(at);
    // Select what is there and type over it.
    let mac = cfg!(target_os = "macos");
    window.input(Input::Key {
        key: imaginarium_ui::Key::Character("a".into()),
        text: None,
        pressed: true,
        repeat: false,
        modifiers: Modifiers {
            ctrl: !mac,
            meta: mac,
            ..Modifiers::default()
        },
    });
    type_text(window, text);
    window.input(key_input(imaginarium_ui::Key::Enter, None));
}

#[test]
fn ctrl_i_shows_the_selection_and_its_fields_change_it() {
    let mut window = demo();
    window.input(mod_i());
    assert!(
        window
            .texts()
            .iter()
            .any(|text| text.starts_with("Select something"))
    );
    window.click(CARD);
    let card = window.board().selection().get_untracked()[0];
    assert!(window.texts().contains(&"Rectangle".to_owned()));
    let before = window
        .board()
        .document()
        .view()
        .node(card)
        .unwrap()
        .geometry()
        .unwrap();
    // Width, with Keep proportions on: the height follows.
    enter_in(&mut window, "Width", "440");
    let after = window
        .board()
        .document()
        .view()
        .node(card)
        .unwrap()
        .geometry()
        .unwrap();
    assert_eq!(after.w, 440.0);
    assert!((after.h - before.h * 2.0).abs() < 1e-6);
    // The keys stay in the field after the panel caught up.
    let (ui, fonts) = window.window.ui();
    let width = ui.find_by_label(fonts, "Width");
    assert_eq!(ui.focused(), width);
    // A name.
    enter_in(&mut window, "Name", "First step");
    assert_eq!(
        window.board().document().view().node(card).unwrap().name(),
        Some("First step")
    );
    // Another type, from its menu.
    let at = window.item_center("Shape type", accesskit::Role::Button);
    window.click(at);
    let at = window.item_center("Ellipse", accesskit::Role::MenuItem);
    window.click(at);
    let view = window.board().document().view();
    assert_eq!(
        imaginarium_canvas::commands_style::preset_of(view.node(card).unwrap()),
        "ellipse"
    );
    // Lock: the panel then offers Unlock.
    let at = window.item_center("Lock", accesskit::Role::Button);
    window.click(at);
    assert!(
        window
            .board()
            .document()
            .view()
            .node(card)
            .unwrap()
            .locked()
    );
    assert!(shows(&mut window, "Unlock"));
    // Ctrl+I closes it.
    window.input(mod_i());
    assert!(!shows(&mut window, "Inspector"));
}

#[test]
fn reset_to_style_takes_the_nodes_own_values_away() {
    let mut window = demo();
    window.input(mod_i());
    window.click(CARD);
    let card = window.board().selection().get_untracked()[0];
    enter_in(&mut window, "Stroke width", "4");
    let view = window.board().document().view();
    assert!(!view.node(card).unwrap().style_overrides().is_empty());
    // The panel scrolls at this height: bring the button into view.
    {
        let (ui, fonts) = window.window.ui();
        let reset = ui.find_by_label(fonts, "Reset to style").unwrap();
        ui.reveal(fonts, reset);
    }
    window.frame();
    let at = window.item_center("Reset to style", accesskit::Role::Button);
    window.click(at);
    assert!(
        window
            .board()
            .document()
            .view()
            .node(card)
            .unwrap()
            .style_overrides()
            .is_empty()
    );
}

#[test]
fn a_hidden_card_comes_back_from_the_boards_menu() {
    let mut window = demo();
    window.input(mod_i());
    window.click(CARD);
    let card = window.board().selection().get_untracked()[0];
    let at = window.item_center("Hide", accesskit::Role::Button);
    window.click(at);
    assert!(
        window
            .board()
            .document()
            .view()
            .node(card)
            .unwrap()
            .hidden()
    );
    let empty = Point::new(900.0, 750.0);
    window.input(Input::PointerMoved(empty));
    window.input(Input::PointerButton {
        primary: false,
        pressed: true,
        position: empty,
        clicks: 1,
        modifiers: Modifiers::default(),
    });
    window.frame();
    let at = window.item_center("Show hidden", accesskit::Role::MenuItem);
    window.click(at);
    assert!(
        !window
            .board()
            .document()
            .view()
            .node(card)
            .unwrap()
            .hidden()
    );
}

#[test]
fn a_slider_keeps_the_keys_when_the_panel_catches_up() {
    let mut window = demo();
    window.input(mod_i());
    window.click(CARD);
    let card = window.board().selection().get_untracked()[0];
    let before = window
        .board()
        .document()
        .view()
        .node(card)
        .unwrap()
        .geometry()
        .unwrap();
    let slider = window.named("Opacity");
    window
        .window
        .ui()
        .0
        .set_focus(&mut Fonts::bundled_only(), Some(slider));
    for _ in 0..3 {
        window.input(key_input(imaginarium_ui::Key::ArrowLeft, None));
    }
    let view = window.board().document().view();
    assert_eq!(
        view.style_prop(card, "opacity"),
        Some(&imaginarium_model::Value::Number(0.85))
    );
    // The arrows never reached the board.
    assert_eq!(view.node(card).unwrap().geometry().unwrap(), before);
}

#[test]
fn a_field_with_nonsense_shows_the_value_again() {
    let mut window = demo();
    window.input(mod_i());
    window.click(CARD);
    let steps = window
        .board()
        .document()
        .can_undo(imaginarium_model::UndoScope::User);
    let _ = steps;
    enter_in(&mut window, "X", "left");
    let (ui, fonts) = window.window.ui();
    let x = ui.find_by_label(fonts, "X").unwrap();
    let shown = ui.widget::<imaginarium_ui::TextInput>(x).unwrap().text();
    assert_eq!(shown, "36");
}

#[test]
fn a_typed_hex_keeps_the_fills_opacity() {
    let mut window = demo();
    window.input(mod_i());
    window.click(CARD);
    let card = window.board().selection().get_untracked()[0];
    enter_in(&mut window, "Fill opacity, percent", "50");
    enter_in(&mut window, "Fill, hex", "16A34A");
    let view = window.board().document().view();
    let fill = view
        .style_prop(card, "fill")
        .and_then(imaginarium_model::Value::as_str)
        .map(str::to_ascii_lowercase);
    assert_eq!(fill.as_deref(), Some("#16a34a80"));
}

#[test]
fn the_inspector_leaves_help_and_a_right_bar_free() {
    let mut window = demo();
    window.input(mod_i());
    window.click(CARD);
    window
        .window
        .execute(
            "view.dock_toolbar",
            &imaginarium_commands::Args::new().with(
                "edge",
                imaginarium_commands::ArgValue::String("right".into()),
            ),
        )
        .unwrap();
    window.frame();
    window.frame();
    let panel = window.named("Inspector");
    let (panel, bar, help) = (
        window.bounds(panel),
        {
            let bar = window.named("Tools");
            window.bounds(bar)
        },
        {
            let help = window.named("Help and shortcuts");
            window.bounds(help)
        },
    );
    let overlap = |a: Rect, b: Rect| a.intersect(b).area() > 0.0;
    assert!(!overlap(panel, bar), "{panel:?} {bar:?}");
    assert!(!overlap(panel, help), "{panel:?} {help:?}");
}

#[test]
fn the_palette_hides_window_commands_outside_the_app() {
    // A board window on its own (as here) can't open windows.
    let mut window = Harness::new();
    window.input(mod_k());
    type_text(&mut window, "new board");
    let (ui, fonts) = window.window.ui();
    assert!(ui.find_by_label(fonts, "New board").is_none());
    assert!(ui.find_by_label(fonts, "Close window").is_none());
}

#[test]
fn the_board_background_follows_its_setting() {
    let mut window = Harness::new();
    assert_eq!(
        window.board().background(),
        imaginarium_canvas::Background::Dots
    );
    window
        .settings
        .borrow_mut()
        .set(
            &imaginarium_canvas::BACKGROUND,
            imaginarium_canvas::Background::Grid,
        )
        .unwrap();
    window.frame();
    assert_eq!(
        window.board().background(),
        imaginarium_canvas::Background::Grid
    );
}
