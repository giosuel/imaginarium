// SPDX-License-Identifier: AGPL-3.0-or-later

//! The inspector (`shell.md` §3.6, the Inspector mockups): a panel on the
//! right, 300 px wide, `Ctrl+I`, for what the contextual toolbar is too
//! small for: name and kind; the shape's type and corner radius;
//! position, size, rotation; fill; outline; text; layer (opacity, lock,
//! hide); the named style and Reset to style. Every field runs a command
//! with arguments, the ones the palette and the AI run.

use std::collections::HashMap;

use accesskit::Role;
use imaginarium_canvas::BoardCanvas;
use imaginarium_canvas::commands_style::preset_of;
use imaginarium_canvas::commands_style::{StyleTarget, first_target};
use imaginarium_canvas::node_look;
use imaginarium_canvas::style::{parse_color, text_look};
use imaginarium_commands::{ArgValue, Args, Availability, Registry};
use imaginarium_model::{Geometry, NodeId, NodeKind, Value};
use imaginarium_render::peniko::Color;
use imaginarium_text::{Fonts, TextStyle};
use imaginarium_ui::{
    ButtonBox, ButtonColors, Flex, Icon, IconButton, InputColors, Label, Placement, Scroll, Slider,
    Swatch, SwatchShape, TextInput, Token, Ui, WidgetId, theme,
};
use taffy::prelude::length;

use crate::window::{Ask, Asks};

/// A field of the inspector that keeps its keys across a rebuild.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Field {
    Name,
    X,
    Y,
    W,
    H,
    Rotation,
    Radius,
    FillHex,
    FillOpacity,
    StrokeHex,
    StrokeWidth,
    TextHex,
    TextSize,
    /// Controls other than text fields, which keep the keys across a
    /// rebuild too.
    RadiusSlider,
    OpacitySlider,
    KeepProportions,
    Lock,
    Hide,
    Reset,
    Popup(Popup),
}

/// A control that opens a popover next to it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Popup {
    Type,
    Fill,
    Stroke,
    TextColor,
    Style,
}

/// What the inspector shows: rebuilt when this changes.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Snapshot {
    pub(crate) selection: Vec<NodeId>,
    title: String,
    badge: &'static str,
    name: Option<String>,
    /// One unlocked node's box (position and size are per node).
    pub(crate) geometry: Option<Geometry>,
    pub(crate) preset: Option<String>,
    /// The corner radius and the most it can be.
    radius: Option<(f64, f64)>,
    /// Stored (`#rrggbb[aa]`) and shown colors.
    pub(crate) fill: Option<(Option<String>, Option<Color>)>,
    pub(crate) stroke: Option<(Option<String>, Option<Color>, f64)>,
    pub(crate) text: Option<(Option<String>, Color, f64)>,
    /// The layer's opacity, where it applies.
    opacity: Option<f64>,
    /// Lock (false) or Unlock (true), where either applies.
    lock: Option<bool>,
    hide: bool,
    /// The named style (id, name) and the board's styles.
    pub(crate) style: Option<(String, String)>,
    pub(crate) styles: Vec<(String, String)>,
    overrides: Vec<String>,
}

fn number(value: f64) -> String {
    if (value - value.round()).abs() < 0.05 {
        format!("{value:.0}")
    } else {
        format!("{value:.1}")
    }
}

/// What a kind is called in the header, and its badge.
fn kind_title(kind: &NodeKind, preset: Option<&str>) -> (&'static str, &'static str) {
    match kind {
        NodeKind::Shape => (
            match preset.unwrap_or("rect") {
                "ellipse" => "Ellipse",
                "diamond" => "Diamond",
                "triangle" => "Triangle",
                "line" => "Line",
                _ => "Rectangle",
            },
            "Shape",
        ),
        NodeKind::Text => ("Text", "Text"),
        NodeKind::Frame => ("Frame", "Frame"),
        NodeKind::Group => ("Group", "Group"),
        _ => ("Node", "Node"),
    }
}

