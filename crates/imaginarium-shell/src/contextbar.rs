// SPDX-License-Identifier: AGPL-3.0-or-later

//! The contextual toolbar (`shell.md` §3.5, the PartContextBar mockup):
//! above the selection (below it when there's no room), built from the
//! commands available for it: fill, outline and its width, corner radius
//! or text color; arrange, duplicate, delete; AI and More. Each style
//! control opens a popover of choices that run the style commands. It
//! steps aside while the pointer moves something.

use accesskit::Role;
use imaginarium_canvas::BoardCanvas;
use imaginarium_canvas::style::{PALETTE, parse_color, shape_style, shown, text_look};
use imaginarium_commands::{ArgValue, Args, Availability, Keymap, Registry};
use imaginarium_model::{NodeId, Value};
use imaginarium_render::kurbo::{Point, Rect, Size};
use imaginarium_render::peniko::Color;
use imaginarium_text::TextStyle;
use imaginarium_ui::{
    ButtonBox, ButtonColors, Flex, Icon, IconButton, InputColors, Label, Placement, Swatch,
    SwatchShape, TextInput, Token, Ui, WidgetId, theme,
};
use taffy::prelude::length;

use crate::toolbar::shortcut;
use crate::window::{Ask, Asks};

/// A control of the bar.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Control {
    Fill,
    Stroke,
    Width,
    Radius,
    TextColor,
    Arrange,
    Duplicate,
    Delete,
    Ai,
    More,
}

/// What the bar shows: rebuilt when this changes.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct BarState {
    selection: Vec<NodeId>,
    controls: Vec<Control>,
    /// The first target's look, as shown in the theme.
    fill: Option<Color>,
    stroke: Option<Color>,
    width: f64,
    radius: f64,
    text: Option<Color>,
    /// The stored values, to mark the current choice in a popover.
    stored_fill: Option<String>,
    stored_stroke: Option<String>,
    stored_text: Option<String>,
    disabled: Vec<Control>,
}

impl BarState {
    /// The outline width shown.
    pub(crate) fn width(&self) -> f64 {
        self.width
    }

    /// The corner radius shown.
    pub(crate) fn radius(&self) -> f64 {
        self.radius
    }
}

/// The commands behind the style controls.
const STYLE: [(Control, &str); 5] = [
    (Control::Fill, "style.set_fill"),
    (Control::Stroke, "style.set_stroke"),
    (Control::Width, "style.set_stroke_width"),
    (Control::Radius, "style.set_radius"),
    (Control::TextColor, "style.set_text_color"),
];

const Z_ORDER: [&str; 4] = [
    "board.bring_to_front",
    "board.bring_forward",
    "board.send_backward",
    "board.send_to_back",
];

/// The bar for `board`'s selection now, or `None` when it has none.
pub(crate) fn state(board: &BoardCanvas, commands: &Registry<BoardCanvas>) -> Option<BarState> {
    let selection = board.selection().get_untracked();
    if selection.is_empty() {
        return None;
    }
    let availability = |id: &str| {
        commands
            .get(id)
            .map_or(Availability::Hidden, |command| command.availability(board))
    };
    let mut controls = Vec::new();
    let mut disabled = Vec::new();
    for (control, id) in STYLE {
        // Text color only where there's nothing to fill: text boxes. A
        // shape's label color is in the inspector.
        if control == Control::TextColor && controls.contains(&Control::Fill) {
            continue;
        }
        if availability(id) != Availability::Hidden {
            controls.push(control);
        }
    }
    let shows = |id: &str| availability(id) != Availability::Hidden;
    if Z_ORDER.iter().any(|id| shows(id)) {
        controls.push(Control::Arrange);
    }
    for (control, id) in [
        (Control::Duplicate, "board.duplicate"),
        (Control::Delete, "board.delete"),
    ] {
        match availability(id) {
            Availability::Hidden => {}
            Availability::Disabled(_) => {
                controls.push(control);
                disabled.push(control);
            }
            Availability::Enabled => controls.push(control),
        }
    }
    controls.extend([Control::Ai, Control::More]);
    let view = board.document().view();
    let kind = theme().kind();
    let first = selection[0];
    let look = shape_style(view, first, kind);
    let text = text_look(view, first, kind, theme().color(Token::Text), 16.0, 400.0);
    let stored = |prop: &str| {
        view.style_prop(first, prop)
            .and_then(Value::as_str)
            .map(str::to_ascii_lowercase)
    };
    Some(BarState {
        selection,
        controls,
        fill: look.fill,
        stroke: look.stroke,
        width: look.stroke_width,
        radius: look.radius,
        text: Some(text.color),
        stored_fill: stored("fill"),
        stored_stroke: stored("stroke"),
        stored_text: stored("color"),
        disabled,
    })
}

