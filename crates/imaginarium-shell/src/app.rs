// SPDX-License-Identifier: AGPL-3.0-or-later

//! What the app's windows share (`shell.md` §2): one window per document,
//! the start window at startup and when the last board closes.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use imaginarium_canvas::BoardCanvas;
use imaginarium_model::{Document, DocumentKind, ModelError};
use imaginarium_render::TileWorkers;
use imaginarium_settings::{Registry, Scope, Setting};
use imaginarium_text::Fonts;
use imaginarium_ui::{View, WindowOptions};

use crate::start::StartWindow;
use crate::window::BoardWindow;

/// Whether the start window opens with the app (`shell.md` §4).
pub static SHOW_START_WINDOW: Setting<bool> = Setting::new(
    "startup.show_start_window",
    || true,
    Scope::App,
    "Show the start window at startup",
    "Open the start window (new board, recent boards) when the app starts, instead of a new, \
     empty board.",
)
.synonyms(&["start window", "splash", "welcome", "startup"]);

/// The app's windows and what they share: the fonts (scanned once), the
/// settings, the tile workers. Windows hold it in an `Rc`; they all run on
/// the UI thread.
pub struct App {
    fonts: Fonts,
    settings: Rc<RefCell<Registry>>,
    workers: TileWorkers,
    version: String,
    /// The board windows open now.
    boards: Cell<usize>,
    /// The Settings window is open.
    settings_open: Cell<bool>,
    /// Whether the settings file is saved, and a notice from reading it.
    saving: RefCell<Saving>,
}

/// What the Settings window says about the settings file.
#[derive(Default)]
struct Saving {
    status: Option<imaginarium_settings::SaveStatus>,
    notice: Option<String>,
}

impl App {
    /// The app, with `fonts` for its windows and `settings` (which must
    /// hold [`crate::SETTINGS`]). `version` is shown in the start window.
    pub fn new(
        fonts: Fonts,
        settings: Rc<RefCell<Registry>>,
        workers: TileWorkers,
        version: impl Into<String>,
    ) -> Rc<Self> {
        Rc::new(Self {
            fonts,
            settings,
            workers,
            version: version.into(),
            boards: Cell::new(0),
            settings_open: Cell::new(false),
            saving: RefCell::default(),
        })
    }

    /// Tells the Settings window how saving goes (`status`), and what
    /// happened reading the file at startup (`notice`: it was unreadable
    /// and moved aside), so users who never see standard error learn it.
    pub fn report_saving(
        &self,
        status: Option<imaginarium_settings::SaveStatus>,
        notice: Option<String>,
    ) {
        *self.saving.borrow_mut() = Saving { status, notice };
    }

    /// What the Settings window shows about the file: a failed save, else
    /// the notice from startup.
    pub(crate) fn save_notice(&self) -> Option<String> {
        let saving = self.saving.borrow();
        saving
            .status
            .as_ref()
            .and_then(imaginarium_settings::SaveStatus::error)
            .map(|error| format!("Settings can't be saved: {error}"))
            .or_else(|| saving.notice.clone())
    }

    /// The Settings window, unless one is open already.
    pub fn settings_window(self: &Rc<Self>) -> Option<crate::settings::SettingsWindow> {
        if self.settings_open.replace(true) {
            return None;
        }
        Some(crate::settings::SettingsWindow::new(self.clone()))
    }

    pub(crate) fn settings_closed(&self) {
        self.settings_open.set(false);
    }

    /// The first window: the start window, or a new board when `new` or
    /// the settings say not to show it. `close_after` closes it after that
    /// many frames (scripted runs).
    pub fn first_window(
        self: &Rc<Self>,
        new: bool,
        close_after: Option<u64>,
    ) -> Result<(WindowOptions, Box<dyn View>), ModelError> {
        if !new && self.shows_start_window() {
            let mut window = self.start_window();
            if let Some(frames) = close_after {
                window = window.close_after(frames);
            }
            Ok((window.options(), Box::new(window)))
        } else {
            let mut window = self.new_board()?;
            if let Some(frames) = close_after {
                window = window.close_after(frames);
            }
            Ok((window.options(), Box::new(window)))
        }
    }

    /// Whether the app starts with the start window (the setting).
    pub fn shows_start_window(&self) -> bool {
        self.settings
            .borrow()
            .get(&SHOW_START_WINDOW)
            .unwrap_or(true)
    }

    /// The start window.
    pub fn start_window(self: &Rc<Self>) -> StartWindow {
        StartWindow::new(self.clone())
    }

