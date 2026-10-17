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
        if self.changed.swap(false, Ordering::Relaxed) {
            rebuild = true;
        }
        let notice = self.app.save_notice();
        if notice != self.notice {
            rebuild = true;
        }
        if rebuild {
            self.rebuild();
        }
        any || rebuild
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
            Ask::Typed(key, text) => {
                let settings = self.app.settings().clone();
                let declaration = settings.borrow().declaration(key);
                let Ok(declaration) = declaration else {
                    return false;
                };
                match parse(declaration, &text) {
                    Ok(value) => self.set(key, value),
                    Err(reason) => {
                        self.errors.insert(key, reason);
                    }
                }
                true
            }
            Ask::Reset(key) => {
                self.errors.remove(key);
                let _ = self.app.settings().borrow_mut().reset(key);
                true
            }
            Ask::Choices(key, anchor) => {
                self.open_choices(key, anchor);
                false
            }
        }
    }

    fn set(&mut self, key: &'static str, value: Value) {
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
        let settings = self.app.settings().clone();
        let settings = settings.borrow();
        let (Ok(declaration), Ok(current)) = (settings.declaration(key), settings.value(key))
        else {
            return;
        };
        let rows: Vec<WidgetId> = choices(declaration)
            .into_iter()
            .map(|(value, name)| {
                let checked = value == current;
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
                .role(Role::MenuItem)
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
        let focused = self.focused_label();
        self.notice = self.app.save_notice();
        let searching = !self.query.trim().is_empty();
        let buttons: Vec<WidgetId> = SECTIONS
            .iter()
            .map(|&section| self.section_button(section, !searching && section == self.section))
            .collect();
        self.ui.set_children(self.sidebar, &buttons);
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
        if let Some(label) = focused
            && let Some(widget) = self.ui.find_by_label(&mut self.fonts, &label)
        {
            self.ui.set_focus(&mut self.fonts, Some(widget));
        }
    }

    /// The access label of the focused control, unless it is the search.
    fn focused_label(&mut self) -> Option<String> {
        let focused = self.ui.focused()?;
        if focused == self.search {
            return None;
        }
        let mut tree = AccessTree::default();
        self.ui.access(&mut self.fonts, &mut tree);
        let base = focused.access_base();
        tree.nodes()
            .iter()
            .find(|(id, _)| *id == accesskit::NodeId(base))
            .and_then(|(_, node)| node.label().map(str::to_owned))
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
        let mut children = vec![self.header(section.title(), "Changes apply right away.")];
        let declarations: Vec<&'static dyn AnySetting> = self
            .app
            .settings()
            .borrow()
            .declarations()
            .filter(|declaration| Section::of(&declaration.meta()) == section)
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
        self.ui.add_with_children(
            Flex::column().gap(4.0).style(|style| {
                style.padding.bottom = length(18.0);
            }),
            &[title, subtitle],
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
        let meta = declaration.meta();
        let modified = self.app.settings().borrow().is_modified(key);
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
        if let Some(error) = self.errors.get(key).cloned() {
            text.push(
                self.ui
                    .add(Label::new(error, TextStyle::new(12.0)).color(Token::Danger)),
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
        controls.push(self.editor(declaration));
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

    /// The control that edits `declaration`, chosen by its kind.
    fn editor(&mut self, declaration: &'static dyn AnySetting) -> WidgetId {
        let key = declaration.key();
        let meta = declaration.meta();
        let value = self
            .app
            .settings()
            .borrow()
            .value(key)
            .unwrap_or_else(|_| declaration.default_dyn());
        let asks = self.asks.clone();
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
            .find(|(choice, _)| choice == value)
            .map_or_else(|| format_value(value), |(_, name)| name);
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
        let input = self.ui.add(
            TextInput::new(label, "", TextStyle::new(13.0), InputColors::themed())
                .on_submit(move |text| {
                    submit
                        .borrow_mut()
                        .push(Ask::Typed(key, text.trim().to_owned()));
                })
                .on_blur(move |text| {
                    blur.borrow_mut()
                        .push(Ask::Typed(key, text.trim().to_owned()));
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
            let keys: Vec<String> = keymaps
                .iter()
                .filter_map(|keymap| crate::toolbar::shortcut(keymap, id))
                .collect();
            let Some(keys) = keys.first().cloned() else {
                continue;
            };
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
            let count = ((max - min) / step).round() as i64;
            (0..=count)
                .map(|at| {
                    let value = min + step * at as f64;
                    let value = if kind == ValueKind::Int {
                        Value::Int(value.round() as i64)
                    } else {
                        Value::Float(value)
                    };
                    let name = format_value(&value);
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
        Value::Float(x) => {
            let text = format!("{x:.3}");
            text.trim_end_matches('0').trim_end_matches('.').to_owned()
        }
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
    let one = |kind: ValueKind, text: &str| -> Result<Value, String> {
        let text = text.trim();
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
            ValueKind::String => Ok(Value::String(text.to_owned())),
            ValueKind::Enum => Ok(Value::Enum(text.to_owned())),
            ValueKind::Color => text
                .parse::<Rgba>()
                .map(Value::Color)
                .map_err(|_| "Type a color like #3a4bd6.".to_owned()),
            ValueKind::Path => Ok(Value::Path(text.into())),
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
    declaration
        .validate(&value)
        .map(|()| value)
        .map_err(|err| reason_of(&err.to_string()))
}

/// The user-facing part of a validation error ("`files.x`: must be at
/// most 600" → "Must be at most 600.").
fn reason_of(error: &str) -> String {
    let reason = error.rsplit(": ").next().unwrap_or(error).trim();
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

fn color_of(color: Rgba) -> Color {
    Color::from_rgba8(color.r, color.g, color.b, color.a)
}

impl View for SettingsWindow {
    fn paint(&mut self, scene: &mut Scene, size: Size) {
        self.handle_asks();
        self.ui
            .layout(&mut self.fonts, Rect::from_origin_size((0.0, 0.0), size));
        self.ui.paint(&mut self.fonts, scene);
    }

    fn input(&mut self, input: &Input) -> bool {
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
        "ui.scale",
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
        let below = parse(&SECONDS, "1").unwrap_err();
        assert!(below.contains('5') && below.ends_with('.'), "{below}");
        assert!(!below.contains("files.autosave_seconds"), "{below}");
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
        assert!(
            sections_of(&registry)
                .iter()
                .all(|(_, section)| *section != Section::Shortcuts)
        );
    }
}
