// SPDX-License-Identifier: AGPL-3.0-or-later

//! The start window (`shell.md` §4, the Start mockups): the app's name and
//! version, New board and Open… on the left, recent boards on the right.
//!
//! After a crash, a notice on the right offers the boards that were kept
//! (`App::look_for_kept_boards`). Open… asks for a board file on a thread
//! of its own (`crate::files`). Recent boards show as tiles with a
//! picture of each (`crate::recent`), with a filter, then Open another
//! board…; a board file dropped on the window opens. The "Start
//! from" templates come with M4's shape libraries, the Settings, Shortcuts
//! and What's new links with the windows they open.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Instant;

use accesskit::Role;
use imaginarium_commands::{Keymap, Platform};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use imaginarium_reactive::{Signal, signal};
use imaginarium_render::Scene;
use imaginarium_render::kurbo::{Rect, Size};
use imaginarium_text::{Fonts, TextStyle};
use imaginarium_ui::{
    AccessTree, ButtonBox, ButtonColors, Flex, FrameInfo, Icon, Input, InputColors, Label, Logo,
    Switch, TextInput, Token, Ui, View, WidgetId, WindowOptions, WindowRequest, theme,
};
use taffy::prelude::length;

use crate::app::{App, SHOW_START_WINDOW};
use crate::toolbar::shortcut;

/// What the start window's controls asked for.
#[derive(Clone, Debug, PartialEq)]
enum StartAsk {
    NewBoard,
    /// Choose a board file and open it.
    Open,
    /// Open this recent board.
    OpenRecent(std::path::PathBuf),
    /// The recent boards' filter changed.
    Filter(String),
    /// Open the boards kept after a crash.
    OpenKept,
    /// Open Settings at this section.
    Settings(crate::settings::Section),
}

type StartAsks = Rc<RefCell<Vec<StartAsk>>>;

/// The start window. Run it in the app's event loop with
/// [`StartWindow::options`]; New board opens a board window and closes it.
pub struct StartWindow {
    app: Rc<App>,
    fonts: Fonts,
    ui: Ui,
    asks: StartAsks,
    keymap: Keymap,
    new_board: WidgetId,
    requests: Vec<WindowRequest>,
    /// Show at startup, as the switch shows it.
    shown: Signal<bool>,
    startup_changed: Arc<AtomicBool>,
    subscription: imaginarium_settings::SubscriptionId,
    /// It asked to close (after New board).
    closing: bool,
    /// The right side, and its Recent header (the title and the filter).
    right: WidgetId,
    header: WidgetId,
    title: WidgetId,
    filter_field: WidgetId,
    /// The recent boards' rows (or their empty tile), and the drop hint.
    list: Vec<WidgetId>,
    hint: Option<WidgetId>,
    /// What the filter says.
    filter: String,
    /// The recent boards' change number the list was built for.
    recent_seen: Option<u64>,
    /// It asked to be woken when the recent boards change.
    waiting_for_recent: bool,
    /// The crash notice, while it shows.
    notice: Option<WidgetId>,
    /// It asked to be woken when the search for kept boards is done.
    waiting_for_kept: bool,
    /// The open dialog, then the file, while they run.
    opening: Option<crate::files::Pending<Option<crate::files::OpenedFile>>>,
    /// Why the last file couldn't be opened, while it shows.
    error: Option<(WidgetId, String)>,
    close_after: Option<u64>,
    frames: u64,
}

