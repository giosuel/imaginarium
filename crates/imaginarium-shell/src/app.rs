// SPDX-License-Identifier: AGPL-3.0-or-later

//! What the app's windows share (`shell.md` §2): one window per document,
//! the start window at startup and when the last board closes.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::{Arc, Mutex};

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
    /// Closed boards' saving threads finishing, and what their failure
    /// means.
    finishing: RefCell<Vec<Finishing>>,
    /// Whether the settings file is saved, and a notice from reading it.
    saving: RefCell<Saving>,
    /// Where untitled boards are kept for crash recovery (`None`: they
    /// aren't).
    recovery: RefCell<Option<std::path::PathBuf>>,
    /// The boards a crashed app left there, once looked for.
    kept: Arc<Mutex<KeptScan>>,
    /// The file dialogs.
    dialogs: RefCell<Arc<dyn crate::files::Dialogs>>,
}

/// A closed board's saving thread finishing, and what its failure means.
type Finishing = (std::thread::JoinHandle<Option<String>>, &'static str);

/// The search for boards a crashed app left (`App::look_for_kept_boards`).
#[derive(Default)]
struct KeptScan {
    /// The search is done.
    done: bool,
    /// What it found, until opened.
    boards: Vec<imaginarium_io_native::KeptBoard>,
    /// Wake the windows that wait for it.
    wakers: Vec<Box<dyn Fn() + Send>>,
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

/// An untitled board's name.
const UNTITLED: &str = "Untitled board";

/// A kept board's name: its title, if it has one.
fn kept_name(document: &Document) -> String {
    document
        .view()
        .meta()
        .title()
        .filter(|title| !title.trim().is_empty())
        .unwrap_or(UNTITLED)
        .to_owned()
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
            recovery: RefCell::default(),
            kept: Arc::default(),
            dialogs: RefCell::new(Arc::new(crate::files::SystemDialogs)),
        })
    }

    /// Uses `dialogs` instead of the system's (tests).
    pub fn set_dialogs(&self, dialogs: Arc<dyn crate::files::Dialogs>) {
        *self.dialogs.borrow_mut() = dialogs;
    }

    /// The file dialogs, for a thread to call.
    pub(crate) fn dialogs(&self) -> Arc<dyn crate::files::Dialogs> {
        self.dialogs.borrow().clone()
    }

    /// What new files say made them.
    pub(crate) fn created_with(&self) -> String {
        format!("Imaginarium {}", self.version)
    }

    /// Keeps untitled boards in `folder` from now on, so they survive a
    /// crash (`shell.md` §4); it is made when the first one changes.
    pub fn keep_untitled_in(&self, folder: std::path::PathBuf) {
        *self.recovery.borrow_mut() = Some(folder);
    }

    /// Looks, on a thread of its own, for boards an app that closed
    /// unexpectedly left in the recovery folder; the start window offers
    /// them. Call it once at startup, after [`App::keep_untitled_in`].
    pub fn look_for_kept_boards(&self) {
        let Some(folder) = self.recovery.borrow().clone() else {
            return;
        };
        let scan = self.kept.clone();
        let looked = std::thread::Builder::new()
            .name("kept-boards".into())
            .spawn(move || {
                let boards = imaginarium_io_native::kept_boards(&folder);
                if let Ok(mut scan) = scan.lock() {
                    scan.boards = boards;
                    scan.done = true;
                    for wake in &scan.wakers {
                        wake();
                    }
                }
            });
        if looked.is_err()
            && let Ok(mut scan) = self.kept.lock()
        {
            scan.done = true;
        }
    }

    /// Wakes the caller's window (`wake` comes from
    /// `imaginarium_ui::window_waker`) when the search for kept boards is
    /// done; at once if it is.
    pub(crate) fn when_kept_boards_found(&self, wake: Box<dyn Fn() + Send>) {
        if let Ok(mut scan) = self.kept.lock() {
            if scan.done {
                wake();
            } else {
                scan.wakers.push(wake);
            }
        }
    }

    /// Whether the search for kept boards is done (or never started).
    #[cfg(test)]
    pub(crate) fn kept_search_done(&self) -> bool {
        self.recovery.borrow().is_none() || self.kept.lock().is_ok_and(|scan| scan.done)
    }

    /// The boards found after a crash and not opened yet: how many, and
    /// the first one's name.
    pub(crate) fn kept_boards(&self) -> Option<(usize, String)> {
        let scan = self.kept.lock().ok()?;
        let first = scan.boards.first()?;
        Some((scan.boards.len(), kept_name(&first.opened.document)))
    }

    /// Windows on the boards found after a crash, which go on being kept
    /// (they are still untitled).
    pub(crate) fn open_kept_boards(self: &Rc<Self>) -> Vec<BoardWindow> {
        let boards = self
            .kept
            .lock()
            .map(|mut scan| std::mem::take(&mut scan.boards))
            .unwrap_or_default();
        boards
            .into_iter()
            .map(|kept| {
                let name = kept_name(&kept.opened.document);
                self.board_window(kept.opened.document, &name)
                    .kept_from(kept.path, *kept.session)
            })
            .collect()
    }

    /// Where the next untitled board is kept: a name no other board, or
    /// another running app, has.
    fn next_kept_path(&self) -> Option<std::path::PathBuf> {
        thread_local! {
            static NEXT: Cell<u64> = const { Cell::new(0) };
        }
        let folder = self.recovery.borrow().clone()?;
        let at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| since.as_millis());
        let n = NEXT.with(|next| next.replace(next.get() + 1));
        Some(folder.join(format!("untitled-{at}-{}-{n}.imag", std::process::id())))
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

    /// A window on a new, untitled board, kept for crash recovery (when
    /// the app keeps them: [`App::keep_untitled_in`]).
    pub fn new_board(self: &Rc<Self>) -> Result<BoardWindow, ModelError> {
        let document = Document::new(DocumentKind::Board)?;
        let window = self.board_window(document, UNTITLED);
        Ok(match self.next_kept_path() {
            Some(path) => window.kept_in(path, &self.created_with()),
            None => window,
        })
    }

    /// A window on the board in the file at `path`, saving every change
    /// into it. A file from a newer version opens read-only, without
    /// saving.
    ///
    /// # Errors
    /// If the file can't be opened or read, as a sentence to show.
    pub fn open_board(self: &Rc<Self>, path: &std::path::Path) -> Result<BoardWindow, String> {
        let opening = imaginarium_io_native::Session::open(path)
            .map_err(|err| format!("{} can't be opened: {err}", path.display()))?;
        Ok(self.opened_board(path, opening))
    }

    /// A window on the board opened (on another thread) from `path`.
    pub(crate) fn opened_board(
        self: &Rc<Self>,
        path: &std::path::Path,
        opening: imaginarium_io_native::Opening,
    ) -> BoardWindow {
        let name = crate::files::board_name(path);
        match opening {
            imaginarium_io_native::Opening::Editable(opened, session) => self
                .board_window(opened.document, &name)
                .saved_in(path.to_owned(), *session),
            // Nothing is written into it; the pill says why.
            imaginarium_io_native::Opening::ReadOnly(opened) => {
                self.board_window(opened.document, &name)
            }
        }
    }

    /// A closed board's saving thread finishing (the app waits for it
    /// before it exits).
    pub(crate) fn finishing(
        &self,
        save: std::thread::JoinHandle<Option<String>>,
        what: &'static str,
    ) {
        self.finishing.borrow_mut().push((save, what));
    }

    /// Waits for the closed boards' saves to finish; what went wrong with
    /// the ones that failed, as sentences (their last changes aren't in the
    /// file, a recovery file is left).
    pub fn wait_for_saves(&self) -> Vec<String> {
        std::mem::take(&mut *self.finishing.borrow_mut())
            .into_iter()
            .filter_map(|(save, what)| {
                let reason = save.join().unwrap_or_else(|_| Some("it stopped".into()))?;
                Some(format!("{what}: {reason}"))
            })
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

    /// Dialogs that answer at once: the board to open, and where to save
    /// (each answer used once); what they were asked is kept.
    #[derive(Default)]
    struct Answers {
        open: std::sync::Mutex<Option<std::path::PathBuf>>,
        save: std::sync::Mutex<Option<std::path::PathBuf>>,
        asked: std::sync::Mutex<Vec<String>>,
    }

    impl crate::files::Dialogs for Answers {
        fn open_board(&self) -> Option<std::path::PathBuf> {
            self.asked.lock().unwrap().push("open".into());
            self.open.lock().unwrap().take()
        }

        fn save_board(
            &self,
            title: &str,
            folder: Option<&std::path::Path>,
            name: &str,
        ) -> Option<std::path::PathBuf> {
            self.asked
                .lock()
                .unwrap()
                .push(format!("{title} | {folder:?} | {name}"));
            self.save.lock().unwrap().take()
        }
    }

    fn answering(app: &App) -> Arc<Answers> {
        let answers = Arc::new(Answers::default());
        app.set_dialogs(answers.clone());
        answers
    }

    /// A board file with `frames` frames in it.
    fn board_file(path: &std::path::Path, frames: usize) {
        let mut document = Document::new(DocumentKind::Board).unwrap();
        for _ in 0..frames {
            document
                .transact(&imaginarium_model::Actor::User, |t| {
                    t.create_node(
                        imaginarium_model::Parent::Root,
                        None,
                        &imaginarium_model::NodeKind::Frame,
                    )
                    .map(|_| ())
                })
                .unwrap();
        }
        imaginarium_io_native::save_new_path(
            path,
            &document,
            &imaginarium_io_native::SaveOptions {
                protection: imaginarium_io_native::Protection::None,
                created_with: "imaginarium tests".into(),
            },
        )
        .unwrap();
    }

    fn nodes_in(path: &std::path::Path) -> usize {
        imaginarium_io_native::open_path(path, imaginarium_io_native::Credentials::None)
            .map_or(0, |opened| opened.document.view().node_count())
    }

    /// Draws `board` (which runs its timers and takes what its threads
    /// finished) until `done`, at most 10 s.
    fn run_until(board: &mut BoardWindow, mut done: impl FnMut(&mut BoardWindow) -> bool) {
        until(|| {
            let now = std::time::Instant::now();
            if board.wake_at().is_some_and(|at| at <= now) {
                board.wake(now);
            }
            board_draw(board);
            done(board)
        });
    }

    #[test]
    fn open_in_the_start_window_opens_the_board_and_closes_it() {
        let dir = scratch("start-open");
        let path = dir.join("Launch plan.imag");
        board_file(&path, 2);
        let app = app();
        let answers = answering(&app);
        *answers.open.lock().unwrap() = Some(path.clone());
        let mut start = app.start_window();
        frame(&mut start);
        let (ui, fonts) = start.ui();
        let button = ui.find_by_label(fonts, "Open…").unwrap();
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
        let mut requests = Vec::new();
        until(|| {
            frame(&mut start);
            requests.extend(start.take_requests());
            !requests.is_empty()
        });
        assert_eq!(
            names(&requests),
            ["Open(\"Launch plan — Imaginarium\")", "Close"]
        );
        // A file that isn't a board: said in the window, which stays.
        let junk = dir.join("notes.imag");
        std::fs::write(&junk, "not a board").unwrap();
        *answers.open.lock().unwrap() = Some(junk);
        let mut start = app.start_window();
        start.input(&ctrl("o"));
        until(|| {
            frame(&mut start);
            start_has_prefix(&mut start, "notes can't be opened")
        });
        assert!(start_has_prefix(&mut start, "notes can't be opened"));
        assert!(start.take_requests().is_empty());
        // Cancelled: nothing happens.
        start.input(&ctrl("o"));
        assert!(start.is_opening());
        until(|| {
            frame(&mut start);
            !start.is_opening()
        });
        assert!(!start.is_opening());
        assert!(start.take_requests().is_empty());
    }

    fn start_has_prefix(start: &mut StartWindow, prefix: &str) -> bool {
        let (ui, fonts) = start.ui();
        let mut tree = imaginarium_ui::AccessTree::default();
        ui.access(fonts, &mut tree);
        tree.nodes()
            .iter()
            .any(|(_, node)| node.label().is_some_and(|label| label.starts_with(prefix)))
    }

    fn requests_of(board: &mut BoardWindow) -> Vec<String> {
        names(&board.take_requests())
    }

    #[test]
    fn open_in_a_board_opens_another_window() {
        let dir = scratch("board-open");
        let path = dir.join("Plan.imag");
        board_file(&path, 1);
        let app = app();
        let answers = answering(&app);
        *answers.open.lock().unwrap() = Some(path);
        let mut board = app.new_board().unwrap();
        board_draw(&mut board);
        board.input(&ctrl("o"));
        let mut seen = Vec::new();
        run_until(&mut board, |board| {
            seen.extend(requests_of(board));
            !seen.is_empty()
        });
        assert_eq!(seen, ["Open(\"Plan — Imaginarium\")"]);
    }

    #[test]
    fn save_gives_an_untitled_board_its_file_and_saving_goes_on_there() {
        let dir = scratch("save");
        let recovery = dir.join("recovery");
        let app = app();
        app.keep_untitled_in(recovery.clone());
        let answers = answering(&app);
        let mut board = app.new_board().unwrap();
        board_draw(&mut board);
        board
            .execute("board.insert_frame", &imaginarium_commands::Args::new())
            .unwrap();
        board_draw(&mut board);
        let kept = board.kept_path().unwrap().to_owned();
        // Without the extension: it gets it.
        *answers.save.lock().unwrap() = Some(dir.join("Roadmap"));
        board.input(&ctrl("s"));
        let target = dir.join("Roadmap.imag");
        let mut seen = Vec::new();
        run_until(&mut board, |board| {
            seen.extend(requests_of(board));
            board.path() == Some(target.as_path())
        });
        assert_eq!(board.path(), Some(target.as_path()));
        assert_eq!(seen, ["SetTitle(\"Roadmap — Imaginarium\")"]);
        assert_eq!(board.options().title, "Roadmap — Imaginarium");
        assert!(status_named(&mut board, "Saved"));
        assert_eq!(nodes_in(&target), 1);
        // The recovery file became the board's file.
        assert!(!kept.exists());
        assert_eq!(
            answers.asked.lock().unwrap()[0],
            "Save the board | None | Untitled board.imag"
        );
        // Later changes go into it.
        board
            .execute("board.insert_frame", &imaginarium_commands::Args::new())
            .unwrap();
        run_until(&mut board, |_| nodes_in(&target) == 2);
        assert_eq!(nodes_in(&target), 2);
        // Saving again has nothing to do.
        assert!(
            board
                .execute("file.save", &imaginarium_commands::Args::new())
                .is_err()
        );
        drop(board);
        assert!(app.wait_for_saves().is_empty());
        assert_eq!(nodes_in(&target), 2);
    }

    #[test]
    fn save_writes_an_untitled_board_that_never_changed() {
        // Kept from its first change on: it has no file yet.
        let dir = scratch("save-unchanged");
        let app = app();
        app.keep_untitled_in(dir.join("recovery"));
        let answers = answering(&app);
        let mut board = app.new_board().unwrap();
        board_draw(&mut board);
        let target = dir.join("Empty.imag");
        *answers.save.lock().unwrap() = Some(target.clone());
        board
            .execute("file.save", &imaginarium_commands::Args::new())
            .unwrap();
        run_until(&mut board, |board| board.path().is_some());
        assert_eq!(board.path(), Some(target.as_path()));
        assert!(
            imaginarium_io_native::open_path(&target, imaginarium_io_native::Credentials::None)
                .is_ok()
        );
    }

    #[test]
    fn save_writes_a_board_that_has_no_file_at_all() {
        let dir = scratch("save-new");
        let app = app();
        let answers = answering(&app);
        let mut board = app.new_board().unwrap();
        assert!(board.kept_path().is_none());
        board
            .execute("board.insert_frame", &imaginarium_commands::Args::new())
            .unwrap();
        let target = dir.join("Plan.imag");
        *answers.save.lock().unwrap() = Some(target.clone());
        board
            .execute("file.save", &imaginarium_commands::Args::new())
            .unwrap();
        run_until(&mut board, |board| board.path().is_some());
        assert_eq!(nodes_in(&target), 1);
        drop(board);
        assert!(app.wait_for_saves().is_empty());
    }

    #[test]
    fn rename_or_move_takes_the_file_along() {
        let dir = scratch("move");
        let from = dir.join("Plan.imag");
        board_file(&from, 1);
        let id = imaginarium_io_native::open_reader(
            std::fs::File::open(&from).unwrap(),
            imaginarium_io_native::Credentials::None,
        )
        .unwrap()
        .header()
        .doc_id;
        let app = app();
        let answers = answering(&app);
        let mut board = app.open_board(&from).unwrap();
        board_draw(&mut board);
        board
            .execute("board.insert_frame", &imaginarium_commands::Args::new())
            .unwrap();
        std::fs::create_dir_all(dir.join("Work")).unwrap();
        let to = dir.join("Work/Plan 2027.imag");
        *answers.save.lock().unwrap() = Some(to.clone());
        // The pill's name is the button.
        let (ui, fonts) = board.ui();
        let name = ui.find_by_label(fonts, "Rename or move the board").unwrap();
        let at = ui.bounds(name).unwrap().center();
        for pressed in [true, false] {
            board.input(&Input::PointerButton {
                primary: true,
                pressed,
                position: at,
                clicks: 1,
                modifiers: Modifiers::default(),
            });
        }
        run_until(&mut board, |board| board.path() == Some(to.as_path()));
        assert!(!from.exists());
        assert_eq!(nodes_in(&to), 2);
        // The same document: it moved, it wasn't copied.
        let moved = imaginarium_io_native::open_reader(
            std::fs::File::open(&to).unwrap(),
            imaginarium_io_native::Credentials::None,
        )
        .unwrap()
        .header()
        .doc_id;
        assert_eq!(moved, id);
        assert_eq!(
            answers.asked.lock().unwrap()[0],
            format!("Rename or move the board | Some({dir:?}) | Plan.imag")
        );
        drop(board);
        assert!(app.wait_for_saves().is_empty());
    }

    #[test]
    fn a_board_open_elsewhere_is_never_replaced() {
        let dir = scratch("move-onto-open");
        let from = dir.join("Plan.imag");
        let other = dir.join("Other.imag");
        board_file(&from, 1);
        board_file(&other, 3);
        let app = app();
        let answers = answering(&app);
        let mut board = app.open_board(&from).unwrap();
        let _open = app.open_board(&other).unwrap();
        board_draw(&mut board);
        *answers.save.lock().unwrap() = Some(other.clone());
        board
            .execute("file.move", &imaginarium_commands::Args::new())
            .unwrap();
        run_until(&mut board, |board| {
            let (ui, fonts) = board.ui();
            ui.find_by_label(fonts, "Can't save the board there")
                .is_some()
        });
        assert_eq!(nodes_in(&other), 3);
        // It goes on saving where it was.
        assert_eq!(board.path(), Some(from.as_path()));
        board
            .execute("board.insert_frame", &imaginarium_commands::Args::new())
            .unwrap();
        run_until(&mut board, |_| nodes_in(&from) == 2);
        assert_eq!(nodes_in(&from), 2);
    }

    #[test]
    fn a_change_while_the_file_moves_survives_closing() {
        let dir = scratch("move-close");
        let from = dir.join("Plan.imag");
        board_file(&from, 1);
        let app = app();
        let answers = answering(&app);
        let mut board = app.open_board(&from).unwrap();
        board_draw(&mut board);
        let to = dir.join("Moved.imag");
        *answers.save.lock().unwrap() = Some(to.clone());
        board
            .execute("file.move", &imaginarium_commands::Args::new())
            .unwrap();
        // The move starts (the pill says so) …
        run_until(&mut board, |board| status_named(board, "Saving…"));
        assert!(status_named(&mut board, "Saving…"));
        // … a change, and the window closes before the move's result is in.
        board
            .execute("board.insert_frame", &imaginarium_commands::Args::new())
            .unwrap();
        assert!(board.close_requested());
        drop(board);
        assert!(app.wait_for_saves().is_empty());
        assert_eq!(nodes_in(&to), 2);
    }

    #[test]
    fn the_added_extension_never_replaces_a_board_unasked() {
        let dir = scratch("extension-taken");
        let budget = dir.join("Budget.imag");
        board_file(&budget, 3);
        let from = dir.join("Plan.imag");
        board_file(&from, 1);
        let app = app();
        let answers = answering(&app);
        let mut board = app.open_board(&from).unwrap();
        board_draw(&mut board);
        // The dialog asked about replacing "Budget", not "Budget.imag".
        *answers.save.lock().unwrap() = Some(dir.join("Budget"));
        board
            .execute("file.move", &imaginarium_commands::Args::new())
            .unwrap();
        run_until(&mut board, |board| {
            let (ui, fonts) = board.ui();
            ui.find_by_label(fonts, "Can't save the board there")
                .is_some()
        });
        assert_eq!(nodes_in(&budget), 3);
        assert_eq!(board.path(), Some(from.as_path()));
    }

    #[test]
    fn a_board_this_version_cant_write_has_no_copy() {
        // Unknown records or a newer format: a copy would drop them.
        let mut document = Document::new(DocumentKind::Board).unwrap();
        document.set_read_only(imaginarium_model::ReadOnlyReason::File(
            "it holds data this version doesn't know".into(),
        ));
        let app = app();
        let mut board = app.board_window(document, "Newer");
        assert!(
            board
                .execute("file.save_copy", &imaginarium_commands::Args::new())
                .is_err()
        );
        // Open elsewhere: a copy is fine.
        let mut document = Document::new(DocumentKind::Board).unwrap();
        document.set_read_only(imaginarium_model::ReadOnlyReason::File(
            imaginarium_io_native::IN_USE.into(),
        ));
        let mut board = app.board_window(document, "Elsewhere");
        assert!(
            board
                .execute("file.save_copy", &imaginarium_commands::Args::new())
                .is_ok()
        );
    }

    #[test]
    fn an_untitled_board_that_cant_be_saved_there_stays_kept() {
        let dir = scratch("save-refused");
        let other = dir.join("Other.imag");
        board_file(&other, 2);
        let app = app();
        app.keep_untitled_in(dir.join("recovery"));
        let answers = answering(&app);
        let _open = app.open_board(&other).unwrap();
        let mut board = app.new_board().unwrap();
        board_draw(&mut board);
        // Never changed: no recovery file yet; then saved onto a board open
        // elsewhere, which is refused.
        *answers.save.lock().unwrap() = Some(other.clone());
        board
            .execute("file.save", &imaginarium_commands::Args::new())
            .unwrap();
        run_until(&mut board, |board| {
            let (ui, fonts) = board.ui();
            ui.find_by_label(fonts, "Can't save the board there")
                .is_some()
        });
        assert_eq!(nodes_in(&other), 2);
        // Still kept: a change reaches its recovery file.
        board
            .execute("board.insert_frame", &imaginarium_commands::Args::new())
            .unwrap();
        let kept = board.kept_path().unwrap().to_owned();
        run_until(&mut board, |_| nodes_in(&kept) == 1);
        assert_eq!(nodes_in(&kept), 1);
    }

    #[test]
    fn save_a_copy_leaves_the_board_where_it_is() {
        let dir = scratch("copy");
        let from = dir.join("Plan.imag");
        board_file(&from, 2);
        let app = app();
        let answers = answering(&app);
        let mut board = app.open_board(&from).unwrap();
        board_draw(&mut board);
        let copy = dir.join("Plan copy.imag");
        *answers.save.lock().unwrap() = Some(copy.clone());
        board
            .execute("file.save_copy", &imaginarium_commands::Args::new())
            .unwrap();
        run_until(&mut board, |_| nodes_in(&copy) == 2);
        assert_eq!(nodes_in(&copy), 2);
        assert_eq!(board.path(), Some(from.as_path()));
        assert_eq!(
            answers.asked.lock().unwrap()[0],
            format!("Save a copy | Some({dir:?}) | Plan copy.imag")
        );
    }

    #[test]
    fn the_main_menu_has_the_file_commands() {
        let app = app();
        let mut board = app.new_board().unwrap();
        board_draw(&mut board);
        let (ui, fonts) = board.ui();
        let button = ui.find_by_label(fonts, "Main menu").unwrap();
        let at = ui.bounds(button).unwrap().center();
        for pressed in [true, false] {
            board.input(&Input::PointerButton {
                primary: true,
                pressed,
                position: at,
                clicks: 1,
                modifiers: Modifiers::default(),
            });
        }
        board_draw(&mut board);
        for row in [
            "New board",
            "Open…",
            "Save…",
            "Rename or move…",
            "Save a copy…",
        ] {
            assert!(status_named(&mut board, row), "{row}");
        }
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

    /// A fresh folder under the workspace's `target/` (never `/tmp`).
    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/test-scratch")
            .join(format!("shell-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Waits until `done`, at most 10 s.
    fn until(mut done: impl FnMut() -> bool) {
        let start = std::time::Instant::now();
        while !done() && start.elapsed() < std::time::Duration::from_secs(10) {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    /// Leaves a board in `folder` as a crashed app would.
    fn leave_board(folder: &std::path::Path, name: &str) -> std::path::PathBuf {
        let path = folder.join(name);
        let mut document = Document::new(DocumentKind::Board).unwrap();
        let snapshot = document.export_snapshot().unwrap();
        let autosave =
            imaginarium_io_native::Autosave::create(path.clone(), &snapshot, "tests".into(), || {});
        let before = document.version();
        document
            .transact(&imaginarium_model::Actor::User, |t| {
                t.create_node(
                    imaginarium_model::Parent::Root,
                    None,
                    &imaginarium_model::NodeKind::Frame,
                )
                .map(|_| ())
            })
            .unwrap();
        autosave.update(document.export_updates_since(&before).unwrap());
        assert!(autosave.finish().is_none());
        path
    }

    #[test]
    fn an_untitled_board_is_kept_until_it_closes() {
        let folder = scratch("untitled").join("recovery");
        let app = app();
        app.keep_untitled_in(folder.clone());
        let mut board = app.new_board().unwrap();
        board_draw(&mut board);
        // Not saved anywhere of its own, and nothing kept while it's empty.
        assert!(status_named(&mut board, "Save…"));
        assert_eq!(board.path(), None);
        let kept = board.kept_path().unwrap().to_owned();
        assert!(kept.starts_with(&folder));
        board
            .execute("board.insert_frame", &imaginarium_commands::Args::new())
            .unwrap();
        board_draw(&mut board);
        // The change is due at the latest 2 s later.
        until(|| {
            let now = std::time::Instant::now();
            if board.wake_at().is_some_and(|at| at <= now) {
                board.wake(now);
            }
            kept.exists()
                && imaginarium_io_native::open_path(&kept, imaginarium_io_native::Credentials::None)
                    .is_ok_and(|opened| opened.document.view().node_count() == 1)
        });
        assert!(kept.exists());
        assert!(status_named(&mut board, "Save…"));
        // Closed on purpose: the file goes.
        assert!(board.close_requested());
        drop(board);
        assert!(app.wait_for_saves().is_empty());
        assert!(!kept.exists());
    }

    #[test]
    fn a_board_gone_without_a_close_stays_kept() {
        // A window that failed, the app ending with an error, a panic: the
        // window is dropped without being closed.
        let folder = scratch("untitled-dropped").join("recovery");
        let app = app();
        app.keep_untitled_in(folder.clone());
        let mut board = app.new_board().unwrap();
        board_draw(&mut board);
        board
            .execute("board.insert_frame", &imaginarium_commands::Args::new())
            .unwrap();
        // Dropped before its change was even sent: that goes too.
        let kept = board.kept_path().unwrap().to_owned();
        drop(board);
        assert!(app.wait_for_saves().is_empty());
        assert_eq!(
            imaginarium_io_native::open_path(&kept, imaginarium_io_native::Credentials::None)
                .unwrap()
                .document
                .view()
                .node_count(),
            1
        );
        // The next start offers it.
        let next = super::tests::app();
        next.keep_untitled_in(folder);
        next.look_for_kept_boards();
        until(|| next.kept_search_done());
        assert_eq!(next.kept_boards().map(|(count, _)| count), Some(1));
    }

    fn start_has(start: &mut StartWindow, label: &str) -> bool {
        let (ui, fonts) = start.ui();
        ui.find_by_label(fonts, label).is_some()
    }

    #[test]
    fn after_a_crash_the_start_window_offers_the_kept_board() {
        let folder = scratch("crash");
        let left = leave_board(&folder, "untitled-1.imag");
        let app = app();
        app.keep_untitled_in(folder.clone());
        app.look_for_kept_boards();
        until(|| app.kept_boards().is_some());
        let mut start = app.start_window();
        frame(&mut start);
        let notice = "Imaginarium closed unexpectedly. Untitled board was kept, with every change.";
        assert!(start_has(&mut start, notice));
        let (ui, fonts) = start.ui();
        let button = ui.find_by_label(fonts, "Open it").unwrap();
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
        let requests = start.take_requests();
        // The board opens, still untitled; the start window stays.
        assert_eq!(names(&requests), ["Open(\"Untitled board — Imaginarium\")"]);
        frame(&mut start);
        assert!(!start_has(&mut start, notice));
        let Some(WindowRequest::Open(_, mut view)) = requests.into_iter().next() else {
            panic!("no board");
        };
        assert!(view.close_requested());
        drop(view);
        // Closed on purpose: no longer kept.
        assert!(app.wait_for_saves().is_empty());
        assert!(!left.exists());
    }

    #[test]
    fn several_kept_boards_are_offered_together() {
        let folder = scratch("crash-several");
        leave_board(&folder, "untitled-1.imag");
        leave_board(&folder, "untitled-2.imag");
        let app = app();
        app.keep_untitled_in(folder.clone());
        app.look_for_kept_boards();
        until(|| app.kept_boards().is_some());
        let mut start = app.start_window();
        frame(&mut start);
        assert!(start_has(
            &mut start,
            "Imaginarium closed unexpectedly. 2 boards were kept, with every change."
        ));
        assert!(start_has(&mut start, "Open them"));
        // Without a crash, no notice.
        let quiet = super::tests::app();
        quiet.keep_untitled_in(scratch("no-crash"));
        quiet.look_for_kept_boards();
        until(|| quiet.kept_search_done());
        assert!(quiet.kept_search_done());
        let mut start = quiet.start_window();
        frame(&mut start);
        assert!(!start_has(&mut start, "Open it"));
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
