// SPDX-License-Identifier: AGPL-3.0-or-later

//! The command palette (`shell.md` §3.7, the Palette mockups): every
//! command the board and the shell have, and every setting, found by name.
//! `Ctrl+K` or the Search button opens it; ↑↓ move, Enter runs, Tab
//! changes a setting's value, Esc closes it.

use std::collections::HashMap;

use accesskit::Role;
use imaginarium_commands::{Availability, CommandId, Keymap, SearchIndex, UsageStats};
use imaginarium_reactive::{Signal, signal};
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

/// A row the palette shows: the item, its widget, and the value it shows
/// (a setting's).
struct Row {
    item: usize,
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
    /// The groups and rows listed now, removed when the query changes.
    shown: Vec<WidgetId>,
    row_widgets: Vec<WidgetId>,
    rows: Vec<Row>,
    active: Signal<usize>,
    /// The commands' availability when it opened (`Registry::availability`
    /// is a snapshot by design: cheap checks, taken once).
    availability: HashMap<&'static str, Availability>,
    query: String,
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
            key, pressed: true, ..
        } = input
        else {
            return false;
        };
        let ask = match key {
            Key::ArrowUp => Ask::PaletteMove(-1),
            Key::ArrowDown => Ask::PaletteMove(1),
            Key::Enter => Ask::PaletteRun(None),
            Key::Tab => Ask::PaletteTab,
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
        let list = ui.add(Flex::column().gap(2.0).style(|style| {
            style.padding = taffy::Rect {
                left: length(8.0),
                right: length(8.0),
                top: length(6.0),
                bottom: length(8.0),
            };
        }));
        let room = (height - 96.0 - 56.0 - 40.0 - 48.0).clamp(160.0, 560.0);
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
            .anchored(Anchor::Rect(Rect::from_points(
                Point::new(width / 2.0, 96.0),
                Point::new(width / 2.0, 96.0),
            )))
            .placement(Placement::Below)
            .gap(0.0)
            .scrim(0.16)
            .focus(input)
            .on_close(move || on_close.borrow_mut().push(Ask::PaletteClosed));
        let layer = ui.open_layer(content, options);
        let mut palette = Self {
            layer,
            panel,
            field: input,
            list,
            shown: Vec::new(),
            row_widgets: Vec::new(),
            rows: Vec::new(),
            active: signal(0),
            availability,
            query: String::new(),
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

    /// Moves the active row by `by`, wrapping, and brings it into view.
    pub(crate) fn step(&mut self, by: i32) -> Option<WidgetId> {
        let count = self.rows.len();
        if count == 0 {
            return None;
        }
        let at = self.active.get_untracked() as i64 + i64::from(by);
        let at = at.rem_euclid(count as i64) as usize;
        self.active.set(at);
        self.row_widgets.get(at).copied()
    }

    /// The item of the active row (or of row `row`), if any.
    pub(crate) fn item(&self, row: Option<usize>) -> Option<usize> {
        let row = row.unwrap_or_else(|| self.active.get_untracked());
        self.rows.get(row).map(|row| row.item)
    }

    /// Whether the command `id` can run now.
    pub(crate) fn available(&self, id: &str) -> bool {
        self.availability
            .get(id)
            .is_none_or(|availability| availability.is_enabled())
    }

    /// Shows a setting's new value in its row.
    pub(crate) fn show_value(&self, item: usize, settings: &Registry, catalog: &Catalog) {
        for row in self.rows.iter().filter(|row| row.item == item) {
            let id = catalog.items[item].id;
            if let Some(on) = row.on {
                on.set(matches!(settings.value(id), Ok(Value::Bool(true))));
            }
            if let Some(value) = row.value {
                value.set(value_label(settings, id));
            }
        }
    }

    fn fill(&mut self, ui: &mut Ui, sources: &Sources<'_>) {
        for widget in self.shown.drain(..) {
            ui.remove(widget);
        }
        self.rows.clear();
        self.row_widgets.clear();
        self.active.set(0);
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
            // The recent ones first, then everything in its group.
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
        if groups.is_empty() {
            let none = ui.add(
                Label::new(format!("Nothing found for “{query}”"), TextStyle::new(13.0))
                    .color(Token::TextMuted),
            );
            let none = ui.add_with_children(Flex::row().padding(8.0, 16.0), &[none]);
            children.push(none);
        }
        for (name, items) in groups {
            let heading = ui.add(
                Label::new(name, TextStyle::new(12.0).weight(600.0))
                    .color(Token::TextSubtle)
                    .single_line(),
            );
            let heading = ui.add_with_children(
                Flex::row().style(|style| {
                    style.padding = taffy::Rect {
                        left: length(8.0),
                        right: length(8.0),
                        top: length(if children.is_empty() { 8.0 } else { 10.0 }),
                        bottom: length(4.0),
                    };
                }),
                &[heading],
            );
            children.push(heading);
            for at in items {
                let row = self.row(ui, at, &query, sources);
                children.push(row);
            }
        }
        ui.set_children(self.list, &children);
        self.shown = children;
    }

    fn row(&mut self, ui: &mut Ui, at: usize, query: &str, sources: &Sources<'_>) -> WidgetId {
        let catalog = sources.catalog;
        let item = &catalog.items[at];
        let index = self.rows.len();
        let enabled = item.kind == Kind::Setting || self.available(item.id);
        let explanation = if query.is_empty() {
            None
        } else {
            catalog.index.explain(CommandId::new(item.id), query)
        };
        let ink = if enabled {
            Token::Text
        } else {
            Token::TextSubtle
        };
        let icon = ui.add(Icon::named(
            item.icon.unwrap_or("chevron-right"),
            16.0,
            Token::TextSecondary,
        ));
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
        let title_text = match item.kind {
            Kind::Setting => item.title.to_owned(),
            Kind::Command => item.title.to_owned(),
        };
        let marks = explanation
            .as_ref()
            .map(|explained| byte_ranges(&title_text, &explained.title))
            .unwrap_or_default();
        let title = ui.add(
            Label::new(title_text, TextStyle::new(14.0).weight(500.0))
                .color(ink)
                .single_line()
                .highlight(marks, 600.0, Token::AccentInk),
        );
        // The second line: why it can't run, or what it does, and the
        // synonym that matched.
        let reason = match self.availability.get(item.id) {
            Some(Availability::Disabled(reason)) => Some(*reason),
            _ => None,
        };
        let (line, marks) = match (reason, explanation.and_then(|explained| explained.synonym)) {
            (Some(reason), _) => (reason.to_owned(), Vec::new()),
            (None, Some((synonym, at))) => {
                let synonym = item.synonyms.get(synonym).copied().unwrap_or("");
                let start = format!("{} · also ", first_sentence(item.description));
                let offset = start.len();
                let marks = byte_ranges(synonym, &at)
                    .into_iter()
                    .map(|range| range.start + offset..range.end + offset)
                    .collect();
                (format!("{start}{synonym}"), marks)
            }
            (None, None) => (first_sentence(item.description).to_owned(), Vec::new()),
        };
        let description = ui.add(
            Label::new(line, TextStyle::new(12.0))
                .color(Token::TextMuted)
                .single_line()
                .highlight(marks, 600.0, Token::AccentInk),
        );
        let texts = ui.add_with_children(
            Flex::column().gap(2.0).grow(1.0).clip(),
            &[title, description],
        );
        let mut children = vec![tile, texts];
        let mut value = None;
        let mut on = None;
        match item.kind {
            Kind::Setting => match sources.settings.value(item.id) {
                Ok(Value::Bool(current)) => {
                    let state = signal(current);
                    on = Some(state);
                    let asks = sources.asks.clone();
                    let switch = Switch::new(
                        item.title,
                        move || state.get(),
                        move || {
                            asks.borrow_mut().push(Ask::PaletteRun(Some(index)));
                        },
                    )
                    .add_to(ui);
                    children.push(switch);
                }
                Ok(_) => {
                    let shown = signal(value_label(sources.settings, item.id));
                    value = Some(shown);
                    let text = label(ui, TextStyle::new(12.0), Token::Text, move || shown.get());
                    let chip = ui.add_with_children(
                        Flex::row()
                            .height(26.0)
                            .padding(8.0, 0.0)
                            .radius(7.0)
                            .background(Token::PanelBg)
                            .border(1.0, Token::ControlBorder),
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
        let label_text = match reason {
            Some(reason) => format!("{}, unavailable: {reason}", item.title),
            None => item.title.to_owned(),
        };
        let row = ButtonBox::new(label_text, ButtonColors::quiet(), move || {
            asks.borrow_mut().push(Ask::PaletteRun(Some(index)));
        })
        .height(52.0)
        .padding(8.0, 8.0)
        .gap(12.0)
        .start()
        .radius(10.0)
        .keep_focus()
        .selected(move || active.get() == index)
        .add_to(ui, &children);
        self.rows.push(Row {
            item: at,
            value,
            on,
        });
        self.row_widgets.push(row);
        row
    }
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

/// The next value of the setting `key` (a boolean flips, an enum steps to
/// its next variant), or `None` for other kinds.
pub(crate) fn next_value(settings: &Registry, key: &str) -> Option<Value> {
    match settings.value(key).ok()? {
        Value::Bool(on) => Some(Value::Bool(!on)),
        Value::Enum(id) => {
            let setting = settings
                .declarations()
                .find(|setting| setting.key() == key)?;
            let variants = setting.variants();
            let at = variants
                .iter()
                .position(|variant| variant.id == id)
                .unwrap_or(0);
            let next = variants.get((at + 1) % variants.len().max(1))?;
            Some(Value::Enum(next.id.to_owned()))
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
