// SPDX-License-Identifier: AGPL-3.0-or-later

//! The tool bar (`shell.md` §3.3): the board's tools in two groups, and a
//! grip that moves the bar to another edge of the window or lets it float.

use accesskit::{Node, Role};
use imaginarium_canvas::{Preset, Tool};
use imaginarium_commands::{CommandId, Keymap};
use imaginarium_reactive::Signal;
use imaginarium_render::Scene;
use imaginarium_render::kurbo::{Circle, Point, Rect, RoundedRect, Size, Stroke};
use imaginarium_settings::{Scope, Setting, setting_enum};
use imaginarium_ui::{Cx, Flex, IconButton, Input, Placement, Token, Ui, Widget, WidgetId, theme};
use taffy::prelude::length;

use crate::window::{Ask, Asks};

/// Where the tool bar is: docked to an edge of the window (centered along
/// it), or floating where it was dropped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dock {
    /// Along the left edge, as a column (the default).
    Left,
    /// Along the top edge, as a row.
    Top,
    /// Along the right edge, as a column.
    Right,
    /// Along the bottom edge, as a row, between zoom and help.
    Bottom,
    /// Where it was dropped.
    Floating,
}

setting_enum!(Dock {
    Left => "left", "Left",
    Top => "top", "Top",
    Right => "right", "Right",
    Bottom => "bottom", "Bottom",
    Floating => "floating", "Floating",
});

impl Dock {
    /// Whether the bar is a column there (a floating bar keeps the
    /// direction it had).
    pub fn is_vertical(self) -> Option<bool> {
        match self {
            Dock::Left | Dock::Right => Some(true),
            Dock::Top | Dock::Bottom => Some(false),
            Dock::Floating => None,
        }
    }

    /// The side of a tool its tooltip shows on: away from the edge.
    pub(crate) fn tooltip_side(self) -> Placement {
        match self {
            Dock::Left => Placement::Right,
            Dock::Right => Placement::Left,
            Dock::Bottom => Placement::Above,
            Dock::Top | Dock::Floating => Placement::Below,
        }
    }

    /// The words of the drop hint ("on the left").
    pub(crate) fn words(self) -> &'static str {
        match self {
            Dock::Left => "on the left",
            Dock::Top => "at the top",
            Dock::Right => "on the right",
            Dock::Bottom => "at the bottom",
            Dock::Floating => "floating",
        }
    }
}

/// Where the board window's tool bar is (`shell.md` §3.3).
pub static TOOLBAR_POSITION: Setting<Dock> = Setting::new(
    "ui.toolbar.position",
    || Dock::Left,
    Scope::App,
    "Tool bar position",
    "Which edge of the board window the tool bar is docked to, or floating where it was \
     dropped. Dragging the bar by its grip changes it too.",
)
.synonyms(&["toolbar", "tools", "dock", "tool bar"]);

/// A tool in the bar: its icon, name, command and the tool it selects.
struct ToolSpec {
    icon: &'static str,
    label: &'static str,
    command: &'static str,
    tool: Tool,
}

/// The tools, in their groups (`shell.md` §3.3). Draw (M4) and the third
/// group (documents, sheets, insert) come with their milestones.
const GROUPS: [&[ToolSpec]; 2] = [
    &[
        ToolSpec {
            icon: "mouse-pointer-2",
            label: "Select",
            command: "tool.select",
            tool: Tool::Select,
        },
        ToolSpec {
            icon: "hand",
            label: "Hand",
            command: "tool.hand",
            tool: Tool::Hand,
        },
    ],
    &[
        ToolSpec {
            icon: "rectangle-horizontal",
            label: "Rectangle",
            command: "tool.rectangle",
            tool: Tool::Shape(Preset::Rectangle),
        },
        ToolSpec {
            icon: "circle",
            label: "Ellipse",
            command: "tool.ellipse",
            tool: Tool::Shape(Preset::Ellipse),
        },
        ToolSpec {
            icon: "diamond",
            label: "Diamond",
            command: "tool.diamond",
            tool: Tool::Shape(Preset::Diamond),
        },
        ToolSpec {
            icon: "triangle",
            label: "Triangle",
            command: "tool.triangle",
            tool: Tool::Shape(Preset::Triangle),
        },
        ToolSpec {
            icon: "slash",
            label: "Line",
            command: "tool.line",
            tool: Tool::Shape(Preset::Line),
        },
        ToolSpec {
            icon: "type",
            label: "Text",
            command: "tool.text",
            tool: Tool::Text,
        },
        ToolSpec {
            icon: "frame",
            label: "Frame",
            command: "tool.frame",
            tool: Tool::Frame,
        },
    ],
];

/// The first shortcut of `command`, as the platform writes it.
pub(crate) fn shortcut(keymap: &Keymap, command: &'static str) -> Option<String> {
    keymap
        .shortcuts_for(CommandId::new(command))
        .next()
        .map(|shortcut| shortcut.display(keymap.platform()))
}

