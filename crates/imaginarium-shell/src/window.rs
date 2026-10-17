// SPDX-License-Identifier: AGPL-3.0-or-later

//! The board window as a [`View`]: the board, the panels over it, and what
//! their buttons ask for.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use imaginarium_canvas::{BACKGROUND, BoardCanvas};
use imaginarium_commands::{Args, CommandError, Keymap, Outcome, Registry as Commands, UsageStats};
use imaginarium_io_native::{Autosave, SaveState};
use imaginarium_render::Scene;
use imaginarium_render::kurbo::{Point, Rect, Size};
use imaginarium_settings::{Registry, SubscriptionId};
use imaginarium_text::{Fonts, TextStyle};
use imaginarium_ui::{
    AccessTree, Anchor, Flex, FrameInfo, Input, Key, Label, LayerId, LayerOptions, Placement,
    Token, Ui, View, WidgetId, WindowOptions, WindowRequest, theme,
};
use taffy::prelude::length;

use crate::app::App;
use crate::chrome::{self, Chrome, Placeholder};
use crate::commands::{ShellCx, shell_commands, shell_keymap};
use crate::contextbar::{self, ContextBar, Control};
use crate::inspector::{self, Field, Inspector, Popup};
use crate::palette::{self, Catalog, Kind, Palette, Sources};
use crate::toolbar::{self, Dock, DropZones, TOOLBAR_POSITION, ToolBar};

/// Saving… shows only after this long (`shell.md` §3.1).
const SAVING_SHOWN_AFTER: std::time::Duration = std::time::Duration::from_millis(500);

/// Changes are sent once the board has been still this long (a drag is one
/// save, not one per frame)...
const SAVE_AFTER_STILL: std::time::Duration = std::time::Duration::from_millis(300);

/// ...or at the latest this long after the first unsent change.
const SAVE_AT_LATEST: std::time::Duration = std::time::Duration::from_secs(2);

/// Wakes a board's window from its saving thread, once the window is
/// known.
type SaveWaker = Arc<std::sync::Mutex<Option<Box<dyn Fn() + Send>>>>;

/// What the saving thread calls to wake the window through `waker`.
fn wake_with(waker: &SaveWaker) -> impl Fn() + Send + Sync + 'static {
    let waker = waker.clone();
    move || {
        if let Ok(waker) = waker.lock()
            && let Some(wake) = waker.as_ref()
        {
            wake();
        }
    }
}

/// The file a board window saves into.
struct FileLink {
    path: PathBuf,
    /// The board is untitled: the file only keeps it for crash recovery
    /// (it has no place of its own yet), and goes when the board closes.
    untitled: bool,
    /// `None` once the window closed (it finishes on its own).
    autosave: Option<Autosave>,
    /// The document's version the file holds (or was sent).
    saved: imaginarium_model::Version,
    /// The board's revision last seen (a cheap check for changes).
    revision: u64,
    /// Unsent changes: since when, and the last one.
    unsent: Option<(Instant, Instant)>,
    /// Wakes this window when the save's state changes (set at the first
    /// frame, when the window is known).
    waker: SaveWaker,
}