/// The inspector's view of `board`'s selection now (`None`: nothing is
/// selected).
pub(crate) fn snapshot(board: &BoardCanvas, commands: &Registry<BoardCanvas>) -> Option<Snapshot> {
    let selection = board.selection().get_untracked();
    let first = *selection.first()?;
    let view = board.document().view();
    let node = view.node(first)?;
    // Each command's availability, asked once.
    let availability = |id: &str| {
        commands
            .get(id)
            .map_or(Availability::Hidden, |command| command.availability(board))
    };
    let shows = |id: &str| availability(id) != Availability::Hidden;
    let theme = theme();
    let kind = theme.kind();
    let preset = matches!(node.kind(), NodeKind::Shape).then(|| preset_of(node));
    let title = if selection.len() > 1 {
        format!("{} selected", selection.len())
    } else {
        kind_title(node.kind(), preset.as_deref()).0.to_owned()
    };
    let badge = if selection.len() > 1 {
        "Selection"
    } else {
        kind_title(node.kind(), preset.as_deref()).1
    };
    // Each section shows the first node it applies to, as the board draws
    // it (a frame with its frame colors).
    let target = |which| first_target(board, which);
    let stored = |node: NodeId, prop: &str| {
        view.style_prop(node, prop)
            .and_then(Value::as_str)
            .map(str::to_ascii_lowercase)
    };
    let look = |node: NodeId| node_look(view, node, &theme);
    let locked = board.is_locked(first);
    let single = selection.len() == 1;
    let style_name = |id: &imaginarium_model::StyleId| {
        view.style(id)
            .and_then(|style| style.name())
            .map_or_else(|| id.as_str().to_owned(), str::to_owned)
    };
    let mut styles: Vec<(String, String)> = view
        .style_ids()
        .map(|id| (id.as_str().to_owned(), style_name(id)))
        .collect();
    styles.sort_by(|a, b| a.1.cmp(&b.1));
    let fill_shows = shows("style.set_fill");
    let fill = fill_shows
        .then(|| target(StyleTarget::Fill))
        .flatten()
        .map(|node| (stored(node, "fill"), look(node).fill));
    let stroke = shows("style.set_stroke")
        .then(|| target(StyleTarget::Stroke))
        .flatten()
        .map(|node| {
            let look = look(node);
            (stored(node, "stroke"), look.stroke, look.stroke_width)
        });
    let text = (!fill_shows && shows("style.set_text_color"))
        .then(|| target(StyleTarget::Text))
        .flatten()
        .map(|node| {
            let look = text_look(view, node, kind, theme.color(Token::Text), 16.0, 400.0);
            (stored(node, "color"), look.color, f64::from(look.size))
        });
    let radius = shows("style.set_radius")
        .then(|| target(StyleTarget::Radius))
        .flatten()
        .and_then(|node| {
            let size = view.node(node)?.geometry()?;
            Some((look(node).radius, (size.w.min(size.h) / 2.0).max(1.0)))
        });
    // Unlock for a locked node, Lock otherwise, where the command runs.
    let lock = if locked {
        availability("board.unlock").is_enabled().then_some(true)
    } else {
        availability("board.lock").is_enabled().then_some(false)
    };
    Some(Snapshot {
        selection: selection.clone(),
        title,
        badge,
        name: (single && shows("board.rename")).then(|| node.name().unwrap_or("").to_owned()),
        geometry: (single && !locked).then(|| node.geometry()).flatten(),
        preset: shows("shape.set_preset").then_some(preset).flatten(),
        radius,
        fill,
        stroke,
        text,
        opacity: shows("style.set_opacity").then(|| f64::from(look(first).opacity)),
        lock,
        hide: shows("board.set_hidden"),
        style: node
            .style()
            .map(|id| (id.as_str().to_owned(), style_name(id))),
        styles,
        overrides: node
            .style_overrides()
            .iter()
            .map(|(key, _)| key.to_owned())
            .collect(),
    })
}

/// What the name field shows without a name: the node's first line of
/// text (built only for a rebuild, not compared each frame).
pub(crate) fn label_of(board: &BoardCanvas, node: NodeId) -> String {
    board
        .document()
        .view()
        .node(node)
        .and_then(|node| node.text())
        .map(|text| {
            text.to_plain_string()
                .trim()
                .lines()
                .next()
                .unwrap_or("")
                .to_owned()
        })
        .filter(|text| !text.is_empty())
        .unwrap_or_else(|| "Name".to_owned())
}