    /// A window on a new, untitled board.
    pub fn new_board(self: &Rc<Self>) -> Result<BoardWindow, ModelError> {
        let document = Document::new(DocumentKind::Board)?;
        Ok(self.board_window(document, "Untitled board"))
    }

    /// A window on `document`, named `name`.
    pub fn board_window(self: &Rc<Self>, document: Document, name: &str) -> BoardWindow {
        let board = BoardCanvas::new(document).with_workers(self.workers.clone());
        BoardWindow::new(board, name, self.fonts.share(), self.settings.clone())
            .in_app(self.clone())
    }

    /// The settings.
    pub fn settings(&self) -> &Rc<RefCell<Registry>> {
        &self.settings
    }

    /// The fonts, for another window.
    pub(crate) fn fonts(&self) -> Fonts {
        self.fonts.share()
    }

    /// The version shown in the start window.
    pub(crate) fn version(&self) -> &str {
        &self.version
    }

    /// The board windows open now.
    pub fn boards(&self) -> usize {
        self.boards.get()
    }

    pub(crate) fn board_opened(&self) {
        self.boards.set(self.boards.get() + 1);
    }

    /// A board window closes; returns how many stay open.
    pub(crate) fn board_closed(&self) -> usize {
        let left = self.boards.get().saturating_sub(1);
        self.boards.set(left);
        left
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use imaginarium_render::kurbo::Size;
    use imaginarium_render::{CpuMode, Scene};
    use imaginarium_ui::{Input, Key, Modifiers, View, WindowRequest};

    use super::*;

    fn app() -> Rc<App> {
        let mut registry = Registry::new();
        registry.register_all(crate::SETTINGS).unwrap();
        App::new(
            Fonts::bundled_only(),
            Rc::new(RefCell::new(registry)),
            TileWorkers::new(1, CpuMode::Fast),
            "0.0.0-test",
        )
    }

    fn names(requests: &[WindowRequest]) -> Vec<String> {
        requests
            .iter()
            .map(|request| format!("{request:?}"))
            .collect()
    }

    /// Mod+`key`: Ctrl, or ⌘ on macOS.
    fn ctrl(key: &str) -> Input {
        let mac = cfg!(target_os = "macos");
        Input::Key {
            key: Key::Character(key.into()),
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

    fn frame(view: &mut dyn View) {
        let mut scene = Scene::new();
        view.paint(&mut scene, Size::new(960.0, 600.0));
    }

    #[test]
    fn new_board_in_the_start_window_opens_a_board_and_closes_it() {
        let app = app();
        let mut start = app.start_window();
        frame(&mut start);
        let (ui, fonts) = start.ui();
        let button = ui.find_by_label(fonts, "New board").unwrap();
        let at = ui.bounds(button).unwrap().center();
        for pressed in [true, false] {
            start.input(&Input::PointerButton {
                primary: true,
                pressed,
                position: at,
                clicks: 1,
                modifiers: Modifiers::default(),
            });
        }
        assert_eq!(
            names(&start.take_requests()),
            ["Open(\"Untitled board — Imaginarium\")", "Close"]
        );
        // Ctrl+N does the same.
        let mut start = app.start_window();
        start.input(&ctrl("n"));
        assert_eq!(
            names(&start.take_requests()),
            ["Open(\"Untitled board — Imaginarium\")", "Close"]
        );
    }

    #[test]
    fn closing_the_last_board_opens_the_start_window() {
        let app = app();
        let mut first = app.new_board().unwrap();
        let mut second = app.new_board().unwrap();
        assert_eq!(app.boards(), 2);
        assert!(second.close_requested());
        assert!(names(&second.take_requests()).is_empty());
        drop(second);
        assert_eq!(app.boards(), 1);
        // Ctrl+W on the last one.
        first.input(&ctrl("w"));
        assert_eq!(
            names(&first.take_requests()),
            ["Open(\"Imaginarium\")", "Close"]
        );
        drop(first);
        assert_eq!(app.boards(), 0);
    }

    #[test]
    fn boards_closing_together_still_open_the_start_window() {
        // Both are asked before either is dropped (one batch of events).
        let app = app();
        let mut first = app.new_board().unwrap();
        let mut second = app.new_board().unwrap();
        assert!(first.close_requested());
        assert!(second.close_requested());
        let opened = [first.take_requests(), second.take_requests()]
            .iter()
            .flat_map(|requests| names(requests))
            .filter(|name| name == "Open(\"Imaginarium\")")
            .count();
        assert_eq!(opened, 1);
        drop((first, second));
        assert_eq!(app.boards(), 0);
    }

    #[test]
    fn new_board_in_a_board_opens_another_window() {
        let app = app();
        let mut board = app.new_board().unwrap();
        board.input(&ctrl("n"));
        let requests = board.take_requests();
        assert_eq!(names(&requests), ["Open(\"Untitled board — Imaginarium\")"]);
        assert_eq!(app.boards(), 2);
        drop(requests);
        assert_eq!(app.boards(), 1);
    }

    #[test]
    fn the_setting_picks_the_first_window_and_the_switch_changes_it() {
        let app = app();
        let (options, _) = app.first_window(false, None).unwrap();
        assert_eq!(options.title, "Imaginarium");
        let (options, _) = app.first_window(true, None).unwrap();
        assert_eq!(options.title, "Untitled board — Imaginarium");
        let mut start = app.start_window();
        frame(&mut start);
        let (ui, fonts) = start.ui();
        let switch = ui.find_by_label(fonts, "Show at startup").unwrap();
        let at = ui.bounds(switch).unwrap().center();
        for pressed in [true, false] {
            start.input(&Input::PointerButton {
                primary: true,
                pressed,
                position: at,
                clicks: 1,
                modifiers: Modifiers::default(),
            });
        }
        assert!(!app.shows_start_window());
        let (options, _) = app.first_window(false, None).unwrap();
        assert_eq!(options.title, "Untitled board — Imaginarium");
    }

    fn repeated(input: Input) -> Input {
        match input {
            Input::Key {
                key,
                text,
                pressed,
                modifiers,
                ..
            } => Input::Key {
                key,
                text,
                pressed,
                repeat: true,
                modifiers,
            },
            other => other,
        }
    }

    #[test]
    fn a_held_ctrl_n_opens_one_board() {
        let app = app();
        let mut board = app.new_board().unwrap();
        board.input(&ctrl("n"));
        board.input(&repeated(ctrl("n")));
        board.input(&repeated(ctrl("n")));
        assert_eq!(board.take_requests().len(), 1);
        // The start window asks once, even pressed twice before it closes.
        let mut start = app.start_window();
        start.input(&ctrl("n"));
        start.input(&ctrl("n"));
        assert_eq!(
            names(&start.take_requests()),
            ["Open(\"Untitled board — Imaginarium\")", "Close"]
        );
        start.input(&ctrl("n"));
        assert!(start.take_requests().is_empty());
    }

    #[test]
    fn a_held_ctrl_w_closes_once() {
        let app = app();
        let mut first = app.new_board().unwrap();
        let _second = app.new_board().unwrap();
        first.input(&ctrl("w"));
        first.input(&repeated(ctrl("w")));
        assert_eq!(names(&first.take_requests()), ["Close"]);
        assert_eq!(app.boards(), 1);
    }

    #[test]
    fn window_commands_need_the_app() {
        // A board window outside the app (tests, embedding) can't open
        // windows: the palette doesn't offer it.
        let mut registry = Registry::new();
        registry.register_all(crate::SETTINGS).unwrap();
        let mut alone = BoardWindow::new(
            BoardCanvas::new(Document::new(DocumentKind::Board).unwrap()),
            "Untitled board",
            Fonts::bundled_only(),
            Rc::new(RefCell::new(registry)),
        );
        assert!(
            alone
                .execute("file.new_board", &imaginarium_commands::Args::new())
                .is_err()
        );
        let app = app();
        let mut board = app.new_board().unwrap();
        assert!(
            board
                .execute("file.new_board", &imaginarium_commands::Args::new())
                .is_ok()
        );
    }

    #[test]
    fn open_waits_for_files() {
        let app = app();
        let mut start = app.start_window();
        frame(&mut start);
        let (ui, fonts) = start.ui();
        assert!(
            ui.find_by_label(
                fonts,
                "Open…, unavailable: opening files comes in a later preview"
            )
            .is_some()
        );
    }

    /// The middle of the control a screen reader calls `label`.
    fn center(
        window: &mut crate::settings::SettingsWindow,
        label: &str,
    ) -> Option<imaginarium_render::kurbo::Point> {
        let (ui, fonts) = window.ui();
        let widget = ui.find_by_label(fonts, label)?;
        Some(ui.bounds(widget)?.center())
    }

    fn settings_frame(window: &mut crate::settings::SettingsWindow) {
        let mut scene = Scene::new();
        window.paint(&mut scene, Size::new(1120.0, 720.0));
    }

    fn has(window: &mut crate::settings::SettingsWindow, label: &str) -> bool {
        let (ui, fonts) = window.ui();
        ui.find_by_label(fonts, label).is_some()
    }

    fn press(window: &mut crate::settings::SettingsWindow, label: &str) {
        let at = center(window, label).unwrap_or_else(|| panic!("no {label:?}"));
        for pressed in [true, false] {
            window.input(&Input::PointerButton {
                primary: true,
                pressed,
                position: at,
                clicks: 1,
                modifiers: Modifiers::default(),
            });
        }
        settings_frame(window);
    }

    #[test]
    fn settings_show_by_section_and_a_switch_changes_one() {
        let app = app();
        let mut window = app.settings_window().unwrap();
        settings_frame(&mut window);
        // General: Show at startup, as a switch.
        assert!(has(&mut window, "Show the start window at startup"));
        assert!(!has(&mut window, "Reset Show the start window at startup"));
        press(&mut window, "Show the start window at startup");
        assert!(!app.shows_start_window());
        // Changed, with a reset button that puts it back.
        assert!(has(&mut window, "Reset Show the start window at startup"));
        press(&mut window, "Reset Show the start window at startup");
        assert!(app.shows_start_window());
        // Appearance has the tool bar's position, as a dropdown.
        press(&mut window, "Appearance");
        assert!(has(&mut window, "Tool bar position"));
        assert!(!has(&mut window, "Show the start window at startup"));
    }

    #[test]
    fn a_dropdown_lists_the_values_and_sets_one() {
        let app = app();
        let mut window = app.settings_window().unwrap();
        settings_frame(&mut window);
        press(&mut window, "Appearance");
        press(&mut window, "Tool bar position");
        settings_frame(&mut window);
        press(&mut window, "Top");
        assert_eq!(
            app.settings()
                .borrow()
                .get(&crate::TOOLBAR_POSITION)
                .unwrap(),
            crate::Dock::Top
        );
        // The list closed.
        assert!(!has(&mut window, "Floating"));
    }

    #[test]
    fn the_search_finds_settings_and_shortcuts() {
        let app = app();
        let mut window = app.settings_window().unwrap();
        settings_frame(&mut window);
        let at = center(&mut window, "Search settings").unwrap();
        window.input(&Input::PointerButton {
            primary: true,
            pressed: true,
            position: at,
            clicks: 1,
            modifiers: Modifiers::default(),
        });
        for c in "dock".chars() {
            window.input(&Input::Key {
                key: Key::Character(c.to_string()),
                text: Some(c.to_string()),
                pressed: true,
                repeat: false,
                modifiers: Modifiers::default(),
            });
        }
        settings_frame(&mut window);
        // By a synonym, from another section.
        assert!(has(&mut window, "Tool bar position"));
        assert!(!has(&mut window, "Show the start window at startup"));
    }

    #[test]
    fn one_settings_window_at_a_time() {
        let app = app();
        let first = app.settings_window();
        assert!(first.is_some());
        assert!(app.settings_window().is_none());
        drop(first);
        assert!(app.settings_window().is_some());
    }

    #[test]
    fn ctrl_comma_opens_settings_from_a_board_and_the_start_window() {
        let app = app();
        let mut board = app.new_board().unwrap();
        board.input(&ctrl(","));
        let requests = board.take_requests();
        assert_eq!(names(&requests), ["Open(\"Settings — Imaginarium\")"]);
        drop(requests);
        let mut start = app.start_window();
        start.input(&ctrl(","));
        assert_eq!(
            names(&start.take_requests()),
            ["Open(\"Settings — Imaginarium\")"]
        );
    }

    #[test]
    fn a_failed_save_shows_in_settings() {
        let app = app();
        app.report_saving(None, Some("Your settings file couldn't be read.".into()));
        let mut window = app.settings_window().unwrap();
        settings_frame(&mut window);
        assert!(has(&mut window, "Your settings file couldn't be read."));
    }

    #[test]
    fn the_start_windows_switch_follows_a_change_in_settings() {
        let app = app();
        let mut start = app.start_window();
        frame(&mut start);
        app.settings()
            .borrow_mut()
            .set(&SHOW_START_WINDOW, false)
            .unwrap();
        frame(&mut start);
        let mut tree = imaginarium_ui::AccessTree::default();
        start.accessibility(&mut tree);
        let toggled = tree
            .nodes()
            .iter()
            .find(|(_, node)| node.label() == Some("Show at startup") && node.toggled().is_some())
            .and_then(|(_, node)| node.toggled());
        assert_eq!(toggled, Some(accesskit::Toggled::False));
    }
}
