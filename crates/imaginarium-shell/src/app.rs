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
    /// The board window used last, for This board in Settings.
    active_board: RefCell<std::rc::Weak<BoardLink>>,
    /// Closed boards' saving threads finishing.
    finishing: RefCell<Vec<std::thread::JoinHandle<Option<String>>>>,
    /// Whether the settings file is saved, and a notice from reading it.
    saving: RefCell<Saving>,
}

/// A board window, as the Settings window sees it ("This board"): its
/// name, its own setting values, and changes to make to them. The board
/// window keeps it current and does the changes (as commands) at its next
/// frame.
pub(crate) struct BoardLink {
    /// Tells boards apart (a closed board's address may be reused).
    pub(crate) id: u64,
    pub(crate) name: RefCell<String>,
    /// The board is read-only (a newer file): no This board for it.
    pub(crate) read_only: Cell<bool>,
    /// Why the last change Settings asked for failed, for its row.
    pub(crate) error: RefCell<Option<(&'static str, String)>>,
    /// The board's own values, by setting key.
    pub(crate) own: RefCell<std::collections::BTreeMap<&'static str, imaginarium_settings::Value>>,
    /// Counts changes of `own` and `name`, for Settings to notice.
    pub(crate) generation: Cell<u64>,
    /// Values to set (`None`: follow the user's again), from Settings.
    asks: RefCell<Vec<(&'static str, Option<imaginarium_settings::Value>)>>,
}

impl Default for BoardLink {
    fn default() -> Self {
        thread_local! {
            static NEXT: Cell<u64> = const { Cell::new(0) };
        }
        Self {
            id: NEXT.with(|next| next.replace(next.get() + 1)),
            name: RefCell::default(),
            read_only: Cell::new(false),
            error: RefCell::default(),
            own: RefCell::default(),
            generation: Cell::new(0),
            asks: RefCell::default(),
        }
    }
}

impl BoardLink {
    /// Asks the board to set `key` (`None`: follow the user's again); its
    /// window does it at its next frame, which this asks for (an idle
    /// window draws nothing otherwise).
    pub(crate) fn ask(&self, key: &'static str, value: Option<imaginarium_settings::Value>) {
        self.asks.borrow_mut().push((key, value));
        imaginarium_ui::request_frame_everywhere();
    }

    /// What Settings asked for since the last frame.
    pub(crate) fn take_asks(&self) -> Vec<(&'static str, Option<imaginarium_settings::Value>)> {
        std::mem::take(&mut *self.asks.borrow_mut())
    }

    /// Something Settings shows changed.
    pub(crate) fn changed(&self) {
        self.generation.set(self.generation.get() + 1);
        imaginarium_ui::request_frame_everywhere();
    }
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
            active_board: RefCell::default(),
            finishing: RefCell::default(),
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

    /// `board` is the board window used last.
    pub(crate) fn board_active(&self, board: &Rc<BoardLink>) {
        let was = self.active_board.replace(Rc::downgrade(board));
        if !was.ptr_eq(&Rc::downgrade(board)) {
            // Settings' This board names it.
            imaginarium_ui::request_frame_everywhere();
        }
    }

    /// The board window used last, while it is open.
    pub(crate) fn active_board(&self) -> Option<Rc<BoardLink>> {
        self.active_board.borrow().upgrade()
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

    /// A window on the board in the file at `path`, saving every change
    /// into it. A file from a newer version opens read-only, without
    /// saving.
    ///
    /// # Errors
    /// If the file can't be opened or read, as a sentence to show.
    pub fn open_board(self: &Rc<Self>, path: &std::path::Path) -> Result<BoardWindow, String> {
        let name = path.file_stem().map_or_else(
            || "Board".to_owned(),
            |stem| stem.to_string_lossy().into_owned(),
        );
        let opening = imaginarium_io_native::Session::open(path)
            .map_err(|err| format!("{} can't be opened: {err}", path.display()))?;
        Ok(match opening {
            imaginarium_io_native::Opening::Editable(opened, session) => self
                .board_window(opened.document, &name)
                .saved_in(path.to_owned(), *session),
            // Nothing is written into it; the pill says why.
            imaginarium_io_native::Opening::ReadOnly(opened) => {
                self.board_window(opened.document, &name)
            }
        })
    }

    /// A closed board's saving thread finishing (the app waits for it
    /// before it exits).
    pub(crate) fn finishing(&self, save: std::thread::JoinHandle<Option<String>>) {
        self.finishing.borrow_mut().push(save);
    }

    /// Waits for the closed boards' saves to finish; why the ones that
    /// failed did (their last changes aren't in the file).
    pub fn wait_for_saves(&self) -> Vec<String> {
        std::mem::take(&mut *self.finishing.borrow_mut())
            .into_iter()
            .filter_map(|save| save.join().ok().flatten())
            .collect()
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

    static SECONDS: imaginarium_settings::Setting<i64> = imaginarium_settings::Setting::new(
        "files.autosave_seconds",
        || 30,
        imaginarium_settings::Scope::App,
        "Autosave interval",
        "How often to save, in seconds.",
    )
    .range(5, 600);

    fn app() -> Rc<App> {
        let mut registry = Registry::new();
        registry.register_all(crate::SETTINGS).unwrap();
        registry.register_all(imaginarium_canvas::SETTINGS).unwrap();
        registry.register(&SECONDS).unwrap();
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
        registry.register_all(imaginarium_canvas::SETTINGS).unwrap();
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

    fn key(window: &mut crate::settings::SettingsWindow, key: Key, text: Option<&str>) {
        window.input(&Input::Key {
            key,
            text: text.map(str::to_owned),
            pressed: true,
            repeat: false,
            modifiers: Modifiers::default(),
        });
        settings_frame(window);
    }

    fn type_into(window: &mut crate::settings::SettingsWindow, text: &str) {
        for c in text.chars() {
            key(window, Key::Character(c.to_string()), Some(&c.to_string()));
        }
    }

    fn focused_label(window: &mut crate::settings::SettingsWindow) -> Option<String> {
        let (ui, fonts) = window.ui();
        let focused = ui.focused()?;
        let mut tree = imaginarium_ui::AccessTree::default();
        ui.access(fonts, &mut tree);
        tree.nodes()
            .iter()
            .find(|(id, _)| *id == accesskit::NodeId(focused.access_base()))
            .and_then(|(_, node)| node.label().map(str::to_owned))
    }

    #[test]
    fn the_keys_stay_on_a_control_after_it_changes_a_setting() {
        let app = app();
        let mut window = app.settings_window().unwrap();
        settings_frame(&mut window);
        // Tab to the switch (past the search and the sections), Space.
        let label = "Show the start window at startup";
        for _ in 0..20 {
            if focused_label(&mut window).as_deref() == Some(label) {
                break;
            }
            key(&mut window, Key::Tab, None);
        }
        assert_eq!(focused_label(&mut window).as_deref(), Some(label));
        key(&mut window, Key::Character(" ".into()), Some(" "));
        assert!(!app.shows_start_window());
        settings_frame(&mut window);
        assert_eq!(focused_label(&mut window).as_deref(), Some(label));
    }

    #[test]
    fn rebuilding_doesnt_keep_the_old_widgets() {
        let app = app();
        let mut window = app.settings_window().unwrap();
        settings_frame(&mut window);
        let before = window.ui().0.stats().widgets;
        press(&mut window, "Search settings");
        type_into(&mut window, "dockx");
        for _ in 0..5 {
            key(&mut window, Key::Backspace, None);
        }
        settings_frame(&mut window);
        let after = window.ui().0.stats().widgets;
        assert!(after <= before + 2, "{before} widgets, then {after}");
    }

    #[test]
    fn a_field_keeps_a_typed_value_and_esc_takes_it_back() {
        let app = app();
        let mut window = app.settings_window().unwrap();
        settings_frame(&mut window);
        press(&mut window, "Files and saving");
        press(&mut window, "Autosave interval");
        key(&mut window, Key::End, None);
        type_into(&mut window, "0");
        key(&mut window, Key::Escape, None);
        assert_eq!(app.settings().borrow().get(&SECONDS).unwrap(), 30);
        // The field shows the value again, and leaving it changes nothing.
        press(&mut window, "Autosave interval");
        key(&mut window, Key::Tab, None);
        assert_eq!(app.settings().borrow().get(&SECONDS).unwrap(), 30);
        // Enter keeps a typed value.
        press(&mut window, "Autosave interval");
        key(&mut window, Key::End, None);
        type_into(&mut window, "0");
        key(&mut window, Key::Enter, None);
        assert_eq!(app.settings().borrow().get(&SECONDS).unwrap(), 300);
        // Out of range: said in words.
        press(&mut window, "Autosave interval");
        key(&mut window, Key::End, None);
        type_into(&mut window, "0");
        key(&mut window, Key::Enter, None);
        assert_eq!(app.settings().borrow().get(&SECONDS).unwrap(), 300);
        assert!(has(&mut window, "Must be between 5 and 600."));
    }

    #[test]
    fn a_failed_save_shows_while_settings_is_open() {
        let app = app();
        let mut window = app.settings_window().unwrap();
        settings_frame(&mut window);
        // A file where the settings folder should be: saving fails.
        let blocked =
            std::env::temp_dir().join(format!("imaginarium-shell-{}", std::process::id()));
        std::fs::write(&blocked, "").unwrap();
        let writer = imaginarium_settings::FileWriter::new(blocked.join("settings.toml"));
        let status = writer.status();
        let mut sink = writer.sink();
        sink("a = 1\n".to_owned());
        assert!(writer.finish().is_some());
        app.report_saving(Some(status), None);
        settings_frame(&mut window);
        let (ui, fonts) = window.ui();
        let mut tree = imaginarium_ui::AccessTree::default();
        ui.access(fonts, &mut tree);
        assert!(tree.nodes().iter().any(|(_, node)| {
            node.label()
                .is_some_and(|label| label.starts_with("Settings can't be saved"))
        }));
        std::fs::remove_file(&blocked).unwrap();
    }

    #[test]
    fn ctrl_w_closes_settings() {
        let app = app();
        let mut window = app.settings_window().unwrap();
        settings_frame(&mut window);
        window.input(&ctrl("w"));
        assert_eq!(names(&window.take_requests()), ["Close"]);
    }

    #[test]
    fn the_board_background_is_a_segmented_control_in_appearance() {
        let app = app();
        let mut window = app.settings_window().unwrap();
        settings_frame(&mut window);
        press(&mut window, "Appearance");
        press(&mut window, "Grid");
        assert_eq!(
            app.settings()
                .borrow()
                .get(&imaginarium_canvas::BACKGROUND)
                .unwrap(),
            imaginarium_canvas::Background::Grid
        );
    }

    #[test]
    fn this_board_gives_the_board_its_own_value() {
        use imaginarium_canvas::{BACKGROUND, Background};
        let app = app();
        let mut board = app.new_board().unwrap();
        let mut scene = Scene::new();
        board.draw(&mut scene, Size::new(1440.0, 900.0));
        let mut window = app.settings_window().unwrap();
        settings_frame(&mut window);
        press(&mut window, "Appearance");
        // Only where a board's value can differ: not in General.
        press(&mut window, "This board");
        press(&mut window, "Grid");
        board.draw(&mut scene, Size::new(1440.0, 900.0));
        let own = board.board().unwrap().own_setting(BACKGROUND.key());
        assert_eq!(own, Some(imaginarium_settings::Value::Enum("grid".into())));
        // Mine is untouched.
        assert_eq!(
            app.settings().borrow().get(&BACKGROUND).unwrap(),
            Background::Dots
        );
        // The board's own value shows as changed; reset follows mine again.
        settings_frame(&mut window);
        press(&mut window, "Reset Board background");
        board.draw(&mut scene, Size::new(1440.0, 900.0));
        assert_eq!(board.board().unwrap().own_setting(BACKGROUND.key()), None);
        // General has no board values: no tabs there.
        settings_frame(&mut window);
        press(&mut window, "General");
        assert!(!has(&mut window, "This board"));
    }

    fn board_draw(board: &mut BoardWindow) {
        let mut scene = Scene::new();
        board.draw(&mut scene, Size::new(1440.0, 900.0));
    }

    #[test]
    fn a_closed_board_never_turns_this_board_into_mine() {
        use imaginarium_canvas::{BACKGROUND, Background};
        let app = app();
        let mut board = app.new_board().unwrap();
        board_draw(&mut board);
        let mut window = app.settings_window().unwrap();
        settings_frame(&mut window);
        press(&mut window, "Appearance");
        press(&mut window, "This board");
        let grid = {
            let (ui, fonts) = window.ui();
            ui.find_by_label(fonts, "Grid").unwrap()
        };
        // The board closes; a screen reader's click on Grid comes before
        // Settings draws again.
        drop(board);
        window.accessibility_action(&accesskit::ActionRequest {
            action: accesskit::Action::Click,
            target_tree: accesskit::TreeId::ROOT,
            target_node: accesskit::NodeId(grid.access_base()),
            data: None,
        });
        assert_eq!(
            app.settings().borrow().get(&BACKGROUND).unwrap(),
            Background::Dots
        );
        // Settings shows mine again, without the tabs.
        settings_frame(&mut window);
        assert!(!has(&mut window, "This board"));
    }

    #[test]
    fn a_read_only_board_has_no_this_board() {
        let mut newer = Document::new(DocumentKind::Board).unwrap();
        newer.set_schema_version_for_tests(99).unwrap();
        let opened = Document::open(&[&newer.export_snapshot().unwrap()]).unwrap();
        let app = app();
        let mut board = app.board_window(opened, "From a newer version");
        board_draw(&mut board);
        let mut window = app.settings_window().unwrap();
        settings_frame(&mut window);
        press(&mut window, "Appearance");
        assert!(!has(&mut window, "This board"));
    }

    #[test]
    fn this_board_follows_the_board_used_last() {
        use imaginarium_canvas::BACKGROUND;
        let app = app();
        let mut first = app.board_window(Document::new(DocumentKind::Board).unwrap(), "First");
        let mut second = app.board_window(Document::new(DocumentKind::Board).unwrap(), "Second");
        board_draw(&mut first);
        board_draw(&mut second);
        // A click in the first one makes it the board This board edits.
        first.input(&Input::PointerButton {
            primary: true,
            pressed: true,
            position: imaginarium_render::kurbo::Point::new(700.0, 450.0),
            clicks: 1,
            modifiers: Modifiers::default(),
        });
        let mut window = app.settings_window().unwrap();
        settings_frame(&mut window);
        press(&mut window, "Appearance");
        press(&mut window, "This board");
        press(&mut window, "Plain");
        board_draw(&mut first);
        board_draw(&mut second);
        assert!(
            first
                .board()
                .unwrap()
                .own_setting(BACKGROUND.key())
                .is_some()
        );
        assert!(
            second
                .board()
                .unwrap()
                .own_setting(BACKGROUND.key())
                .is_none()
        );
    }

    #[test]
    fn the_settings_a_board_takes_are_the_per_document_ones() {
        let app = app();
        let declared: Vec<&str> = app
            .settings()
            .borrow()
            .declarations()
            .filter(|setting| setting.meta().per_document)
            .map(|setting| setting.key())
            .collect();
        let taken: Vec<&str> = imaginarium_canvas::board_settings::BOARD_SETTINGS
            .iter()
            .map(|setting| setting.key())
            .collect();
        assert_eq!(declared, taken);
    }

    fn status_named(board: &mut BoardWindow, name: &str) -> bool {
        let (ui, fonts) = board.ui();
        ui.find_by_label(fonts, name).is_some()
    }

    #[test]
    fn a_board_opened_from_a_file_saves_every_change() {
        let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/test-scratch")
            .join(format!("shell-autosave-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("Launch plan.imag");
        imaginarium_io_native::save_new_path(
            &path,
            &Document::new(DocumentKind::Board).unwrap(),
            &imaginarium_io_native::SaveOptions {
                protection: imaginarium_io_native::Protection::None,
                created_with: "imaginarium tests".into(),
            },
        )
        .unwrap();
        let app = app();
        let mut board = app.open_board(&path).unwrap();
        assert_eq!(board.options().title, "Launch plan — Imaginarium");
        board_draw(&mut board);
        assert!(status_named(&mut board, "Saved"));
        // A second window on the same file can only read it, and says so.
        let mut second = app.open_board(&path).unwrap();
        board_draw(&mut second);
        assert!(status_named(
            &mut second,
            "Read-only · open in another window"
        ));
        drop(second);
        // Two frames, then the window closes right after a change, before
        // any frame could send it.
        board
            .execute("board.insert_frame", &imaginarium_commands::Args::new())
            .unwrap();
        board_draw(&mut board);
        board
            .execute("board.insert_frame", &imaginarium_commands::Args::new())
            .unwrap();
        assert_eq!(board.board().unwrap().document().view().node_count(), 2);
        drop(board);
        assert!(app.wait_for_saves().is_empty());
        let reopened =
            imaginarium_io_native::open_path(&path, imaginarium_io_native::Credentials::None)
                .unwrap();
        assert_eq!(reopened.document.view().node_count(), 2);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