/// The panel's widgets.
pub(crate) struct Inspector {
    pub(crate) panel: WidgetId,
    pub(crate) open: bool,
    pub(crate) shown: Option<Option<Snapshot>>,
    children: Vec<WidgetId>,
    pub(crate) fields: HashMap<Field, WidgetId>,
    /// What each text field showed when built: Enter on it unchanged does
    /// nothing (its value may be rounded).
    pub(crate) texts: HashMap<Field, String>,
    /// The snapshot shown, for the window to act on fields and popups.
    pub(crate) snapshot: Option<Snapshot>,
    pub(crate) popups: HashMap<Popup, WidgetId>,
    /// Width and height change together (the Keep proportions toggle).
    pub(crate) keep_proportions: bool,
}

/// A section's heading and its rows, with a line above.
fn section(ui: &mut Ui, title: &str, rows: &[WidgetId]) -> WidgetId {
    let heading = ui.add(
        Label::new(title, TextStyle::new(12.0).weight(600.0))
            .color(Token::TextMuted)
            .single_line(),
    );
    let mut children = vec![heading];
    children.extend_from_slice(rows);
    let body = ui.add_with_children(Flex::column().gap(10.0).padding(16.0, 14.0), &children);
    let line = ui.add(Flex::row().height(1.0).background(Token::Divider));
    ui.add_with_children(Flex::column(), &[line, body])
}

/// A row: a label 92 px wide, then `controls`.
fn labelled(ui: &mut Ui, label: &str, controls: &[WidgetId]) -> WidgetId {
    let text = ui.add(
        Label::new(label, TextStyle::new(13.0))
            .color(Token::TextMuted)
            .single_line(),
    );
    let text = ui.add_with_children(Flex::row().width(92.0), &[text]);
    let mut children = vec![text];
    children.extend_from_slice(controls);
    ui.add_with_children(Flex::row().gap(8.0), &children)
}

impl Inspector {
    /// A closed inspector.
    pub(crate) fn new(ui: &mut Ui) -> Self {
        let panel = ui.add(
            Flex::column()
                .width(300.0)
                .background(Token::PanelBg)
                .border(1.0, Token::PanelBorder)
                .radius(14.0)
                .shadow()
                .clip()
                .role(Role::Pane, "Inspector")
                .style(|style| {
                    style.position = taffy::Position::Absolute;
                    style.display = taffy::Display::None;
                    style.inset = taffy::Rect {
                        left: taffy::LengthPercentageAuto::auto(),
                        right: length(16.0),
                        top: length(72.0),
                        // Above the bottom row (help, a bottom tool bar).
                        bottom: length(16.0 + 44.0 + 8.0),
                    };
                }),
        );
        Self {
            panel,
            open: false,
            shown: None,
            children: Vec::new(),
            fields: HashMap::new(),
            texts: HashMap::new(),
            snapshot: None,
            popups: HashMap::new(),
            keep_proportions: true,
        }
    }

    /// Opens or closes it.
    pub(crate) fn set_open(&mut self, ui: &mut Ui, open: bool) {
        self.open = open;
        self.shown = None;
        if let Some(panel) = ui.widget_mut::<Flex>(self.panel) {
            panel.style_mut().display = if open {
                taffy::Display::Flex
            } else {
                taffy::Display::None
            };
        }
    }