/// The tool bar's widgets, to turn it when it moves.
pub(crate) struct ToolBar {
    pub(crate) bar: WidgetId,
    grip: WidgetId,
    dividers: Vec<WidgetId>,
    pub(crate) buttons: Vec<WidgetId>,
}

/// Builds the tool bar. Its tools run their commands through `asks`, and
/// the one `tool` names is shown selected.
pub(crate) fn tool_bar(ui: &mut Ui, tool: Signal<Tool>, keymap: &Keymap, asks: &Asks) -> ToolBar {
    let grip = ui.add(Grip {
        vertical: true,
        asks: asks.clone(),
        held: false,
        placement: Placement::Right,
    });
    let mut children = vec![grip];
    let mut dividers = Vec::new();
    let mut buttons = Vec::new();
    for (index, group) in GROUPS.iter().enumerate() {
        if index > 0 {
            let divider = ui.add(Flex::row().background(Token::PanelBorder));
            dividers.push(divider);
            children.push(divider);
        }
        for spec in group.iter() {
            let asks = asks.clone();
            let (command, selects) = (spec.command, spec.tool);
            let mut button = IconButton::new(spec.icon, spec.label, move || {
                asks.borrow_mut().push(Ask::Run(command));
            })
            .size(40.0, 40.0, 10.0, 20.0)
            .selected(move || tool.get() == selects);
            if let Some(keys) = shortcut(keymap, command) {
                button = button.shortcut(keys);
            }
            let button = button.add_to(ui);
            buttons.push(button);
            children.push(button);
        }
    }
    let bar = ui.add_with_children(
        Flex::row()
            .padding(6.0, 6.0)
            .gap(4.0)
            .background(Token::PanelBg)
            .border(1.0, Token::PanelBorder)
            .radius(14.0)
            .shadow()
            .role(Role::Toolbar, "Tools"),
        &children,
    );
    ToolBar {
        bar,
        grip,
        dividers,
        buttons,
    }
}

impl ToolBar {
    /// Lays the bar out as a column (`vertical`) or a row, with tooltips on
    /// the `side` away from its edge.
    pub(crate) fn orient(&self, ui: &mut Ui, vertical: bool, side: Placement) {
        if let Some(bar) = ui.widget_mut::<Flex>(self.bar) {
            bar.style_mut().flex_direction = if vertical {
                taffy::FlexDirection::Column
            } else {
                taffy::FlexDirection::Row
            };
        }
        for divider in &self.dividers {
            if let Some(divider) = ui.widget_mut::<Flex>(*divider) {
                let style = divider.style_mut();
                let (width, height) = if vertical { (24.0, 1.0) } else { (1.0, 24.0) };
                style.size = taffy::Size {
                    width: length(width),
                    height: length(height),
                };
                let (across, along) = (length(0.0), length(4.0));
                style.margin = if vertical {
                    taffy::Rect {
                        left: across,
                        right: across,
                        top: along,
                        bottom: along,
                    }
                } else {
                    taffy::Rect {
                        left: along,
                        right: along,
                        top: across,
                        bottom: across,
                    }
                };
            }
        }
        if let Some(grip) = ui.widget_mut::<Grip>(self.grip) {
            grip.vertical = vertical;
            grip.placement = side;
        }
        for button in &self.buttons {
            if let Some(button) = ui.widget_mut::<IconButton>(*button) {
                button.set_tooltip_placement(side);
            }
        }
    }
}

/// The handle the tool bar is dragged by: six dots. A double-click puts
/// the bar back on the left.
pub(crate) struct Grip {
    vertical: bool,
    asks: Asks,
    held: bool,
    placement: Placement,
}

impl Widget for Grip {
    fn style(&self) -> taffy::Style {
        let (width, height) = if self.vertical {
            (40.0, 16.0)
        } else {
            (16.0, 40.0)
        };
        taffy::Style {
            size: taffy::Size {
                width: length(width),
                height: length(height),
            },
            flex_shrink: 0.0,
            ..taffy::Style::default()
        }
    }

    fn paint(&mut self, cx: &mut Cx<'_>, scene: &mut Scene) {
        let center = cx.bounds().center();
        let color = theme().color(Token::TextSubtle);
        // Three dots across the bar and two along it, 6 apart.
        let (columns, rows) = if self.vertical { (3, 2) } else { (2, 3) };
        for column in 0..columns {
            for row in 0..rows {
                let x = center.x + (f64::from(column) - f64::from(columns - 1) / 2.0) * 6.0;
                let y = center.y + (f64::from(row) - f64::from(rows - 1) / 2.0) * 6.0;
                scene.fill(&Circle::new((x, y), 1.5), color);
            }
        }
    }

