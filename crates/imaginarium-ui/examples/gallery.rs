// SPDX-License-Identifier: AGPL-3.0-or-later

//! The component gallery: every toolkit component in its states, in the
//! current theme, to look at before the shell uses it (like Storybook).
//! New components get a section here when they are built.
//!
//! ```sh
//! cargo run --release -p imaginarium-ui --example gallery
//! cargo run --release -p imaginarium-ui --example gallery -- --light          # start in the light theme
//! cargo run --release -p imaginarium-ui --example gallery -- --open dialog    # popover, dialog or tooltip
//! cargo run --release -p imaginarium-ui --example gallery -- --frames 30      # close after 30 frames
//! ```

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Instant;

use imaginarium_render::Scene;
use imaginarium_render::kurbo::{Rect, Size};
use imaginarium_text::{Fonts, TextStyle};
use imaginarium_ui::{
    AccessTree, Anchor, Button, ButtonColors, DEFAULT_ACCENT, Flex, FrameInfo, Icon, Input,
    InputColors, Label, LayerId, LayerOptions, Scroll, TextInput, Theme, ThemeKind, Token, Ui,
    UiError, View, VirtualList, WidgetId, WindowOptions, set_theme, theme,
};

/// What a button asked for; the view does it after the input, when it
/// can reach the tree.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Ask {
    Popover,
    Dialog,
    Tooltip,
    CloseDialog,
}

type Asks = Rc<RefCell<Vec<Ask>>>;

struct Gallery {
    fonts: Fonts,
    ui: Ui,
    asks: Asks,
    /// The buttons that open the layers, their anchors.
    openers: Rc<Cell<[Option<WidgetId>; 3]>>,
    dialog: Option<LayerId>,
    tooltip: Option<LayerId>,
    close_after: Option<u64>,
    frames: u64,
}

const POPOVER: usize = 0;
const TOOLTIP: usize = 2;

impl Gallery {
    fn handle_asks(&mut self) {
        let asks: Vec<Ask> = self.asks.borrow_mut().drain(..).collect();
        for ask in asks {
            match ask {
                Ask::Popover => {
                    let anchor = self.openers.get()[POPOVER];
                    let content = menu(&mut self.ui);
                    if let Some(anchor) = anchor {
                        self.ui
                            .open_layer(content, LayerOptions::popover(Anchor::Widget(anchor)));
                    }
                }
                Ask::Dialog => {
                    let (content, save) = dialog(&mut self.ui, &self.asks);
                    self.dialog = Some(
                        self.ui
                            .open_layer(content, LayerOptions::modal().focus(save)),
                    );
                }
                Ask::Tooltip => match self.tooltip.take() {
                    Some(open) if self.ui.is_layer_open(open) => {
                        self.ui.close_layer(&mut self.fonts, open);
                    }
                    _ => {
                        let anchor = self.openers.get()[TOOLTIP];
                        let content = tooltip(&mut self.ui);
                        if let Some(anchor) = anchor {
                            self.tooltip = Some(self.ui.open_layer(
                                content,
                                LayerOptions::tooltip(Anchor::Widget(anchor)),
                            ));
                        }
                    }
                },
                Ask::CloseDialog => {
                    if let Some(dialog) = self.dialog.take() {
                        self.ui.close_layer(&mut self.fonts, dialog);
                    }
                }
            }
        }
    }
}

impl View for Gallery {
    fn paint(&mut self, scene: &mut Scene, size: Size) {
        self.handle_asks();
        self.ui
            .layout(&mut self.fonts, Rect::from_origin_size((0.0, 0.0), size));
        self.ui.paint(&mut self.fonts, scene);
    }

    fn input(&mut self, input: &Input) -> bool {
        let handled = self.ui.input(&mut self.fonts, input);
        let asked = !self.asks.borrow().is_empty();
        if asked {
            self.handle_asks();
        }
        handled || asked
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
        self.ui.access_action(&mut self.fonts, request)
    }
}

fn body() -> TextStyle {
    TextStyle::new(13.0)
}