    /// Builds the panel for `snapshot` (none: nothing selected).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn rebuild(
        &mut self,
        ui: &mut Ui,
        fonts: &mut Fonts,
        snapshot: Option<Snapshot>,
        label: &str,
        close_keys: Option<String>,
        asks: &Asks,
    ) {
        for child in self.children.drain(..) {
            ui.remove(child);
        }
        self.fields.clear();
        self.texts.clear();
        self.popups.clear();
        let mut children = Vec::new();
        let close = IconButton::new("x", "Close the inspector", {
            let asks = asks.clone();
            move || asks.borrow_mut().push(Ask::ToggleInspector)
        })
        .size(30.0, 30.0, 8.0, 16.0)
        .shortcut(close_keys.unwrap_or_default())
        .tooltip_placement(Placement::Left)
        .add_to(ui);
        match &snapshot {
            None => {
                let title = ui.add(
                    Label::new("Inspector", TextStyle::new(15.0).weight(600.0))
                        .color(Token::Text)
                        .single_line()
                        .grow(1.0),
                );
                let header = ui.add_with_children(
                    Flex::row().gap(8.0).style(|style| {
                        style.padding = padding(16.0, 10.0, 12.0, 10.0);
                    }),
                    &[title, close],
                );
                let hint = ui.add(
                    Label::new(
                        "Select something on the board to see and change it here.",
                        TextStyle::new(13.0),
                    )
                    .color(Token::TextMuted),
                );
                let hint = ui.add_with_children(Flex::column().padding(16.0, 8.0), &[hint]);
                children.extend([header, hint]);
            }
            Some(snapshot) => {
                children.push(self.header(ui, fonts, snapshot, label, close, asks));
                let mut sections = Vec::new();
                self.sections(ui, fonts, snapshot, asks, &mut sections);
                let body = ui.add_with_children(Flex::column(), &sections);
                let scroll = ui.add_with_children(
                    Scroll::new().label("Inspector").grow(1.0).style(|style| {
                        style.min_size.height = length(0.0);
                    }),
                    &[body],
                );
                children.push(scroll);
            }
        }
        ui.set_children(self.panel, &children);
        self.children = children;
        self.snapshot = snapshot.clone();
        self.shown = Some(snapshot);
    }

    #[allow(clippy::too_many_arguments)]
    fn header(
        &mut self,
        ui: &mut Ui,
        fonts: &mut Fonts,
        snapshot: &Snapshot,
        label: &str,
        close: WidgetId,
        asks: &Asks,
    ) -> WidgetId {
        let title = ui.add(
            Label::new(snapshot.title.clone(), TextStyle::new(15.0).weight(600.0))
                .color(Token::Text)
                .single_line(),
        );
        let badge_text = ui.add(
            Label::new(snapshot.badge, TextStyle::new(12.0).weight(500.0))
                .color(Token::TextMuted)
                .single_line(),
        );
        let badge = ui.add_with_children(
            Flex::row()
                .padding(7.0, 2.0)
                .radius(6.0)
                .background(Token::ControlHover),
            &[badge_text],
        );
        let spring = ui.add(Flex::row().grow(1.0));
        let row = ui.add_with_children(
            Flex::row().gap(8.0).style(|style| {
                style.padding = padding(16.0, 10.0, 12.0, 10.0);
            }),
            &[title, badge, spring, close],
        );
        let Some(name) = &snapshot.name else {
            return row;
        };
        let field = self.input(ui, fonts, Field::Name, "Name", label, name, false, asks);
        let field = ui.add_with_children(
            Flex::row().style(|style| style.padding = padding(16.0, 16.0, 0.0, 14.0)),
            &[field],
        );
        ui.add_with_children(Flex::column(), &[row, field])
    }

    /// A text field for `field`, showing `value`; Enter asks to set it.
    #[allow(clippy::too_many_arguments)]
    fn input(
        &mut self,
        ui: &mut Ui,
        fonts: &mut Fonts,
        field: Field,
        label: &str,
        placeholder: &str,
        value: &str,
        bare: bool,
        asks: &Asks,
    ) -> WidgetId {
        let asks = asks.clone();
        let input = TextInput::new(
            label,
            placeholder,
            TextStyle::new(13.0),
            InputColors::themed(),
        )
        .on_submit(move |text| {
            asks.borrow_mut()
                .push(Ask::InspectorField(field, text.trim().to_owned()))
        });
        let input = ui.add(if bare { input.bare() } else { input });
        if let Some(widget) = ui.widget_mut::<TextInput>(input) {
            widget.set_text(fonts, value);
        }
        self.fields.insert(field, input);
        self.texts.insert(field, value.to_owned());
        input
    }

    /// A small field in a 30 px box with a prefix inside ("X", "#").
    #[allow(clippy::too_many_arguments)]
    fn boxed(
        &mut self,
        ui: &mut Ui,
        fonts: &mut Fonts,
        field: Field,
        prefix: &str,
        label: &str,
        value: &str,
        placeholder: &str,
        width: Option<f32>,
        asks: &Asks,
    ) -> WidgetId {
        let input = self.input(ui, fonts, field, label, placeholder, value, true, asks);
        let input = ui.add_with_children(
            Flex::row()
                .grow(1.0)
                .style(|style| style.min_size.width = length(0.0)),
            &[input],
        );
        let mut children = Vec::new();
        if !prefix.is_empty() {
            children.push(
                ui.add(
                    Label::new(prefix, TextStyle::new(12.0))
                        .color(Token::TextSubtle)
                        .single_line(),
                ),
            );
        }
        children.push(input);
        let boxed = Flex::row()
            .height(30.0)
            .gap(2.0)
            .border(1.0, Token::ControlBorder)
            .radius(8.0)
            .style(|style| {
                style.padding.left = length(8.0);
                style.min_size.width = length(0.0);
            });
        let boxed = match width {
            Some(width) => boxed.width(width),
            None => boxed.grow(1.0),
        };
        ui.add_with_children(boxed, &children)
    }

    /// A 30 px swatch of `color` that opens the colors of `popup`.
    fn swatch_button(
        &mut self,
        ui: &mut Ui,
        popup: Popup,
        label: &str,
        color: Option<Color>,
        asks: &Asks,
    ) -> WidgetId {
        let swatch = ui.add(Swatch::shown(color, SwatchShape::Square, 30.0));
        let asks = asks.clone();
        let button = ui.add_with_children(
            ButtonBox::new(label, ButtonColors::quiet(), move || {
                asks.borrow_mut().push(Ask::InspectorOpen(popup))
            })
            .size(30.0, 30.0)
            .padding(0.0, 0.0)
            .keep_focus(),
            &[swatch],
        );
        self.popups.insert(popup, button);
        self.fields.insert(Field::Popup(popup), button);
        button
    }

    fn popup_button(
        &mut self,
        ui: &mut Ui,
        popup: Popup,
        label: &str,
        children: &[WidgetId],
        asks: &Asks,
    ) -> WidgetId {
        let asks = asks.clone();
        let button = ui.add_with_children(
            ButtonBox::new(
                label,
                ButtonColors {
                    border: Some(Token::ControlBorder.into()),
                    ..ButtonColors::quiet()
                },
                move || asks.borrow_mut().push(Ask::InspectorOpen(popup)),
            )
            .height(30.0)
            .padding(8.0, 8.0)
            .start()
            .grow(1.0)
            .keep_focus(),
            children,
        );
        self.popups.insert(popup, button);
        self.fields.insert(Field::Popup(popup), button);
        button
    }

    #[allow(clippy::too_many_lines)]
    fn sections(
        &mut self,
        ui: &mut Ui,
        fonts: &mut Fonts,
        snapshot: &Snapshot,
        asks: &Asks,
        out: &mut Vec<WidgetId>,
    ) {
        let run = |command: &'static str, args: Args| {
            let asks = asks.clone();
            move || asks.borrow_mut().push(Ask::RunWith(command, args.clone()))
        };
        // The shape: its type and corner radius.
        if snapshot.preset.is_some() || snapshot.radius.is_some() {
            let mut rows = Vec::new();
            if let Some(preset) = &snapshot.preset {
                let (title, _) = kind_title(&NodeKind::Shape, Some(preset));
                let text = ui.add(
                    Label::new(title, TextStyle::new(13.0))
                        .color(Token::Text)
                        .single_line()
                        .grow(1.0),
                );
                let chevron = ui.add(Icon::named("chevron-down", 14.0, Token::TextMuted));
                let button =
                    self.popup_button(ui, Popup::Type, "Shape type", &[text, chevron], asks);
                rows.push(labelled(ui, "Type", &[button]));
            }
            if let Some((radius, most)) = snapshot.radius {
                let asks_radius = asks.clone();
                let slider = Slider::new(
                    "Corner radius",
                    0.0,
                    most,
                    move || radius,
                    move |value| {
                        let args = Args::new().with("radius", ArgValue::Float(value.round()));
                        asks_radius
                            .borrow_mut()
                            .push(Ask::RunWith("style.set_radius", args));
                    },
                )
                .add_to(ui);
                self.fields.insert(Field::RadiusSlider, slider);
                let field = self.boxed(
                    ui,
                    fonts,
                    Field::Radius,
                    "",
                    "Corner radius",
                    &number(radius),
                    "",
                    Some(52.0),
                    asks,
                );
                rows.push(labelled(ui, "Corner radius", &[slider, field]));
            }
            out.push(section(ui, "Shape", &rows));
        }
        // Position and size: one node's box.
        if let Some(geometry) = snapshot.geometry {
            let mut cells = Vec::new();
            for (field, prefix, label, value) in [
                (Field::X, "X", "X", geometry.x),
                (Field::Y, "Y", "Y", geometry.y),
                (Field::W, "W", "Width", geometry.w),
                (Field::H, "H", "Height", geometry.h),
                (Field::Rotation, "°", "Rotation", geometry.r),
            ] {
                cells.push(self.boxed(
                    ui,
                    fonts,
                    field,
                    prefix,
                    label,
                    &number(value),
                    "",
                    None,
                    asks,
                ));
            }
            let keep = self.keep_proportions;
            let icon = ui.add(Icon::named(
                if keep { "lock" } else { "link" },
                14.0,
                Token::TextMuted,
            ));
            let text = ui.add(
                Label::new("Keep proportions", TextStyle::new(12.0))
                    .color(if keep {
                        Token::AccentInk
                    } else {
                        Token::TextMuted
                    })
                    .single_line(),
            );
            let toggle = ui.add_with_children(
                ButtonBox::new(
                    "Keep proportions",
                    ButtonColors {
                        border: Some(Token::ControlBorder.into()),
                        ..ButtonColors::quiet()
                    },
                    {
                        let asks = asks.clone();
                        move || asks.borrow_mut().push(Ask::KeepProportions)
                    },
                )
                .height(30.0)
                .padding(8.0, 8.0)
                .gap(6.0)
                .grow(1.0)
                .keep_focus()
                .selected(move || keep),
                &[icon, text],
            );
            self.fields.insert(Field::KeepProportions, toggle);
            cells.push(toggle);
            let rows: Vec<WidgetId> = cells
                .chunks(2)
                .map(|pair| ui.add_with_children(Flex::row().gap(8.0), pair))
                .collect();
            out.push(section(ui, "Position and size", &rows));
        }
        // Fill, stroke and text colors: a swatch that opens the colors,
        // the hex, and the opacity or width.
        if let Some((stored, color)) = &snapshot.fill {
            let pick = self.swatch_button(ui, Popup::Fill, "Fill color", *color, asks);
            let hex = hex_of(stored.as_deref());
            let hex = self.boxed(
                ui,
                fonts,
                Field::FillHex,
                "#",
                "Fill, hex",
                &hex,
                "none",
                None,
                asks,
            );
            let opacity = opacity_of(stored.as_deref());
            let opacity = self.boxed(
                ui,
                fonts,
                Field::FillOpacity,
                "",
                "Fill opacity, percent",
                &opacity,
                "",
                Some(64.0),
                asks,
            );
            let row = ui.add_with_children(Flex::row().gap(8.0), &[pick, hex, opacity]);
            out.push(section(ui, "Fill", &[row]));
        }
        if let Some((stored, color, width)) = &snapshot.stroke {
            let pick = self.swatch_button(ui, Popup::Stroke, "Stroke color", *color, asks);
            // Without a color of its own, the outline is the palette's ink.
            let hex = hex_of(stored.as_deref().or(Some("#1e293b")));
            let hex = self.boxed(
                ui,
                fonts,
                Field::StrokeHex,
                "#",
                "Stroke, hex",
                &hex,
                "",
                None,
                asks,
            );
            let width = self.boxed(
                ui,
                fonts,
                Field::StrokeWidth,
                "≡",
                "Stroke width",
                &number(*width),
                "",
                Some(64.0),
                asks,
            );
            let row = ui.add_with_children(Flex::row().gap(8.0), &[pick, hex, width]);
            out.push(section(ui, "Stroke", &[row]));
        }
        if let Some((stored, color, size)) = &snapshot.text {
            let pick = self.swatch_button(ui, Popup::TextColor, "Text color", Some(*color), asks);
            let hex = hex_of(stored.as_deref());
            let hex = self.boxed(
                ui,
                fonts,
                Field::TextHex,
                "#",
                "Text, hex",
                &hex,
                "auto",
                None,
                asks,
            );
            let size = self.boxed(
                ui,
                fonts,
                Field::TextSize,
                "pt",
                "Text size",
                &number(*size),
                "",
                Some(64.0),
                asks,
            );
            let row = ui.add_with_children(Flex::row().gap(8.0), &[pick, hex, size]);
            out.push(section(ui, "Text", &[row]));
        }
        // The layer: opacity, lock, hide, each where it applies.
        {
            let mut rows = Vec::new();
            if let Some(opacity) = snapshot.opacity {
                let asks_opacity = asks.clone();
                let slider = Slider::new(
                    "Opacity",
                    0.0,
                    1.0,
                    move || opacity,
                    move |value| {
                        let args = Args::new()
                            .with("opacity", ArgValue::Float((value * 100.0).round() / 100.0));
                        asks_opacity
                            .borrow_mut()
                            .push(Ask::RunWith("style.set_opacity", args));
                    },
                )
                .add_to(ui);
                self.fields.insert(Field::OpacitySlider, slider);
                let value = ui.add(
                    Label::new(format!("{:.0} %", opacity * 100.0), TextStyle::new(13.0))
                        .color(Token::Text)
                        .single_line(),
                );
                let value = ui.add_with_children(Flex::row().width(44.0), &[value]);
                rows.push(labelled(ui, "Opacity", &[slider, value]));
            }
            let toggle = |ui: &mut Ui, icon: &str, name: &str, action: Box<dyn FnMut()>| {
                let icon = ui.add(Icon::named(icon, 14.0, Token::TextSecondary));
                let text = ui.add(
                    Label::new(name, TextStyle::new(13.0))
                        .color(Token::TextSecondary)
                        .single_line(),
                );
                ui.add_with_children(
                    ButtonBox::new(
                        name,
                        ButtonColors {
                            border: Some(Token::ControlBorder.into()),
                            ..ButtonColors::quiet()
                        },
                        action,
                    )
                    .height(30.0)
                    .gap(6.0)
                    .grow(1.0)
                    .keep_focus(),
                    &[icon, text],
                )
            };
            let mut buttons = Vec::new();
            if let Some(locked) = snapshot.lock {
                let lock = if locked {
                    toggle(
                        ui,
                        "lock",
                        "Unlock",
                        Box::new(run("board.unlock", Args::new())),
                    )
                } else {
                    toggle(ui, "lock", "Lock", Box::new(run("board.lock", Args::new())))
                };
                self.fields.insert(Field::Lock, lock);
                buttons.push(lock);
            }
            if snapshot.hide {
                let hide = toggle(
                    ui,
                    "eye-off",
                    "Hide",
                    Box::new(run(
                        "board.set_hidden",
                        Args::new().with("hidden", ArgValue::Bool(true)),
                    )),
                );
                self.fields.insert(Field::Hide, hide);
                buttons.push(hide);
            }
            if !buttons.is_empty() {
                rows.push(ui.add_with_children(Flex::row().gap(8.0), &buttons));
            }
            if !rows.is_empty() {
                out.push(section(ui, "Layer", &rows));
            }
        }
        // The named style.
        if !snapshot.styles.is_empty() || !snapshot.overrides.is_empty() {
            let mut rows = Vec::new();
            if !snapshot.styles.is_empty() {
                let name = snapshot
                    .style
                    .as_ref()
                    .map_or("No style".to_owned(), |(_, name)| name.clone());
                let text = ui.add(
                    Label::new(name, TextStyle::new(13.0))
                        .color(Token::Text)
                        .single_line()
                        .grow(1.0),
                );
                let chevron = ui.add(Icon::named("chevron-down", 14.0, Token::TextMuted));
                rows.push(self.popup_button(
                    ui,
                    Popup::Style,
                    "Named style",
                    &[text, chevron],
                    asks,
                ));
            }
            let changed = match snapshot.overrides.len() {
                0 => "Nothing changed here".to_owned(),
                1 => format!("{} changed here", snapshot.overrides[0]),
                count => format!("{count} values changed here"),
            };
            let note = ui.add(
                Label::new(changed, TextStyle::new(12.0))
                    .color(Token::TextSubtle)
                    .single_line()
                    .grow(1.0),
            );
            let mut line = vec![note];
            if !snapshot.overrides.is_empty() {
                let text = ui.add(
                    Label::new("Reset to style", TextStyle::new(12.0).weight(500.0))
                        .color(Token::AccentInk)
                        .single_line(),
                );
                let reset = ui.add_with_children(
                    ButtonBox::new(
                        "Reset to style",
                        ButtonColors::quiet(),
                        run("style.reset", Args::new()),
                    )
                    .height(24.0)
                    .padding(4.0, 4.0)
                    .keep_focus(),
                    &[text],
                );
                self.fields.insert(Field::Reset, reset);
                line.push(reset);
            }
            rows.push(ui.add_with_children(Flex::row().gap(8.0), &line));
            out.push(section(ui, "Style", &rows));
        }
    }
}