/// The bar's widgets.
pub(crate) struct ContextBar {
    pub(crate) panel: WidgetId,
    pub(crate) state: Option<BarState>,
    /// The control buttons, for popovers to open next to.
    pub(crate) buttons: Vec<(Control, WidgetId)>,
    /// Everything in the bar now (buttons and dividers).
    children: Vec<WidgetId>,
    /// Where it was placed, and whether it shows.
    pub(crate) placed: Option<Point>,
    /// Rebuilt since it was placed: place it again, even at the same spot.
    stale: bool,
    /// It sits below the selection (no room above): popovers go below it.
    pub(crate) below: bool,
}

impl ContextBar {
    /// An empty, hidden bar.
    pub(crate) fn new(ui: &mut Ui) -> Self {
        let panel = ui.add(
            Flex::row()
                .height(44.0)
                .padding(4.0, 0.0)
                .gap(2.0)
                .background(Token::PanelBg)
                .border(1.0, Token::PanelBorder)
                .radius(12.0)
                .shadow()
                .role(Role::Toolbar, "Selection")
                .style(|style| {
                    style.position = taffy::Position::Absolute;
                    style.display = taffy::Display::None;
                }),
        );
        Self {
            panel,
            state: None,
            buttons: Vec::new(),
            children: Vec::new(),
            placed: None,
            stale: true,
            below: false,
        }
    }

    /// The stored color of the first selected node for a color command
    /// (`#rrggbb[aa]`), if it has one of its own.
    pub(crate) fn stored_color(&self, command: &str) -> Option<String> {
        let state = self.state.as_ref()?;
        match command {
            "style.set_fill" => state.stored_fill.clone(),
            "style.set_stroke" => state.stored_stroke.clone(),
            _ => state.stored_text.clone(),
        }
    }

    /// The button of `control`, if the bar shows it.
    pub(crate) fn button(&self, control: Control) -> Option<WidgetId> {
        self.buttons
            .iter()
            .find(|(shown, _)| *shown == control)
            .map(|(_, id)| *id)
    }

    /// Builds the controls for `state` (none: the bar hides).
    pub(crate) fn rebuild(
        &mut self,
        ui: &mut Ui,
        state: Option<BarState>,
        keymap: &Keymap,
        asks: &Asks,
    ) {
        for child in self.children.drain(..) {
            ui.remove(child);
        }
        self.buttons.clear();
        let mut children = Vec::new();
        if let Some(state) = &state {
            let groups: [&[Control]; 3] = [
                &[
                    Control::Fill,
                    Control::Stroke,
                    Control::Width,
                    Control::Radius,
                    Control::TextColor,
                ],
                &[Control::Arrange, Control::Duplicate, Control::Delete],
                &[Control::Ai, Control::More],
            ];
            for group in groups {
                let shown: Vec<Control> = group
                    .iter()
                    .copied()
                    .filter(|control| state.controls.contains(control))
                    .collect();
                if shown.is_empty() {
                    continue;
                }
                if !children.is_empty() {
                    children.push(
                        ui.add(
                            Flex::row()
                                .width(1.0)
                                .height(20.0)
                                .background(Token::PanelBorder)
                                .style(|style| {
                                    style.margin.left = length(4.0);
                                    style.margin.right = length(4.0);
                                    style.flex_shrink = 0.0;
                                }),
                        ),
                    );
                }
                for control in shown {
                    let button = control_button(ui, control, state, keymap, asks);
                    self.buttons.push((control, button));
                    children.push(button);
                }
            }
        }
        ui.set_children(self.panel, &children);
        self.children = children;
        self.state = state;
        self.stale = true;
    }