    fn event(&mut self, _cx: &mut Cx<'_>, input: &Input) -> bool {
        match input {
            Input::PointerButton {
                primary: true,
                pressed: true,
                position,
                clicks,
                ..
            } => {
                if *clicks >= 2 {
                    self.held = false;
                    self.asks.borrow_mut().push(Ask::ResetBar);
                } else {
                    self.held = true;
                    self.asks.borrow_mut().push(Ask::Grab(*position));
                }
                true
            }
            Input::PointerButton {
                primary: true,
                pressed: false,
                position,
                modifiers,
                ..
            } => {
                if std::mem::take(&mut self.held) {
                    self.asks
                        .borrow_mut()
                        .push(Ask::Drop(*position, modifiers.shift));
                }
                true
            }
            Input::PointerMoved(position) if self.held => {
                self.asks.borrow_mut().push(Ask::Drag(*position));
                true
            }
            _ => false,
        }
    }

    fn tooltip(&self) -> Option<String> {
        Some("Drag to move · snaps to the edges".into())
    }

    fn tooltip_placement(&self) -> Placement {
        self.placement
    }

    fn accessibility(&mut self, cx: &mut Cx<'_>) -> Option<Node> {
        let mut node = Node::new(Role::Button);
        node.set_label("Move the tool bar: drag it to an edge");
        let bounds = cx.bounds();
        node.set_bounds(accesskit::Rect {
            x0: bounds.x0,
            y0: bounds.y0,
            x1: bounds.x1,
            y1: bounds.y1,
        });
        Some(node)
    }
}

/// Where the tool bar docks when dropped with the pointer at `pointer` in
/// a window of `size`: the nearest edge, when the pointer is in the outer
/// quarter of the window toward it, else nowhere (it floats).
pub(crate) fn snap(pointer: Point, size: Size) -> Option<Dock> {
    let (width, height) = (size.width.max(1.0), size.height.max(1.0));
    [
        (Dock::Left, pointer.x / width),
        (Dock::Right, (width - pointer.x) / width),
        (Dock::Top, pointer.y / height),
        (Dock::Bottom, (height - pointer.y) / height),
    ]
    .into_iter()
    .filter(|(_, share)| *share <= 0.25)
    .min_by(|a, b| a.1.total_cmp(&b.1))
    .map(|(edge, _)| edge)
}

/// The drop zone of `edge` in a window of `size` (the DockMove mockup's).
pub(crate) fn zone(edge: Dock, size: Size) -> Option<Rect> {
    let (width, height) = (size.width, size.height);
    let across = (width / 2.0 - 320.0).max(96.0);
    match edge {
        Dock::Left => Some(Rect::new(8.0, 84.0, 80.0, (height - 84.0).max(84.0))),
        Dock::Right => Some(Rect::new(
            width - 80.0,
            84.0,
            width - 8.0,
            (height - 84.0).max(84.0),
        )),
        Dock::Top => Some(Rect::new(across, 8.0, (width - across).max(across), 68.0)),
        Dock::Bottom => Some(Rect::new(
            across,
            height - 68.0,
            (width - across).max(across),
            height - 8.0,
        )),
        Dock::Floating => None,
    }
}

/// The edges' drop zones while the tool bar is dragged, the one it would
/// dock to stronger. Paints only; the pointer goes through.
pub(crate) struct DropZones {
    /// While dragging: the edge it would dock to (none: it floats).
    pub(crate) target: Option<Option<Dock>>,
}

impl Widget for DropZones {
    fn style(&self) -> taffy::Style {
        taffy::Style {
            position: taffy::Position::Absolute,
            inset: taffy::Rect {
                left: length(0.0),
                right: length(0.0),
                top: length(0.0),
                bottom: length(0.0),
            },
            ..taffy::Style::default()
        }
    }

    fn passes_pointer(&self) -> bool {
        true
    }

    fn paint(&mut self, cx: &mut Cx<'_>, scene: &mut Scene) {
        let Some(target) = self.target else {
            return;
        };
        let bounds = cx.bounds();
        let theme = theme();
        let accent = theme.color(Token::Accent);
        let line = theme.color(Token::AccentLine);
        for edge in [Dock::Left, Dock::Top, Dock::Right, Dock::Bottom] {
            let Some(rect) = zone(edge, bounds.size()) else {
                continue;
            };
            let rect = RoundedRect::from_rect(rect + bounds.origin().to_vec2(), 16.0);
            let strong = target == Some(edge);
            let (fill, color, width) = if strong {
                (accent.with_alpha(0.16), accent, 2.0)
            } else {
                (accent.with_alpha(0.06), line, 1.5)
            };
            scene.fill(&rect, fill);
            let stroke = Stroke::new(width).with_dashes(0.0, [6.0, 5.0]);
            scene.stroke_with(&rect, &stroke, color);
        }
    }
}
