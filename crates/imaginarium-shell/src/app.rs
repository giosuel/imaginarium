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
        })
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

    fn ctrl(key: &str) -> Input {
        Input::Key {
            key: Key::Character(key.into()),
            text: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers {
                ctrl: true,
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
}
