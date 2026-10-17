// SPDX-License-Identifier: AGPL-3.0-or-later

//! The command palette (`shell.md` §3.7, the Palette mockups): every
//! command the board and the shell have, and every setting, found by name.
//! `Ctrl+K` or the Search button opens it; ↑↓ move, Enter runs, Tab
//! changes a setting's value, Esc closes it.

use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;

use accesskit::Role;
use imaginarium_commands::{Availability, CommandId, Keymap, SearchIndex, UsageStats};
use imaginarium_reactive::{Scope, Signal, signal};
use imaginarium_render::kurbo::{Point, Rect};
use imaginarium_settings::{Registry, Value, ValueKind};
use imaginarium_text::TextStyle;
use imaginarium_ui::{
    Anchor, ButtonBox, ButtonColors, Cx, Flex, Icon, Input, InputColors, Key, Label, LayerId,
    LayerOptions, Placement, Scroll, Switch, TextInput, Token, Ui, Widget, WidgetId, label,
};
use taffy::prelude::length;

use crate::toolbar::shortcut;
use crate::window::{Ask, Asks};

/// How many results a query lists.
const LIMIT: usize = 40;

/// What a palette entry is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    /// A command without required arguments.
    Command,
    /// A setting, changed in place.
    Setting,
}

/// One thing the palette finds.
#[derive(Clone, Debug)]
pub(crate) struct Item {
    pub(crate) kind: Kind,
    pub(crate) id: &'static str,
    title: &'static str,
    description: &'static str,
    /// The heading it is listed under.
    group: &'static str,
    synonyms: &'static [&'static str],
    icon: Option<&'static str>,
}

/// Everything the palette can find, indexed once per window.
pub(crate) struct Catalog {
    items: Vec<Item>,
    by_id: HashMap<&'static str, usize>,
    index: SearchIndex,
}

/// The parts of a command the catalog lists: its id, metadata and whether
/// it needs arguments the palette can't give.
pub(crate) struct Listed {
    pub(crate) id: &'static str,
    pub(crate) meta: imaginarium_commands::CommandMeta,
    pub(crate) needs_args: bool,
}

impl Catalog {
    /// The catalog of `commands` (those that run without arguments; the
    /// others are for the AI and the panels, which give them) and the
    /// settings in `settings`.
    pub(crate) fn new(commands: impl IntoIterator<Item = Listed>, settings: &Registry) -> Self {
        let mut items = Vec::new();
        for command in commands {
            if command.needs_args || command.id == "palette.open" {
                continue;
            }
            let meta = command.meta;
            items.push(Item {
                kind: Kind::Command,
                id: command.id,
                title: meta.title,
                description: meta.description,
                group: if meta.category.is_empty() {
                    "Commands"
                } else {
                    meta.category
                },
                synonyms: meta.synonyms,
                icon: meta.icon,
            });
        }
        let mut declared: Vec<_> = settings
            .declarations()
            .filter(|setting| !setting.meta().advanced)
            .filter(|setting| matches!(setting.kind(), ValueKind::Bool | ValueKind::Enum))
            .collect();
        declared.sort_by_key(|setting| setting.key());
        for setting in declared {
            let meta = setting.meta();
            items.push(Item {
                kind: Kind::Setting,
                id: setting.key(),
                title: meta.title,
                description: meta.description,
                group: "Settings",
                synonyms: meta.synonyms,
                icon: Some("settings"),
            });
        }
        let mut index = SearchIndex::new();
        let mut by_id = HashMap::new();
        for (at, item) in items.iter().enumerate() {
            index.insert(
                CommandId::new(item.id),
                item.title,
                item.synonyms,
                item.group,
            );
            by_id.insert(item.id, at);
        }
        Self {
            items,
            by_id,
            index,
        }
    }

    /// What item `at` is, and its id.
    pub(crate) fn item(&self, at: usize) -> (Kind, &'static str) {
        let item = &self.items[at];
        (item.kind, item.id)
    }
}

/// A row the palette built for an item, reused for every query while it
/// is open: only its highlights and its place change.
struct Row {
    widget: WidgetId,
    title: WidgetId,
    /// "· also <synonym>" when a synonym matched, apart from the
    /// description so the description clips first.
    also: WidgetId,
    /// Its place among the rows listed now (what Enter and a click run).
    position: Rc<Cell<usize>>,
    value: Option<Signal<String>>,
    on: Option<Signal<bool>>,
}

