// SPDX-License-Identifier: AGPL-3.0-or-later

//! The board window's panels other than the tool bar (`shell.md` §3.1,
//! §3.2, §3.4), laid out in an overlay that lets the pointer through to
//! the board between them.

use std::collections::HashMap;

use accesskit::Role;
use imaginarium_canvas::BoardStatus;
use imaginarium_commands::Keymap;
use imaginarium_reactive::Signal;
use imaginarium_render::Scene;
use imaginarium_render::kurbo::{Affine, BezPath, Cap, Circle, Rect, RoundedRect, Shape, Stroke};
use imaginarium_render::peniko::Color;
use imaginarium_text::TextStyle;
use imaginarium_ui::{
    ButtonBox, ButtonColors, Cx, Flex, Icon, IconButton, Label, Placement, Token, Ui, Widget,
    WidgetId, label,
};
use taffy::prelude::length;

use crate::toolbar::shortcut;
use crate::window::{Ask, Asks};

/// A control that is visible but not built yet: a click says what is
/// coming instead of pretending to work (`shell.md` §3.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Placeholder {
    MainMenu,
    Title,
    Save,
    Ai,
    Share,
    Help,
}

impl Placeholder {
    /// The note's title and text.
    pub(crate) fn note(self) -> (&'static str, &'static str) {
        match self {
            Placeholder::MainMenu => (
                "The main menu",
                "New, open, save a copy and settings come with files, in M3.6.",
            ),
            Placeholder::Title => (
                "Rename or move",
                "Naming the board and choosing its folder come with files, in M3.6.",
            ),
            Placeholder::Save => (
                "Saving",
                "Saving to a file comes in M3.6. Until then this board lives only in this \
                 window, and closing it loses it.",
            ),
            Placeholder::Ai => (
                "AI assistant",
                "The assistant arrives in M6. It works on the board through the same \
                 commands as you, and asks before it changes anything.",
            ),
            Placeholder::Share => (
                "Sharing",
                "Editing together, end-to-end encrypted, comes with the Hub in M8.",
            ),
            Placeholder::Help => (
                "Help and shortcuts",
                "The list of shortcuts comes with the docs, in M3.9. Tooltips show each \
                 tool's key meanwhile.",
            ),
        }
    }
}

/// The panels' widgets the window needs again.
pub(crate) struct Chrome {
    /// The overlay holding them all.
    pub(crate) overlay: WidgetId,
    /// What each placeholder's note opens next to.
    pub(crate) anchors: HashMap<Placeholder, WidgetId>,
    /// The zoom percentage, which opens the zoom menu.
    pub(crate) zoom: WidgetId,
    /// The panels in the corners, which a docked tool bar keeps clear of:
    /// top left, top right, bottom left, bottom right.
    pub(crate) corners: [WidgetId; 4],
    /// The pill's save state, filled again when it changes
    /// ([`save_state`]).
    pub(crate) save_slot: WidgetId,
}

/// What the document pill says about the file (`shell.md` §3.1, the
/// SaveStates mockup).
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum SaveShown {
    /// A new board without a file: "Not saved" and Save….
    Draft,
    /// Every change is in the file.
    Saved,
    /// A write takes a while.
    Saving,
    /// The write failed; it is retried.
    Failed(String),
    /// Opened read-only (made with a newer version).
    ReadOnly,
}

