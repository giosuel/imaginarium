// SPDX-License-Identifier: AGPL-3.0-or-later

//! The Settings window (`shell.md` §5, the Settings mockups), generated
//! from the settings registry (ADR-0014): sections in a sidebar, a search
//! across all of them, and a row per setting with its title, description
//! and an editor chosen by the value's kind. A changed value shows
//! "Changed" and a reset button; changes apply at once and are saved.
//!
//! The keyboard shortcuts section lists the commands' shortcuts; changing
//! them comes later. The This board scope comes with M3.5 part 4.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use accesskit::Role;
use imaginarium_commands::{Command, Keymap, Platform};
use imaginarium_render::Scene;
use imaginarium_render::kurbo::{Rect, Size};
use imaginarium_render::peniko::Color;
use imaginarium_settings::{
    AnySetting, Constraint, ListKind, Rgba, SettingMeta, SubscriptionId, Value, ValueKind,
};
use imaginarium_text::{Fonts, TextStyle};
use imaginarium_ui::{
    AccessTree, Anchor, ButtonBox, ButtonColors, Flex, FrameInfo, Icon, IconButton, Input,
    InputColors, Label, LayerId, LayerOptions, Placement, Scroll, Segmented, Swatch, Switch,
    TextInput, Token, Ui, View, WidgetId, WindowOptions, WindowRequest, badge, dropdown, theme,
};
use taffy::prelude::length;

use crate::app::App;

/// The sidebar's sections, in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Section {
    General,
    Appearance,
    Board,
    Files,
    Shortcuts,
    Advanced,
}

pub(crate) const SECTIONS: [Section; 6] = [
    Section::General,
    Section::Appearance,
    Section::Board,
    Section::Files,
    Section::Shortcuts,
    Section::Advanced,
];

impl Section {
    fn title(self) -> &'static str {
        match self {
            Self::General => "General",
            Self::Appearance => "Appearance",
            Self::Board => "Board",
            Self::Files => "Files and saving",
            Self::Shortcuts => "Keyboard shortcuts",
            Self::Advanced => "Advanced",
        }
    }

    fn icon(self) -> &'static str {
        match self {
            Self::General => "sliders-horizontal",
            Self::Appearance => "palette",
            Self::Board => "square",
            Self::Files => "folder-open",
            Self::Shortcuts => "keyboard",
            Self::Advanced => "code",
        }
    }

    /// The section a setting is shown in: Advanced if it is advanced, else
    /// by its group (`.group("appearance")`, or the key's first segment).
    /// Groups no section names go to General.
    pub(crate) fn of(meta: &SettingMeta) -> Self {
        if meta.advanced {
            return Self::Advanced;
        }
        match meta.group {
            "appearance" | "ui" => Self::Appearance,
            "board" | "canvas" => Self::Board,
            "files" => Self::Files,
            _ => Self::General,
        }
    }
}

/// Whose values the window edits.
enum Target {
    Mine,
    Board(Rc<crate::app::BoardLink>),
    /// This board was shown, but the board is gone (or read-only).
    Gone,
}