/// An open palette.
pub(crate) struct Palette {
    pub(crate) layer: LayerId,
    /// The palette's box: a click outside it closes it.
    pub(crate) panel: WidgetId,
    field: WidgetId,
    list: WidgetId,
    scroll: WidgetId,
    /// Owns the rows' signals, disposed when the palette closes.
    scope: Scope,
    /// The rows built so far, by item, and the headings by name.
    rows: HashMap<usize, Row>,
    headings: HashMap<&'static str, WidgetId>,
    nothing: Option<WidgetId>,
    /// The items listed now, in their order.
    listed: Vec<usize>,
    active: Signal<usize>,
    /// The commands' availability when it opened (`Registry::availability`
    /// is a snapshot by design: cheap checks, taken once).
    availability: HashMap<&'static str, Availability>,
    query: String,
    size: (f64, f64),
}

/// Keys the palette's field passes on, for the palette to move and run.
const PASSED: [Key; 5] = [
    Key::ArrowUp,
    Key::ArrowDown,
    Key::Enter,
    Key::Tab,
    Key::Escape,
];

/// Takes ↑↓, Enter and Tab from the field, as they bubble up.
struct PaletteKeys {
    asks: Asks,
}

impl Widget for PaletteKeys {
    fn style(&self) -> taffy::Style {
        taffy::Style {
            flex_direction: taffy::FlexDirection::Column,
            ..taffy::Style::default()
        }
    }

    fn event(&mut self, _cx: &mut Cx<'_>, input: &Input) -> bool {
        let Input::Key {
            key,
            pressed: true,
            modifiers,
            ..
        } = input
        else {
            return false;
        };
        let ask = match key {
            Key::ArrowUp => Ask::PaletteMove(-1),
            Key::ArrowDown => Ask::PaletteMove(1),
            Key::Enter => Ask::PaletteRun(None),
            Key::Tab => Ask::PaletteTab(!modifiers.shift),
            _ => return false,
        };
        self.asks.borrow_mut().push(ask);
        true
    }
}

/// What a palette needs from the window to fill its rows.
pub(crate) struct Sources<'a> {
    pub(crate) catalog: &'a Catalog,
    pub(crate) settings: &'a Registry,
    pub(crate) keymaps: [&'a Keymap; 2],
    pub(crate) usage: &'a UsageStats,
    pub(crate) now: u64,
    pub(crate) asks: &'a Asks,
}

impl Palette {
    /// Opens the palette over a window `width` × `height`, with the
    /// commands' `availability` now.
    pub(crate) fn open(
        ui: &mut Ui,
        width: f64,
        height: f64,
        availability: HashMap<&'static str, Availability>,
        sources: &Sources<'_>,
    ) -> Self {
        let asks = sources.asks;
        let icon = ui.add(Icon::named("search", 18.0, Token::TextSubtle));
        let input = ui.add(
            TextInput::new(
                "Search commands, settings and help",
                "Search commands and settings",
                TextStyle::new(16.0),
                InputColors::themed(),
            )
            .bare()
            .passing(PASSED)
            .on_change({
                let asks = asks.clone();
                move |text| asks.borrow_mut().push(Ask::PaletteQuery(text.to_owned()))
            }),
        );
        let field = ui.add_with_children(Flex::row().grow(1.0), &[input]);
        let esc = chip(ui, "Esc");
        let header = ui.add_with_children(
            Flex::row().height(56.0).gap(12.0).padding(16.0, 0.0),
            &[icon, field, esc],
        );
        let line = ui.add(Flex::row().height(1.0).background(Token::Divider));
        let list = ui.add(
            Flex::column()
                .gap(2.0)
                .role(Role::ListBox, "Results")
                .style(|style| {
                    style.padding = taffy::Rect {
                        left: length(8.0),
                        right: length(8.0),
                        top: length(6.0),
                        bottom: length(8.0),
                    };
                }),
        );
        let room = room(height);
        let scroll = ui.add_with_children(
            Scroll::new()
                .label("Results")
                .style(|style| style.max_size.height = length(room as f32)),
            &[list],
        );
        let footer_line = ui.add(Flex::row().height(1.0).background(Token::Divider));
        let footer = footer(ui);
        let panel = ui.add_with_children(
            Flex::column()
                .width(640.0)
                .background(Token::PanelBg)
                .border(1.0, Token::PanelBorder)
                .radius(16.0)
                .shadow()
                .clip()
                .role(Role::Dialog, "Command palette"),
            &[header, line, scroll, footer_line, footer],
        );
        let content = ui.add_with_children(PaletteKeys { asks: asks.clone() }, &[panel]);
        let on_close = asks.clone();
        let options = LayerOptions::modal()
            .anchored(anchor(width))
            .placement(Placement::Below)
            .gap(0.0)
            .scrim(0.16)
            .focus(input)
            .on_close(move || on_close.borrow_mut().push(Ask::PaletteClosed));
        let layer = ui.open_layer(content, options);
        let scope = Scope::new();
        let active = scope.run(|| signal(0));
        let mut palette = Self {
            layer,
            panel,
            field: input,
            list,
            scroll,
            scope,
            rows: HashMap::new(),
            headings: HashMap::new(),
            nothing: None,
            listed: Vec::new(),
            active,
            availability,
            query: String::new(),
            size: (width, height),
        };
        palette.fill(ui, sources);
        palette
    }