    /// Places the bar for a selection at `selection` (window space) in a
    /// window of `size`, or hides it (`None`). Returns whether it moved.
    pub(crate) fn place(&mut self, ui: &mut Ui, selection: Option<Rect>, size: Size) -> bool {
        let bar = ui
            .bounds(self.panel)
            .map_or(Size::ZERO, |bounds| bounds.size());
        let spot = selection.map(|selection| {
            // Above the selection, clear of its turn handle; below it when
            // the panels at the top would cover it.
            const ABOVE: f64 = 32.0;
            const BELOW: f64 = 16.0;
            const TOP: f64 = 16.0 + 44.0 + 8.0;
            let above = selection.y0 - ABOVE - bar.height;
            let below = above < TOP;
            let y = if below { selection.y1 + BELOW } else { above };
            let x = selection.center().x - bar.width / 2.0;
            let x = x.clamp(16.0, (size.width - 16.0 - bar.width).max(16.0));
            let y = y.clamp(
                TOP.min(size.height),
                (size.height - 16.0 - 44.0 - 8.0 - bar.height).max(0.0),
            );
            (Point::new(x.round(), y.round()), below)
        });
        let placed = spot.map(|(at, _)| at);
        if placed == self.placed && !self.stale {
            return false;
        }
        self.placed = placed;
        self.stale = false;
        self.below = spot.is_some_and(|(_, below)| below);
        if let Some(panel) = ui.widget_mut::<Flex>(self.panel) {
            let style = panel.style_mut();
            match placed {
                Some(at) => {
                    style.display = taffy::Display::Flex;
                    style.inset = taffy::Rect {
                        left: length(at.x as f32),
                        top: length(at.y as f32),
                        right: taffy::LengthPercentageAuto::auto(),
                        bottom: taffy::LengthPercentageAuto::auto(),
                    };
                }
                None => style.display = taffy::Display::None,
            }
        }
        true
    }
}

fn control_button(
    ui: &mut Ui,
    control: Control,
    state: &BarState,
    keymap: &Keymap,
    asks: &Asks,
) -> WidgetId {
    let open = {
        let asks = asks.clone();
        move || asks.borrow_mut().push(Ask::BarOpen(control))
    };
    let run = |command: &'static str| {
        let asks = asks.clone();
        move || asks.borrow_mut().push(Ask::Run(command))
    };
    let plain = |ui: &mut Ui, label: &str, child: WidgetId, action: Box<dyn FnMut()>| {
        ui.add_with_children(
            ButtonBox::new(label, ButtonColors::quiet(), action)
                .size(36.0, 36.0)
                .keep_focus()
                .tooltip(label, Placement::Above),
            &[child],
        )
    };
    match control {
        Control::Fill => {
            let dot = ui.add(Swatch::shown(state.fill, SwatchShape::Dot, 18.0));
            plain(ui, "Fill", dot, Box::new(open))
        }
        Control::Stroke => {
            let ring = ui.add(Swatch::shown(state.stroke, SwatchShape::Ring, 18.0));
            plain(ui, "Outline", ring, Box::new(open))
        }
        Control::TextColor => {
            let dot = ui.add(Swatch::shown(state.text, SwatchShape::Dot, 18.0));
            plain(ui, "Text color", dot, Box::new(open))
        }
        Control::Width => {
            // The mockup's three lines (one stroke width here, for now).
            let icon = ui.add(Icon::new(
                &["M4 6h16", "M4 12h16", "M4 18h16"],
                20.0,
                Token::TextSecondary,
            ));
            plain(ui, "Outline width", icon, Box::new(open))
        }
        Control::Radius => {
            let icon = ui.add(Icon::new(
                &["M5 19v-8a6 6 0 0 1 6-6h8"],
                18.0,
                Token::TextSecondary,
            ));
            let value = ui.add(
                Label::new(
                    format!("{}", state.radius.round()),
                    TextStyle::new(13.0).weight(500.0),
                )
                .color(Token::TextSecondary)
                .single_line(),
            );
            ui.add_with_children(
                ButtonBox::new(
                    format!("Corner radius, {}", state.radius.round()),
                    ButtonColors::quiet(),
                    open,
                )
                .height(36.0)
                .padding(8.0, 8.0)
                .gap(6.0)
                .keep_focus()
                .tooltip("Corner radius", Placement::Above),
                &[icon, value],
            )
        }
        Control::Arrange => IconButton::new("layers", "Arrange", open)
            .tooltip_placement(Placement::Above)
            .add_to(ui),
        Control::Duplicate | Control::Delete => {
            let (icon, label, command) = if control == Control::Duplicate {
                ("copy-plus", "Duplicate", "board.duplicate")
            } else {
                ("trash", "Delete", "board.delete")
            };
            let disabled = state.disabled.contains(&control);
            let mut button = IconButton::new(icon, label, run(command))
                .tooltip_placement(Placement::Above)
                .enabled(move || !disabled);
            if let Some(keys) = shortcut(keymap, command) {
                button = button.shortcut(keys);
            }
            button.add_to(ui)
        }
        Control::Ai => {
            let icon = ui.add(Icon::named("sparkles", 18.0, Token::AccentInk));
            plain(ui, "AI actions", icon, Box::new(open))
        }
        Control::More => IconButton::new("ellipsis", "More", open)
            .tooltip_placement(Placement::Above)
            .add_to(ui),
    }
}