impl StartWindow {
    /// The start window of `app`.
    pub fn new(app: Rc<App>) -> Self {
        let fonts = app.fonts();
        let mut ui = Ui::new();
        let asks: StartAsks = Rc::default();
        let mut keymap = Keymap::new(Platform::current());
        // A fixed, valid binding: this can't fail.
        let _ = keymap.bind(
            "Mod+N",
            imaginarium_commands::CommandId::new("file.new_board"),
        );
        let _ = keymap.bind(
            "Mod+,",
            imaginarium_commands::CommandId::new("settings.open"),
        );
        let _ = keymap.bind("Mod+O", imaginarium_commands::CommandId::new("file.open"));
        let shown = signal(app.shows_start_window());
        let (side, new_board) = side(&mut ui, &app, &keymap, &asks, shown);
        // Show at startup changed elsewhere (Settings): the switch follows.
        let startup_changed = Arc::new(AtomicBool::new(false));
        let flag = startup_changed.clone();
        let subscription = app.settings().borrow_mut().subscribe(move |change| {
            if change.key == SHOW_START_WINDOW.key() {
                flag.store(true, Ordering::Relaxed);
                imaginarium_ui::request_frame_everywhere();
            }
        });
        let divider = ui.add(Flex::column().width(1.0).background(Token::Divider));
        let (right, header, title, filter_field) = recent(&mut ui, &asks);
        let root = ui.add_with_children(
            Flex::row()
                .fill()
                .background(Token::BoardBg)
                .style(|style| style.align_items = taffy::AlignItems::STRETCH),
            &[side, divider, right],
        );
        ui.set_root(root);
        // Nothing has the keys at first, so no focus ring shows (the
        // mockup): Ctrl+N works anywhere, Tab reaches New board first.
        Self {
            app,
            fonts,
            ui,
            asks,
            keymap,
            new_board,
            requests: Vec::new(),
            shown,
            startup_changed,
            subscription,
            closing: false,
            right,
            header,
            title,
            filter_field,
            list: Vec::new(),
            hint: None,
            filter: String::new(),
            recent_seen: None,
            waiting_for_recent: false,
            notice: None,
            waiting_for_kept: false,
            opening: None,
            error: None,
            close_after: None,
            frames: 0,
        }
    }

    /// Shows the crash notice while there are kept boards to open, and
    /// asks to be woken when the search for them is done.
    fn sync_notice(&mut self) {
        if !self.waiting_for_kept
            && let Some(wake) = imaginarium_ui::window_waker()
        {
            self.app.when_kept_boards_found(Box::new(wake));
            self.waiting_for_kept = true;
        }
        let kept = self.app.kept_boards();
        if kept.is_some() == self.notice.is_some() {
            return;
        }
        if let Some(notice) = self.notice.take() {
            self.ui.remove(notice);
        }
        if let Some((count, name)) = kept {
            self.notice = Some(notice(&mut self.ui, count, &name, &self.asks));
        }
        self.lay_out_right();
    }

    /// The right side: an error, the crash notice, then Recent and the
    /// drop hint.
    fn lay_out_right(&mut self) {
        let children: Vec<WidgetId> = self
            .error
            .as_ref()
            .map(|(error, _)| *error)
            .into_iter()
            .chain(self.notice)
            .chain([self.header])
            .chain(self.list.iter().copied())
            .chain(self.hint)
            .collect();
        self.ui.set_children(self.right, &children);
    }

    /// Whether a board is being chosen or opened.
    #[cfg(test)]
    pub(crate) fn is_opening(&self) -> bool {
        self.opening.is_some()
    }

    /// Builds the recent boards' tiles again when they or the filter
    /// changed.
    fn sync_recent(&mut self, force: bool) {
        if !self.waiting_for_recent
            && let Some(wake) = imaginarium_ui::window_waker()
        {
            self.app.when_recent_changes(Box::new(wake));
            self.waiting_for_recent = true;
        }
        let (generation, boards) = self.app.recent_boards();
        if !force && self.recent_seen == Some(generation) {
            return;
        }
        self.recent_seen = Some(generation);
        for widget in std::mem::take(&mut self.list) {
            self.ui.remove(widget);
        }
        if let Some(hint) = self.hint.take() {
            self.ui.remove(hint);
        }
        // The filter only once there is something to filter.
        let header: Vec<WidgetId> = if boards.is_empty() {
            vec![self.title]
        } else {
            vec![self.title, self.filter_field]
        };
        self.ui.set_children(self.header, &header);
        let query = self.filter.trim().to_lowercase();
        let home = home_folder();
        let now = crate::recent::now_ms();
        let shown: Vec<_> = boards
            .iter()
            .map(|board| {
                let name = crate::files::board_name(&board.path);
                let folder = crate::recent::folder_text(&board.path, home.as_deref());
                (board, name, folder)
            })
            .filter(|(_, name, folder)| {
                query.is_empty()
                    || name.to_lowercase().contains(&query)
                    || folder.to_lowercase().contains(&query)
            })
            .take(RECENT_SHOWN)
            .collect();
        self.list = if boards.is_empty() {
            vec![empty_tile(&mut self.ui, "The boards you open show here.")]
        } else {
            let mut tiles: Vec<WidgetId> = shown
                .iter()
                .map(|(board, name, folder)| {
                    let when = crate::recent::when_text(board.at_ms, now);
                    recent_tile(
                        &mut self.ui,
                        board,
                        name,
                        &format!("{folder} · {when}"),
                        &self.asks,
                    )
                })
                .collect();
            if shown.is_empty() {
                let text = format!(
                    "No recent board matches \u{201c}{}\u{201d}.",
                    self.filter.trim()
                );
                tiles.push(empty_tile(&mut self.ui, &text));
            }
            tiles.push(open_tile(&mut self.ui, &self.asks));
            rows(&mut self.ui, tiles)
        };
        if imaginarium_ui::file_drops_supported() {
            let hint = self.ui.add(
                Label::new("Drop an .imag file here to open it.", TextStyle::new(12.0))
                    .color(Token::TextSubtle)
                    .single_line(),
            );
            self.hint = Some(self.ui.add_with_children(
                Flex::row().style(|style| style.margin.top = taffy::LengthPercentageAuto::auto()),
                &[hint],
            ));
        }
        self.lay_out_right();
    }