    /// The search field.
    pub(crate) fn field(&self) -> WidgetId {
        self.field
    }

    /// Lists what `query` finds.
    pub(crate) fn search(&mut self, ui: &mut Ui, query: &str, sources: &Sources<'_>) {
        self.query = query.to_owned();
        self.fill(ui, sources);
    }

    /// Places it for a window `width` × `height` (after a resize).
    pub(crate) fn resize(&mut self, ui: &mut Ui, width: f64, height: f64) {
        if self.size == (width, height) {
            return;
        }
        self.size = (width, height);
        ui.move_layer(self.layer, anchor(width));
        if let Some(scroll) = ui.widget_mut::<Scroll>(self.scroll) {
            scroll.style_mut().max_size.height = length(room(height) as f32);
        }
    }

    /// Removes its rows (also those not listed now) and disposes their
    /// signals; the layer itself is closed by then.
    pub(crate) fn dispose(self, ui: &mut Ui) {
        for row in self.rows.values() {
            ui.remove(row.widget);
        }
        for heading in self.headings.values() {
            ui.remove(*heading);
        }
        if let Some(nothing) = self.nothing {
            ui.remove(nothing);
        }
        self.scope.dispose();
    }

    /// Moves the active row by `by`, wrapping, and brings it into view.
    pub(crate) fn step(&mut self, by: i32) -> Option<WidgetId> {
        let count = self.listed.len();
        if count == 0 {
            return None;
        }
        let at = self.active.get_untracked() as i64 + i64::from(by);
        let at = at.rem_euclid(count as i64) as usize;
        self.active.set(at);
        let item = self.listed.get(at)?;
        self.rows.get(item).map(|row| row.widget)
    }

    /// The item of the active row (or of row `row`), if any.
    pub(crate) fn item(&self, row: Option<usize>) -> Option<usize> {
        let row = row.unwrap_or_else(|| self.active.get_untracked());
        self.listed.get(row).copied()
    }

    /// Whether the command `id` can run now.
    pub(crate) fn available(&self, id: &str) -> bool {
        self.availability
            .get(id)
            .is_none_or(|availability| availability.is_enabled())
    }

    /// Shows a setting's new value in its row.
    pub(crate) fn show_value(&self, item: usize, settings: &Registry, catalog: &Catalog) {
        let Some(row) = self.rows.get(&item) else {
            return;
        };
        let id = catalog.items[item].id;
        if let Some(on) = row.on {
            on.set(matches!(settings.value(id), Ok(Value::Bool(true))));
        }
        if let Some(value) = row.value {
            value.set(value_label(settings, id));
        }
    }