/// A titled panel of the gallery.
fn section(ui: &mut Ui, title: &str, items: &[WidgetId]) -> WidgetId {
    let heading =
        ui.add(Label::new(title, TextStyle::new(12.0).weight(600.0)).color(Token::TextMuted));
    let row = ui.add_with_children(Flex::row().gap(10.0), items);
    ui.add_with_children(
        Flex::column()
            .gap(12.0)
            .padding(16.0, 14.0)
            .background(Token::PanelBg)
            .border(1.0, Token::PanelBorder)
            .radius(theme().panel_radius),
        &[heading, row],
    )
}

fn ask_button(ui: &mut Ui, text: &str, colors: ButtonColors, asks: &Asks, ask: Ask) -> WidgetId {
    let asks = asks.clone();
    ui.add(Button::new(text, body(), colors, move || {
        asks.borrow_mut().push(ask);
    }))
}

/// A popover menu, as the contextual toolbar's will be.
fn menu(ui: &mut Ui) -> WidgetId {
    let items: Vec<WidgetId> = ["Duplicate", "Bring to front", "Delete"]
        .into_iter()
        .map(|item| ui.add(Button::new(item, body(), ButtonColors::quiet(), || {})))
        .collect();
    ui.add_with_children(
        Flex::column()
            .gap(2.0)
            .padding(6.0, 6.0)
            .width(200.0)
            .background(Token::PanelBg)
            .border(1.0, Token::PanelBorder)
            .radius(theme().panel_radius)
            .shadow()
            .role(accesskit::Role::Menu, "Shape"),
        &items,
    )
}

/// The unsaved-changes dialog, as the mockups draw it ("Closing a board
/// with unsaved changes"): an icon tile beside the title and text, then
/// Don't save on the left and Cancel and Save on the right. The focus
/// starts on Save, so Enter doesn't throw the changes away.
fn dialog(ui: &mut Ui, asks: &Asks) -> (WidgetId, WidgetId) {
    let icon = ui.add(Icon::named("file-plus", 20.0, Token::OnWarningSoft));
    let tile = ui.add_with_children(
        Flex::row()
            .width(40.0)
            .height(40.0)
            .background(Token::WarningSoft)
            .radius(10.0)
            .style(|style| {
                style.justify_content = taffy::JustifyContent::CENTER;
                style.flex_shrink = 0.0;
            }),
        &[icon],
    );
    let title = ui.add(Label::new(
        "Save changes to “Untitled board” before closing?",
        TextStyle::new(17.0).weight(600.0),
    ));
    let text = ui.add(
        Label::new(
            "If you don't save, your changes are lost. Once it's saved, every change is saved on its own.",
            TextStyle::new(14.0).line_height(1.5),
        )
        .color(Token::TextSecondary),
    );
    let words = ui.add_with_children(Flex::column().gap(6.0).grow(1.0), &[title, text]);
    let top = ui.add_with_children(
        Flex::row()
            .gap(14.0)
            .style(|style| style.align_items = taffy::AlignItems::FLEX_START),
        &[tile, words],
    );
    let button = |ui: &mut Ui, text: &str, colors: ButtonColors, padding: f64, weight: f32| {
        let asks = asks.clone();
        ui.add(
            Button::new(
                text,
                TextStyle::new(14.0).weight(weight),
                colors,
                move || {
                    asks.borrow_mut().push(Ask::CloseDialog);
                },
            )
            .padding(padding, 0.0)
            .min_height(36.0),
        )
    };
    let discard = button(ui, "Don't save", ButtonColors::danger(), 12.0, 500.0);
    let spacer = ui.add(Flex::row().grow(1.0));
    let cancel = button(ui, "Cancel", ButtonColors::secondary(), 14.0, 500.0);
    let save = button(ui, "Save…", ButtonColors::primary(), 16.0, 600.0);
    let buttons = ui.add_with_children(
        Flex::row()
            .gap(8.0)
            .style(|style| style.margin.top = taffy::prelude::length(6.0)),
        &[discard, spacer, cancel, save],
    );
    let content = ui.add_with_children(
        Flex::column()
            .gap(14.0)
            .width(460.0)
            .background(Token::PanelBg)
            .border(1.0, Token::PanelBorder)
            .radius(16.0)
            .shadow()
            .role(accesskit::Role::Dialog, "Save changes")
            .style(|style| {
                style.padding = taffy::Rect {
                    left: taffy::prelude::length(22.0),
                    right: taffy::prelude::length(22.0),
                    top: taffy::prelude::length(22.0),
                    bottom: taffy::prelude::length(18.0),
                };
            }),
        &[top, buttons],
    );
    (content, save)
}