fn padding(left: f32, right: f32, top: f32, bottom: f32) -> taffy::Rect<taffy::LengthPercentage> {
    taffy::Rect {
        left: length(left),
        right: length(right),
        top: length(top),
        bottom: length(bottom),
    }
}

/// A stored color's hex digits for its field, without `#` and alpha.
fn hex_of(stored: Option<&str>) -> String {
    stored
        .and_then(parse_color)
        .map(|color| {
            let rgba = color.to_rgba8();
            format!("{:02X}{:02X}{:02X}", rgba.r, rgba.g, rgba.b)
        })
        .unwrap_or_default()
}

/// A stored color's opacity in percent.
fn opacity_of(stored: Option<&str>) -> String {
    stored.and_then(parse_color).map_or_else(
        || "100".to_owned(),
        |color| format!("{:.0}", f64::from(color.components[3]) * 100.0),
    )
}

/// The geometry a field's text asks for, from the current one, or `None`
/// when the text isn't a number.
pub(crate) fn geometry_for(
    field: Field,
    text: &str,
    current: Geometry,
    keep_proportions: bool,
) -> Option<Geometry> {
    let value: f64 = text.trim().trim_end_matches('°').trim().parse().ok()?;
    if !value.is_finite() {
        return None;
    }
    let mut next = current;
    match field {
        Field::X => next.x = value,
        Field::Y => next.y = value,
        Field::W => {
            next.w = value.max(0.0);
            if keep_proportions && current.w > 0.0 {
                next.h = current.h * next.w / current.w;
            }
        }
        Field::H => {
            next.h = value.max(0.0);
            if keep_proportions && current.h > 0.0 {
                next.w = current.w * next.h / current.h;
            }
        }
        Field::Rotation => next.r = value,
        _ => return None,
    }
    Some(next)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fields_turn_into_geometry() {
        let current = Geometry::new(10.0, 20.0, 200.0, 100.0);
        let wider = geometry_for(Field::W, "400", current, true).unwrap();
        assert_eq!((wider.w, wider.h), (400.0, 200.0));
        let free = geometry_for(Field::W, "400", current, false).unwrap();
        assert_eq!((free.w, free.h), (400.0, 100.0));
        assert_eq!(
            geometry_for(Field::Rotation, "45°", current, true)
                .unwrap()
                .r,
            45.0
        );
        assert!(geometry_for(Field::X, "left", current, true).is_none());
        assert_eq!(hex_of(Some("#e0e7ff80")), "E0E7FF");
        assert_eq!(opacity_of(Some("#e0e7ff80")), "50");
        assert_eq!(number(1.5), "1.5");
        assert_eq!(number(12.0), "12");
    }
}