/// The pill's save state as widgets, for its slot. The Save… button is
/// the placeholder's anchor.
pub(crate) fn save_state(
    ui: &mut Ui,
    shown: &SaveShown,
    asks: &Asks,
    anchors: &mut HashMap<Placeholder, WidgetId>,
) -> Vec<WidgetId> {
    let status = |ui: &mut Ui, icon: &str, text: &str, ink: Token, weight: f32| {
        let icon = ui.add(Icon::named(icon, 14.0, ink));
        let label = ui.add(
            Label::new(text, body().weight(weight))
                .color(ink)
                .single_line(),
        );
        ui.add_with_children(Flex::row().gap(6.0), &[icon, label])
    };
    match shown {
        SaveShown::Saved => vec![status(ui, "check", "Saved", Token::TextMuted, 400.0)],
        SaveShown::Saving => vec![status(
            ui,
            "loader-circle",
            "Saving…",
            Token::TextMuted,
            400.0,
        )],
        SaveShown::Failed(reason) => {
            let row = status(
                ui,
                "triangle-alert",
                "Can't save · retrying",
                Token::Warning,
                500.0,
            );
            // Why, for the pointer and for screen readers.
            vec![ui.add_with_children(
                Flex::row().role(accesskit::Role::Status, format!("Can't save: {reason}")),
                &[row],
            )]
        }
        SaveShown::ReadOnly => vec![status(
            ui,
            "lock",
            "Read-only · made with a newer version",
            Token::TextMuted,
            400.0,
        )],
        SaveShown::Draft => {
            // A new board has no file yet: "Not saved" and Save….
            let badge_text = ui.add(
                Label::new("Not saved", TextStyle::new(12.0).weight(600.0))
                    .color(Token::OnWarningSoft)
                    .single_line(),
            );
            let badge = ui.add_with_children(
                Flex::row()
                    .height(22.0)
                    .padding(8.0, 0.0)
                    .radius(6.0)
                    .background(Token::WarningSoft),
                &[badge_text],
            );
            let save_text = ui.add(
                Label::new("Save…", body().weight(600.0))
                    .color(Token::AccentInk)
                    .single_line(),
            );
            let save = ui.add_with_children(
                ButtonBox::new(
                    "Save…",
                    ButtonColors {
                        text: Token::AccentInk.into(),
                        border: Some(Token::AccentLine.into()),
                        ..ButtonColors::quiet()
                    },
                    explain(asks, Placeholder::Save),
                )
                .height(28.0)
                .padding(10.0, 10.0)
                .keep_focus(),
                &[save_text],
            );
            anchors.insert(Placeholder::Save, save);
            vec![badge, save]
        }
    }
}

fn body() -> TextStyle {
    TextStyle::new(13.0)
}

/// The colors of a button that is a panel itself (search, AI, help).
fn panel_colors() -> ButtonColors {
    ButtonColors {
        border: Some(Token::PanelBorder.into()),
        ..ButtonColors::quiet()
    }
}

/// A box that only lays out: the pointer goes through it.
fn through(direction: taffy::FlexDirection) -> Flex {
    let flex = match direction {
        taffy::FlexDirection::Column => Flex::column(),
        _ => Flex::row(),
    };
    flex.pass_through()
}

/// A 44 px panel holding controls in a row (zoom, undo).
fn panel(ui: &mut Ui, children: &[WidgetId]) -> WidgetId {
    ui.add_with_children(
        Flex::row()
            .height(44.0)
            .padding(4.0, 0.0)
            .gap(2.0)
            .background(Token::PanelBg)
            .border(1.0, Token::PanelBorder)
            .radius(12.0)
            .shadow(),
        children,
    )
}

fn divider(ui: &mut Ui, margin: f32) -> WidgetId {
    ui.add(
        Flex::row()
            .width(1.0)
            .height(20.0)
            .background(Token::PanelBorder)
            .style(|style| {
                style.margin.left = length(margin);
                style.margin.right = length(margin);
                style.flex_shrink = 0.0;
            }),
    )
}

/// A click on `placeholder`'s control asks for its note.
fn explain(asks: &Asks, placeholder: Placeholder) -> impl FnMut() + 'static {
    let asks = asks.clone();
    move || asks.borrow_mut().push(Ask::Explain(placeholder))
}

