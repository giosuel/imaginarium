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