/// The color choices: none, then the board's palette in this theme.
fn palette_choices() -> Vec<(Option<Color>, &'static str, String)> {
    let kind = theme().kind();
    let mut choices = vec![(None, "None", "#00000000".to_owned())];
    for color in PALETTE {
        let stored = color.light.to_rgba8();
        let hex = format!("#{:02x}{:02x}{:02x}", stored.r, stored.g, stored.b);
        choices.push((Some(shown(color.light, kind)), color.name, hex));
    }
    choices
}

/// A popover choosing a color for `command` (fill, outline, text): the
/// palette, then the current color as hex and its opacity.
pub(crate) fn color_popover(
    ui: &mut Ui,
    title: &str,
    command: &'static str,
    current: Option<&str>,
    asks: &Asks,
) -> WidgetId {
    let heading = ui.add(
        Label::new(title, TextStyle::new(13.0).weight(600.0))
            .color(Token::Text)
            .single_line()
            .grow(1.0),
    );
    let note = ui.add(
        Label::new("Colors of this board", TextStyle::new(12.0).weight(500.0))
            .color(Token::TextMuted)
            .single_line(),
    );
    let header = ui.add_with_children(Flex::row().space_between(), &[heading, note]);
    let current_rgb = current.map(|hex| hex.get(..7).unwrap_or(hex).to_owned());
    let none =
        current.is_some_and(|hex| parse_color(hex).is_some_and(|color| color.components[3] == 0.0));
    let swatches: Vec<WidgetId> = palette_choices()
        .into_iter()
        .map(|(color, name, hex)| {
            let selected = match color {
                None => none,
                Some(_) => !none && current_rgb.as_deref() == Some(hex.as_str()),
            };
            let asks = asks.clone();
            ui.add(
                Swatch::new(color, name, move || {
                    let args = Args::new().with("color", ArgValue::String(hex.clone()));
                    asks.borrow_mut().push(Ask::RunWith(command, args));
                })
                .selected(selected),
            )
        })
        .collect();
    let rows: Vec<WidgetId> = swatches
        .chunks(6)
        .map(|row| ui.add_with_children(Flex::row().gap(8.0), row))
        .collect();
    let grid = ui.add_with_children(Flex::column().gap(8.0), &rows);
    let line = ui.add(Flex::row().height(1.0).background(Token::Divider));
    let shown_color = current
        .and_then(parse_color)
        .map(|color| shown(color, theme().kind()));
    let sample = ui.add(Swatch::shown(shown_color, SwatchShape::Dot, 20.0));
    let placeholder = current.unwrap_or("#rrggbb").to_owned();
    let hex_field = ui.add(
        TextInput::new(
            "Hex color",
            placeholder,
            TextStyle::new(13.0),
            InputColors::themed(),
        )
        .on_submit({
            let asks = asks.clone();
            move |text| {
                asks.borrow_mut()
                    .push(Ask::BarColor(command, text.trim().to_owned()))
            }
        }),
    );
    let hex_box = ui.add_with_children(Flex::row().grow(1.0), &[hex_field]);
    let opacity = ui.add(
        TextInput::new(
            "Opacity, percent",
            "100 %",
            TextStyle::new(13.0),
            InputColors::themed(),
        )
        .on_submit({
            let asks = asks.clone();
            move |text| {
                asks.borrow_mut().push(Ask::BarOpacity(
                    command,
                    text.trim().trim_end_matches('%').trim().to_owned(),
                ))
            }
        }),
    );
    let opacity_box = ui.add_with_children(Flex::row().width(72.0), &[opacity]);
    let custom = ui.add_with_children(Flex::row().gap(8.0), &[sample, hex_box, opacity_box]);
    ui.add_with_children(
        Flex::column()
            .width(234.0)
            .padding(12.0, 12.0)
            .gap(12.0)
            .background(Token::PanelBg)
            .border(1.0, Token::PanelBorder)
            .radius(12.0)
            .shadow()
            .role(Role::Dialog, title),
        &[header, grid, line, custom],
    )
}