fn run(asks: &Asks, command: &'static str) -> impl FnMut() + 'static {
    let asks = asks.clone();
    move || asks.borrow_mut().push(Ask::Run(command))
}

/// An icon button running `command`, its shortcut from the keymap.
fn command_button(
    icon: &str,
    label: &str,
    command: &'static str,
    keymap: &Keymap,
    asks: &Asks,
) -> IconButton {
    let button =
        IconButton::new(icon, label, run(asks, command)).tooltip_placement(Placement::Above);
    match shortcut(keymap, command) {
        Some(keys) => button.shortcut(keys),
        None => button,
    }
}

/// Builds the overlay with the pill, the top right, zoom and undo, and
/// help, wired to the board's `status`.
pub(crate) fn chrome(
    ui: &mut Ui,
    name: &str,
    status: Signal<BoardStatus>,
    keymaps: [&Keymap; 2],
    asks: &Asks,
) -> Chrome {
    let [keymap, shell_keys] = keymaps;
    let mut anchors = HashMap::new();
    let (pill, save_slot) = pill(ui, name, asks, &mut anchors);
    let top_right = top_right(ui, shell_keys, asks, &mut anchors);
    let spring = ui.add(through(taffy::FlexDirection::Row).grow(1.0));
    let top = ui.add_with_children(
        through(taffy::FlexDirection::Row).style(|style| {
            style.align_items = taffy::AlignItems::FLEX_START;
        }),
        &[pill, spring, top_right],
    );

    // Bottom left: zoom, then undo and redo.
    let zoom_out = command_button("minus", "Zoom out", "view.zoom_out", keymap, asks).add_to(ui);
    let percent = label(ui, body().weight(500.0), Token::Text, move || {
        format!("{:.0} %", status.get().zoom * 100.0)
    });
    let zoom = ui.add_with_children(
        ButtonBox::new(
            "Zoom: fit, 100 % or the selection",
            ButtonColors::quiet(),
            {
                let asks = asks.clone();
                move || asks.borrow_mut().push(Ask::ZoomMenu)
            },
        )
        .size(56.0, 36.0)
        .keep_focus(),
        &[percent],
    );
    let zoom_in = command_button("plus", "Zoom in", "view.zoom_in", keymap, asks).add_to(ui);
    let gap = divider(ui, 4.0);
    let fit = command_button("scan", "Zoom to fit", "view.zoom_to_fit", keymap, asks).add_to(ui);
    let zoom_panel = panel(ui, &[zoom_out, zoom, zoom_in, gap, fit]);
    let undo = command_button("undo-2", "Undo", "edit.undo", keymap, asks)
        .enabled(move || status.get().can_undo)
        .add_to(ui);
    let redo = command_button("redo-2", "Redo", "edit.redo", keymap, asks)
        .enabled(move || status.get().can_redo)
        .add_to(ui);
    let history = panel(ui, &[undo, redo]);
    let bottom_left = ui.add_with_children(
        through(taffy::FlexDirection::Row).gap(8.0),
        &[zoom_panel, history],
    );

    // Bottom right: help.
    let help_icon = ui.add(Icon::named(
        "circle-question-mark",
        18.0,
        Token::TextSecondary,
    ));
    let help = ui.add_with_children(
        ButtonBox::new(
            "Help and shortcuts",
            panel_colors(),
            explain(asks, Placeholder::Help),
        )
        .size(44.0, 44.0)
        .radius(12.0)
        .shadow()
        .keep_focus()
        .tooltip("Help and shortcuts", Placement::Above),
        &[help_icon],
    );
    anchors.insert(Placeholder::Help, help);
    let spring = ui.add(through(taffy::FlexDirection::Row).grow(1.0));
    let bottom = ui.add_with_children(
        through(taffy::FlexDirection::Row).style(|style| {
            style.align_items = taffy::AlignItems::FLEX_END;
        }),
        &[bottom_left, spring, help],
    );

    let overlay = ui.add_with_children(
        through(taffy::FlexDirection::Column).style(|style| {
            style.position = taffy::Position::Absolute;
            style.inset = taffy::Rect {
                left: length(0.0),
                right: length(0.0),
                top: length(0.0),
                bottom: length(0.0),
            };
            style.padding = taffy::Rect {
                left: length(16.0),
                right: length(16.0),
                top: length(16.0),
                bottom: length(16.0),
            };
            style.justify_content = taffy::JustifyContent::SPACE_BETWEEN;
        }),
        &[top, bottom],
    );
    Chrome {
        overlay,
        anchors,
        zoom,
        corners: [pill, top_right, bottom_left, help],
        save_slot,
    }
}

/// The document pill: the app button, the board's name, the save state.
fn pill(
    ui: &mut Ui,
    name: &str,
    asks: &Asks,
    anchors: &mut HashMap<Placeholder, WidgetId>,
) -> (WidgetId, WidgetId) {
    let logo = ui.add(Logo);
    let menu = ui.add_with_children(
        ButtonBox::new(
            "Main menu",
            ButtonColors::quiet(),
            explain(asks, Placeholder::MainMenu),
        )
        .size(34.0, 34.0)
        .radius(9.0)
        .keep_focus(),
        &[logo],
    );
    anchors.insert(Placeholder::MainMenu, menu);
    let title = ui.add(
        Label::new(name, TextStyle::new(14.0).weight(600.0))
            .color(Token::Text)
            .single_line(),
    );
    let chevron = ui.add(Icon::named("chevron-down", 14.0, Token::TextMuted));
    let rename = ui.add_with_children(
        ButtonBox::new(
            "Rename or move the board",
            ButtonColors::quiet(),
            explain(asks, Placeholder::Title),
        )
        .height(32.0)
        .padding(6.0, 6.0)
        .gap(4.0)
        .keep_focus(),
        &[title, chevron],
    );
    anchors.insert(Placeholder::Title, rename);
    let line = divider(ui, 0.0);
    let shown = save_state(ui, &SaveShown::Draft, asks, anchors);
    let state = ui.add_with_children(Flex::row().gap(8.0), &shown);
    let pill = ui.add_with_children(
        Flex::row()
            .height(44.0)
            .gap(8.0)
            .background(Token::PanelBg)
            .border(1.0, Token::PanelBorder)
            .radius(12.0)
            .shadow()
            .style(|style| {
                style.padding.left = length(6.0);
                style.padding.right = length(12.0);
            }),
        &[menu, rename, line, state],
    );
    (pill, state)
}

/// Search, the AI assistant and Share.
fn top_right(
    ui: &mut Ui,
    keymap: &Keymap,
    asks: &Asks,
    anchors: &mut HashMap<Placeholder, WidgetId>,
) -> WidgetId {
    let search_icon = ui.add(Icon::named("search", 16.0, Token::TextMuted));
    let search_text = ui.add(
        Label::new("Search", body())
            .color(Token::TextMuted)
            .single_line()
            .grow(1.0),
    );
    let keys = keymap_text(keymap, "palette.open").unwrap_or_else(|| mod_key(keymap, "K"));
    let chip_text = ui.add(
        Label::new(keys.clone(), TextStyle::new(11.0).weight(500.0))
            .color(Token::TextMuted)
            .single_line(),
    );
    let chip = ui.add_with_children(
        Flex::row()
            .padding(6.0, 2.0)
            .border(1.0, Token::PanelBorder)
            .radius(5.0),
        &[chip_text],
    );
    let search = ui.add_with_children(
        ButtonBox::new(
            format!("Search commands and settings ({keys})"),
            panel_colors(),
            {
                let asks = asks.clone();
                move || asks.borrow_mut().push(Ask::OpenPalette)
            },
        )
        .size(176.0, 44.0)
        .padding(12.0, 8.0)
        .start()
        .radius(12.0)
        .shadow()
        .keep_focus(),
        &[search_icon, search_text, chip],
    );
    let sparkles = ui.add(Icon::named("sparkles", 19.0, Token::AccentInk));
    let ai_keys = mod_key(keymap, "J");
    let ai = ui.add_with_children(
        ButtonBox::new(
            format!("AI assistant ({ai_keys})"),
            panel_colors(),
            explain(asks, Placeholder::Ai),
        )
        .size(44.0, 44.0)
        .radius(12.0)
        .shadow()
        .keep_focus()
        .tooltip(format!("AI assistant · {ai_keys}"), Placement::Below),
        &[sparkles],
    );
    anchors.insert(Placeholder::Ai, ai);
    let users = ui.add(Icon::named("users", 17.0, Token::OnAccent));
    let share_text = ui.add(
        Label::new("Share", TextStyle::new(14.0).weight(600.0))
            .color(Token::OnAccent)
            .single_line(),
    );
    let share = ui.add_with_children(
        ButtonBox::new(
            "Share",
            ButtonColors::primary(),
            explain(asks, Placeholder::Share),
        )
        .height(44.0)
        .padding(16.0, 16.0)
        .radius(12.0)
        .shadow()
        .keep_focus(),
        &[users, share_text],
    );
    anchors.insert(Placeholder::Share, share);
    ui.add_with_children(
        through(taffy::FlexDirection::Row).gap(8.0),
        &[search, ai, share],
    )
}

/// The first shortcut of `command`, if it has one.
fn keymap_text(keymap: &Keymap, command: &'static str) -> Option<String> {
    shortcut(keymap, command)
}

/// `key` with the platform's Mod, as its menus write it (Ctrl+K, ⌘K),
/// for shortcuts whose commands don't exist yet.
fn mod_key(keymap: &Keymap, key: &str) -> String {
    if keymap.platform() == imaginarium_commands::Platform::MacOs {
        format!("⌘{key}")
    } else {
        format!("Ctrl+{key}")
    }
}

/// The note a placeholder opens: what is coming.
pub(crate) fn note(ui: &mut Ui, placeholder: Placeholder) -> WidgetId {
    let (title, text) = placeholder.note();
    let title = ui.add(
        Label::new(title, body().weight(600.0))
            .color(Token::Text)
            .single_line(),
    );
    let text = ui.add(Label::new(text, body()).color(Token::TextMuted));
    ui.add_with_children(
        Flex::column()
            .width(280.0)
            .padding(14.0, 12.0)
            .gap(6.0)
            .background(Token::PanelBg)
            .border(1.0, Token::PanelBorder)
            .radius(12.0)
            .shadow()
            .role(Role::Dialog, placeholder.note().0),
        &[title, text],
    )
}

/// The zoom menu: fit, 100 % and the selection, with their keys.
pub(crate) fn zoom_menu(ui: &mut Ui, keymap: &Keymap, asks: &Asks) -> WidgetId {
    let items = [
        ("Zoom to fit", "view.zoom_to_fit"),
        ("Zoom to 100 %", "view.zoom_100"),
        ("Zoom to selection", "view.zoom_to_selection"),
    ];
    let rows: Vec<WidgetId> = items
        .into_iter()
        .map(|(text, command)| {
            let text = ui.add(
                Label::new(text, body())
                    .color(Token::Text)
                    .single_line()
                    .grow(1.0),
            );
            let keys = ui.add(
                Label::new(
                    shortcut(keymap, command).unwrap_or_default(),
                    TextStyle::new(12.0),
                )
                .color(Token::TextSubtle)
                .single_line(),
            );
            let asks = asks.clone();
            ui.add_with_children(
                ButtonBox::new(text_of(command), ButtonColors::quiet(), move || {
                    let mut asks = asks.borrow_mut();
                    asks.push(Ask::CloseMenu);
                    asks.push(Ask::Run(command));
                })
                .size(220.0, 32.0)
                .padding(10.0, 10.0)
                .start(),
                &[text, keys],
            )
        })
        .collect();
    ui.add_with_children(
        Flex::column()
            .padding(4.0, 4.0)
            .gap(2.0)
            .background(Token::PanelBg)
            .border(1.0, Token::PanelBorder)
            .radius(12.0)
            .shadow()
            .role(Role::Menu, "Zoom"),
        &rows,
    )
}

/// A zoom command's name, for screen readers.
fn text_of(command: &str) -> &'static str {
    match command {
        "view.zoom_to_fit" => "Zoom to fit",
        "view.zoom_100" => "Zoom to 100 %",
        _ => "Zoom to selection",
    }
}