    /// Opens the recent board at `path` (on a thread of its own), or says
    /// why it can't be (and forgets it if its file is gone).
    fn open_recent(&mut self, path: std::path::PathBuf) {
        if self.closing || self.opening.is_some() {
            return;
        }
        if !path.exists() {
            self.app.forget(&path);
        }
        self.opening = Some(crate::files::Pending::spawn(
            "open-board",
            move || Some(crate::files::open_file(path)),
            || Some(Err("no thread to open with".into())),
        ));
    }

    /// Asks for a board file to open, on a thread of its own (once at a
    /// time).
    pub fn open_file(&mut self) {
        if self.closing || self.opening.is_some() {
            return;
        }
        let dialogs = self.app.dialogs();
        self.opening = Some(crate::files::Pending::spawn(
            "open-dialog",
            move || dialogs.open_board().map(crate::files::open_file),
            || Some(Err("no thread to open with".into())),
        ));
    }

    /// Opens the board the dialog chose, and closes this window; or says
    /// why it can't be opened.
    fn poll_opening(&mut self) {
        let Some(result) = self.opening.as_ref().and_then(crate::files::Pending::take) else {
            return;
        };
        self.opening = None;
        if let Some((error, _)) = self.error.take() {
            self.ui.remove(error);
        }
        match result {
            None => {}
            Some(Ok((path, opening))) => {
                let window = self.app.opened_board(&path, opening);
                self.requests
                    .push(WindowRequest::Open(window.options(), Box::new(window)));
                self.requests.push(WindowRequest::Close);
                self.closing = true;
            }
            Some(Err(reason)) => {
                let error = error_notice(&mut self.ui, &reason);
                self.error = Some((error, reason));
            }
        }
        self.lay_out_right();
    }

    /// Opens the boards kept after a crash, each in its window. The start
    /// window stays (for the other boards and New board).
    pub fn open_kept_boards(&mut self) {
        for window in self.app.open_kept_boards() {
            self.requests
                .push(WindowRequest::Open(window.options(), Box::new(window)));
        }
        self.sync_notice();
    }

    /// Closes the window after `frames` frames (for scripted runs).
    pub fn close_after(mut self, frames: u64) -> Self {
        self.close_after = Some(frames);
        self
    }

    /// The window's options: titled `Imaginarium`, 960 × 600 and not
    /// resizable, so tiling compositors float it like Blender's splash.
    pub fn options(&self) -> WindowOptions {
        WindowOptions {
            title: "Imaginarium".into(),
            size: Size::new(960.0, 600.0),
            background: theme().color(Token::BoardBg),
            app_id: Some("ch.giosuel.Imaginarium".into()),
            resizable: false,
            more_windows: true,
            ..WindowOptions::default()
        }
    }

    /// The window's widgets, for tests.
    pub fn ui(&mut self) -> (&mut Ui, &mut Fonts) {
        (&mut self.ui, &mut self.fonts)
    }

    /// Opens a board window on a new board and closes this one (once: a
    /// second press before it closes does nothing).
    pub fn new_board(&mut self) {
        if self.closing {
            return;
        }
        if let Ok(window) = self.app.new_board() {
            self.requests
                .push(WindowRequest::Open(window.options(), Box::new(window)));
            self.requests.push(WindowRequest::Close);
            self.closing = true;
        }
    }