/// A popover of values for `command`'s `arg` (outline widths, corner
/// radii), the current one checked.
pub(crate) fn value_popover(
    ui: &mut Ui,
    title: &str,
    command: &'static str,
    arg: &'static str,
    values: &[(f64, &str)],
    current: f64,
    asks: &Asks,
) -> WidgetId {
    let rows: Vec<WidgetId> = values
        .iter()
        .map(|&(value, name)| {
            let check = ui.add(Icon::named(
                "check",
                16.0,
                if (value - current).abs() < 1e-6 {
                    Token::AccentInk
                } else {
                    Token::PanelBg
                },
            ));
            let text = ui.add(
                Label::new(name, TextStyle::new(13.0))
                    .color(Token::Text)
                    .single_line()
                    .grow(1.0),
            );
            let asks = asks.clone();
            ui.add_with_children(
                ButtonBox::new(name, ButtonColors::quiet(), move || {
                    let args = Args::new().with(arg, ArgValue::Float(value));
                    let mut asks = asks.borrow_mut();
                    asks.push(Ask::CloseMenu);
                    asks.push(Ask::RunWith(command, args));
                })
                .size(168.0, 32.0)
                .padding(10.0, 10.0)
                .gap(10.0)
                .start()
                .radius(7.0)
                .keep_focus(),
                &[check, text],
            )
        })
        .collect();
    ui.add_with_children(
        Flex::column()
            .padding(6.0, 6.0)
            .gap(1.0)
            .background(Token::PanelBg)
            .border(1.0, Token::PanelBorder)
            .radius(12.0)
            .shadow()
            .role(Role::Menu, title),
        &rows,
    )
}

/// Outline widths, in points.
pub(crate) const WIDTHS: [(f64, &str); 6] = [
    (0.0, "None"),
    (1.0, "Thin · 1"),
    (1.5, "Regular · 1.5"),
    (2.0, "Medium · 2"),
    (3.0, "Bold · 3"),
    (6.0, "Heavy · 6"),
];

/// Corner radii, in points ("Round" asks for more than any side: the
/// command stops at half the shorter one).
pub(crate) const RADII: [(f64, &str); 7] = [
    (0.0, "Sharp · 0"),
    (4.0, "4"),
    (8.0, "8"),
    (12.0, "12"),
    (16.0, "16"),
    (24.0, "24"),
    (1.0e6, "Round ends"),
];

/// The stored color with its alpha at `percent` (0 to 100).
pub(crate) fn with_opacity(stored: &str, percent: f64) -> Option<String> {
    let color = parse_color(stored)?;
    let rgba = color.to_rgba8();
    let alpha = (percent.clamp(0.0, 100.0) / 100.0 * 255.0).round() as u8;
    Some(format!(
        "#{:02x}{:02x}{:02x}{alpha:02x}",
        rgba.r, rgba.g, rgba.b
    ))
}

/// A typed hex color as the commands take it (`#rrggbb[aa]`), or `None`.
pub(crate) fn typed_color(text: &str) -> Option<String> {
    let hex = text.trim().trim_start_matches('#');
    let valid = (hex.len() == 6 || hex.len() == 8) && hex.chars().all(|c| c.is_ascii_hexdigit());
    valid.then(|| format!("#{}", hex.to_ascii_lowercase()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_colors_and_opacity() {
        assert_eq!(typed_color("E0E7FF"), Some("#e0e7ff".into()));
        assert_eq!(typed_color("#e0e7ff80"), Some("#e0e7ff80".into()));
        assert_eq!(typed_color("#e0e7f"), None);
        assert_eq!(typed_color("zzzzzz"), None);
        assert_eq!(with_opacity("#e0e7ff", 50.0), Some("#e0e7ff80".into()));
        assert_eq!(with_opacity("#e0e7ffff", 0.0), Some("#e0e7ff00".into()));
    }
}