/// Imaginarium's logo (the app button): a card, a node and the line
/// between them, white on the logo's blue, at 24 px.
struct Logo;

/// The logo's blue: the brand's, not the accent setting's.
const LOGO_BLUE: Color = Color::from_rgb8(0x3a, 0x4b, 0xd6);

impl Widget for Logo {
    fn style(&self) -> taffy::Style {
        taffy::Style {
            size: taffy::Size {
                width: length(24.0),
                height: length(24.0),
            },
            flex_shrink: 0.0,
            ..taffy::Style::default()
        }
    }

    fn paint(&mut self, cx: &mut Cx<'_>, scene: &mut Scene) {
        // Drawn on the logo's 64-unit grid.
        let at = cx.bounds().origin().to_vec2();
        let grid = Affine::translate(at) * Affine::scale(24.0 / 64.0);
        let white = Color::WHITE;
        let faded = Color::WHITE.with_alpha(0.7);
        scene.fill(
            &(grid * RoundedRect::new(0.0, 0.0, 64.0, 64.0, 14.0).to_path(0.1)),
            LOGO_BLUE,
        );
        scene.fill(
            &(grid * RoundedRect::new(12.0, 14.0, 30.0, 28.0, 3.0).to_path(0.1)),
            white,
        );
        scene.fill(&(grid * Circle::new((44.0, 44.0), 8.0).to_path(0.1)), white);
        let mut link = BezPath::new();
        link.move_to((30.0, 21.0));
        link.line_to((44.0, 21.0));
        link.line_to((44.0, 36.0));
        scene.stroke_with(
            &(grid * link),
            &Stroke::new(3.0 * 24.0 / 64.0).with_caps(Cap::Round),
            white,
        );
        for bar in [
            Rect::new(12.0, 38.0, 28.0, 41.0),
            Rect::new(12.0, 45.0, 23.0, 48.0),
        ] {
            scene.fill(
                &(grid * RoundedRect::from_rect(bar, 1.5).to_path(0.1)),
                faded,
            );
        }
    }
}