    /// Opens the Settings window at `section`, unless it is open.
    fn open_settings(&mut self, section: crate::settings::Section) {
        if let Some(mut window) = self.app.settings_window() {
            window.show(section);
            self.requests
                .push(WindowRequest::Open(window.options(), Box::new(window)));
        }
    }

    /// The New board button, for tests.
    pub fn new_board_button(&self) -> WidgetId {
        self.new_board
    }

    fn handle_asks(&mut self) -> bool {
        if self.startup_changed.swap(false, Ordering::Relaxed) {
            self.shown.set(self.app.shows_start_window());
        }
        let asks = std::mem::take(&mut *self.asks.borrow_mut());
        let asked = !asks.is_empty();
        for ask in asks {
            match ask {
                StartAsk::NewBoard => self.new_board(),
                StartAsk::Open => self.open_file(),
                StartAsk::OpenRecent(path) => self.open_recent(path),
                StartAsk::Filter(text) => {
                    self.filter = text;
                    self.sync_recent(true);
                }
                StartAsk::OpenKept => self.open_kept_boards(),
                StartAsk::Settings(section) => self.open_settings(section),
            }
        }
        asked
    }
}

/// Padding on each side: left, right, top, bottom.
fn padding(left: f32, right: f32, top: f32, bottom: f32) -> taffy::Rect<taffy::LengthPercentage> {
    taffy::Rect {
        left: length(left),
        right: length(right),
        top: length(top),
        bottom: length(bottom),
    }
}

/// The left side: the name, New board and Open…, Show at startup.
fn side(
    ui: &mut Ui,
    app: &App,
    keymap: &Keymap,
    asks: &StartAsks,
    shown: Signal<bool>,
) -> (WidgetId, WidgetId) {
    let logo = ui.add(Logo::new(44.0));
    let name = ui.add(
        Label::new("Imaginarium", TextStyle::new(20.0).weight(700.0))
            .color(Token::Text)
            .single_line(),
    );
    let version = ui.add(
        Label::new(
            format!("Version {} · preview", app.version()),
            TextStyle::new(12.0),
        )
        .color(Token::TextSubtle)
        .single_line(),
    );
    let titles = ui.add_with_children(Flex::column().gap(2.0), &[name, version]);
    let brand = ui.add_with_children(Flex::row().gap(12.0).align_center(), &[logo, titles]);

    let keys = shortcut(keymap, "file.new_board");
    let asks_new = asks.clone();
    let new_board = big_button(
        ui,
        "plus",
        "New board",
        keys,
        ButtonColors::primary(),
        Box::new(move || {
            asks_new.borrow_mut().push(StartAsk::NewBoard);
        }),
    );
    let asks_open = asks.clone();
    let open = big_button(
        ui,
        "folder-open",
        "Open…",
        shortcut(keymap, "file.open"),
        ButtonColors::secondary(),
        Box::new(move || {
            asks_open.borrow_mut().push(StartAsk::Open);
        }),
    );
    let buttons = ui.add_with_children(Flex::column().gap(6.0), &[new_board, open]);

    let settings = app.settings().clone();
    let switch = Switch::new(
        "Show at startup",
        move || shown.get(),
        move || {
            let on = !shown.get();
            shown.set(on);
            // A registered bool: this can't fail.
            let _ = settings.borrow_mut().set(&SHOW_START_WINDOW, on);
        },
    )
    .add_to(ui);
    let switch_label = ui.add(
        Label::new("Show at startup", TextStyle::new(13.0))
            .color(Token::TextSecondary)
            .single_line(),
    );
    let startup =
        ui.add_with_children(Flex::row().gap(8.0).align_center(), &[switch, switch_label]);
    // The mockup's links; What's new comes with the release notes.
    let links = [
        ("Settings", crate::settings::Section::General),
        ("Shortcuts", crate::settings::Section::Shortcuts),
    ]
    .map(|(title, section)| link(ui, title, asks, StartAsk::Settings(section)));
    let links = ui.add_with_children(Flex::row().gap(14.0), &links);
    let footer = ui.add_with_children(
        Flex::column().gap(10.0).style(|style| {
            style.margin.top = taffy::LengthPercentageAuto::auto();
        }),
        &[startup, links],
    );

    let side = ui.add_with_children(
        Flex::column()
            .width(287.0)
            .gap(22.0)
            .background(Token::PanelBg)
            .style(|style| {
                style.padding = padding(28.0, 24.0, 28.0, 20.0);
                style.flex_shrink = 0.0;
            }),
        &[brand, buttons, footer],
    );
    (side, new_board)
}