    fn fill(&mut self, ui: &mut Ui, sources: &Sources<'_>) {
        let catalog = sources.catalog;
        let query = self.query.trim().to_owned();
        // Which items, in which order: for a query the search's ranking;
        // without one, the recent ones first, then everything.
        let hits = catalog
            .index
            .search(&query, sources.usage, sources.now, LIMIT);
        let found = hits
            .iter()
            .filter_map(|hit| catalog.by_id.get(hit.id.as_str()).copied());
        let mut order: Vec<(usize, &'static str)> = if query.is_empty() {
            let recent: Vec<usize> = found.take(5).collect();
            let rest = (0..catalog.items.len()).filter(|at| !recent.contains(at));
            recent
                .iter()
                .map(|&at| (at, "Recent"))
                .chain(rest.map(|at| (at, catalog.items[at].group)))
                .collect()
        } else {
            found.map(|at| (at, catalog.items[at].group)).collect()
        };
        order.retain(|(at, _)| {
            let item = &catalog.items[*at];
            item.kind == Kind::Setting
                || !matches!(self.availability.get(item.id), Some(Availability::Hidden))
        });
        // Grouped under their headings, the groups in the order their best
        // item comes.
        let mut groups: Vec<(&'static str, Vec<usize>)> = Vec::new();
        for (at, group) in order {
            match groups.iter_mut().find(|(name, _)| *name == group) {
                Some((_, items)) => items.push(at),
                None => groups.push((group, vec![at])),
            }
        }
        let mut children = Vec::new();
        self.listed.clear();
        if groups.is_empty() {
            let nothing = match self.nothing {
                Some(nothing) => nothing,
                None => {
                    let text = ui.add(
                        Label::new("Nothing found", TextStyle::new(13.0)).color(Token::TextMuted),
                    );
                    ui.add_with_children(Flex::row().padding(8.0, 16.0), &[text])
                }
            };
            self.nothing = Some(nothing);
            children.push(nothing);
        }
        for (name, items) in groups {
            let heading = match self.headings.get(name) {
                Some(heading) => *heading,
                None => {
                    let text = ui.add(
                        Label::new(name, TextStyle::new(12.0).weight(600.0))
                            .color(Token::TextSubtle)
                            .single_line(),
                    );
                    let heading = ui.add_with_children(
                        Flex::row().style(|style| {
                            style.padding = taffy::Rect {
                                left: length(8.0),
                                right: length(8.0),
                                top: length(9.0),
                                bottom: length(4.0),
                            };
                        }),
                        &[text],
                    );
                    self.headings.insert(name, heading);
                    heading
                }
            };
            children.push(heading);
            for at in items {
                let position = self.listed.len();
                let row = self.row(ui, at, sources);
                self.mark(ui, at, &query, catalog);
                if let Some(built) = self.rows.get(&at) {
                    built.position.set(position);
                }
                self.listed.push(at);
                children.push(row);
            }
        }
        // One replacement of the list: rows not listed now stay built,
        // detached, for the next query.
        ui.set_children(self.list, &children);
        // Every row repaints (its place may have changed); the list starts
        // at the top again, with the first row active.
        self.active.set(0);
        if let Some(scroll) = ui.widget_mut::<Scroll>(self.scroll) {
            scroll.set_offset(0.0);
        }
    }

    /// Highlights what `query` matched in item `at`'s row.
    fn mark(&mut self, ui: &mut Ui, at: usize, query: &str, catalog: &Catalog) {
        let Some(row) = self.rows.get(&at) else {
            return;
        };
        let item = &catalog.items[at];
        let explanation = if query.is_empty() {
            None
        } else {
            catalog.index.explain(CommandId::new(item.id), query)
        };
        let title_marks = explanation
            .as_ref()
            .map(|explained| byte_ranges(item.title, &explained.title))
            .unwrap_or_default();
        if let Some(title) = ui.widget_mut::<Label>(row.title) {
            title.set_highlights(title_marks);
        }
        let disabled = matches!(
            self.availability.get(item.id),
            Some(Availability::Disabled(_))
        );
        let (also, marks) = match explanation.and_then(|explained| explained.synonym) {
            Some((synonym, at)) if !disabled => {
                let synonym = item.synonyms.get(synonym).copied().unwrap_or("");
                let start = " · also ";
                let marks = byte_ranges(synonym, &at)
                    .into_iter()
                    .map(|range| range.start + start.len()..range.end + start.len())
                    .collect();
                (format!("{start}{synonym}"), marks)
            }
            _ => (String::new(), Vec::new()),
        };
        if let Some(label) = ui.widget_mut::<Label>(row.also) {
            label.set_text(also);
            label.set_highlights(marks);
        }
    }

    /// The row of item `at`, built the first time it is listed.
    fn row(&mut self, ui: &mut Ui, at: usize, sources: &Sources<'_>) -> WidgetId {
        if let Some(row) = self.rows.get(&at) {
            return row.widget;
        }
        let catalog = sources.catalog;
        let item = &catalog.items[at];
        let enabled = item.kind == Kind::Setting || self.available(item.id);
        let ink = if enabled {
            Token::Text
        } else {
            Token::TextSubtle
        };
        // An add-on may name an icon we don't have: a plain chevron then.
        let icon_name = item
            .icon
            .filter(|name| imaginarium_ui::icons::icon_paths(name).is_some())
            .unwrap_or("chevron-right");
        let icon = ui.add(Icon::named(icon_name, 16.0, Token::TextSecondary));
        let tile = ui.add_with_children(
            Flex::row()
                .width(32.0)
                .height(32.0)
                .radius(8.0)
                .background(Token::PanelBg)
                .border(1.0, Token::ControlBorder)
                .style(|style| {
                    style.justify_content = taffy::JustifyContent::CENTER;
                    style.flex_shrink = 0.0;
                }),
            &[icon],
        );
        let title = ui.add(
            Label::new(item.title, TextStyle::new(14.0).weight(500.0))
                .color(ink)
                .single_line()
                .highlight(Vec::new(), 600.0, Token::AccentInk),
        );
        // The second line: why it can't run, or what it does; then the
        // synonym that matched, which stays when the line is too long.
        let reason = match self.availability.get(item.id) {
            Some(Availability::Disabled(reason)) => Some(*reason),
            _ => None,
        };
        let line = reason.map_or_else(
            || first_sentence(item.description).to_owned(),
            str::to_owned,
        );
        let description = ui.add(
            Label::new(line, TextStyle::new(12.0))
                .color(Token::TextMuted)
                .single_line(),
        );
        let description = ui.add_with_children(
            Flex::row().clip().style(|style| {
                style.flex_shrink = 1.0;
                style.min_size.width = length(0.0);
            }),
            &[description],
        );
        let also = ui.add(
            Label::new("", TextStyle::new(12.0))
                .color(Token::TextMuted)
                .single_line()
                .highlight(Vec::new(), 600.0, Token::AccentInk),
        );
        let second = ui.add_with_children(Flex::row(), &[description, also]);
        let texts = ui.add_with_children(
            Flex::column()
                .gap(2.0)
                .grow(1.0)
                .clip()
                .style(|style| style.min_size.width = length(0.0)),
            &[title, second],
        );
        let position = Rc::new(Cell::new(0));
        let mut children = vec![tile, texts];
        let mut value = None;
        let mut on = None;
        match item.kind {
            Kind::Setting => match sources.settings.value(item.id) {
                Ok(Value::Bool(current)) => {
                    let state = self.scope.run(|| signal(current));
                    on = Some(state);
                    let asks = sources.asks.clone();
                    let place = position.clone();
                    let switch = Switch::new(
                        item.title,
                        move || state.get(),
                        move || {
                            asks.borrow_mut().push(Ask::PaletteRun(Some(place.get())));
                        },
                    )
                    .add_to(ui);
                    children.push(switch);
                }
                Ok(_) => {
                    let shown = self
                        .scope
                        .run(|| signal(value_label(sources.settings, item.id)));
                    value = Some(shown);
                    let text = label(ui, TextStyle::new(12.0), Token::Text, move || shown.get());
                    let chip = ui.add_with_children(
                        Flex::row()
                            .height(26.0)
                            .padding(8.0, 0.0)
                            .radius(7.0)
                            .background(Token::PanelBg)
                            .border(1.0, Token::ControlBorder)
                            .style(|style| style.flex_shrink = 0.0),
                        &[text],
                    );
                    children.push(chip);
                    children.push(self::chip(ui, "Tab"));
                }
                Err(_) => {}
            },
            Kind::Command => {
                let keys = sources
                    .keymaps
                    .iter()
                    .find_map(|keymap| shortcut(keymap, item.id));
                if let Some(keys) = keys {
                    children.push(self::chip(ui, &keys));
                }
            }
        }
        let active = self.active;
        let asks = sources.asks.clone();
        let label_text = match (reason, item.kind) {
            (Some(reason), _) => format!("{}, unavailable: {reason}", item.title),
            (None, Kind::Setting) => format!("{}, setting", item.title),
            (None, Kind::Command) => item.title.to_owned(),
        };
        let place = position.clone();
        let mine = position.clone();
        let widget = ButtonBox::new(label_text, ButtonColors::quiet(), move || {
            asks.borrow_mut().push(Ask::PaletteRun(Some(place.get())));
        })
        .height(52.0)
        .padding(8.0, 8.0)
        .gap(12.0)
        .start()
        .radius(10.0)
        .keep_focus()
        .selected(move || active.get() == mine.get())
        .add_to(ui, &children);
        self.rows.insert(
            at,
            Row {
                widget,
                title,
                also,
                position,
                value,
                on,
            },
        );
        widget
    }
}

/// Where the palette goes in a window `width` wide: centered, 96 from the
/// top.
fn anchor(width: f64) -> Anchor {
    let at = Point::new(width / 2.0, 96.0);
    Anchor::Rect(Rect::from_points(at, at))
}

/// How tall its list may be in a window `height` tall.
fn room(height: f64) -> f64 {
    (height - 96.0 - 56.0 - 40.0 - 48.0).clamp(120.0, 560.0)
}

/// A key's chip ("Esc", "Ctrl+G").
fn chip(ui: &mut Ui, text: &str) -> WidgetId {
    let text = ui.add(
        Label::new(text, TextStyle::new(11.0).weight(500.0))
            .color(Token::TextMuted)
            .single_line(),
    );
    ui.add_with_children(
        Flex::row()
            .padding(6.0, 2.0)
            .radius(5.0)
            .border(1.0, Token::ControlBorder)
            .style(|style| style.flex_shrink = 0.0),
        &[text],
    )
}

/// The footer: the keys and what the palette searches.
fn footer(ui: &mut Ui) -> WidgetId {
    let mut children = Vec::new();
    for (keys, what) in [
        ("↑↓", "Move"),
        ("Enter", "Run"),
        ("Tab", "Change the value"),
    ] {
        let keys = chip(ui, keys);
        let what = ui.add(
            Label::new(what, TextStyle::new(12.0))
                .color(Token::TextMuted)
                .single_line(),
        );
        children.push(ui.add_with_children(Flex::row().gap(6.0), &[keys, what]));
    }
    let spring = ui.add(Flex::row().grow(1.0));
    let scope = ui.add(
        Label::new("Commands and settings", TextStyle::new(12.0))
            .color(Token::TextMuted)
            .single_line(),
    );
    children.extend([spring, scope]);
    ui.add_with_children(
        Flex::row().height(40.0).gap(16.0).padding(16.0, 0.0),
        &children,
    )
}

/// A setting's value as its row shows it: the variant's label.
fn value_label(settings: &Registry, key: &str) -> String {
    let Ok(value) = settings.value(key) else {
        return String::new();
    };
    let Value::Enum(id) = &value else {
        return String::new();
    };
    settings
        .declarations()
        .find(|setting| setting.key() == key)
        .and_then(|setting| {
            setting
                .variants()
                .into_iter()
                .find(|variant| variant.id == id)
                .map(|variant| variant.label.to_owned())
        })
        .unwrap_or_else(|| id.clone())
}

/// The next value of the setting `key`, or the previous one (`forward`
/// false): a boolean flips, an enum steps through its variants. `None`
/// for other kinds.
pub(crate) fn step_value(settings: &Registry, key: &str, forward: bool) -> Option<Value> {
    match settings.value(key).ok()? {
        Value::Bool(on) => Some(Value::Bool(!on)),
        Value::Enum(id) => {
            let setting = settings
                .declarations()
                .find(|setting| setting.key() == key)?;
            let variants = setting.variants();
            let count = variants.len().max(1);
            let at = variants
                .iter()
                .position(|variant| variant.id == id)
                .unwrap_or(0);
            let next = if forward {
                (at + 1) % count
            } else {
                (at + count - 1) % count
            };
            Some(Value::Enum(variants.get(next)?.id.to_owned()))
        }
        _ => None,
    }
}

/// The first sentence of a description, for the one line a row has.
fn first_sentence(text: &str) -> &str {
    match text.find(". ") {
        Some(end) => &text[..end],
        None => text.strip_suffix('.').unwrap_or(text),
    }
}

/// Character indices of `text` as byte ranges, neighbors merged.
fn byte_ranges(text: &str, chars: &[usize]) -> Vec<std::ops::Range<usize>> {
    let offsets: Vec<(usize, char)> = text.char_indices().collect();
    let mut ranges: Vec<std::ops::Range<usize>> = Vec::new();
    for &index in chars {
        let Some(&(start, c)) = offsets.get(index) else {
            continue;
        };
        let end = start + c.len_utf8();
        match ranges.last_mut() {
            Some(last) if last.end == start => last.end = end,
            _ => ranges.push(start..end),
        }
    }
    ranges
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letters_become_merged_byte_ranges() {
        assert_eq!(byte_ranges("Group", &[1, 2, 3]), vec![1..4]);
        assert_eq!(byte_ranges("Größe", &[2, 3, 4]), vec![2..7]);
        assert_eq!(byte_ranges("ab", &[0, 5]), vec![0..1]);
    }

    #[test]
    fn a_row_shows_the_first_sentence() {
        assert_eq!(
            first_sentence("Rounds the corners. Then more."),
            "Rounds the corners"
        );
        assert_eq!(
            first_sentence("Puts them in a group."),
            "Puts them in a group"
        );
    }
}
