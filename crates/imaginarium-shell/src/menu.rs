// SPDX-License-Identifier: AGPL-3.0-or-later

//! Menus of commands (`shell.md` §3.8, the ContextMenu mockup): an icon,
//! the title and the shortcut per row, separators between groups, Delete
//! in the danger color. The context menu, the contextual toolbar's More
//! and Arrange use them; every row is a board command, shown when it
//! applies and greyed when it can't run now.

use accesskit::Role;
use imaginarium_canvas::BoardCanvas;
use imaginarium_commands::{Availability, Keymap, Registry};
use imaginarium_text::TextStyle;
use imaginarium_ui::{
    ButtonBox, ButtonColors, Cx, Flex, Icon, Input, Key, Label, Token, Ui, Widget, WidgetId,
};

use crate::toolbar::shortcut;
use crate::window::{Ask, Asks};

/// The context menu of a selection, in its groups.
pub(crate) const SELECTION: &[&[&str]] = &[
    &["board.cut", "board.copy", "board.paste", "board.duplicate"],
    &[
        "board.bring_to_front",
        "board.bring_forward",
        "board.send_backward",
        "board.send_to_back",
        "board.group",
        "board.ungroup",
    ],
    &["text.edit", "board.lock", "board.unlock"],
    &["board.delete"],
];

/// The context menu of the empty board.
pub(crate) const BOARD: &[&[&str]] = &[
    &[
        "board.paste",
        "board.select_all",
        "board.unlock",
        "board.show_hidden",
    ],
    &["view.zoom_to_fit", "view.zoom_100"],
    &["edit.undo", "edit.redo"],
];

/// The contextual toolbar's Arrange.
pub(crate) const ARRANGE: &[&[&str]] = &[
    &[
        "board.bring_to_front",
        "board.bring_forward",
        "board.send_backward",
        "board.send_to_back",
    ],
    &["board.group", "board.ungroup"],
];

/// Destructive commands, in the danger color.
const DANGER: &[&str] = &["board.delete"];

/// One row.
pub(crate) struct Entry {
    command: &'static str,
    title: &'static str,
    icon: Option<&'static str>,
    keys: Option<String>,
    /// Why it can't run now, if it can't.
    disabled: Option<&'static str>,
}

/// The rows of `groups` for `board` now: commands that don't apply are
/// left out, and groups left empty with them.
pub(crate) fn entries(
    groups: &[&[&'static str]],
    commands: &Registry<BoardCanvas>,
    board: &BoardCanvas,
    keymap: &Keymap,
) -> Vec<Vec<Entry>> {
    groups
        .iter()
        .map(|group| {
            group
                .iter()
                .filter_map(|&id| {
                    let command = commands.get(id)?;
                    let disabled = match command.availability(board) {
                        Availability::Hidden => return None,
                        Availability::Disabled(reason) => Some(reason),
                        Availability::Enabled => None,
                    };
                    let meta = command.meta();
                    Some(Entry {
                        command: id,
                        title: meta.title,
                        icon: meta.icon,
                        keys: shortcut(keymap, id),
                        disabled,
                    })
                })
                .collect::<Vec<_>>()
        })
        .filter(|group| !group.is_empty())
        .collect()
}

/// Moves the focus through a menu with ↑↓, as Tab does.
struct MenuKeys {
    asks: Asks,
}

impl Widget for MenuKeys {
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
        let forward = match key {
            Key::ArrowDown => true,
            Key::ArrowUp => false,
            _ => return false,
        };
        self.asks.borrow_mut().push(Ask::FocusStep(forward));
        true
    }
}

/// A menu of `groups`, named `label` for screen readers. Choosing a row
/// closes the menu and runs its command.
pub(crate) fn menu(ui: &mut Ui, label: &str, groups: Vec<Vec<Entry>>, asks: &Asks) -> WidgetId {
    let mut children = Vec::new();
    for (index, group) in groups.into_iter().enumerate() {
        if index > 0 {
            children.push(
                ui.add(
                    Flex::row()
                        .height(1.0)
                        .background(Token::Divider)
                        .style(|style| {
                            style.margin = taffy::Rect {
                                left: taffy::prelude::length(6.0),
                                right: taffy::prelude::length(6.0),
                                top: taffy::prelude::length(4.0),
                                bottom: taffy::prelude::length(4.0),
                            };
                        }),
                ),
            );
        }
        for entry in group {
            children.push(row(ui, entry, asks));
        }
    }
    let panel = ui.add_with_children(
        Flex::column()
            .width(260.0)
            .padding(6.0, 6.0)
            .gap(1.0)
            .background(Token::PanelBg)
            .border(1.0, Token::PanelBorder)
            .radius(12.0)
            .shadow()
            .role(Role::Menu, label),
        &children,
    );
    with_keys(ui, panel, asks)
}

/// `panel` (a menu) with ↑↓ moving through its rows.
pub(crate) fn with_keys(ui: &mut Ui, panel: WidgetId, asks: &Asks) -> WidgetId {
    ui.add_with_children(MenuKeys { asks: asks.clone() }, &[panel])
}

fn row(ui: &mut Ui, entry: Entry, asks: &Asks) -> WidgetId {
    let danger = DANGER.contains(&entry.command);
    let (ink, icon_ink) = match (entry.disabled, danger) {
        (Some(_), _) => (Token::TextSubtle, Token::TextSubtle),
        (None, true) => (Token::Danger, Token::Danger),
        (None, false) => (Token::Text, Token::TextSecondary),
    };
    let icon = ui.add(Icon::named(
        entry.icon.unwrap_or("chevron-right"),
        16.0,
        icon_ink,
    ));
    let title = ui.add(
        Label::new(entry.title, TextStyle::new(13.0))
            .color(ink)
            .single_line()
            .grow(1.0),
    );
    let mut children = vec![icon, title];
    if let Some(keys) = &entry.keys {
        children.push(
            ui.add(
                Label::new(keys.clone(), TextStyle::new(12.0))
                    .color(Token::TextSubtle)
                    .single_line(),
            ),
        );
    }
    let label = match entry.disabled {
        Some(reason) => format!("{}, unavailable: {reason}", entry.title),
        None => entry.title.to_owned(),
    };
    let command = entry.command;
    let enabled = entry.disabled.is_none();
    let asks = asks.clone();
    ui.add_with_children(
        ButtonBox::new(label, ButtonColors::quiet(), move || {
            if enabled {
                let mut asks = asks.borrow_mut();
                asks.push(Ask::CloseMenu);
                asks.push(Ask::Run(command));
            }
        })
        .size(248.0, 32.0)
        .padding(10.0, 10.0)
        .gap(10.0)
        .start()
        .radius(7.0)
        .keep_focus()
        .role(Role::MenuItem)
        .disabled(!enabled),
        &children,
    )
}