/// A link: accent text that runs `ask`.
fn link(ui: &mut Ui, title: &str, asks: &StartAsks, ask: StartAsk) -> WidgetId {
    let text = ui.add(
        Label::new(title, TextStyle::new(13.0))
            .color(Token::AccentInk)
            .single_line(),
    );
    let asks = asks.clone();
    ButtonBox::new(title, ButtonColors::quiet(), move || {
        asks.borrow_mut().push(ask.clone());
    })
    .height(22.0)
    .padding(2.0, 2.0)
    .radius(4.0)
    .role(Role::Link)
    .add_to(ui, &[text])
}

/// A 40 px button with an icon, a title and its shortcut.
fn big_button(
    ui: &mut Ui,
    icon: &str,
    title: &str,
    keys: Option<String>,
    colors: ButtonColors,
    action: Box<dyn FnMut()>,
) -> WidgetId {
    let primary = colors.border.is_none();
    let (ink, icon_ink, keys_ink) = if primary {
        (Token::OnAccent, Token::OnAccent, Token::OnAccent)
    } else {
        (Token::Text, Token::TextMuted, Token::TextSubtle)
    };
    let icon = ui.add(Icon::named(icon, 17.0, icon_ink));
    let weight = if primary { 600.0 } else { 500.0 };
    let label = ui.add(
        Label::new(title, TextStyle::new(14.0).weight(weight))
            .color(ink)
            .single_line()
            .grow(1.0),
    );
    let mut children = vec![icon, label];
    if let Some(keys) = &keys {
        children.push(
            ui.add(
                Label::new(keys.clone(), TextStyle::new(11.0).weight(500.0))
                    .color(keys_ink)
                    .single_line(),
            ),
        );
    }
    ButtonBox::new(title, colors, action)
        .height(40.0)
        .padding(12.0, 12.0)
        .gap(10.0)
        .start()
        .radius(10.0)
        .add_to(ui, &children)
}

/// Why a board couldn't be opened, above the rest of the right side.
fn error_notice(ui: &mut Ui, text: &str) -> WidgetId {
    let icon = ui.add(Icon::named("triangle-alert", 16.0, Token::OnWarningSoft));
    let label = ui.add(
        Label::new(text, TextStyle::new(13.0))
            .color(Token::OnWarningSoft)
            .grow(1.0),
    );
    ui.add_with_children(
        Flex::row()
            .gap(10.0)
            .align_center()
            .padding(12.0, 10.0)
            .radius(10.0)
            .background(Token::WarningSoft)
            .role(Role::Alert, text),
        &[icon, label],
    )
}

/// The notice after a crash (the Start mockup): which boards were kept,
/// and a button that opens them.
fn notice(ui: &mut Ui, count: usize, name: &str, asks: &StartAsks) -> WidgetId {
    let icon = ui.add(Icon::named("rotate-ccw", 16.0, Token::OnNotice));
    let lead = "Imaginarium closed unexpectedly. ";
    let (text, bold, button) = if count == 1 {
        (
            format!("{lead}{name} was kept, with every change."),
            lead.len()..lead.len() + name.len(),
            "Open it",
        )
    } else {
        let boards = format!("{count} boards");
        (
            format!("{lead}{boards} were kept, with every change."),
            lead.len()..lead.len() + boards.len(),
            "Open them",
        )
    };
    let label = ui.add(
        Label::new(text.clone(), TextStyle::new(13.0))
            .color(Token::OnNotice)
            .highlight(vec![bold], 700.0, Token::OnNotice)
            .grow(1.0),
    );
    let caption = ui.add(
        Label::new(button, TextStyle::new(12.0).weight(600.0))
            .color(Token::OnNotice)
            .single_line(),
    );
    let asks = asks.clone();
    let colors = ButtonColors {
        background: Token::PanelBg.into(),
        text: Token::OnNotice.into(),
        border: Some(Token::NoticeControlBorder.into()),
        ..ButtonColors::secondary()
    };
    let open = ButtonBox::new(button, colors, move || {
        asks.borrow_mut().push(StartAsk::OpenKept);
    })
    .height(28.0)
    .padding(10.0, 10.0)
    .radius(7.0)
    .add_to(ui, &[caption]);
    ui.add_with_children(
        Flex::row()
            .gap(10.0)
            .align_center()
            .padding(12.0, 10.0)
            .radius(10.0)
            .background(Token::Notice)
            .border(1.0, Token::NoticeBorder)
            .role(Role::Alert, text),
        &[icon, label, open],
    )
}