fn tooltip(ui: &mut Ui) -> WidgetId {
    let text = ui.add(Label::new("Rectangle · R", TextStyle::new(12.0)).color(Token::OnAccent));
    ui.add_with_children(
        Flex::row()
            .padding(10.0, 5.0)
            .background(Token::TooltipBg)
            .radius(7.0)
            .role(accesskit::Role::Tooltip, "Rectangle · R"),
        &[text],
    )
}

fn main() -> Result<(), UiError> {
    let args: Vec<String> = std::env::args().collect();
    let flag = |name: &str| args.iter().position(|arg| arg == name);
    let value = |name: &str| flag(name).and_then(|at| args.get(at + 1)).cloned();
    if flag("--light").is_some() {
        set_theme(Theme::light(DEFAULT_ACCENT));
    }
    let asks: Asks = Rc::default();
    let openers: Rc<Cell<[Option<WidgetId>; 3]>> = Rc::default();
    let mut ui = Ui::new();

    // The page title and the theme switch.
    let title = ui.add(Label::new(
        "Component gallery",
        TextStyle::new(20.0).weight(700.0),
    ));
    let spring = ui.add(Flex::row().grow(1.0));
    let switch = ui.add(Button::new(
        "Switch theme",
        body(),
        ButtonColors::quiet(),
        || {
            set_theme(match theme().kind() {
                ThemeKind::Dark => Theme::light(DEFAULT_ACCENT),
                ThemeKind::Light => Theme::dark(DEFAULT_ACCENT),
            });
        },
    ));
    let header = ui.add_with_children(Flex::row().gap(12.0), &[title, spring, switch]);

    // Buttons: hover, press and Tab show their other states.
    let styles = [
        ("Save…", ButtonColors::primary()),
        ("Cancel", ButtonColors::secondary()),
        ("More", ButtonColors::quiet()),
        ("Don't save", ButtonColors::danger()),
    ];
    let buttons: Vec<WidgetId> = styles
        .into_iter()
        .map(|(text, colors)| {
            ui.add(
                Button::new(text, body(), colors, || {})
                    .padding(14.0, 0.0)
                    .min_height(36.0),
            )
        })
        .collect();
    let buttons = section(
        &mut ui,
        "Buttons · primary, secondary, quiet, danger",
        &buttons,
    );

    // Inputs.
    let empty = ui.add(TextInput::new(
        "Name",
        "Board name",
        body(),
        InputColors::themed(),
    ));
    let empty = ui.add_with_children(Flex::column().width(220.0), &[empty]);
    let inputs = section(
        &mut ui,
        "Text input · placeholder, focus with a click",
        &[empty],
    );

    // Text in every text token.
    let texts: Vec<WidgetId> = [
        ("Text", Token::Text),
        ("Muted", Token::TextMuted),
        ("Subtle", Token::TextSubtle),
        ("Accent", Token::AccentInk),
        ("Danger", Token::Danger),
        ("Warning", Token::Warning),
    ]
    .into_iter()
    .map(|(name, token)| ui.add(Label::new(name, body()).color(token)))
    .collect();
    let text = section(&mut ui, "Text colors", &texts);

    // Layers.
    let popover = ask_button(
        &mut ui,
        "Open a menu",
        ButtonColors::quiet(),
        &asks,
        Ask::Popover,
    );
    let dialog = ask_button(
        &mut ui,
        "Open a dialog",
        ButtonColors::quiet(),
        &asks,
        Ask::Dialog,
    );
    let tip = ask_button(
        &mut ui,
        "Toggle a tooltip",
        ButtonColors::quiet(),
        &asks,
        Ask::Tooltip,
    );
    openers.set([Some(popover), Some(dialog), Some(tip)]);
    let layers = section(
        &mut ui,
        "Layers · Esc or a click outside closes a menu",
        &[popover, dialog, tip],
    );

    // Scrolling: 10,000 rows, of which only those in view exist.
    let rows = ui.add_virtual_list(
        VirtualList::new(
            32.0,
            || 10_000,
            |ui, row| {
                let name = ui.add(Label::new(format!("Board {}", row + 1), body()));
                let spring = ui.add(Flex::row().grow(1.0));
                let days = row % 30 + 1;
                let when = if days == 1 {
                    "yesterday".to_owned()
                } else {
                    format!("{days} days ago")
                };
                let when = ui.add(Label::new(when, TextStyle::new(12.0)).color(Token::TextSubtle));
                // Room on the right for the scroll bar.
                ui.add_with_children(
                    Flex::row().fill().style(|style| {
                        style.padding.left = taffy::prelude::length(10.0);
                        style.padding.right = taffy::prelude::length(20.0);
                    }),
                    &[name, spring, when],
                )
            },
        )
        .label("Recent boards"),
        Scroll::new().height(192.0).label("Recent boards"),
    );
    let rows = ui.add_with_children(Flex::column().grow(1.0), &[rows]);
    let scrolling = section(
        &mut ui,
        "Scrolling · 10,000 rows, built as they come into view",
        &[rows],
    );

    // Every vendored icon, with its name.
    let cells: Vec<WidgetId> = imaginarium_ui::icons::icon_names()
        .map(|name| {
            let icon = ui.add(Icon::named(name, 20.0, Token::Text).label(name));
            let text = ui.add(
                Label::new(name, TextStyle::new(10.5))
                    .color(Token::TextSubtle)
                    .single_line(),
            );
            ui.add_with_children(
                Flex::column()
                    .gap(6.0)
                    .width(92.0)
                    .height(56.0)
                    .style(|style| style.align_items = taffy::AlignItems::CENTER),
                &[icon, text],
            )
        })
        .collect();
    let grid = ui.add_with_children(
        Flex::row()
            .gap(4.0)
            .style(|style| style.flex_wrap = taffy::FlexWrap::Wrap),
        &cells,
    );
    let grid = ui.add_with_children(Flex::column().grow(1.0), &[grid]);
    let icons_title = format!(
        "Icons · Lucide {}, {} vendored",
        imaginarium_ui::icons::LUCIDE_VERSION,
        imaginarium_ui::icons::icon_names().count()
    );
    let icons = section(&mut ui, &icons_title, &[grid]);

    let page = ui.add_with_children(
        Flex::column()
            .gap(16.0)
            .padding(24.0, 24.0)
            .background(Token::BoardBg),
        &[header, buttons, inputs, text, layers, scrolling, icons],
    );
    // The gallery itself scrolls once it is taller than the window.
    let page = ui.add_with_children(
        Scroll::new().style(|style| {
            style.size.height = taffy::prelude::percent(1.0);
        }),
        &[page],
    );
    let window = ui.add_with_children(Flex::column().fill().background(Token::BoardBg), &[page]);
    ui.set_root(window);

    match value("--open").as_deref() {
        Some("popover") => asks.borrow_mut().push(Ask::Popover),
        Some("dialog") => asks.borrow_mut().push(Ask::Dialog),
        Some("tooltip") => asks.borrow_mut().push(Ask::Tooltip),
        _ => {}
    }
    let options = WindowOptions {
        title: "Component gallery".into(),
        size: Size::new(720.0, 560.0),
        ..WindowOptions::default()
    };
    let view = Gallery {
        fonts: Fonts::new(),
        ui,
        asks,
        openers,
        dialog: None,
        tooltip: None,
        close_after: value("--frames").and_then(|frames| frames.parse().ok()),
        frames: 0,
    };
    imaginarium_ui::run(options, view)?;
    Ok(())
}