/// What a panel's button asked for. The window does it after the input
/// that asked, when it can reach the board and the tree.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Ask {
    /// Run a command, the board's or the shell's.
    Run(&'static str),
    /// Open a window on a new board.
    NewBoard,
    /// Close this window.
    CloseWindow,
    /// Open the Settings window.
    OpenSettings,
    /// Show what a placeholder will do.
    Explain(Placeholder),
    /// Open the zoom menu.
    ZoomMenu,
    /// Close the menu that is open.
    CloseMenu,
    /// The tool bar's grip was pressed here (window space).
    Grab(Point),
    /// …dragged here…
    Drag(Point),
    /// …and released here, with Shift held or not.
    Drop(Point, bool),
    /// The drag ended without a release (the window lost the focus): the
    /// bar goes back where it was.
    CancelDrag,
    /// Put the tool bar there, and remember it.
    Dock(Dock),
    /// Open the command palette.
    OpenPalette,
    /// The palette's query changed.
    PaletteQuery(String),
    /// Move the palette's active row.
    PaletteMove(i32),
    /// Run the palette's row (the active one: `None`).
    PaletteRun(Option<usize>),
    /// Change the active row's value (a setting): forward, or back with
    /// Shift.
    PaletteTab(bool),
    /// The palette closed (Esc, a click outside, a command run).
    PaletteClosed,
    /// Open a contextual toolbar control's popover.
    BarOpen(crate::contextbar::Control),
    /// Run a command with arguments (a color, a width).
    RunWith(&'static str, Args),
    /// A typed hex color for a color command.
    BarColor(&'static str, String),
    /// A typed opacity (percent) for a color command.
    BarOpacity(&'static str, String),
    /// Move the focus to the next (or previous) control: ↓ and ↑ in menus.
    FocusStep(bool),
    /// Open the context menu from the keyboard (Shift+F10, the Menu key).
    ContextMenu,
    /// Move the keys to the next area (F6) or the previous one (Shift+F6):
    /// the board, the contextual toolbar, the tool bar, the panels.
    NextArea(bool),
    /// Open or close the inspector.
    ToggleInspector,
    /// An inspector field was entered.
    InspectorField(crate::inspector::Field, String),
    /// Open an inspector control's popover.
    InspectorOpen(crate::inspector::Popup),
    /// Width and height change together, or not.
    KeepProportions,
}

/// What the contextual toolbar's state depends on.
#[derive(Clone, Debug, PartialEq)]
struct BarKey {
    revision: u64,
    selection: Vec<imaginarium_model::NodeId>,
    gesturing: bool,
    theme: usize,
}

/// The queue of what the panels asked for.
pub(crate) type Asks = Rc<RefCell<Vec<Ask>>>;

/// A press on the grip travels this far (logical pixels) before it drags:
/// a click, with the pointer's jitter, leaves the bar where it is.
const DRAG_THRESHOLD: f64 = 4.0;

/// Room between a docked tool bar and the corner panels.
const CLEARANCE: f64 = 8.0;

/// The tool bar being dragged by its grip.
struct Drag {
    /// Where the grip was pressed, and the bar's corner then.
    grab: Point,
    origin: Point,
    /// Where the bar was before, to go back to if the drag is cancelled.
    before: (Dock, Point),
    /// Whether the press travelled [`DRAG_THRESHOLD`] yet.
    moved: bool,
    /// Where it would dock if dropped now (none: it floats).
    target: Option<Dock>,
}

/// How the stage around a docked tool bar is laid out: its padding (left,
/// right, top, bottom) and whether the bar is centered along its edge or
/// starts at the beginning (too long for the space).
#[derive(Clone, Copy, Debug, PartialEq)]
struct Placing {
    padding: [f64; 4],
    centered: bool,
}

/// A board window: the board fills it, the panels float over it
/// (`shell.md` §3). Run it with [`imaginarium_ui::run`] and
/// [`BoardWindow::options`].
pub struct BoardWindow {
    fonts: Fonts,
    ui: Ui,
    board: WidgetId,
    name: String,
    asks: Asks,
    chrome: Chrome,
    tools: ToolBar,
    commands: Commands<ShellCx>,
    /// The board's commands, for their availability in the palette.
    board_commands: Commands<BoardCanvas>,
    /// The shell's shortcuts (`Ctrl+K`).
    keymap: Keymap,
    catalog: Catalog,
    usage: UsageStats,
    palette: Option<Palette>,
    bar: ContextBar,
    /// What the bar's state was computed for.
    bar_key: Option<BarKey>,
    inspector: Inspector,
    /// What the inspector's snapshot was taken for.
    inspector_key: Option<BarKey>,
    /// Holds the tool bar; laid out for where it docks.
    stage: WidgetId,
    placing: Option<Placing>,
    zones: WidgetId,
    hint: (WidgetId, WidgetId),
    dock: Dock,
    vertical: bool,
    /// How the bar was last turned, so a drag turns it only on a change.
    oriented: Option<(bool, Placement)>,
    /// The floating tool bar's top left corner (window space).
    floating_at: Point,
    drag: Option<Drag>,
    /// The menu or note open, if any.
    popup: Option<LayerId>,
    settings: Rc<RefCell<Registry>>,
    /// Set when a setting changed (from the palette, Settings, another
    /// window); read at the next frame.
    settings_changed: Arc<AtomicBool>,
    /// Set when the board background changed; read at the next frame
    /// (apart from the tool bar's flag, which its own moves clear).
    background_changed: Arc<AtomicBool>,
    subscription: SubscriptionId,
    size: Size,
    close_after: Option<u64>,
    frames: u64,
    /// The app it is a window of, if any (not in tests).
    app: Option<Rc<App>>,
    /// The file the board is saved into, if it has one.
    file: Option<FileLink>,
    /// What the pill says about the file now.
    shown: chrome::SaveShown,
    /// What Settings sees of this board ("This board").
    link: Rc<crate::app::BoardLink>,
    /// The board's revision `link` was last brought up to date at.
    link_seen: Option<u64>,
    /// Windows to open, or this one to close.
    requests: Vec<WindowRequest>,
    /// It asked to close, or was asked to.
    closed: bool,
}

impl BoardWindow {
    /// A window showing `board`, named `name` ("Untitled board"). The
    /// tool bar starts where `settings` say ([`TOOLBAR_POSITION`], which
    /// must be registered), moves when they change, and moving it changes
    /// them.
    pub fn new(
        board: BoardCanvas,
        name: &str,
        fonts: Fonts,
        settings: Rc<RefCell<Registry>>,
    ) -> Self {
        let mut ui = Ui::new();
        let asks: Asks = Rc::default();
        let (tool, status) = (board.tool(), board.status());
        let keymap = board.keymap().clone();
        let shell_keys = shell_keymap(keymap.platform());
        let board = ui.add(board);
        let chrome = chrome::chrome(&mut ui, name, status, [&keymap, &shell_keys], &asks);
        let tools = toolbar::tool_bar(&mut ui, tool, &keymap, &asks);
        let zones = ui.add(DropZones { target: None });
        let hint_text = ui.add(
            Label::new("", TextStyle::new(12.0).weight(500.0))
                .color(Token::OnAccent)
                .single_line(),
        );
        let hint = ui.add_with_children(
            Flex::row()
                .height(30.0)
                .padding(10.0, 0.0)
                .radius(8.0)
                .background(Token::TooltipBg)
                .style(|style| {
                    style.position = taffy::Position::Absolute;
                    style.display = taffy::Display::None;
                }),
            &[hint_text],
        );
        let stage = ui.add_with_children(
            Flex::row().pass_through().style(|style| {
                style.position = taffy::Position::Absolute;
                style.inset = full();
            }),
            &[tools.bar],
        );
        let bar = ContextBar::new(&mut ui);
        let inspector = Inspector::new(&mut ui);
        let root = ui.add_with_children(
            Flex::column().fill(),
            &[
                board,
                chrome.overlay,
                zones,
                stage,
                inspector.panel,
                bar.panel,
                hint,
            ],
        );
        ui.set_root(root);
        let mut commands = Commands::new();
        for command in shell_commands() {
            // The shell's own ids are fixed and distinct: this can't fail.
            let _ = commands.register(*command);
        }
        let mut board_commands = Commands::new();
        for command in imaginarium_canvas::commands::board_commands() {
            // The board's ids are distinct (its own tests check it).
            let _ = board_commands.register(*command);
        }
        let listed =
            |id: imaginarium_commands::CommandId,
             meta: &imaginarium_commands::CommandMeta,
             args: &imaginarium_commands::ArgsSchema| palette::Listed {
                id: id.as_str(),
                meta: *meta,
                needs_args: args.args.iter().any(|arg| arg.required),
            };
        let catalog = Catalog::new(
            board_commands
                .commands()
                .map(|command| listed(command.id(), command.meta(), command.args()))
                .chain(
                    commands
                        .commands()
                        .map(|command| listed(command.id(), command.meta(), command.args())),
                )
                .collect::<Vec<_>>(),
            &settings.borrow(),
        );
        let settings_changed = Arc::new(AtomicBool::new(false));
        let flag = settings_changed.clone();
        let background_changed = Arc::new(AtomicBool::new(false));
        let background_flag = background_changed.clone();
        let subscription = settings.borrow_mut().subscribe(move |change| {
            if change.key == BACKGROUND.key() {
                background_flag.store(true, Ordering::Relaxed);
                imaginarium_ui::request_frame_everywhere();
            }
            if change.key == TOOLBAR_POSITION.key() {
                flag.store(true, Ordering::Relaxed);
                // The change may come from another window: this one
                // draws to follow it.
                imaginarium_ui::request_frame_everywhere();
            }
        });
        let dock = settings
            .borrow()
            .get(&TOOLBAR_POSITION)
            .unwrap_or(Dock::Left);
        let mut window = Self {
            fonts,
            ui,
            board,
            name: name.to_owned(),
            asks,
            chrome,
            tools,
            commands,
            board_commands,
            keymap: shell_keys,
            catalog,
            usage: UsageStats::new(),
            palette: None,
            bar,
            bar_key: None,
            inspector,
            inspector_key: None,
            stage,
            placing: None,
            zones,
            hint: (hint, hint_text),
            dock: Dock::Left,
            vertical: true,
            oriented: None,
            floating_at: Point::new(96.0, 96.0),
            drag: None,
            popup: None,
            settings,
            settings_changed,
            background_changed,
            subscription,
            size: Size::new(1440.0, 900.0),
            close_after: None,
            frames: 0,
            app: None,
            file: None,
            shown: chrome::SaveShown::Draft,
            link: Rc::default(),
            link_seen: None,
            requests: Vec::new(),
            closed: false,
        };
        // Shown where the setting says, without writing it back.
        window.put_bar(dock);
        window.apply_background();
        window.ui.set_focus(&mut window.fonts, Some(board));
        window
    }

    /// Makes it a window of `app`: New board (`Ctrl+N`) opens another one,
    /// and closing the last board opens the start window.
    pub fn in_app(mut self, app: Rc<App>) -> Self {
        app.board_opened();
        app.board_active(&self.link);
        if let Some(old) = self.app.replace(app) {
            old.board_closed();
        }
        self
    }

    /// Saves every change of the board into its file, through `session`
    /// (opened on that file: [`imaginarium_io_native::Session::open`]).
    /// Changes the document has that the file doesn't (migrations on open)
    /// are saved first.
    pub fn saved_in(self, path: PathBuf, session: imaginarium_io_native::Session) -> Self {
        self.linked(path, session, false)
    }

    /// Keeps an untitled board in a file at `path` for crash recovery,
    /// from its first change on (`shell.md` §4); the file is deleted when
    /// the board closes. `created_with` names the app in the file.
    pub fn kept_in(mut self, path: PathBuf, created_with: &str) -> Self {
        let Some(document) = self
            .ui
            .widget::<BoardCanvas>(self.board)
            .map(BoardCanvas::document)
        else {
            return self;
        };
        let Ok(snapshot) = document.export_snapshot() else {
            return self;
        };
        let saved = document.version();
        let revision = self.board_revision();
        let waker: SaveWaker = Arc::default();
        let autosave = Autosave::create(
            path.clone(),
            &snapshot,
            created_with.to_owned(),
            wake_with(&waker),
        );
        self.file = Some(FileLink {
            path,
            untitled: true,
            autosave: Some(autosave),
            saved,
            revision,
            unsent: None,
            waker,
        });
        self.update_save_state();
        self
    }

    /// Goes on keeping an untitled board found after a crash in its file
    /// at `path`, through `session`.
    pub fn kept_from(self, path: PathBuf, session: imaginarium_io_native::Session) -> Self {
        self.linked(path, session, true)
    }

    fn linked(
        mut self,
        path: PathBuf,
        session: imaginarium_io_native::Session,
        untitled: bool,
    ) -> Self {
        let Some(document) = self
            .ui
            .widget::<BoardCanvas>(self.board)
            .map(BoardCanvas::document)
        else {
            return self;
        };
        let saved = document
            .version_at_open()
            .unwrap_or_else(|| document.version());
        let revision = self.board_revision();
        let waker: SaveWaker = Arc::default();
        let id = imaginarium_io_native::FileId {
            path: path.clone(),
            doc_id: session.doc_id(),
        };
        let autosave = Autosave::start(session, id, wake_with(&waker));
        self.file = Some(FileLink {
            path,
            untitled,
            autosave: Some(autosave),
            saved,
            revision,
            // Whatever the opening added goes out at once.
            unsent: Some((Instant::now(), Instant::now())),
            waker,
        });
        self.save_changes(true);
        self.update_save_state();
        self
    }

    /// The file the board is saved into, if it has one (an untitled
    /// board's recovery file isn't its place).
    pub fn path(&self) -> Option<&std::path::Path> {
        self.file
            .as_ref()
            .filter(|file| !file.untitled)
            .map(|file| file.path.as_path())
    }

    /// The file an untitled board is kept in for crash recovery.
    pub fn kept_path(&self) -> Option<&std::path::Path> {
        self.file
            .as_ref()
            .filter(|file| file.untitled)
            .map(|file| file.path.as_path())
    }

    fn board_revision(&self) -> u64 {
        self.ui
            .widget::<BoardCanvas>(self.board)
            .map_or(0, BoardCanvas::revision)
    }

    /// Sends the board's changes since the last time to the file's saving
    /// thread, once the board has been still a moment (or at the latest
    /// after a while, or at once with `now`); never in the middle of a
    /// gesture, unless `now`.
    fn save_changes(&mut self, now: bool) {
        let revision = self.board_revision();
        let gesturing = self
            .ui
            .widget::<BoardCanvas>(self.board)
            .is_some_and(BoardCanvas::is_gesturing);
        let Some(file) = &mut self.file else {
            return;
        };
        let at = Instant::now();
        if file.revision != revision {
            file.revision = revision;
            let first = file.unsent.map_or(at, |(first, _)| first);
            file.unsent = Some((first, at));
        }
        let Some((first, last)) = file.unsent else {
            return;
        };
        let due = at.duration_since(last) >= SAVE_AFTER_STILL
            || at.duration_since(first) >= SAVE_AT_LATEST;
        if !now && (gesturing || !due) {
            return;
        }
        // Nothing to send them to: no deadline stays behind, in the past.
        let (Some(board), Some(autosave)) =
            (self.ui.widget::<BoardCanvas>(self.board), &file.autosave)
        else {
            file.unsent = None;
            return;
        };
        let document = board.document();
        let version = document.version();
        if version != file.saved {
            match document.export_updates_since(&file.saved) {
                Ok(update) => {
                    autosave.update(update);
                    file.saved = version;
                }
                // Not sent: tried again in a moment (the baseline stays, so
                // nothing is skipped); a deadline left in the past would
                // wake the window without end.
                Err(_) => {
                    file.unsent = Some((at, at));
                    return;
                }
            }
        }
        file.unsent = None;
    }

    /// When the unsent changes are due, if there are any and they can go
    /// then: not during a gesture (its end is input, which saves).
    fn save_deadline(&self) -> Option<Instant> {
        let gesturing = self
            .ui
            .widget::<BoardCanvas>(self.board)
            .is_some_and(BoardCanvas::is_gesturing);
        if gesturing {
            return None;
        }
        let (first, last) = self.file.as_ref()?.unsent?;
        Some((last + SAVE_AFTER_STILL).min(first + SAVE_AT_LATEST))
    }

    /// Shows in the pill how the file is doing, when that changed. Saving…
    /// only after a while, so quick saves don't flicker (`shell.md` §3.1).
    fn update_save_state(&mut self) {
        let mode = self
            .ui
            .widget::<BoardCanvas>(self.board)
            .map(|board| board.document().mode().clone());
        let shown = match (&mode, &self.file) {
            (Some(imaginarium_model::Mode::ReadOnly(reason)), _) => {
                chrome::SaveShown::ReadOnly(read_only_text(reason))
            }
            // Kept for recovery, but not saved anywhere of its own.
            (_, None) => chrome::SaveShown::Draft,
            (_, Some(file)) if file.untitled => chrome::SaveShown::Draft,
            (_, Some(file)) => match file.autosave.as_ref().map(Autosave::state) {
                None | Some(SaveState::Saved) => chrome::SaveShown::Saved,
                Some(SaveState::Saving(since)) if since.elapsed() >= SAVING_SHOWN_AFTER => {
                    chrome::SaveShown::Saving
                }
                // Not long yet: still the state before.
                Some(SaveState::Saving(_)) => match &self.shown {
                    chrome::SaveShown::Draft => chrome::SaveShown::Saved,
                    shown => shown.clone(),
                },
                Some(SaveState::Failed(reason)) => chrome::SaveShown::Failed(reason),
            },
        };
        if shown == self.shown {
            return;
        }
        self.shown = shown;
        let slot = self.chrome.save_slot;
        let old: Vec<WidgetId> = self.ui.children(slot).to_vec();
        for widget in old {
            self.ui.remove(widget);
        }
        let new = chrome::save_state(
            &mut self.ui,
            &self.shown,
            &self.asks,
            &mut self.chrome.anchors,
        );
        self.ui.set_children(slot, &new);
    }

    /// When the pill should show Saving…, while a save runs and it doesn't
    /// yet.
    fn saving_deadline(&self) -> Option<Instant> {
        if self.shown == chrome::SaveShown::Saving {
            return None;
        }
        match self.file.as_ref()?.autosave.as_ref()?.state() {
            SaveState::Saving(since) => Some(since + SAVING_SHOWN_AFTER),
            SaveState::Saved | SaveState::Failed(_) => None,
        }
    }

    /// Sends what isn't saved yet, and lets the saving thread finish on its
    /// own (the app waits for it before it exits). An untitled board's
    /// recovery file is deleted instead: it closed on purpose.
    fn finish_saving(&mut self) {
        let untitled = self.file.as_ref().is_some_and(|file| file.untitled);
        if !untitled {
            self.save_changes(true);
        }
        let Some(autosave) = self.file.as_mut().and_then(|file| file.autosave.take()) else {
            return;
        };
        let finishing = if untitled {
            autosave.discard_in_background()
        } else {
            autosave.finish_in_background()
        };
        match &self.app {
            Some(app) => app.finishing(finishing),
            None => {
                let _ = finishing.join();
            }
        }
    }

    /// Closes the window after `frames` frames (for scripted runs).
    pub fn close_after(mut self, frames: u64) -> Self {
        self.close_after = Some(frames);
        self
    }

    /// The window's options: its title (`<name> — Imaginarium`), app ID,
    /// size and background.
    pub fn options(&self) -> WindowOptions {
        WindowOptions {
            title: format!("{} — Imaginarium", self.name),
            size: Size::new(1440.0, 900.0),
            background: theme().color(Token::BoardBg),
            app_id: Some("ch.giosuel.Imaginarium".into()),
            more_windows: true,
            ..WindowOptions::default()
        }
    }

    /// Draws the board's background as the settings say.
    fn apply_background(&mut self) {
        let background = self.settings.borrow().get(&BACKGROUND).unwrap_or_default();
        if let Some(board) = self.ui.widget_mut::<BoardCanvas>(self.board)
            && board.background() != background
        {
            board.set_background(background);
            self.ui.refresh(self.board);
        }
    }

    /// Where the tool bar is.
    pub fn dock(&self) -> Dock {
        self.dock
    }

    /// The tree, for tests and scripted runs.
    pub fn ui(&mut self) -> (&mut Ui, &mut Fonts) {
        (&mut self.ui, &mut self.fonts)
    }

    #[cfg(test)]
    pub(crate) fn board_widget(&self) -> WidgetId {
        self.board
    }

    /// The stored fill the contextual toolbar shows.
    #[cfg(test)]
    pub(crate) fn bar_fill_for_tests(&self) -> Option<String> {
        self.bar.stored_color("style.set_fill")
    }

    /// Whether the board's state was looked at in the last frame.
    #[cfg(test)]
    pub(crate) fn board_mut_for_tests(&mut self) -> Option<&mut BoardCanvas> {
        self.ui.widget_mut::<BoardCanvas>(self.board)
    }

    /// The board.
    pub fn board(&self) -> Option<&BoardCanvas> {
        self.ui.widget::<BoardCanvas>(self.board)
    }

    /// Runs the command `id`, the shell's (`view.dock_toolbar`) or the
    /// board's, as the palette and the AI do.
    pub fn execute(&mut self, id: &str, args: &Args) -> Result<Outcome, CommandError> {
        let outcome = if self.commands.get(id).is_some() {
            let mut cx = ShellCx {
                in_app: self.app.is_some(),
                ..ShellCx::default()
            };
            let outcome = self.commands.execute(&mut cx, id, args)?;
            self.asks.borrow_mut().extend(cx.asks);
            outcome
        } else {
            let Some(board) = self.ui.widget_mut::<BoardCanvas>(self.board) else {
                return Err(CommandError::Unknown(id.to_owned()));
            };
            board.execute(id, args)?
        };
        self.handle_asks();
        Ok(outcome)
    }

    /// Does what Settings asked of this board, and tells it the board's own
    /// values when they changed.
    fn sync_link(&mut self) {
        let asks = self.link.take_asks();
        let asked = !asks.is_empty();
        for (key, value) in asks {
            let mut args =
                Args::new().with("key", imaginarium_commands::ArgValue::String(key.into()));
            if let Some(value) = &value {
                args = args.with(
                    "value",
                    imaginarium_commands::ArgValue::String(crate::settings::format_value(value)),
                );
            }
            // Settings checked the value; what can still fail (a
            // read-only board) shows on its row.
            let error = self
                .execute("board.set_setting", &args)
                .err()
                .map(|err| (key, err.to_string()));
            if error.is_some() || self.link.error.borrow().is_some() {
                *self.link.error.borrow_mut() = error;
                self.link.changed();
            }
        }
        let Some(board) = self.ui.widget::<BoardCanvas>(self.board) else {
            return;
        };
        // Worked out again only after a document change (or a request).
        let revision = board.revision();
        if !asked && self.link_seen == Some(revision) {
            return;
        }
        self.link_seen = Some(revision);
        let read_only = !matches!(board.document().mode(), imaginarium_model::Mode::Editable);
        let own: std::collections::BTreeMap<&'static str, imaginarium_settings::Value> =
            imaginarium_canvas::board_settings::BOARD_SETTINGS
                .iter()
                .filter_map(|setting| Some((setting.key(), board.own_setting(setting.key())?)))
                .collect();
        let changed = *self.link.own.borrow() != own
            || *self.link.name.borrow() != self.name
            || self.link.read_only.get() != read_only;
        if changed {
            *self.link.own.borrow_mut() = own;
            *self.link.name.borrow_mut() = self.name.clone();
            self.link.read_only.set(read_only);
            // Settings, in its own window, shows them.
            self.link.changed();
        }
    }

    /// Lays the window out and draws it into `scene` at `size`.
    pub fn draw(&mut self, scene: &mut Scene, size: Size) {
        self.size = size;
        self.sync_link();
        // This window wakes when its save's state changes.
        if let Some(file) = &self.file
            && let Ok(mut waker) = file.waker.lock()
            && waker.is_none()
            && let Some(wake) = imaginarium_ui::window_waker()
        {
            *waker = Some(Box::new(wake));
        }
        self.save_changes(false);
        self.update_save_state();
        if let Some(palette) = &mut self.palette {
            palette.resize(&mut self.ui, size.width, size.height);
        }
        // During a drag the bar stays where the pointer has it; the change
        // waits until it is dropped.
        if self.drag.is_none() && self.settings_changed.swap(false, Ordering::Relaxed) {
            let dock = self
                .settings
                .borrow()
                .get(&TOOLBAR_POSITION)
                .unwrap_or(self.dock);
            if dock != self.dock {
                self.put_bar(dock);
            }
        }
        // The background doesn't move the bar: it follows at once, even
        // during a drag.
        if self.background_changed.swap(false, Ordering::Relaxed) {
            self.apply_background();
        }
        if self.dock == Dock::Floating && self.drag.is_none() {
            // A smaller window must not lose the bar.
            let clamped = self.clamp(self.floating_at);
            if clamped != self.floating_at {
                self.place_floating(clamped);
            }
        }
        // The contextual toolbar follows the selection; it steps aside
        // while the pointer moves something.
        // Its state is computed only when something it shows can have
        // changed: the document, the selection, the theme, a gesture.
        let fonts = &mut self.fonts;
        if let Some(board) = self.ui.widget_mut::<BoardCanvas>(self.board) {
            // The board's items as of now, so a new node's box is there.
            board.update(fonts);
        }
        let key = self.board().map(|board| BarKey {
            revision: board.revision(),
            selection: board.selection().get_untracked(),
            gesturing: board.is_gesturing(),
            theme: std::rc::Rc::as_ptr(&theme()) as usize,
        });
        let changed = key != self.bar_key;
        self.bar_key = key;
        let bar_state = if changed {
            self.board().and_then(|board| {
                (!board.is_gesturing())
                    .then(|| contextbar::state(board, &self.board_commands))
                    .flatten()
            })
        } else {
            self.bar.state.clone()
        };
        if bar_state != self.bar.state {
            let keymap = self.board().map(|board| board.keymap().clone());
            if let Some(keymap) = keymap {
                self.bar
                    .rebuild(&mut self.ui, bar_state, &keymap, &self.asks);
            }
        }
        self.refresh_inspector();
        let area = Rect::from_origin_size((0.0, 0.0), size);
        self.ui.layout(&mut self.fonts, area);
        // A docked bar keeps clear of the corner panels, which the first
        // layout measured; lay out again if that moves it.
        let mut again = false;
        if self.dock != Dock::Floating {
            let placing = self.placing();
            if self.placing != Some(placing) {
                self.apply_placing(placing);
                again = true;
            }
        }
        let selection = self
            .bar
            .state
            .as_ref()
            .and_then(|_| self.board())
            .and_then(BoardCanvas::selection_on_screen);
        if again {
            self.ui.layout(&mut self.fonts, area);
        }
        // A bar just rebuilt (or shown) was measured by the layout before:
        // placed once with that size, laid out, placed again if it moved.
        for _ in 0..2 {
            if !self.bar.place(&mut self.ui, selection, size) {
                break;
            }
            self.ui.layout(&mut self.fonts, area);
        }
        self.ui.paint(&mut self.fonts, scene);
    }

    /// Does what the panels asked for, until they ask for nothing more.
    fn handle_asks(&mut self) {
        loop {
            let asks = std::mem::take(&mut *self.asks.borrow_mut());
            if asks.is_empty() {
                break;
            }
            for ask in asks {
                self.handle(ask);
            }
        }
        // Clicks on a panel's background, or a closed popup, leave no
        // focus: the keyboard goes back to the board.
        if self.ui.focused().is_none() {
            self.ui.set_focus(&mut self.fonts, Some(self.board));
        }
    }

    fn handle(&mut self, ask: Ask) {
        match ask {
            Ask::Run(command) => {
                // A command that isn't available now (redo with nothing to
                // redo) does nothing, as its key would.
                let _ = self.execute(command, &Args::new());
            }
            Ask::Explain(placeholder) => {
                let Some(&anchor) = self.chrome.anchors.get(&placeholder) else {
                    return;
                };
                let content = chrome::note(&mut self.ui, placeholder);
                let placement = match placeholder {
                    Placeholder::Help => Placement::Above,
                    _ => Placement::Below,
                };
                self.open_popup(content, anchor, placement);
            }
            Ask::ZoomMenu => {
                let keymap = match self.board() {
                    Some(board) => board.keymap().clone(),
                    None => return,
                };
                let content = chrome::zoom_menu(&mut self.ui, &keymap, &self.asks);
                self.open_popup(content, self.chrome.zoom, Placement::Above);
            }
            Ask::CloseMenu => {
                if let Some(popup) = self.popup.take() {
                    self.ui.close_layer(&mut self.fonts, popup);
                }
            }
            Ask::Grab(at) => {
                let origin = self
                    .ui
                    .bounds(self.tools.bar)
                    .map_or(Point::ZERO, |bounds| bounds.origin());
                self.drag = Some(Drag {
                    grab: at,
                    origin,
                    before: (self.dock, self.floating_at),
                    moved: false,
                    target: None,
                });
            }
            Ask::Drag(at) => self.drag_to(at),
            Ask::Drop(at, shift) => {
                let Some(drag) = self.drag.take() else {
                    return;
                };
                if !drag.moved {
                    // A click on the grip: the bar stays where it is.
                    return;
                }
                self.show_drop(None, Point::ZERO);
                let target = if shift {
                    None
                } else {
                    toolbar::snap(at, self.size)
                };
                match target {
                    Some(edge) => self.move_bar(edge),
                    None => {
                        let corner = self.clamp(drag.origin + (at - drag.grab));
                        self.place_floating(corner);
                        self.remember(Dock::Floating);
                    }
                }
            }
            Ask::CancelDrag => {
                let Some(drag) = self.drag.take() else {
                    return;
                };
                self.show_drop(None, Point::ZERO);
                if drag.moved {
                    let (dock, corner) = drag.before;
                    self.floating_at = corner;
                    self.put_bar(dock);
                }
            }
            Ask::Dock(dock) => {
                if self.drag.take().is_some() {
                    self.show_drop(None, Point::ZERO);
                }
                self.move_bar(dock);
            }
            Ask::OpenPalette => self.open_palette(),
            Ask::PaletteQuery(query) => {
                let board_keys = self.board().map(|board| board.keymap().clone());
                let settings = self.settings.clone();
                let settings = settings.borrow();
                let (Some(palette), Some(board_keys)) = (&mut self.palette, board_keys) else {
                    return;
                };
                let sources = Sources {
                    catalog: &self.catalog,
                    settings: &settings,
                    keymaps: [&board_keys, &self.keymap],
                    usage: &self.usage,
                    now: now(),
                    asks: &self.asks,
                };
                palette.search(&mut self.ui, &query, &sources);
            }
            Ask::PaletteMove(by) => {
                if let Some(row) = self.palette.as_mut().and_then(|palette| palette.step(by)) {
                    self.ui.reveal(&mut self.fonts, row);
                }
            }
            Ask::PaletteRun(row) => self.run_palette(row, None),
            Ask::PaletteTab(forward) => self.run_palette(None, Some(forward)),
            Ask::PaletteClosed => {
                if let Some(palette) = self.palette.take() {
                    palette.dispose(&mut self.ui);
                }
            }
            Ask::BarOpen(control) => self.open_bar_popup(control),
            Ask::RunWith(command, args) => {
                let _ = self.execute(command, &args);
            }
            Ask::BarColor(command, text) => {
                if let Some(color) = contextbar::typed_color(&text) {
                    // Six digits typed keep the opacity the color had.
                    let color = keep_alpha(&color, self.bar.stored_color(command).as_deref());
                    let args =
                        Args::new().with("color", imaginarium_commands::ArgValue::String(color));
                    let _ = self.execute(command, &args);
                }
            }
            Ask::BarOpacity(command, text) => {
                // A percent for a color opacity command (each node keeps
                // its own color).
                if let Ok(percent) = text.trim().trim_end_matches('%').trim().parse::<f64>() {
                    let args = Args::new().with(
                        "opacity",
                        imaginarium_commands::ArgValue::Float((percent / 100.0).clamp(0.0, 1.0)),
                    );
                    let _ = self.execute(command, &args);
                }
            }
            Ask::ContextMenu => {
                // At the selection's middle, or the board's.
                let at = self
                    .board()
                    .and_then(BoardCanvas::selection_on_screen)
                    .map_or(
                        Point::new(self.size.width / 2.0, self.size.height / 2.0),
                        |selection| selection.center(),
                    );
                self.context_menu(at);
            }
            Ask::NextArea(forward) => self.next_area(forward),
            Ask::ToggleInspector => {
                let open = !self.inspector.open;
                self.inspector.set_open(&mut self.ui, open);
                self.refresh_inspector();
            }
            Ask::InspectorField(field, text) => self.inspector_field(field, &text),
            Ask::InspectorOpen(popup) => self.open_inspector_popup(popup),
            Ask::KeepProportions => {
                self.inspector.keep_proportions = !self.inspector.keep_proportions;
                self.inspector.shown = None;
                self.refresh_inspector();
            }
            Ask::FocusStep(forward) => self.ui.focus_next(&mut self.fonts, forward),
            Ask::NewBoard => {
                let Some(app) = &self.app else {
                    return;
                };
                if let Ok(window) = app.new_board() {
                    self.requests
                        .push(WindowRequest::Open(window.options(), Box::new(window)));
                }
            }
            Ask::CloseWindow => {
                self.closing();
                self.requests.push(WindowRequest::Close);
            }
            Ask::OpenSettings => {
                if let Some(window) = self.app.as_ref().and_then(App::settings_window) {
                    self.requests
                        .push(WindowRequest::Open(window.options(), Box::new(window)));
                }
            }
        }
    }

    /// The window closes: the start window opens if it was the last board.
    fn closing(&mut self) {
        if std::mem::replace(&mut self.closed, true) {
            return;
        }
        // Nothing waits for another frame now.
        self.finish_saving();
        // It stops counting at once: several boards closing in one go
        // (before any is dropped) see each other gone.
        if let Some(app) = &self.app
            && app.board_closed() == 0
        {
            let start = app.start_window();
            self.requests
                .push(WindowRequest::Open(start.options(), Box::new(start)));
        }
    }

    /// Rebuilds the inspector when what it shows changed (not during a
    /// gesture: it would follow every pointer move), keeping the keys in
    /// the control that had them. Its snapshot is taken only when the
    /// document, the selection or the theme changed.
    fn refresh_inspector(&mut self) {
        if !self.inspector.open {
            return;
        }
        let Some(board) = self.ui.widget::<BoardCanvas>(self.board) else {
            return;
        };
        if board.is_gesturing() {
            return;
        }
        let key = BarKey {
            revision: board.revision(),
            selection: board.selection().get_untracked(),
            gesturing: false,
            theme: std::rc::Rc::as_ptr(&theme()) as usize,
        };
        if self.inspector.shown.is_some() && self.inspector_key.as_ref() == Some(&key) {
            return;
        }
        self.inspector_key = Some(key);
        let snapshot = inspector::snapshot(board, &self.board_commands);
        if self.inspector.shown.as_ref() == Some(&snapshot) {
            return;
        }
        let label = snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.selection.first().copied())
            .map_or_else(String::new, |node| inspector::label_of(board, node));
        let close_keys = crate::toolbar::shortcut(&self.keymap, "view.inspector");
        let focused = self.ui.focused();
        let field = self
            .inspector
            .fields
            .iter()
            .find(|(_, id)| Some(**id) == focused)
            .map(|(field, _)| *field);
        self.inspector.rebuild(
            &mut self.ui,
            &mut self.fonts,
            snapshot,
            &label,
            close_keys,
            &self.asks,
        );
        if let Some(widget) = field.and_then(|field| self.inspector.fields.get(&field).copied()) {
            self.ui.set_focus(&mut self.fonts, Some(widget));
        }
    }

    /// Runs what an inspector field asks for. Text that isn't a value, or
    /// a value the command refuses, puts the field back as it was.
    fn inspector_field(&mut self, field: Field, text: &str) {
        let Some(snapshot) = self.inspector.snapshot.clone() else {
            return;
        };
        let text = text.trim();
        // Enter on a field as it was shown changes nothing (its value may
        // be rounded for showing).
        if self.inspector.texts.get(&field).map(String::as_str) == Some(text) {
            return;
        }
        let number = text
            .trim_end_matches(['%', '°'])
            .trim_end_matches("pt")
            .trim()
            .parse::<f64>()
            .ok()
            .filter(|value| value.is_finite());
        let float = |name: &str, low: f64, high: f64| {
            number.map(|value| {
                Args::new().with(
                    name,
                    imaginarium_commands::ArgValue::Float(value.clamp(low, high)),
                )
            })
        };
        use imaginarium_commands::ArgValue;
        let run: Option<(&'static str, Args)> = match field {
            Field::Name => Some((
                "board.rename",
                Args::new().with("name", ArgValue::String(text.to_owned())),
            )),
            Field::X | Field::Y | Field::W | Field::H | Field::Rotation => {
                let (Some(current), [node]) = (snapshot.geometry, &snapshot.selection[..]) else {
                    return;
                };
                inspector::geometry_for(field, text, current, self.inspector.keep_proportions).map(
                    |geometry| {
                        // Rotation within one turn, as the box keeps it.
                        let rotation = (geometry.r + 180.0).rem_euclid(360.0) - 180.0;
                        (
                            "board.set_box",
                            Args::new()
                                .with("node", ArgValue::String(node.to_string()))
                                .with("x", ArgValue::Float(geometry.x))
                                .with("y", ArgValue::Float(geometry.y))
                                .with("w", ArgValue::Float(geometry.w.max(0.0)))
                                .with("h", ArgValue::Float(geometry.h.max(0.0)))
                                .with("rotation", ArgValue::Float(rotation)),
                        )
                    },
                )
            }
            Field::Radius => float("radius", 0.0, 1.0e6).map(|args| ("style.set_radius", args)),
            Field::StrokeWidth => {
                float("width", 0.0, 64.0).map(|args| ("style.set_stroke_width", args))
            }
            Field::TextSize => float("size", 4.0, 400.0).map(|args| ("style.set_text_size", args)),
            Field::FillOpacity => float("opacity", 0.0, 100.0).map(|args| {
                let percent = args.float("opacity").unwrap_or(100.0);
                (
                    "style.set_fill_opacity",
                    Args::new().with("opacity", ArgValue::Float(percent / 100.0)),
                )
            }),
            Field::FillHex | Field::StrokeHex | Field::TextHex => {
                let (command, stored) = match field {
                    Field::FillHex => ("style.set_fill", snapshot.fill.and_then(|fill| fill.0)),
                    Field::StrokeHex => (
                        "style.set_stroke",
                        snapshot.stroke.and_then(|stroke| stroke.0),
                    ),
                    _ => (
                        "style.set_text_color",
                        snapshot.text.and_then(|text| text.0),
                    ),
                };
                contextbar::typed_color(text).map(|color| {
                    let color = keep_alpha(&color, stored.as_deref());
                    (command, Args::new().with("color", ArgValue::String(color)))
                })
            }
            _ => None,
        };
        let applied = run.is_some_and(|(command, args)| {
            matches!(self.execute(command, &args), Ok(Outcome::Done))
        });
        if !applied {
            // Show the real value again.
            self.inspector.shown = None;
            self.inspector_key = None;
            self.refresh_inspector();
        }
    }

    /// Opens an inspector control's popover.
    fn open_inspector_popup(&mut self, popup: Popup) {
        let Some(anchor) = self.inspector.popups.get(&popup).copied() else {
            return;
        };
        let Some(snapshot) = self.inspector.snapshot.clone() else {
            return;
        };
        let content = match popup {
            Popup::Type => {
                let choices: Vec<(String, String)> = [
                    ("rect", "Rectangle"),
                    ("ellipse", "Ellipse"),
                    ("diamond", "Diamond"),
                    ("triangle", "Triangle"),
                    ("line", "Line"),
                ]
                .iter()
                .map(|(value, name)| ((*value).to_owned(), (*name).to_owned()))
                .collect();
                let current = snapshot.preset.clone().unwrap_or_default();
                contextbar::choice_popover(
                    &mut self.ui,
                    "Shape type",
                    "shape.set_preset",
                    "preset",
                    &choices,
                    &current,
                    &self.asks,
                )
            }
            Popup::Style => {
                let mut choices = vec![(String::new(), "No style".to_owned())];
                choices.extend(snapshot.styles.iter().cloned());
                let current = snapshot
                    .style
                    .as_ref()
                    .map_or(String::new(), |(id, _)| id.clone());
                contextbar::choice_popover(
                    &mut self.ui,
                    "Named style",
                    "style.apply",
                    "style",
                    &choices,
                    &current,
                    &self.asks,
                )
            }
            Popup::Fill | Popup::Stroke | Popup::TextColor => {
                let (title, command, stored) = match popup {
                    Popup::Fill => (
                        "Fill",
                        "style.set_fill",
                        snapshot.fill.and_then(|fill| fill.0),
                    ),
                    Popup::Stroke => (
                        "Outline",
                        "style.set_stroke",
                        snapshot.stroke.and_then(|stroke| stroke.0),
                    ),
                    _ => (
                        "Text color",
                        "style.set_text_color",
                        snapshot.text.and_then(|text| text.0),
                    ),
                };
                contextbar::color_popover(
                    &mut self.ui,
                    title,
                    command,
                    stored.as_deref(),
                    &self.asks,
                )
            }
        };
        self.open_popup(content, anchor, Placement::Left);
    }

    /// Moves the keys to the next area, or the previous one: the board,
    /// the contextual toolbar (when it shows), the tool bar, the
    /// document pill, the top right.
    fn next_area(&mut self, forward: bool) {
        let mut areas = vec![self.board];
        areas.extend(self.bar.buttons.first().map(|(_, button)| *button));
        areas.extend(self.tools.buttons.first().copied());
        areas.extend(
            [Placeholder::MainMenu, Placeholder::Ai]
                .iter()
                .filter_map(|placeholder| self.chrome.anchors.get(placeholder).copied()),
        );
        let focused = self.ui.focused();
        // Where the keys are now: the area that holds the focused widget.
        let current = areas
            .iter()
            .position(|area| Some(*area) == focused)
            .or_else(|| {
                let bar = self.bar.buttons.iter().any(|(_, id)| Some(*id) == focused);
                let tools = self.tools.buttons.iter().any(|id| Some(*id) == focused);
                match (bar, tools) {
                    (true, _) => Some(1),
                    (_, true) => Some(areas.len().min(2)),
                    _ => None,
                }
            })
            .unwrap_or(0);
        let count = areas.len();
        let next = if forward {
            (current + 1) % count
        } else {
            (current + count - 1) % count
        };
        self.ui.note_keyboard();
        self.ui.set_focus(&mut self.fonts, Some(areas[next]));
    }

    /// Opens the context menu at `at`: the selection's commands when the
    /// pointer is on a node (selected first, as a click would), the
    /// board's otherwise (`shell.md` §3.8).
    fn context_menu(&mut self, at: Point) {
        let Some(board) = self.ui.widget_mut::<BoardCanvas>(self.board) else {
            return;
        };
        let node = board.node_at(at);
        let selected = board.selection().get_untracked();
        // Inside the selection's box the selection stays, as a click in
        // it would keep it for a drag.
        let inside = board
            .selection_on_screen()
            .is_some_and(|selection| selection.contains(at));
        match node {
            Some(node) if !selected.contains(&node) => board.select(&[node]),
            None if !selected.is_empty() && !inside => board.select(&[]),
            _ => {}
        }
        let Some(board) = self.ui.widget::<BoardCanvas>(self.board) else {
            return;
        };
        let keymap = board.keymap().clone();
        let (label, groups) = if board.selection().get_untracked().is_empty() {
            ("Board", crate::menu::BOARD)
        } else {
            ("Selection", crate::menu::SELECTION)
        };
        let entries = crate::menu::entries(groups, &self.board_commands, board, &keymap);
        let content = crate::menu::menu(&mut self.ui, label, entries, &self.asks);
        if let Some(popup) = self.popup.take() {
            self.ui.close_layer(&mut self.fonts, popup);
        }
        let options = LayerOptions::popover(Anchor::Point(at)).placement(Placement::Below);
        self.popup = Some(self.ui.open_layer(content, options));
    }

    /// Opens the popover of a contextual toolbar control.
    fn open_bar_popup(&mut self, control: Control) {
        let Some(anchor) = self.bar.button(control) else {
            return;
        };
        let Some(state) = self.bar.state.clone() else {
            return;
        };
        let Some(board) = self.ui.widget::<BoardCanvas>(self.board) else {
            return;
        };
        let keymap = board.keymap().clone();
        let entries = |groups| crate::menu::entries(groups, &self.board_commands, board, &keymap);
        let content = match control {
            Control::Fill | Control::Stroke | Control::TextColor => {
                let (title, command) = match control {
                    Control::Fill => ("Fill", "style.set_fill"),
                    Control::Stroke => ("Outline", "style.set_stroke"),
                    _ => ("Text color", "style.set_text_color"),
                };
                let current = self.bar.stored_color(command);
                contextbar::color_popover(
                    &mut self.ui,
                    title,
                    command,
                    current.as_deref(),
                    &self.asks,
                )
            }
            Control::Width => contextbar::value_popover(
                &mut self.ui,
                "Outline width",
                "style.set_stroke_width",
                "width",
                &contextbar::WIDTHS,
                state.width(),
                &self.asks,
            ),
            Control::Radius => contextbar::value_popover(
                &mut self.ui,
                "Corner radius",
                "style.set_radius",
                "radius",
                &contextbar::RADII,
                state.radius(),
                &self.asks,
            ),
            Control::Arrange => {
                let groups = entries(crate::menu::ARRANGE);
                crate::menu::menu(&mut self.ui, "Arrange", groups, &self.asks)
            }
            Control::More => {
                let groups = entries(crate::menu::SELECTION);
                crate::menu::menu(&mut self.ui, "More", groups, &self.asks)
            }
            Control::Ai => chrome::note(&mut self.ui, Placeholder::Ai),
            Control::Duplicate | Control::Delete => return,
        };
        self.open_popup(content, anchor, Placement::Below);
    }

    /// Opens the command palette, or puts the keys back in its field.
    fn open_palette(&mut self) {
        if let Some(palette) = &self.palette {
            let field = palette.field();
            self.ui.set_focus(&mut self.fonts, Some(field));
            return;
        }
        let Some(board) = self.ui.widget::<BoardCanvas>(self.board) else {
            return;
        };
        let board_keys = board.keymap().clone();
        let shell = ShellCx {
            in_app: self.app.is_some(),
            ..ShellCx::default()
        };
        let availability = self
            .board_commands
            .availability(board)
            .into_iter()
            .chain(self.commands.availability(&shell))
            .map(|(id, availability)| (id.as_str(), availability))
            .collect();
        let settings = self.settings.clone();
        let settings = settings.borrow();
        let sources = Sources {
            catalog: &self.catalog,
            settings: &settings,
            keymaps: [&board_keys, &self.keymap],
            usage: &self.usage,
            now: now(),
            asks: &self.asks,
        };
        let (width, height) = (self.size.width, self.size.height);
        self.palette = Some(Palette::open(
            &mut self.ui,
            width,
            height,
            availability,
            &sources,
        ));
    }

    /// Runs a palette row: a command closes the palette and runs; a
    /// setting changes in place. `tab`: only a setting's value changes,
    /// forward (or back).
    fn run_palette(&mut self, row: Option<usize>, tab: Option<bool>) {
        let Some(palette) = &self.palette else {
            return;
        };
        let Some(at) = palette.item(row) else {
            return;
        };
        let (kind, id) = self.catalog.item(at);
        match kind {
            Kind::Command if tab.is_none() => {
                if !palette.available(id) {
                    return;
                }
                // Closing it queues PaletteClosed, which disposes it.
                let layer = palette.layer;
                self.ui.close_layer(&mut self.fonts, layer);
                if self.execute(id, &Args::new()).is_ok() {
                    self.usage
                        .record(imaginarium_commands::CommandId::new(id), now());
                }
            }
            Kind::Command => {}
            Kind::Setting => {
                let settings = self.settings.clone();
                let mut settings = settings.borrow_mut();
                let forward = tab.unwrap_or(true);
                if let Some(value) = palette::step_value(&settings, id, forward) {
                    let _ = settings.set_value(id, value);
                }
                palette.show_value(at, &settings, &self.catalog);
            }
        }
    }

    fn drag_to(&mut self, at: Point) {
        let Some(drag) = &mut self.drag else {
            return;
        };
        if !drag.moved {
            if (at - drag.grab).hypot() < DRAG_THRESHOLD {
                return;
            }
            drag.moved = true;
        }
        let (corner, previous) = (drag.origin + (at - drag.grab), drag.target);
        let corner = self.clamp(corner);
        self.place_floating(corner);
        let target = toolbar::snap(at, self.size);
        if let Some(drag) = &mut self.drag {
            drag.target = target;
        }
        // The words only change with the target; the hint follows the bar.
        let first = !self
            .ui
            .widget::<DropZones>(self.zones)
            .is_some_and(|zones| zones.target.is_some());
        if first || previous != target {
            self.show_drop(Some(target), corner);
        } else {
            self.place_hint(corner);
        }
    }

    fn open_popup(&mut self, content: WidgetId, anchor: WidgetId, placement: Placement) {
        if let Some(popup) = self.popup.take() {
            self.ui.close_layer(&mut self.fonts, popup);
        }
        let options = LayerOptions::popover(Anchor::Widget(anchor)).placement(placement);
        self.popup = Some(self.ui.open_layer(content, options));
    }

    /// Moves the tool bar to `dock` and remembers it in the settings.
    fn move_bar(&mut self, dock: Dock) {
        self.put_bar(dock);
        self.remember(dock);
    }

    /// Shows the tool bar at `dock` (floating: where it last floated).
    fn put_bar(&mut self, dock: Dock) {
        if dock == Dock::Floating {
            self.place_floating(self.clamp(self.floating_at));
            return;
        }
        let vertical = dock.is_vertical().unwrap_or(self.vertical);
        self.vertical = vertical;
        self.dock = dock;
        self.orient(vertical, dock.tooltip_side());
        if let Some(bar) = self.ui.widget_mut::<Flex>(self.tools.bar) {
            let style = bar.style_mut();
            style.position = taffy::Position::Relative;
            style.inset = auto();
        }
        // Placed against its edge at the next frame, clear of the corners.
        self.placing = None;
        let placing = self.placing();
        self.apply_placing(placing);
    }

    /// Lets the tool bar float with its top left corner at `corner`.
    fn place_floating(&mut self, corner: Point) {
        self.floating_at = corner;
        self.dock = Dock::Floating;
        self.orient(self.vertical, Dock::Floating.tooltip_side());
        if let Some(bar) = self.ui.widget_mut::<Flex>(self.tools.bar) {
            let style = bar.style_mut();
            style.position = taffy::Position::Absolute;
            style.inset = taffy::Rect {
                left: length(corner.x as f32),
                top: length(corner.y as f32),
                right: taffy::LengthPercentageAuto::auto(),
                bottom: taffy::LengthPercentageAuto::auto(),
            };
        }
        self.placing = None;
    }

    /// Turns the bar, if it isn't turned so already.
    fn orient(&mut self, vertical: bool, side: Placement) {
        if self.oriented != Some((vertical, side)) {
            self.tools.orient(&mut self.ui, vertical, side);
            self.oriented = Some((vertical, side));
        }
    }

    /// Where a docked bar goes, from the corner panels and the bar's size
    /// in the last layout: centered on the window's edge when that keeps
    /// it clear of the corners (as the mockups place it), else centered in
    /// the space between them, else past them (a narrow or short window).
    fn placing(&self) -> Placing {
        let bounds = |id| self.ui.bounds(id).unwrap_or(Rect::ZERO);
        let [top_left, top_right, bottom_left, bottom_right] = self.chrome.corners.map(bounds);
        let bar = bounds(self.tools.bar).size();
        let Size { width, height } = self.size;
        let edge = 16.0;
        let fits = |start: f64, end: f64, length: f64, total: f64| -> Option<bool> {
            let centered = (total - length) / 2.0;
            if centered >= start && centered + length <= end {
                Some(true)
            } else if length <= end - start {
                Some(false)
            } else {
                None
            }
        };
        match self.dock {
            Dock::Left | Dock::Right => {
                // A bar docked right sits left of an open inspector.
                let right_edge = if self.dock == Dock::Right && self.inspector.open {
                    edge + 300.0 + CLEARANCE
                } else {
                    edge
                };
                let (top, bottom) = if self.dock == Dock::Left {
                    (top_left, bottom_left)
                } else {
                    (top_right, bottom_right)
                };
                let (start, end) = (top.y1 + CLEARANCE, bottom.y0 - CLEARANCE);
                match fits(start, end, bar.height, height) {
                    Some(true) => Placing {
                        padding: [edge, right_edge, edge, edge],
                        centered: true,
                    },
                    Some(false) => Placing {
                        padding: [edge, right_edge, start, height - end],
                        centered: true,
                    },
                    // Too tall: from just below the corner, so its grip
                    // stays in reach.
                    None => Placing {
                        padding: [
                            edge,
                            right_edge,
                            start.min(height - bar.height).max(0.0),
                            0.0,
                        ],
                        centered: false,
                    },
                }
            }
            Dock::Top | Dock::Bottom => {
                let (left, right) = if self.dock == Dock::Top {
                    (top_left, top_right)
                } else {
                    (bottom_left, bottom_right)
                };
                let (start, end) = (left.x1 + CLEARANCE, right.x0 - CLEARANCE);
                match fits(start, end, bar.width, width) {
                    Some(true) => Placing {
                        padding: [edge; 4],
                        centered: true,
                    },
                    Some(false) => Placing {
                        padding: [start, width - end, edge, edge],
                        centered: true,
                    },
                    // Too wide: in a row of its own, past the corners.
                    None => {
                        let past = if self.dock == Dock::Top {
                            [left.y1, right.y1].into_iter().fold(0.0, f64::max) + CLEARANCE
                        } else {
                            height - [left.y0, right.y0].into_iter().fold(height, f64::min)
                                + CLEARANCE
                        };
                        let (top, bottom) = if self.dock == Dock::Top {
                            (past, edge)
                        } else {
                            (edge, past)
                        };
                        Placing {
                            padding: [edge, edge, top, bottom],
                            centered: true,
                        }
                    }
                }
            }
            Dock::Floating => Placing {
                padding: [0.0; 4],
                centered: false,
            },
        }
    }

    fn apply_placing(&mut self, placing: Placing) {
        let dock = self.dock;
        if let Some(stage) = self.ui.widget_mut::<Flex>(self.stage) {
            let style = stage.style_mut();
            let [left, right, top, bottom] = placing.padding.map(|side| length(side as f32));
            style.padding = taffy::Rect {
                left,
                right,
                top,
                bottom,
            };
            let along = if placing.centered {
                taffy::AlignItems::CENTER
            } else {
                taffy::AlignItems::FLEX_START
            };
            match dock {
                Dock::Left | Dock::Right => {
                    style.flex_direction = taffy::FlexDirection::Row;
                    style.justify_content = if dock == Dock::Left {
                        taffy::JustifyContent::FLEX_START
                    } else {
                        taffy::JustifyContent::FLEX_END
                    };
                    style.align_items = along;
                }
                Dock::Top | Dock::Bottom | Dock::Floating => {
                    style.flex_direction = taffy::FlexDirection::Column;
                    style.justify_content = if dock == Dock::Bottom {
                        taffy::JustifyContent::FLEX_END
                    } else {
                        taffy::JustifyContent::FLEX_START
                    };
                    style.align_items = along;
                }
            }
        }
        self.placing = Some(placing);
    }

    /// Keeps a floating bar's corner inside the window.
    fn clamp(&self, corner: Point) -> Point {
        let bar = self
            .ui
            .bounds(self.tools.bar)
            .map_or(Size::ZERO, |bounds| bounds.size());
        Point::new(
            corner.x.clamp(0.0, (self.size.width - bar.width).max(0.0)),
            corner
                .y
                .clamp(0.0, (self.size.height - bar.height).max(0.0)),
        )
    }

    fn remember(&mut self, dock: Dock) {
        // The setting is registered by whoever made the registry; if it
        // isn't, the bar still moves, it just isn't remembered.
        let _ = self.settings.borrow_mut().set(&TOOLBAR_POSITION, dock);
        // Our own change: nothing to follow at the next frame.
        self.settings_changed.store(false, Ordering::Relaxed);
    }

    /// Shows the drop zones and the hint while dragging (`target`: where
    /// the bar would go), or hides them (`None`).
    fn show_drop(&mut self, target: Option<Option<Dock>>, corner: Point) {
        if let Some(zones) = self.ui.widget_mut::<DropZones>(self.zones) {
            zones.target = target;
        }
        let (hint, text) = self.hint;
        let words = match target {
            Some(Some(edge)) => {
                format!(
                    "Release to dock it {} · Shift keeps it floating",
                    edge.words()
                )
            }
            Some(None) => "Release to leave it here · near an edge it docks".into(),
            None => String::new(),
        };
        if let Some(label) = self.ui.widget_mut::<Label>(text) {
            label.set_text(words);
        }
        if let Some(hint) = self.ui.widget_mut::<Flex>(hint) {
            hint.style_mut().display = if target.is_some() {
                taffy::Display::Flex
            } else {
                taffy::Display::None
            };
        }
        if target.is_some() {
            self.place_hint(corner);
        }
    }

    /// Puts the hint under the bar with its corner at `corner`, or above
    /// it when the window ends first; never past the right edge (the hint
    /// is about 360 px wide).
    fn place_hint(&mut self, corner: Point) {
        const HINT_HEIGHT: f64 = 30.0;
        let bar = self
            .ui
            .bounds(self.tools.bar)
            .map_or(0.0, |bounds| bounds.height());
        let y = if corner.y + bar + 8.0 + HINT_HEIGHT <= self.size.height {
            corner.y + bar + 8.0
        } else {
            (corner.y - 8.0 - HINT_HEIGHT).max(0.0)
        };
        let at = Point::new(corner.x.min((self.size.width - 360.0).max(0.0)), y);
        if let Some(hint) = self.ui.widget_mut::<Flex>(self.hint.0) {
            hint.style_mut().inset = taffy::Rect {
                left: length(at.x as f32),
                top: length(at.y as f32),
                right: taffy::LengthPercentageAuto::auto(),
                bottom: taffy::LengthPercentageAuto::auto(),
            };
        }
    }
}

impl Drop for BoardWindow {
    fn drop(&mut self) {
        self.finish_saving();
        // The registry outlives the window (several windows share it).
        if let Ok(mut settings) = self.settings.try_borrow_mut() {
            settings.unsubscribe(self.subscription);
        }
        if let Some(app) = &self.app
            && !self.closed
        {
            app.board_closed();
        }
        // Settings may show this board (This board): it notices it's gone.
        if self.app.is_some() {
            imaginarium_ui::request_frame_everywhere();
        }
    }
}

/// `typed` (`#rrggbb` or `#rrggbbaa`) with the alpha of `stored` when only
/// six digits were typed.
fn keep_alpha(typed: &str, stored: Option<&str>) -> String {
    match stored.filter(|stored| stored.len() == 9 && typed.len() == 7) {
        Some(stored) => format!("{typed}{}", &stored[7..]),
        None => typed.to_owned(),
    }
}

/// Now, for the palette's frecency (seconds).
fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

fn full() -> taffy::Rect<taffy::LengthPercentageAuto> {
    taffy::Rect {
        left: length(0.0),
        right: length(0.0),
        top: length(0.0),
        bottom: length(0.0),
    }
}

fn auto() -> taffy::Rect<taffy::LengthPercentageAuto> {
    taffy::Rect {
        left: taffy::LengthPercentageAuto::auto(),
        right: taffy::LengthPercentageAuto::auto(),
        top: taffy::LengthPercentageAuto::auto(),
        bottom: taffy::LengthPercentageAuto::auto(),
    }
}

impl View for BoardWindow {
    fn paint(&mut self, scene: &mut Scene, size: Size) {
        self.draw(scene, size);
    }

    fn input(&mut self, input: &Input) -> bool {
        // The board Settings' This board edits: the one with the keys, or
        // clicked last (without focus following the pointer).
        if matches!(
            input,
            Input::WindowFocus(true) | Input::PointerButton { pressed: true, .. }
        ) && let Some(app) = &self.app
        {
            app.board_active(&self.link);
        }
        // The Menu key opens the context menu, as Shift+F10 does.
        if let Input::Key {
            key: Key::Other(name),
            pressed: true,
            ..
        } = input
            && name == "ContextMenu"
        {
            self.ui.note_keyboard();
            self.asks.borrow_mut().push(Ask::ContextMenu);
            self.handle_asks();
            return true;
        }
        // The shell's own shortcuts (Ctrl+K) work wherever the keys are.
        if let Input::Key {
            key,
            pressed: true,
            modifiers,
            ..
        } = input
            && let Some(press) = imaginarium_canvas::keys::key_press(key, *modifiers)
            && let Some(command) = self.keymap.command_for_press(press)
        {
            self.ui.note_keyboard();
            // A held Ctrl+N or Ctrl+W would open or close a window per
            // repeat.
            let once = matches!(command.as_str(), "file.new_board" | "window.close");
            if !(once && matches!(input, Input::Key { repeat: true, .. })) {
                let _ = self.execute(command.as_str(), &Args::new());
            }
            return true;
        }
        // A right-click on the board: the context menu of what it is on
        // (the node under it, selected first, or the board).
        if let Input::PointerButton {
            primary: false,
            pressed: true,
            position,
            ..
        } = input
            && self.palette.is_none()
            && self.ui.widget_at(*position) == Some(self.board)
        {
            self.context_menu(*position);
            return true;
        }
        // A click beside the palette closes it, as Esc does.
        if let Input::PointerButton {
            pressed: true,
            position,
            ..
        } = input
            && let Some(palette) = &self.palette
            && self
                .ui
                .bounds(palette.panel)
                .is_some_and(|panel| !panel.contains(*position))
        {
            let layer = palette.layer;
            self.ui.close_layer(&mut self.fonts, layer);
            self.handle_asks();
            return true;
        }
        let mut handled = self.ui.input(&mut self.fonts, input);
        self.handle_asks();
        // A panel control with the keys (reached with Tab) has no use for
        // most of them: Esc goes back to the board (shell.md §6), and
        // other keys are the board's shortcuts, as if it had them.
        if !handled
            && let Input::Key {
                key, pressed: true, ..
            } = input
            && self.ui.focused() != Some(self.board)
        {
            self.ui.set_focus(&mut self.fonts, Some(self.board));
            handled = *key == Key::Escape || self.ui.input(&mut self.fonts, input);
            self.handle_asks();
        }
        handled
    }

    fn presented(&mut self, frame: &FrameInfo) -> bool {
        self.frames = frame.number;
        false
    }

    fn should_close(&mut self) -> bool {
        self.close_after.is_some_and(|frames| self.frames >= frames)
    }

    fn close_requested(&mut self) -> bool {
        self.closing();
        true
    }

    fn take_requests(&mut self) -> Vec<WindowRequest> {
        std::mem::take(&mut self.requests)
    }

    fn wake_at(&mut self) -> Option<Instant> {
        [
            self.ui.wake_at(),
            self.saving_deadline(),
            self.save_deadline(),
        ]
        .into_iter()
        .flatten()
        .min()
    }

    fn wake(&mut self, now: Instant) -> bool {
        self.ui.wake(&mut self.fonts, now);
        // Changes due for saving, or Saving… due in the pill.
        let due = |at: Option<Instant>| at.is_some_and(|at| at <= now);
        if due(self.save_deadline()) {
            self.save_changes(false);
        }
        if due(self.saving_deadline()) {
            self.update_save_state();
            return true;
        }
        false
    }

    fn ime_area(&mut self) -> Option<Rect> {
        self.ui.ime_area(&mut self.fonts)
    }

    fn accessibility(&mut self, tree: &mut AccessTree) {
        self.ui.access(&mut self.fonts, tree);
    }

    fn accessibility_action(&mut self, request: &accesskit::ActionRequest) -> bool {
        let handled = self.ui.access_action(&mut self.fonts, request);
        self.handle_asks();
        handled
    }
}

/// The pill's words for why a board is read-only.
fn read_only_text(reason: &imaginarium_model::ReadOnlyReason) -> String {
    use imaginarium_model::ReadOnlyReason;
    match reason {
        // Short, for the pill (the session says it as a sentence).
        ReadOnlyReason::File(reason) if reason.contains("another window") => {
            "open in another window".to_owned()
        }
        ReadOnlyReason::File(reason) if reason.contains("can't be written") => {
            "the file can't be written".to_owned()
        }
        ReadOnlyReason::File(reason)
            if reason.contains("newer") || reason.contains("lacks") || reason.contains("know") =>
        {
            "made with a newer version".to_owned()
        }
        ReadOnlyReason::File(reason) => reason.clone(),
        ReadOnlyReason::MissingFeatures(_) | ReadOnlyReason::NewerSchema(_) => {
            "made with a newer version".to_owned()
        }
        ReadOnlyReason::NoMigration(_) | ReadOnlyReason::MigrationFailed { .. } => {
            "it couldn't be brought up to date".to_owned()
        }
    }
}