/// Recent boards shown at most: two rows of three with Open another
/// board… (the Start mockup); the filter finds the others.
const RECENT_SHOWN: usize = 5;

/// The right side: the Recent header with its filter; the tiles come
/// with [`StartWindow::sync_recent`]. Returns the side, the header, its
/// title and the filter.
fn recent(ui: &mut Ui, asks: &StartAsks) -> (WidgetId, WidgetId, WidgetId, WidgetId) {
    let title = ui.add(
        Label::new("Recent", TextStyle::new(15.0).weight(600.0))
            .color(Token::Text)
            .single_line()
            .grow(1.0),
    );
    let icon = ui.add(Icon::named("search", 14.0, Token::TextSubtle));
    let input = {
        let asks = asks.clone();
        ui.add(
            TextInput::new(
                "Filter recent boards",
                "Filter",
                TextStyle::new(13.0),
                InputColors::themed(),
            )
            .bare()
            .on_change(move |text| asks.borrow_mut().push(StartAsk::Filter(text.to_owned()))),
        )
    };
    let field = ui.add_with_children(Flex::row().grow(1.0), &[input]);
    let filter = ui.add_with_children(
        Flex::row()
            .width(200.0)
            .height(30.0)
            .gap(6.0)
            .align_center()
            .padding(10.0, 0.0)
            .border(1.0, Token::ControlBorder)
            .radius(8.0)
            .background(Token::PanelBg),
        &[icon, field],
    );
    let header = ui.add_with_children(Flex::row().gap(12.0).align_center(), &[title]);
    let side = ui.add_with_children(
        Flex::column().gap(16.0).grow(1.0).style(|style| {
            style.padding = padding(28.0, 28.0, 24.0, 20.0);
            style.min_size.width = length(0.0);
        }),
        &[header],
    );
    (side, header, title, filter)
}

/// A tile saying there is nothing to show (no recent boards yet, or none
/// matching the filter).
fn empty_tile(ui: &mut Ui, text: &str) -> WidgetId {
    let icon = ui.add(Icon::named("folder-open", 20.0, Token::TextMuted));
    let label = ui.add(
        Label::new(text, TextStyle::new(13.0))
            .color(Token::TextMuted)
            .single_line(),
    );
    ui.add_with_children(
        Flex::column()
            .height(112.0)
            .gap(8.0)
            .align_center()
            .border(1.0, Token::ControlBorder)
            .radius(10.0)
            .style(|style| {
                style.justify_content = taffy::JustifyContent::CENTER;
            })
            .role(Role::Group, "Recent boards"),
        &[icon, label],
    )
}

/// A recent board: its picture, its name, its folder and when.
fn recent_tile(
    ui: &mut Ui,
    board: &crate::recent::RecentBoard,
    name: &str,
    detail: &str,
    asks: &StartAsks,
) -> WidgetId {
    let picture = ui.add(crate::recent::PreviewTile {
        preview: board.preview.clone(),
    });
    let title = ui.add(
        Label::new(name, TextStyle::new(13.0).weight(600.0))
            .color(Token::Text)
            .single_line(),
    );
    let detail = ui.add(
        Label::new(detail, TextStyle::new(12.0))
            .color(Token::TextSubtle)
            .single_line(),
    );
    let words = ui.add_with_children(Flex::column().gap(2.0), &[title, detail]);
    let content = ui.add_with_children(
        Flex::column().gap(8.0).grow(1.0).style(|style| {
            style.min_size.width = length(0.0);
            style.overflow = taffy::Point {
                x: taffy::Overflow::Hidden,
                y: taffy::Overflow::Visible,
            };
        }),
        &[picture, words],
    );
    let asks = asks.clone();
    let path = board.path.clone();
    ButtonBox::new(format!("Open {name}"), ButtonColors::quiet(), move || {
        asks.borrow_mut().push(StartAsk::OpenRecent(path.clone()));
    })
    .padding(0.0, 0.0)
    .radius(10.0)
    .role(Role::Link)
    .add_to(ui, &[content])
}