/// What the window's controls asked for; done after the input, when the
/// tree can be rebuilt.
#[derive(Clone, Debug, PartialEq)]
enum Ask {
    Section(Section),
    Query(String),
    /// A value, from a switch, a segmented control or a list's row.
    Set(&'static str, Value),
    /// Typed text, to parse for the setting's kind.
    Typed(&'static str, String),
    Reset(&'static str),
    /// Open the list of a dropdown.
    Choices(&'static str, WidgetId),
    /// Show the values for me (false) or of this board (true).
    Scope(bool),
}

type Asks = Rc<RefCell<Vec<Ask>>>;

/// The Settings window. Open it with [`App::settings_window`]; run it in
/// the app's event loop with [`SettingsWindow::options`].
pub struct SettingsWindow {
    app: Rc<App>,
    fonts: Fonts,
    ui: Ui,
    asks: Asks,
    search: WidgetId,
    sidebar: WidgetId,
    content: WidgetId,
    scroll: WidgetId,
    section: Section,
    query: String,
    /// Why the last typed value of a setting was refused.
    errors: HashMap<&'static str, String>,
    popup: Option<LayerId>,
    /// A setting changed somewhere (another window, the palette): rebuild.
    changed: Arc<AtomicBool>,
    subscription: SubscriptionId,
    /// The notice about saving shown now, to rebuild when it changes.
    notice: Option<String>,
    /// The sidebar's and the content's widgets, removed at a rebuild.
    built: Vec<WidgetId>,
    /// The control to give the keys back to after the next layout: its
    /// label, and which of the controls with that label it is.
    restore: Option<(String, usize)>,
    /// A change from elsewhere came while a field was being typed in: the
    /// rebuild waits until the field is left.
    deferred: bool,
    /// This board's values are shown (where a setting can have one).
    board_scope: bool,
    /// The board shown, and its values' generation then.
    board_seen: Option<(u64, u64)>,
    requests: Vec<WindowRequest>,
    close_after: Option<u64>,
    frames: u64,
}

impl SettingsWindow {
    /// The Settings window of `app`, showing General.
    pub fn new(app: Rc<App>) -> Self {
        let fonts = app.fonts();
        let mut ui = Ui::new();
        let asks: Asks = Rc::default();
        let query_asks = asks.clone();
        let search = ui.add(
            TextInput::new(
                "Search settings",
                "Search settings",
                TextStyle::new(13.0),
                InputColors::themed(),
            )
            .on_change(move |text| query_asks.borrow_mut().push(Ask::Query(text.to_owned())))
            .bare(),
        );
        let field = ui.add_with_children(Flex::row().grow(1.0), &[search]);
        let magnifier = ui.add(Icon::named("search", 15.0, Token::TextSubtle));
        let search_box = ui.add_with_children(
            Flex::row()
                .height(34.0)
                .gap(4.0)
                .padding(10.0, 0.0)
                .radius(8.0)
                .border(1.0, Token::ControlBorder)
                .background(Token::PanelBg),
            &[magnifier, field],
        );
        let sidebar = ui.add(Flex::column().gap(2.0));
        let side = ui.add_with_children(
            Flex::column()
                .width(248.0)
                .gap(14.0)
                .background(Token::PanelBg)
                .style(|style| {
                    style.padding = padding(12.0, 12.0, 16.0, 12.0);
                    style.flex_shrink = 0.0;
                }),
            &[search_box, sidebar],
        );
        let divider = ui.add(Flex::column().width(1.0).background(Token::Divider));
        let content = ui.add(Flex::column().style(|style| {
            style.padding = padding(36.0, 36.0, 24.0, 32.0);
        }));
        let scroll = ui.add_with_children(
            Scroll::new().label("Settings").style(|style| {
                style.size.height = taffy::prelude::percent(1.0);
                style.flex_grow = 1.0;
                style.min_size.width = length(0.0);
            }),
            &[content],
        );
        let root = ui.add_with_children(
            Flex::row()
                .fill()
                .background(Token::PanelBg)
                .style(|style| style.align_items = taffy::AlignItems::STRETCH),
            &[side, divider, scroll],
        );
        ui.set_root(root);
        let changed = Arc::new(AtomicBool::new(false));
        let flag = changed.clone();
        let subscription = app.settings().borrow_mut().subscribe(move |_| {
            flag.store(true, Ordering::Relaxed);
            // The change may come from another window.
            imaginarium_ui::request_frame_everywhere();
        });
        let mut window = Self {
            app,
            fonts,
            ui,
            asks,
            search,
            sidebar,
            content,
            scroll,
            section: Section::General,
            query: String::new(),
            errors: HashMap::new(),
            popup: None,
            changed,
            subscription,
            notice: None,
            built: Vec::new(),
            restore: None,
            deferred: false,
            board_scope: false,
            board_seen: None,
            requests: Vec::new(),
            close_after: None,
            frames: 0,
        };
        window.rebuild();
        window
    }

    /// Closes the window after `frames` frames (for scripted runs).
    pub fn close_after(mut self, frames: u64) -> Self {
        self.close_after = Some(frames);
        self
    }

    /// The window's options: `Settings — Imaginarium`, 1120 × 720.
    pub fn options(&self) -> WindowOptions {
        WindowOptions {
            title: "Settings — Imaginarium".into(),
            size: Size::new(1120.0, 720.0),
            background: theme().color(Token::PanelBg),
            app_id: Some("ch.giosuel.Imaginarium".into()),
            ..WindowOptions::default()
        }
    }

    /// The window's widgets, for tests.
    pub fn ui(&mut self) -> (&mut Ui, &mut Fonts) {
        (&mut self.ui, &mut self.fonts)
    }

    /// Shows `section` (and clears the search).
    pub(crate) fn show(&mut self, section: Section) {
        self.section = section;
        self.query.clear();
        if let Some(search) = self.ui.widget_mut::<TextInput>(self.search) {
            search.set_text(&mut self.fonts, "");
        }
        if let Some(scroll) = self.ui.widget_mut::<Scroll>(self.scroll) {
            scroll.set_offset(0.0);
        }
        self.rebuild();
    }

    fn handle_asks(&mut self) -> bool {
        let mut rebuild = false;
        let mut any = false;
        loop {
            let asks = std::mem::take(&mut *self.asks.borrow_mut());
            if asks.is_empty() {
                break;
            }
            any = true;
            for ask in asks {
                rebuild |= self.handle(ask);
            }
        }
        // Changes from elsewhere don't rebuild a field being typed in (its
        // draft would be lost): they wait until it is left.
        let mut external = std::mem::take(&mut self.deferred);
        if self.changed.swap(false, Ordering::Relaxed) {
            external = true;
        }
        if self.app.save_notice() != self.notice {
            external = true;
        }
        // Another board became the active one, or this one's values changed.
        let board = self
            .app
            .active_board()
            .map(|board| (board.id, board.generation.get()));
        if board != self.board_seen {
            external = true;
        }
        if external && !rebuild {
            if self.typing() {
                self.deferred = true;
            } else {
                rebuild = true;
            }
        }
        if rebuild {
            self.rebuild();
        }
        any || rebuild
    }

    /// Whether a settings field (not the search) has the keys.
    fn typing(&self) -> bool {
        self.ui.focused().is_some_and(|focused| {
            focused != self.search && self.ui.widget::<TextInput>(focused).is_some()
        })
    }

    /// Does `ask`; whether the window must be rebuilt.
    fn handle(&mut self, ask: Ask) -> bool {
        match ask {
            Ask::Section(section) => {
                self.show(section);
                false
            }
            Ask::Query(query) => {
                if query == self.query {
                    return false;
                }
                self.query = query;
                true
            }
            Ask::Set(key, value) => {
                self.close_menu();
                self.set(key, value);
                true
            }
            Ask::Scope(board) => {
                self.board_scope = board;
                true
            }
            Ask::Typed(key, text) => {
                let settings = self.app.settings().clone();
                let declaration = settings.borrow().declaration(key);
                let Ok(declaration) = declaration else {
                    return false;
                };
                let error = self.errors.get(key).cloned();
                let before = settings.borrow().value(key).ok();
                match parse(declaration, &text) {
                    Ok(value) => self.set(key, value),
                    Err(reason) => {
                        self.errors.insert(key, reason);
                    }
                }
                // Only a new value or a new error is worth a rebuild (a
                // rebuild moves the keys, which could commit again).
                settings.borrow().value(key).ok() != before
                    || self.errors.get(key) != error.as_ref()
            }
            Ask::Reset(key) => {
                self.errors.remove(key);
                match self.target() {
                    // Back to following my setting.
                    Target::Board(board) => board.ask(key, None),
                    Target::Mine => {
                        let _ = self.app.settings().borrow_mut().reset(key);
                    }
                    Target::Gone => self.board_scope = false,
                }
                true
            }
            Ask::Choices(key, anchor) => {
                self.open_choices(key, anchor);
                false
            }
        }
    }

    /// The board whose values are shown and edited, if This board is.
    fn board(&self) -> Option<Rc<crate::app::BoardLink>> {
        match self.target() {
            Target::Board(board) => Some(board),
            Target::Mine | Target::Gone => None,
        }
    }

    /// Whose values the window edits now.
    fn target(&self) -> Target {
        if !self.board_scope || !self.query.trim().is_empty() {
            return Target::Mine;
        }
        match self.app.active_board() {
            Some(board) if !board.read_only.get() => Target::Board(board),
            // Closed (or read-only) since: never my settings instead.
            _ => Target::Gone,
        }
    }

    /// The value shown for `declaration`, and whether it is changed (from
    /// the default, or, for This board, the board has its own).
    fn current(&self, declaration: &'static dyn AnySetting) -> (Value, bool) {
        let key = declaration.key();
        let settings = self.app.settings().borrow();
        let mine = settings
            .value(key)
            .unwrap_or_else(|_| declaration.default_dyn());
        match self.board() {
            Some(board) => match board.own.borrow().get(key) {
                Some(own) => (own.clone(), true),
                None => (mine, false),
            },
            None => (mine, settings.is_modified(key)),
        }
    }

    fn set(&mut self, key: &'static str, value: Value) {
        let board = match self.target() {
            Target::Board(board) => Some(board),
            Target::Mine => None,
            Target::Gone => {
                self.board_scope = false;
                return;
            }
        };
        if let Some(board) = board {
            // Checked here, so a refused value says why at once; the board
            // stores it with `board.set_setting`.
            let checked = self
                .app
                .settings()
                .borrow()
                .declaration(key)
                .map_err(|err| err.to_string())
                .and_then(|declaration| {
                    declaration
                        .validate(&value)
                        .map(|()| declaration.normalize(value))
                        .map_err(|err| err.to_string())
                });
            match checked {
                Ok(value) => {
                    self.errors.remove(key);
                    board.ask(key, Some(value));
                }
                Err(err) => {
                    self.errors.insert(key, reason_of(&err));
                }
            }
            return;
        }
        let set = self.app.settings().borrow_mut().set_value(key, value);
        match set {
            Ok(()) => {
                self.errors.remove(key);
            }
            Err(err) => {
                self.errors.insert(key, reason_of(&err.to_string()));
            }
        }
    }

    fn close_menu(&mut self) {
        if let Some(popup) = self.popup.take() {
            self.ui.close_layer(&mut self.fonts, popup);
        }
    }

    /// Opens the list of values of `key` below `anchor`.
    fn open_choices(&mut self, key: &'static str, anchor: WidgetId) {
        self.close_menu();
        let declaration = self.app.settings().borrow().declaration(key);
        let Ok(declaration) = declaration else {
            return;
        };
        let (current, _) = self.current(declaration);
        let rows: Vec<WidgetId> = choices(declaration)
            .into_iter()
            .map(|(value, name)| {
                let checked = same(&value, &current);
                let check = self.ui.add(Icon::named(
                    "check",
                    16.0,
                    if checked {
                        Token::AccentInk
                    } else {
                        Token::PanelBg
                    },
                ));
                let text = self.ui.add(
                    Label::new(name.clone(), TextStyle::new(13.0))
                        .color(Token::Text)
                        .single_line()
                        .grow(1.0),
                );
                let asks = self.asks.clone();
                ButtonBox::new(name, ButtonColors::quiet(), move || {
                    asks.borrow_mut().push(Ask::Set(key, value.clone()));
                })
                .size(196.0, 32.0)
                .padding(8.0, 8.0)
                .gap(8.0)
                .start()
                .radius(7.0)
                .role(Role::MenuItemRadio)
                .selected(move || checked)
                .add_to(&mut self.ui, &[check, text])
            })
            .collect();
        let menu = self.ui.add_with_children(
            Flex::column()
                .width(208.0)
                .padding(6.0, 6.0)
                .gap(1.0)
                .background(Token::PanelBg)
                .border(1.0, Token::PanelBorder)
                .radius(10.0)
                .shadow()
                .role(Role::Menu, declaration.meta().title),
            &rows,
        );
        let options = LayerOptions::popover(Anchor::Widget(anchor))
            .placement(Placement::Below)
            .focus_first(true);
        self.popup = Some(self.ui.open_layer(menu, options));
    }

    /// Builds the sidebar's sections and the content again, keeping the
    /// keys in the control that had them.
    fn rebuild(&mut self) {
        self.close_menu();
        self.deferred = false;
        if let Some(focused) = self.focused_identity() {
            self.restore = Some(focused);
        }
        // Detaching isn't removing: the old widgets (and their effects)
        // would stay.
        for widget in std::mem::take(&mut self.built) {
            self.ui.remove(widget);
        }
        self.notice = self.app.save_notice();
        self.board_seen = self
            .app
            .active_board()
            .map(|board| (board.id, board.generation.get()));
        let searching = !self.query.trim().is_empty();
        let buttons: Vec<WidgetId> = SECTIONS
            .iter()
            .map(|&section| self.section_button(section, !searching && section == self.section))
            .collect();
        self.ui.set_children(self.sidebar, &buttons);
        self.built.extend(&buttons);
        let mut children = Vec::new();
        if let Some(notice) = self.notice.clone() {
            children.push(notice_box(&mut self.ui, &notice));
        }
        if searching {
            children.extend(self.search_results());
        } else {
            children.extend(self.section_content(self.section));
        }
        self.ui.set_children(self.content, &children);
        self.built.extend(&children);
    }

    /// Gives the keys back to the control that had them before a rebuild,
    /// once the new controls are laid out.
    fn restore_focus(&mut self) {
        let Some((label, nth)) = self.restore.take() else {
            return;
        };
        if let Some(&widget) = self.ui.find_all_by_label(&mut self.fonts, &label).get(nth) {
            self.ui.set_focus(&mut self.fonts, Some(widget));
        }
    }

    /// The focused control (unless it is the search, which stays) as its
    /// label and which of the controls with that label it is: controls
    /// share names ("On" in two settings).
    fn focused_identity(&mut self) -> Option<(String, usize)> {
        let focused = self.ui.focused()?;
        if focused == self.search {
            return None;
        }
        let mut tree = AccessTree::default();
        self.ui.access(&mut self.fonts, &mut tree);
        let base = focused.access_base();
        let label = tree
            .nodes()
            .iter()
            .find(|(id, _)| *id == accesskit::NodeId(base))
            .and_then(|(_, node)| node.label().map(str::to_owned))?;
        let nth = self
            .ui
            .find_all_by_label(&mut self.fonts, &label)
            .iter()
            .position(|&widget| widget == focused)?;
        Some((label, nth))
    }

    fn section_button(&mut self, section: Section, selected: bool) -> WidgetId {
        let ink = if selected {
            Token::AccentInk
        } else {
            Token::TextSecondary
        };
        let icon = self.ui.add(Icon::named(section.icon(), 16.0, ink));
        let title = self.ui.add(
            Label::new(section.title(), TextStyle::new(14.0).weight(500.0))
                .color(if selected {
                    Token::AccentInk
                } else {
                    Token::Text
                })
                .single_line(),
        );
        let asks = self.asks.clone();
        ButtonBox::new(section.title(), ButtonColors::quiet(), move || {
            asks.borrow_mut().push(Ask::Section(section));
        })
        .height(34.0)
        .padding(10.0, 10.0)
        .gap(10.0)
        .start()
        .radius(8.0)
        .keep_focus()
        .selected(move || selected)
        .selected_fill(Token::AccentSoft)
        .add_to(&mut self.ui, &[icon, title])
    }

    /// The header and rows of `section`.
    fn section_content(&mut self, section: Section) -> Vec<WidgetId> {
        if section == Section::Shortcuts {
            let mut children = vec![self.header(
                section.title(),
                "The keys of the board's and the windows' commands.",
            )];
            children.extend(self.shortcut_rows(None));
            return children;
        }
        let mut all: Vec<&'static dyn AnySetting> = self
            .app
            .settings()
            .borrow()
            .declarations()
            .filter(|declaration| Section::of(&declaration.meta()) == section)
            .collect();
        // The Settings mockup's order; others after, as registered.
        all.sort_by_key(|declaration| {
            ORDER
                .iter()
                .position(|key| *key == declaration.key())
                .unwrap_or(ORDER.len())
        });
        // For me and This board, where a setting can have a board's value
        // and a board window was used.
        let board = self.app.active_board();
        let tabs = board.as_ref().is_some_and(|board| !board.read_only.get())
            && all
                .iter()
                .any(|declaration| declaration.meta().per_document);
        if !tabs {
            self.board_scope = false;
        }
        let subtitle = match &board {
            Some(board) if self.board_scope => format!(
                "Changes apply to “{}” only. The others follow your settings.",
                board.name.borrow()
            ),
            _ => "Changes apply right away.".to_owned(),
        };
        let mut children = vec![self.header_with(section.title(), &subtitle, tabs)];
        let declarations: Vec<&'static dyn AnySetting> = all
            .into_iter()
            .filter(|declaration| !self.board_scope || declaration.meta().per_document)
            .collect();
        if declarations.is_empty() {
            children.push(
                self.ui.add(
                    Label::new("Nothing to set here yet.", TextStyle::new(13.0))
                        .color(Token::TextMuted),
                ),
            );
        }
        for declaration in declarations {
            children.push(self.row(declaration));
        }
        children
    }

    /// The settings and shortcuts matching the search, under their
    /// sections' names.
    fn search_results(&mut self) -> Vec<WidgetId> {
        let words: Vec<String> = self
            .query
            .split_whitespace()
            .map(str::to_lowercase)
            .collect();
        let found = |text: &str| {
            let text = text.to_lowercase();
            words.iter().all(|word| text.contains(word.as_str()))
        };
        let declarations: Vec<&'static dyn AnySetting> =
            self.app.settings().borrow().declarations().collect();
        let mut children = vec![self.header("Search", "Settings whose name or description match.")];
        let mut any = false;
        for section in SECTIONS {
            if section == Section::Shortcuts {
                let rows = self.shortcut_rows(Some(&found));
                if !rows.is_empty() {
                    any = true;
                    children.push(self.heading(section.title()));
                    children.extend(rows);
                }
                continue;
            }
            let matching: Vec<_> = declarations
                .iter()
                .filter(|declaration| {
                    let meta = declaration.meta();
                    Section::of(&meta) == section && found(&searchable(declaration.key(), &meta))
                })
                .copied()
                .collect();
            if matching.is_empty() {
                continue;
            }
            any = true;
            children.push(self.heading(section.title()));
            for declaration in matching {
                children.push(self.row(declaration));
            }
        }
        if !any {
            children.push(self.ui.add(
                Label::new("No setting matches.", TextStyle::new(13.0)).color(Token::TextMuted),
            ));
        }
        children
    }

    fn header(&mut self, title: &str, subtitle: &str) -> WidgetId {
        self.header_with(title, subtitle, false)
    }

    /// The section's title and subtitle, and the scope tabs if `tabs`.
    fn header_with(&mut self, title: &str, subtitle: &str, tabs: bool) -> WidgetId {
        let title = self.ui.add(
            Label::new(title, TextStyle::new(20.0).weight(700.0))
                .color(Token::Text)
                .single_line(),
        );
        let subtitle = self.ui.add(
            Label::new(subtitle, TextStyle::new(13.0))
                .color(Token::TextMuted)
                .single_line(),
        );
        let text = self.ui.add_with_children(
            Flex::column().gap(4.0).grow(1.0).style(|style| {
                style.min_size.width = length(0.0);
            }),
            &[title, subtitle],
        );
        let mut children = vec![text];
        if tabs {
            let asks = self.asks.clone();
            let board = self.board_scope;
            children.push(
                Segmented::new(
                    "Settings for",
                    &["For me", "This board"],
                    move || usize::from(board),
                    move |at| asks.borrow_mut().push(Ask::Scope(at == 1)),
                )
                .add_to(&mut self.ui),
            );
        }
        self.ui.add_with_children(
            Flex::row().gap(16.0).style(|style| {
                style.padding.bottom = length(18.0);
                style.align_items = taffy::AlignItems::FLEX_START;
            }),
            &children,
        )
    }

    fn heading(&mut self, title: &str) -> WidgetId {
        let label = self.ui.add(
            Label::new(title, TextStyle::new(12.0).weight(600.0))
                .color(Token::TextMuted)
                .single_line(),
        );
        self.ui.add_with_children(
            Flex::column().style(|style| {
                style.padding = padding(0.0, 0.0, 18.0, 4.0);
            }),
            &[label],
        )
    }

    /// One setting: title, Changed, description and error on the left;
    /// reset and the editor on the right; a divider above.
    fn row(&mut self, declaration: &'static dyn AnySetting) -> WidgetId {
        let key = declaration.key();
        if key == crate::look::THEME.key() {
            return self.theme_row(declaration);
        }
        let meta = declaration.meta();
        let (value, modified) = self.current(declaration);
        let title = self.ui.add(
            Label::new(meta.title, TextStyle::new(14.0).weight(600.0))
                .color(Token::Text)
                .single_line(),
        );
        let mut title_row = vec![title];
        if modified {
            title_row.push(badge(&mut self.ui, "Changed"));
        }
        let title_row = self.ui.add_with_children(Flex::row().gap(8.0), &title_row);
        let description = self
            .ui
            .add(Label::new(meta.description, TextStyle::new(13.0)).color(Token::TextMuted));
        let mut text = vec![title_row, description];
        // A change the board itself refused shows on its row too.
        let board_error = self.board().and_then(|board| {
            board
                .error
                .borrow()
                .as_ref()
                .filter(|(of, _)| *of == key)
                .map(|(_, error)| reason_of(error))
        });
        if let Some(error) = self.errors.get(key).cloned().or(board_error) {
            // An alert, so a screen reader says why the value was refused.
            let label = self
                .ui
                .add(Label::new(error.clone(), TextStyle::new(12.0)).color(Token::Danger));
            text.push(
                self.ui
                    .add_with_children(Flex::column().role(Role::Alert, error), &[label]),
            );
        }
        let text = self.ui.add_with_children(
            Flex::column().gap(3.0).grow(1.0).style(|style| {
                style.min_size.width = length(0.0);
                style.flex_basis = length(0.0);
            }),
            &text,
        );
        let mut controls = Vec::new();
        if modified {
            let asks = self.asks.clone();
            controls.push(
                IconButton::new("rotate-ccw", format!("Reset {}", meta.title), move || {
                    asks.borrow_mut().push(Ask::Reset(key));
                })
                .size(28.0, 28.0, 7.0, 16.0)
                .add_to(&mut self.ui),
            );
        }
        controls.push(self.editor(declaration, value));
        let controls = self.ui.add_with_children(Flex::row().gap(10.0), &controls);
        let row = self.ui.add_with_children(
            Flex::row().gap(24.0).style(|style| {
                style.padding = padding(0.0, 0.0, 16.0, 16.0);
            }),
            &[text, controls],
        );
        let line = self
            .ui
            .add(Flex::row().height(1.0).background(Token::Divider));
        self.ui.add_with_children(Flex::column(), &[line, row])
    }

    /// The theme's row, as the Settings mockup draws it: its text, then
    /// the four cards (System, Light, Dark; High contrast comes with its
    /// own design).
    fn theme_row(&mut self, declaration: &'static dyn AnySetting) -> WidgetId {
        let key = declaration.key();
        let meta = declaration.meta();
        let (value, modified) = self.current(declaration);
        let title = self.ui.add(
            Label::new(meta.title, TextStyle::new(14.0).weight(600.0))
                .color(Token::Text)
                .single_line(),
        );
        let mut title_row = vec![title];
        if modified {
            title_row.push(badge(&mut self.ui, "Changed"));
        }
        let title_row = self.ui.add_with_children(Flex::row().gap(8.0), &title_row);
        let description = self
            .ui
            .add(Label::new(meta.description, TextStyle::new(13.0)).color(Token::TextMuted));
        let text = self
            .ui
            .add_with_children(Flex::column().gap(3.0).grow(1.0), &[title_row, description]);
        let mut top = vec![text];
        if modified {
            let asks = self.asks.clone();
            top.push(
                IconButton::new("rotate-ccw", format!("Reset {}", meta.title), move || {
                    asks.borrow_mut().push(Ask::Reset(key));
                })
                .size(28.0, 28.0, 7.0, 16.0)
                .add_to(&mut self.ui),
            );
        }
        let text = self.ui.add_with_children(Flex::row().gap(10.0), &top);
        let current = match &value {
            Value::Enum(id) => id.clone(),
            _ => String::new(),
        };
        let mut cards = Vec::new();
        for (id, label, look) in [
            ("system", "System", CardLook::System),
            ("light", "Light", CardLook::Light),
            ("dark", "Dark", CardLook::Dark),
        ] {
            let chosen = current == id;
            let asks = self.asks.clone();
            cards.push(theme_card(
                &mut self.ui,
                label,
                look,
                chosen,
                Some(Box::new(move || {
                    asks.borrow_mut()
                        .push(Ask::Set(key, Value::Enum(id.to_owned())));
                })),
            ));
        }
        cards.push(theme_card(
            &mut self.ui,
            "High contrast",
            CardLook::HighContrast,
            false,
            None,
        ));
        let grid = self.ui.add_with_children(
            Flex::row()
                .gap(12.0)
                .role(Role::RadioGroup, meta.title)
                .style(|style| style.max_size.width = length(640.0)),
            &cards,
        );
        let column = self.ui.add_with_children(
            Flex::column().gap(12.0).style(|style| {
                style.padding = padding(0.0, 0.0, 16.0, 16.0);
            }),
            &[text, grid],
        );
        let line = self
            .ui
            .add(Flex::row().height(1.0).background(Token::Divider));
        self.ui.add_with_children(Flex::column(), &[line, column])
    }

    /// The accent's swatches (the Settings mockup's seven).
    fn accent_swatches(&mut self, value: &Value) -> WidgetId {
        use imaginarium_settings::SettingType;
        let key = crate::look::ACCENT.key();
        let current = crate::look::Accent::from_value(value).unwrap_or_default();
        let swatches: Vec<WidgetId> =
            <crate::look::Accent as imaginarium_settings::SettingEnum>::VARIANTS
                .iter()
                .map(|variant| {
                    let accent = variant.value;
                    let asks = self.asks.clone();
                    let id = variant.id;
                    self.ui.add(
                        Swatch::new(Some(accent.color()), variant.label, move || {
                            asks.borrow_mut()
                                .push(Ask::Set(key, Value::Enum(id.to_owned())));
                        })
                        .shape(imaginarium_ui::SwatchShape::Dot)
                        .dark_color(accent.dark_swatch())
                        .radio()
                        .size(26.0)
                        .selected(accent == current),
                    )
                })
                .collect();
        self.ui.add_with_children(
            Flex::row()
                .gap(10.0)
                .align_center()
                .role(Role::RadioGroup, crate::look::ACCENT.meta().title),
            &swatches,
        )
    }

    /// The control that edits `declaration`, chosen by its kind.
    fn editor(&mut self, declaration: &'static dyn AnySetting, value: Value) -> WidgetId {
        let key = declaration.key();
        let meta = declaration.meta();
        let asks = self.asks.clone();
        if key == crate::look::ACCENT.key() {
            return self.accent_swatches(&value);
        }
        // The mockup's dropdown, though it has only three choices.
        if key == crate::look::REDUCE_MOTION.key() {
            return self.dropdown_for(declaration, &value);
        }
        match (declaration.kind(), &value) {
            (ValueKind::Bool, Value::Bool(on)) => {
                let on = *on;
                Switch::new(
                    meta.title,
                    move || on,
                    move || {
                        asks.borrow_mut().push(Ask::Set(key, Value::Bool(!on)));
                    },
                )
                .add_to(&mut self.ui)
            }
            (ValueKind::Enum, Value::Enum(current)) => {
                let variants = declaration.variants();
                if variants.len() <= 3 {
                    let names: Vec<&str> = variants.iter().map(|variant| variant.label).collect();
                    let ids: Vec<&'static str> =
                        variants.iter().map(|variant| variant.id).collect();
                    let at = ids.iter().position(|id| id == current).unwrap_or(0);
                    Segmented::new(
                        meta.title,
                        &names,
                        move || at,
                        move |at| {
                            if let Some(id) = ids.get(at) {
                                asks.borrow_mut()
                                    .push(Ask::Set(key, Value::Enum((*id).to_owned())));
                            }
                        },
                    )
                    .add_to(&mut self.ui)
                } else {
                    self.dropdown_for(declaration, &value)
                }
            }
            (ValueKind::Int | ValueKind::Float, _) if !choices(declaration).is_empty() => {
                self.dropdown_for(declaration, &value)
            }
            (ValueKind::Color, Value::Color(color)) => {
                let swatch = self.ui.add(Swatch::shown(
                    Some(color_of(*color)),
                    imaginarium_ui::SwatchShape::Square,
                    28.0,
                ));
                let field = self.field(key, meta.title, &format_value(&value), 120.0);
                self.ui
                    .add_with_children(Flex::row().gap(8.0), &[swatch, field])
            }
            (ValueKind::SecretRef, Value::SecretRef(secret)) => {
                let text = if secret.id.is_empty() {
                    "Not set".to_owned()
                } else {
                    format!("In the keychain ({})", secret.id)
                };
                self.ui.add(
                    Label::new(text, TextStyle::new(13.0))
                        .color(Token::TextMuted)
                        .single_line(),
                )
            }
            _ => {
                let width = match declaration.kind() {
                    ValueKind::Int | ValueKind::Float => 120.0,
                    _ => 260.0,
                };
                self.field(key, meta.title, &format_value(&value), width)
            }
        }
    }

    fn dropdown_for(&mut self, declaration: &'static dyn AnySetting, value: &Value) -> WidgetId {
        let key = declaration.key();
        let name = choices(declaration)
            .into_iter()
            .find(|(choice, _)| same(choice, value))
            .map_or_else(
                // Off the steps (a value from the file): as the steps read.
                || {
                    if key == crate::look::SCALE.key() {
                        format!("{:.0} %", value_number(value) * 100.0)
                    } else {
                        format_value(value)
                    }
                },
                |(_, name)| name,
            );
        let asks = self.asks.clone();
        let anchor = Rc::new(std::cell::Cell::new(None));
        let at = anchor.clone();
        let button = dropdown(
            &mut self.ui,
            declaration.meta().title,
            200.0,
            move || name.clone(),
            move || {
                if let Some(anchor) = at.get() {
                    asks.borrow_mut().push(Ask::Choices(key, anchor));
                }
            },
        );
        anchor.set(Some(button));
        button
    }

    /// A text field for `key`, committing on Enter and when the focus
    /// leaves.
    fn field(&mut self, key: &'static str, label: &str, text: &str, width: f32) -> WidgetId {
        let submit = self.asks.clone();
        let blur = self.asks.clone();
        // Leaving a field that wasn't edited keeps the value as it is (its
        // text may show it rounded).
        let shown = text.to_owned();
        let input = self.ui.add(
            TextInput::new(label, "", TextStyle::new(13.0), InputColors::themed())
                .on_submit(move |text| {
                    submit.borrow_mut().push(Ask::Typed(key, text.to_owned()));
                })
                .on_blur(move |text| {
                    if text != shown {
                        blur.borrow_mut().push(Ask::Typed(key, text.to_owned()));
                    }
                }),
        );
        if let Some(widget) = self.ui.widget_mut::<TextInput>(input) {
            widget.set_text(&mut self.fonts, text);
        }
        self.ui
            .add_with_children(Flex::column().width(width), &[input])
    }

    /// The commands with shortcuts (those `found` matches, if given): the
    /// board's and the windows'.
    fn shortcut_rows(&mut self, found: Option<&dyn Fn(&str) -> bool>) -> Vec<WidgetId> {
        let platform = Platform::current();
        let keymaps: [Keymap; 2] = [
            imaginarium_canvas::keys::default_keymap(platform),
            crate::commands::shell_keymap(platform),
        ];
        let mut listed: Vec<(&'static str, &'static str, String)> = Vec::new();
        let metas = imaginarium_canvas::commands::board_commands()
            .iter()
            .map(|command| (command.id().as_str(), *command.meta()))
            .chain(
                crate::commands::shell_commands()
                    .iter()
                    .map(|command| (command.id().as_str(), *command.meta())),
            );
        for (id, meta) in metas {
            let mut keys: Vec<String> = Vec::new();
            for keymap in &keymaps {
                for shortcut in keymap.shortcuts_for(imaginarium_commands::CommandId::new(id)) {
                    let text = shortcut.display(keymap.platform());
                    if !keys.contains(&text) {
                        keys.push(text);
                    }
                }
            }
            if keys.is_empty() {
                continue;
            }
            let keys = keys.join("  ");
            if found.is_some_and(|found| {
                !found(&format!("{} {} {}", meta.title, meta.description, keys))
            }) {
                continue;
            }
            listed.push((meta.title, meta.description, keys));
        }
        let mut rows = Vec::new();
        if found.is_none() {
            rows.push(
                self.ui.add(
                    Label::new(
                        "Changing shortcuts comes in a later preview.",
                        TextStyle::new(13.0),
                    )
                    .color(Token::TextMuted),
                ),
            );
        }
        for (title, description, keys) in listed {
            let title = self.ui.add(
                Label::new(title, TextStyle::new(14.0).weight(600.0))
                    .color(Token::Text)
                    .single_line(),
            );
            let description = self.ui.add(
                Label::new(
                    crate::palette::first_sentence(description),
                    TextStyle::new(13.0),
                )
                .color(Token::TextMuted),
            );
            let text = self.ui.add_with_children(
                Flex::column().gap(3.0).grow(1.0).style(|style| {
                    style.min_size.width = length(0.0);
                    style.flex_basis = length(0.0);
                }),
                &[title, description],
            );
            let chip = crate::palette::chip(&mut self.ui, &keys);
            let row = self.ui.add_with_children(
                Flex::row().gap(24.0).style(|style| {
                    style.padding = padding(0.0, 0.0, 12.0, 12.0);
                }),
                &[text, chip],
            );
            let line = self
                .ui
                .add(Flex::row().height(1.0).background(Token::Divider));
            rows.push(self.ui.add_with_children(Flex::column(), &[line, row]));
        }
        rows
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

/// A numeric value as a number.
fn value_number(value: &Value) -> f64 {
    match value {
        Value::Int(n) => *n as f64,
        Value::Float(x) => *x,
        _ => 0.0,
    }
}

/// The order of the Appearance section, as the Settings mockup has it.
const ORDER: &[&str] = &[
    "ui.theme",
    "ui.accent",
    "board.background",
    "ui.reduce_motion",
    "ui.scale",
    "ui.toolbar.position",
];

/// What a theme card's picture shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CardLook {
    System,
    Light,
    Dark,
    HighContrast,
}

/// A theme card: its picture (the Settings mockup's), ringed in the accent
/// when chosen, then its name. Without an action it is disabled, saying
/// what's coming (High contrast).
fn theme_card(
    ui: &mut Ui,
    label: &str,
    look: CardLook,
    chosen: bool,
    action: Option<Box<dyn FnMut()>>,
) -> WidgetId {
    let picture = ui.add(ThemePicture { look, chosen });
    let weight = if chosen { 600.0 } else { 500.0 };
    let name = ui.add(
        Label::new(label, TextStyle::new(13.0).weight(weight))
            .color(if chosen {
                Token::Text
            } else {
                Token::TextSecondary
            })
            .single_line(),
    );
    let content = ui.add_with_children(
        Flex::column().gap(8.0).grow(1.0).style(|style| {
            style.min_size.width = length(0.0);
        }),
        &[picture, name],
    );
    let enabled = action.is_some();
    let mut action = action;
    let mut card = imaginarium_ui::ButtonBox::new(
        label,
        imaginarium_ui::ButtonColors {
            background: Token::PanelBg.into(),
            ..imaginarium_ui::ButtonColors::quiet()
        },
        move || {
            if let Some(action) = &mut action {
                action();
            }
        },
    )
    .padding(0.0, 0.0)
    .radius(10.0)
    .grow(1.0)
    .role(Role::RadioButton)
    .selected(move || chosen)
    .selected_fill(Token::PanelBg)
    .disabled(!enabled);
    if !enabled {
        card = card.tooltip(
            "High contrast comes with its own design, in a later preview",
            Placement::Below,
        );
    }
    let card = card.add_to(ui, &[content]);
    ui.add_with_children(
        Flex::column().grow(1.0).style(|style| {
            style.flex_basis = length(0.0);
            style.min_size.width = length(0.0);
        }),
        &[card],
    )
}

/// A theme card's picture: the Settings mockup's sketch of each theme
/// (fixed colors: it shows the theme, not the current one).
struct ThemePicture {
    look: CardLook,
    chosen: bool,
}

impl imaginarium_ui::Widget for ThemePicture {
    fn style(&self) -> taffy::Style {
        taffy::Style {
            size: taffy::Size {
                width: taffy::Dimension::percent(1.0),
                height: length(76.0),
            },
            ..taffy::Style::default()
        }
    }

    fn paint(&mut self, cx: &mut imaginarium_ui::Cx<'_>, scene: &mut imaginarium_render::Scene) {
        use imaginarium_render::kurbo::{Ellipse, Rect as KRect, RoundedRect, Shape, Stroke};
        let hex = |hex: u32| {
            let [_, r, g, b] = hex.to_be_bytes();
            imaginarium_render::peniko::Color::from_rgb8(r, g, b)
        };
        let bounds = cx.bounds();
        let card = RoundedRect::from_rect(bounds, 10.0);
        let at = |x: f64, y: f64, w: f64, h: f64| {
            KRect::new(
                bounds.x0 + x,
                bounds.y0 + y,
                bounds.x0 + x + w,
                bounds.y0 + y + h,
            )
        };
        scene.push_clip(&card);
        let (light, dark) = (hex(0xfcfcfd), hex(0x141416));
        match self.look {
            CardLook::System => {
                scene.fill(
                    &KRect::new(bounds.x0, bounds.y0, bounds.center().x, bounds.y1),
                    light,
                );
                scene.fill(
                    &KRect::new(bounds.center().x, bounds.y0, bounds.x1, bounds.y1),
                    dark,
                );
            }
            CardLook::Light => scene.fill(&card, light),
            CardLook::Dark => scene.fill(&card, dark),
            CardLook::HighContrast => scene.fill(&card, hex(0x000000)),
        }
        let block = |scene: &mut imaginarium_render::Scene,
                     shape: &dyn Fn() -> imaginarium_render::kurbo::BezPath,
                     fill: u32,
                     line: u32,
                     width: f64| {
            let path = shape();
            scene.fill(&path, hex(fill));
            scene.stroke_with(&path, &Stroke::new(width), hex(line));
        };
        // The pill at the top left.
        let pill = || RoundedRect::from_rect(at(12.0, 12.0, 44.0, 10.0), 4.0).to_path(0.1);
        let card_shape = || RoundedRect::from_rect(at(24.0, 34.0, 40.0, 22.0), 5.0).to_path(0.1);
        let dot = || Ellipse::from_rect(at(76.0, 38.0, 26.0, 18.0)).to_path(0.1);
        match self.look {
            CardLook::System => {
                block(scene, &pill, 0xffffff, 0xe4e4e7, 1.0);
                block(scene, &card_shape, 0xe0e7ff, 0x1e293b, 1.0);
                let right = || {
                    RoundedRect::from_rect(
                        KRect::new(
                            bounds.x1 - 48.0,
                            bounds.y0 + 34.0,
                            bounds.x1 - 18.0,
                            bounds.y0 + 56.0,
                        ),
                        5.0,
                    )
                    .to_path(0.1)
                };
                block(scene, &right, 0x2b3163, 0xc4c4cc, 1.0);
            }
            CardLook::Light => {
                block(scene, &pill, 0xffffff, 0xe4e4e7, 1.0);
                block(scene, &card_shape, 0xe0e7ff, 0x1e293b, 1.0);
                block(scene, &dot, 0xdcfce7, 0x1e293b, 1.0);
            }
            CardLook::Dark => {
                block(scene, &pill, 0x1e1e22, 0x2e2e35, 1.0);
                block(scene, &card_shape, 0x2b3163, 0xc4c4cc, 1.0);
                block(scene, &dot, 0x163a26, 0xc4c4cc, 1.0);
            }
            CardLook::HighContrast => {
                block(scene, &pill, 0x000000, 0xffffff, 1.5);
                block(scene, &card_shape, 0x000000, 0xffd60a, 2.0);
                block(scene, &dot, 0x000000, 0xffffff, 2.0);
            }
        }
        scene.pop();
        // Chosen: ringed in the accent; else a hairline.
        let theme = theme();
        if self.chosen {
            scene.stroke_with(
                &RoundedRect::from_rect(bounds.inflate(1.0, 1.0), 11.0).to_path(0.1),
                &Stroke::new(2.0),
                theme.color(Token::AccentInk),
            );
        } else {
            scene.stroke_with(
                &RoundedRect::from_rect(bounds.inset(-0.5), 9.5).to_path(0.1),
                &Stroke::new(1.0),
                hex(0xe4e4e7),
            );
        }
    }
}

/// A notice above the settings (saving failed, the file was unreadable).
fn notice_box(ui: &mut Ui, text: &str) -> WidgetId {
    let icon = ui.add(Icon::named("triangle-alert", 16.0, Token::OnWarningSoft));
    let label = ui.add(
        Label::new(text, TextStyle::new(13.0))
            .color(Token::OnWarningSoft)
            .grow(1.0),
    );
    ui.add_with_children(
        Flex::row()
            .gap(10.0)
            .padding(12.0, 10.0)
            .radius(10.0)
            .background(Token::WarningSoft)
            .role(Role::Alert, text)
            .style(|style| {
                style.margin.bottom = taffy::LengthPercentageAuto::length(16.0);
            }),
        &[icon, label],
    )
}

/// What a search looks through: the title, description, synonyms and key.
pub(crate) fn searchable(key: &str, meta: &SettingMeta) -> String {
    format!(
        "{} {} {} {}",
        meta.title,
        meta.description,
        meta.synonyms.join(" "),
        key
    )
}

/// The values a dropdown offers: an enum's variants, or the steps of a
/// small numeric range (at most 16). Empty if the setting takes typing.
pub(crate) fn choices(declaration: &dyn AnySetting) -> Vec<(Value, String)> {
    match declaration.kind() {
        ValueKind::Enum => declaration
            .variants()
            .into_iter()
            .map(|variant| (Value::Enum(variant.id.to_owned()), variant.label.to_owned()))
            .collect(),
        kind @ (ValueKind::Int | ValueKind::Float) => {
            let Constraint::Range {
                min,
                max,
                step: Some(step),
            } = declaration.constraint()
            else {
                return Vec::new();
            };
            if step <= 0.0 || (max - min) / step > 15.0 + 1e-9 {
                return Vec::new();
            }
            // Whole steps that fit (a last one past `max` is left out), on
            // the step's decimals: 0.1 · 4 is 0.4, not 0.40000000000000002.
            let count = ((max - min) / step + 1e-9).floor() as i64;
            let decimals = format!("{step}")
                .split_once('.')
                .map_or(0, |(_, fraction)| fraction.len()) as i32;
            let scale = 10f64.powi(decimals);
            (0..=count)
                .map(|at| {
                    let value = ((min + step * at as f64) * scale).round() / scale;
                    let value = value.min(max);
                    let value = if kind == ValueKind::Int {
                        Value::Int(value.round() as i64)
                    } else {
                        Value::Float(value)
                    };
                    // The interface size reads as a percentage (the mockup's
                    // "110 %").
                    let name = if declaration.key() == crate::look::SCALE.key() {
                        format!("{:.0} %", value_number(&value) * 100.0)
                    } else {
                        format_value(&value)
                    };
                    (value, name)
                })
                .collect()
        }
        _ => Vec::new(),
    }
}

/// A value as its field shows it.
pub(crate) fn format_value(value: &Value) -> String {
    match value {
        Value::Bool(on) => on.to_string(),
        Value::Int(n) => n.to_string(),
        // Exactly: a field shows what it would store again.
        Value::Float(x) => format!("{x}"),
        Value::String(text) | Value::Enum(text) => text.clone(),
        Value::Color(color) => color.to_string(),
        Value::Path(path) => path.display().to_string(),
        Value::SecretRef(secret) => secret.id.clone(),
        Value::List(items) => items
            .iter()
            .map(format_value)
            .collect::<Vec<_>>()
            .join(", "),
    }
}

/// Typed `text` as a value of `declaration`'s kind, or why it isn't one.
pub(crate) fn parse(declaration: &dyn AnySetting, text: &str) -> Result<Value, String> {
    let one = |kind: ValueKind, raw: &str| -> Result<Value, String> {
        // Text and paths are kept as typed; numbers and the like may have
        // spaces around them.
        let text = raw.trim();
        match kind {
            ValueKind::Bool => match text.to_lowercase().as_str() {
                "true" | "on" | "yes" => Ok(Value::Bool(true)),
                "false" | "off" | "no" => Ok(Value::Bool(false)),
                _ => Err("Type on or off.".to_owned()),
            },
            ValueKind::Int => text
                .parse::<i64>()
                .map(Value::Int)
                .map_err(|_| "Type a whole number.".to_owned()),
            ValueKind::Float => text
                .replace(',', ".")
                .parse::<f64>()
                .ok()
                .filter(|x| x.is_finite())
                .map(Value::Float)
                .ok_or_else(|| "Type a number.".to_owned()),
            ValueKind::String => Ok(Value::String(raw.to_owned())),
            ValueKind::Enum => Ok(Value::Enum(text.to_owned())),
            ValueKind::Color => text
                .parse::<Rgba>()
                .map(Value::Color)
                .map_err(|_| "Type a color like #3a4bd6.".to_owned()),
            ValueKind::Path => Ok(Value::Path(raw.into())),
            ValueKind::SecretRef | ValueKind::List(_) => {
                Err("This can't be typed here.".to_owned())
            }
        }
    };
    let value = match declaration.kind() {
        ValueKind::List(element) => {
            let kind = match element {
                ListKind::Bool => ValueKind::Bool,
                ListKind::Int => ValueKind::Int,
                ListKind::Float => ValueKind::Float,
                ListKind::String => ValueKind::String,
                ListKind::Enum => ValueKind::Enum,
                ListKind::Color => ValueKind::Color,
                ListKind::Path => ValueKind::Path,
            };
            let items = text
                .split(',')
                .map(str::trim)
                .filter(|item| !item.is_empty())
                .map(|item| one(kind, item))
                .collect::<Result<Vec<_>, _>>()?;
            Value::List(items)
        }
        kind => one(kind, text)?,
    };
    // The limits in words, not as the registry writes them (`5..=600`).
    let number = |x: f64| format_value(&Value::Float(x));
    match (declaration.constraint(), &value) {
        (Constraint::Range { min, max, .. }, Value::Int(n))
            if !(min..=max).contains(&(*n as f64)) =>
        {
            return Err(format!(
                "Must be between {} and {}.",
                number(min),
                number(max)
            ));
        }
        (Constraint::Range { min, max, .. }, Value::Float(x)) if !(min..=max).contains(x) => {
            return Err(format!(
                "Must be between {} and {}.",
                number(min),
                number(max)
            ));
        }
        (Constraint::MaxLen(max), Value::String(text)) if text.chars().count() > max => {
            return Err(format!("At most {max} characters."));
        }
        _ => {}
    }
    declaration
        .validate(&value)
        .map(|()| value)
        .map_err(|err| reason_of(&err.to_string()))
}

/// The user-facing part of a validation error: what follows the key
/// ("invalid `files.x`: not a folder" → "Not a folder.").
fn reason_of(error: &str) -> String {
    let reason = error
        .split_once(": ")
        .map_or(error, |(_, reason)| reason)
        .trim();
    let mut chars = reason.chars();
    let mut text: String = chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default();
    if !text.ends_with('.') {
        text.push('.');
    }
    text
}

/// Whether two values are the same, numbers within rounding.
fn same(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Float(a), Value::Float(b)) => (a - b).abs() <= 1e-9 * a.abs().max(b.abs()).max(1.0),
        (a, b) => a == b,
    }
}

fn color_of(color: Rgba) -> Color {
    Color::from_rgba8(color.r, color.g, color.b, color.a)
}

impl View for SettingsWindow {
    fn paint(&mut self, scene: &mut Scene, size: Size) {
        self.handle_asks();
        let area = Rect::from_origin_size((0.0, 0.0), size);
        self.ui.layout(&mut self.fonts, area);
        if self.restore.is_some() {
            self.restore_focus();
            self.ui.layout(&mut self.fonts, area);
        }
        self.ui.paint(&mut self.fonts, scene);
    }

    fn input(&mut self, input: &Input) -> bool {
        // Ctrl+W closes it, as it closes a board.
        if let Input::Key {
            key,
            pressed: true,
            repeat: false,
            modifiers,
            ..
        } = input
            && let Some(press) = imaginarium_canvas::keys::key_press(key, *modifiers)
            && crate::commands::shell_keymap(Platform::current())
                .command_for_press(press)
                .is_some_and(|command| command.as_str() == "window.close")
        {
            self.requests.push(WindowRequest::Close);
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

impl Drop for SettingsWindow {
    fn drop(&mut self) {
        if let Ok(mut settings) = self.app.settings().try_borrow_mut() {
            settings.unsubscribe(self.subscription);
        }
        self.app.settings_closed();
    }
}

/// The registry's settings, for tests.
#[cfg(test)]
pub(crate) fn sections_of(
    registry: &imaginarium_settings::Registry,
) -> Vec<(&'static str, Section)> {
    registry
        .declarations()
        .map(|declaration| (declaration.key(), Section::of(&declaration.meta())))
        .collect()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use imaginarium_settings::{Scope, Setting};

    use super::*;

    static SECONDS: Setting<i64> = Setting::new(
        "files.autosave_seconds",
        || 30,
        Scope::App,
        "Autosave interval",
        "How often to save.",
    )
    .range(5, 600);
    static SIZE: Setting<f64> = Setting::new(
        "ui.size_for_tests",
        || 1.0,
        Scope::App,
        "Interface size",
        "Scales panels.",
    )
    .range(0.8, 2.0, 0.1);
    static TAGS: Setting<Vec<String>> =
        Setting::new("files.tags", Vec::new, Scope::App, "Tags", "Default tags.");
    static DEBUG: Setting<bool> = Setting::new(
        "advanced.debug_overlay",
        || false,
        Scope::App,
        "Frame timings",
        "Show frame timings.",
    )
    .advanced();

    #[test]
    fn typed_values_are_parsed_for_their_kind_and_checked() {
        assert_eq!(parse(&SECONDS, " 60 "), Ok(Value::Int(60)));
        assert_eq!(
            parse(&SECONDS, "1"),
            Err("Must be between 5 and 600.".to_owned())
        );
        assert!(parse(&SECONDS, "soon").is_err());
        assert_eq!(parse(&SIZE, "1,5"), Ok(Value::Float(1.5)));
        assert_eq!(
            parse(&TAGS, "work, draft,"),
            Ok(Value::List(vec![
                Value::String("work".into()),
                Value::String("draft".into())
            ]))
        );
    }

    #[test]
    fn small_ranges_with_a_step_are_dropdowns() {
        let sizes = choices(&SIZE);
        assert_eq!(sizes.len(), 13);
        assert_eq!(sizes[2].1, "1");
        // Clean steps: 1.2, not 1.2000000000000002, and none past the end.
        assert_eq!(sizes[4].0, Value::Float(1.2));
        assert_eq!(sizes[12].0, Value::Float(2.0));
        for (value, _) in &sizes {
            assert!(SIZE.validate(value).is_ok(), "{value:?}");
        }
        // A wide range (or one without a step) is typed.
        assert!(choices(&SECONDS).is_empty());
    }

    #[test]
    fn settings_go_to_their_sections() {
        assert_eq!(Section::of(&SIZE.meta()), Section::Appearance);
        assert_eq!(Section::of(&SECONDS.meta()), Section::Files);
        assert_eq!(Section::of(&DEBUG.meta()), Section::Advanced);
        let mut registry = imaginarium_settings::Registry::new();
        registry.register_all(crate::SETTINGS).unwrap();
        registry.register_all(imaginarium_canvas::SETTINGS).unwrap();
        assert!(
            sections_of(&registry)
                .iter()
                .all(|(_, section)| *section != Section::Shortcuts)
        );
    }
}