/// The dashed tile after the recent boards: Open another board….
fn open_tile(ui: &mut Ui, asks: &StartAsks) -> WidgetId {
    let icon = ui.add(Icon::named("folder-open", 20.0, Token::TextMuted));
    let label = ui.add(
        Label::new("Open another board…", TextStyle::new(13.0))
            .color(Token::TextMuted)
            .single_line(),
    );
    let tile = ui.add_with_children(crate::recent::DashedTile, &[icon, label]);
    let content = ui.add_with_children(Flex::column().grow(1.0), &[tile]);
    let asks = asks.clone();
    ButtonBox::new("Open another board…", ButtonColors::quiet(), move || {
        asks.borrow_mut().push(StartAsk::Open);
    })
    .padding(0.0, 0.0)
    .radius(10.0)
    .role(Role::Link)
    .add_to(ui, &[content])
}

/// `tiles` in rows of three equal columns, 14 px apart.
fn rows(ui: &mut Ui, tiles: Vec<WidgetId>) -> Vec<WidgetId> {
    tiles
        .chunks(3)
        .map(|row| {
            let mut cells: Vec<WidgetId> = row
                .iter()
                .map(|&tile| {
                    ui.add_with_children(
                        Flex::column().grow(1.0).style(|style| {
                            style.flex_basis = length(0.0);
                            style.min_size.width = length(0.0);
                        }),
                        &[tile],
                    )
                })
                .collect();
            // A short last row keeps the columns' width.
            while cells.len() < 3 {
                cells.push(ui.add(Flex::column().grow(1.0).style(|style| {
                    style.flex_basis = length(0.0);
                })));
            }
            ui.add_with_children(
                Flex::row()
                    .gap(14.0)
                    .style(|style| style.align_items = taffy::AlignItems::FLEX_START),
                &cells,
            )
        })
        .collect()
}

/// The user's home folder, for showing folders from it.
fn home_folder() -> Option<std::path::PathBuf> {
    let var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    std::env::var_os(var)
        .map(std::path::PathBuf::from)
        .filter(|home| home.is_absolute())
}

impl View for StartWindow {
    fn paint(&mut self, scene: &mut Scene, size: Size) {
        self.handle_asks();
        self.poll_opening();
        self.sync_notice();
        self.sync_recent(false);
        self.ui
            .layout(&mut self.fonts, Rect::from_origin_size((0.0, 0.0), size));
        self.ui.paint(&mut self.fonts, scene);
    }

    fn input(&mut self, input: &Input) -> bool {
        if let Input::FileDropped(path) = input {
            self.open_recent(path.clone());
            return true;
        }
        // Ctrl+N wherever the keys are.
        if let Input::Key {
            key,
            pressed: true,
            repeat: false,
            modifiers,
            ..
        } = input
            && let Some(press) = imaginarium_canvas::keys::key_press(key, *modifiers)
            && let Some(command) = self.keymap.command_for_press(press)
        {
            self.ui.note_keyboard();
            match command.as_str() {
                "file.new_board" => self.new_board(),
                "file.open" => self.open_file(),
                "settings.open" => self.open_settings(crate::settings::Section::General),
                _ => {}
            }
            return true;
        }
        let handled = self.ui.input(&mut self.fonts, input);
        self.handle_asks() || handled
    }

    fn presented(&mut self, frame: &FrameInfo) -> bool {
        self.frames = frame.number;
        false
    }

    fn should_close(&mut self) -> bool {
        self.close_after.is_some_and(|frames| self.frames >= frames)
    }

    fn wake_at(&mut self) -> Option<Instant> {
        self.ui.wake_at()
    }

    fn wake(&mut self, now: Instant) -> bool {
        self.ui.wake(&mut self.fonts, now);
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
        self.handle_asks() || handled
    }

    fn take_requests(&mut self) -> Vec<WindowRequest> {
        std::mem::take(&mut self.requests)
    }
}

impl Drop for StartWindow {
    fn drop(&mut self) {
        if let Ok(mut settings) = self.app.settings().try_borrow_mut() {
            settings.unsubscribe(self.subscription);
        }
    }
}
