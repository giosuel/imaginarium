// SPDX-License-Identifier: AGPL-3.0-or-later

//! The basic widgets: containers, labels, buttons and text inputs.

use std::rc::Rc;
use std::time::Instant;

use accesskit::{Action, ActionRequest, Node, NodeId, Role};
use imaginarium_render::Scene;
use imaginarium_render::kurbo::{Point, Rect, RoundedRect, Size, Vec2};
use imaginarium_render::peniko::Color;
use imaginarium_text::{TextBlock, TextStyle};
use taffy::prelude::{FlexDirection, length, percent};
use taffy::{AlignItems, AvailableSpace, JustifyContent, Style};

use crate::access::rect as access_rect;
use crate::text_field::TextField;
use crate::tree::{Cx, Ui, Widget, WidgetId};
use crate::window::{Input, Key};

/// A box that lays out its children in a row or column (CSS flexbox), with
/// an optional background, border and rounded corners.
pub struct Flex {
    style: Style,
    background: Option<Color>,
    border: Option<(f64, Color)>,
    radius: f64,
    shadow: bool,
    clips: bool,
    role: Option<(Role, String)>,
}

impl Flex {
    fn new(direction: FlexDirection) -> Self {
        Self {
            style: Style {
                flex_direction: direction,
                ..Style::default()
            },
            background: None,
            border: None,
            radius: 0.0,
            shadow: false,
            clips: false,
            role: None,
        }
    }

    /// Children side by side.
    pub fn row() -> Self {
        Self::new(FlexDirection::Row).align_center()
    }

    /// Children one below the other.
    pub fn column() -> Self {
        Self::new(FlexDirection::Column)
    }

    /// Space between children.
    pub fn gap(mut self, gap: f32) -> Self {
        self.style.gap = taffy::Size {
            width: length(gap),
            height: length(gap),
        };
        self
    }

    /// Space inside the edges.
    pub fn padding(mut self, horizontal: f32, vertical: f32) -> Self {
        self.style.padding = taffy::Rect {
            left: length(horizontal),
            right: length(horizontal),
            top: length(vertical),
            bottom: length(vertical),
        };
        self
    }

    /// Takes a share of the free space along the parent's direction.
    pub fn grow(mut self, grow: f32) -> Self {
        self.style.flex_grow = grow;
        self.style.flex_basis = length(0.0);
        self
    }

    /// A fixed width.
    pub fn width(mut self, width: f32) -> Self {
        self.style.size.width = length(width);
        self
    }

    /// A fixed height.
    pub fn height(mut self, height: f32) -> Self {
        self.style.size.height = length(height);
        self
    }

    /// The parent's full width and height.
    pub fn fill(mut self) -> Self {
        self.style.size = taffy::Size {
            width: percent(1.0),
            height: percent(1.0),
        };
        self
    }

    /// Children centered across the direction.
    pub fn align_center(mut self) -> Self {
        self.style.align_items = AlignItems::CENTER;
        self
    }

    /// Children spread out, the first at the start, the last at the end.
    pub fn space_between(mut self) -> Self {
        self.style.justify_content = JustifyContent::SPACE_BETWEEN;
        self
    }

    /// Any other style change.
    pub fn style(mut self, change: impl FnOnce(&mut Style)) -> Self {
        change(&mut self.style);
        self
    }

    /// A background color.
    pub fn background(mut self, color: Color) -> Self {
        self.background = Some(color);
        self
    }

    /// A border inside the edges.
    pub fn border(mut self, width: f64, color: Color) -> Self {
        self.border = Some((width, color));
        self
    }

    /// Rounded corners.
    pub fn radius(mut self, radius: f64) -> Self {
        self.radius = radius;
        self
    }

    /// A soft drop shadow (for popups).
    pub fn shadow(mut self) -> Self {
        self.shadow = true;
        self
    }

    /// Clips the children to the box.
    pub fn clip(mut self) -> Self {
        self.clips = true;
        self
    }

    /// Exposes the box to screen readers with `role` and `label` (otherwise
    /// it is transparent).
    pub fn role(mut self, role: Role, label: impl Into<String>) -> Self {
        self.role = Some((role, label.into()));
        self
    }
}

impl Widget for Flex {
    fn style(&self) -> Style {
        self.style.clone()
    }

    fn paint(&mut self, cx: &mut Cx<'_>, scene: &mut Scene) {
        let bounds = cx.bounds();
        if self.shadow {
            for (spread, alpha) in [(12.0, 0.03), (6.0, 0.05), (2.0, 0.08)] {
                let shadow = RoundedRect::from_rect(
                    bounds.inflate(spread, spread) + Vec2::new(0.0, spread / 2.0),
                    self.radius + spread,
                );
                scene.fill(&shadow, Color::BLACK.with_alpha(alpha));
            }
        }
        let shape = RoundedRect::from_rect(bounds, self.radius);
        if let Some(background) = self.background {
            scene.fill(&shape, background);
        }
        if let Some((width, color)) = self.border {
            let inset = RoundedRect::from_rect(
                bounds.inset(-width / 2.0),
                (self.radius - width / 2.0).max(0.0),
            );
            scene.stroke(&inset, width, color);
        }
    }

    fn clips(&self) -> bool {
        self.clips
    }

    fn accessibility(&mut self, cx: &mut Cx<'_>) -> Option<Node> {
        let (role, label) = self.role.as_ref()?;
        let mut node = Node::new(*role);
        node.set_label(label.clone());
        node.set_bounds(access_rect(cx.bounds()));
        Some(node)
    }
}

/// Where a label's text comes from.
enum Source {
    Fixed(String),
    /// Read on every layout; watched by [`label`].
    Dynamic(Rc<dyn Fn() -> String>),
}

/// A piece of text. It wraps when its box is narrower than the text.
pub struct Label {
    source: Source,
    style: TextStyle,
    block: Option<(String, Option<f32>, TextBlock)>,
    layout: Style,
}

impl Label {
    /// A label showing `text`.
    pub fn new(text: impl Into<String>, style: TextStyle) -> Self {
        Self {
            source: Source::Fixed(text.into()),
            style,
            block: None,
            layout: Style::default(),
        }
    }

    /// Changes the text.
    pub fn set_text(&mut self, text: impl Into<String>) {
        self.source = Source::Fixed(text.into());
    }

    /// Takes a share of the free space along the parent's direction.
    pub fn grow(mut self, grow: f32) -> Self {
        self.layout.flex_grow = grow;
        self.layout.flex_shrink = 1.0;
        self
    }

    fn text(&self) -> String {
        match &self.source {
            Source::Fixed(text) => text.clone(),
            Source::Dynamic(text) => imaginarium_reactive::untrack(|| text()),
        }
    }

    fn block(&mut self, cx: &mut Cx<'_>, width: Option<f32>) -> &TextBlock {
        let text = self.text();
        let stale = !matches!(&self.block, Some((t, w, _)) if *t == text && *w == width);
        if stale {
            let block = TextBlock::new(cx.fonts(), &text, &self.style, width);
            self.block = Some((text, width, block));
        }
        match &self.block {
            Some((_, _, block)) => block,
            None => unreachable_block(),
        }
    }
}

/// Only reachable if `block` above stopped filling the cache.
#[cold]
fn unreachable_block() -> ! {
    panic!("Label::block always fills its cache")
}

/// Adds a label whose text `text` computes from signals; it re-lays out
/// when they change.
pub fn label(ui: &mut Ui, style: TextStyle, text: impl Fn() -> String + 'static) -> WidgetId {
    let text: Rc<dyn Fn() -> String> = Rc::new(text);
    let id = ui.add(Label {
        source: Source::Dynamic(text.clone()),
        style,
        block: None,
        layout: Style::default(),
    });
    ui.watch(id, true, move || {
        text();
    });
    id
}

impl Widget for Label {
    fn style(&self) -> Style {
        self.layout.clone()
    }

    fn measure(
        &mut self,
        cx: &mut Cx<'_>,
        known: taffy::Size<Option<f32>>,
        available: taffy::Size<AvailableSpace>,
    ) -> Size {
        let width = known.width.or(match available.width {
            AvailableSpace::Definite(width) => Some(width),
            _ => None,
        });
        let size = self.block(cx, width).size();
        Size::new(size.width.ceil(), size.height.ceil())
    }

    fn paint(&mut self, cx: &mut Cx<'_>, scene: &mut Scene) {
        let bounds = cx.bounds();
        let block = self.block(cx, Some(bounds.width() as f32 + 0.5));
        block.paint(scene, bounds.origin(), true);
    }

    fn accessibility(&mut self, cx: &mut Cx<'_>) -> Option<Node> {
        let mut node = Node::new(Role::Label);
        node.set_value(self.text());
        node.set_bounds(access_rect(cx.bounds()));
        Some(node)
    }
}

/// Colors of a button.
#[derive(Clone, Copy, Debug)]
pub struct ButtonColors {
    /// The resting background.
    pub background: Color,
    /// The background under the pointer.
    pub hover: Color,
    /// The background while pressed.
    pub pressed: Color,
    /// The text.
    pub text: Color,
    /// The focus ring.
    pub focus: Color,
}

/// A button: click, Enter or Space runs its action.
pub struct Button {
    label: Label,
    action: Box<dyn FnMut()>,
    colors: ButtonColors,
    pressed: bool,
    padding: (f64, f64),
}

impl Button {
    /// A button showing `text` that runs `action`.
    pub fn new(
        text: impl Into<String>,
        style: TextStyle,
        colors: ButtonColors,
        action: impl FnMut() + 'static,
    ) -> Self {
        Self {
            label: Label::new(text, style.color(colors.text)),
            action: Box::new(action),
            colors,
            pressed: false,
            padding: (12.0, 6.0),
        }
    }
}

impl Widget for Button {
    fn measure(
        &mut self,
        cx: &mut Cx<'_>,
        _known: taffy::Size<Option<f32>>,
        _available: taffy::Size<AvailableSpace>,
    ) -> Size {
        let text = self.label.block(cx, None).size();
        Size::new(
            (text.width + 2.0 * self.padding.0).ceil(),
            (text.height + 2.0 * self.padding.1).ceil(),
        )
    }

    fn paint(&mut self, cx: &mut Cx<'_>, scene: &mut Scene) {
        let bounds = cx.bounds();
        let background = if self.pressed {
            self.colors.pressed
        } else if cx.is_hovered() {
            self.colors.hover
        } else {
            self.colors.background
        };
        let shape = RoundedRect::from_rect(bounds, 7.0);
        scene.fill(&shape, background);
        if cx.is_focused() {
            scene.stroke(
                &RoundedRect::from_rect(bounds.inflate(1.5, 1.5), 8.5),
                2.0,
                self.colors.focus,
            );
        }
        let (px, py) = self.padding;
        let origin = Point::new(bounds.x0 + px, bounds.y0 + py);
        self.label.block(cx, None).paint(scene, origin, true);
    }

    fn event(&mut self, cx: &mut Cx<'_>, input: &Input) -> bool {
        match input {
            Input::PointerButton {
                primary: true,
                pressed,
                position,
                ..
            } => {
                let inside = cx.bounds().contains(*position);
                if *pressed {
                    self.pressed = true;
                } else {
                    if self.pressed && inside {
                        (self.action)();
                    }
                    self.pressed = false;
                }
                cx.request_paint();
                true
            }
            Input::Key {
                key, pressed: true, ..
            } if *key == Key::Enter || *key == Key::Character(" ".into()) => {
                (self.action)();
                true
            }
            _ => false,
        }
    }

    fn focusable(&self) -> bool {
        true
    }

    fn focus_changed(&mut self, cx: &mut Cx<'_>, _focused: bool) {
        cx.request_paint();
    }

    fn hover_changed(&mut self, cx: &mut Cx<'_>, _hovered: bool) {
        cx.request_paint();
    }

    fn accessibility(&mut self, cx: &mut Cx<'_>) -> Option<Node> {
        let mut node = Node::new(Role::Button);
        node.set_label(self.label.text());
        node.set_bounds(access_rect(cx.bounds()));
        node.add_action(Action::Click);
        Some(node)
    }

    fn access_action(&mut self, _cx: &mut Cx<'_>, request: &ActionRequest) -> bool {
        if request.action == Action::Click {
            (self.action)();
            true
        } else {
            false
        }
    }
}

/// Callbacks of a [`TextInput`].
type TextCallback = Box<dyn FnMut(&str)>;

/// A single-line text input with a placeholder, wrapping a [`TextField`].
pub struct TextInput {
    field: TextField,
    label: String,
    placeholder: Label,
    on_change: TextCallback,
    on_submit: TextCallback,
    /// Keys the field leaves to its ancestors (a list's ↑/↓, Escape).
    passes: Vec<Key>,
    colors: InputColors,
    height: f32,
}

/// Colors of a text input.
#[derive(Clone, Copy, Debug)]
pub struct InputColors {
    /// The field's background.
    pub background: Color,
    /// The border at rest.
    pub border: Color,
    /// The border, caret and selection while focused.
    pub accent: Color,
    /// The placeholder text.
    pub placeholder: Color,
}

impl TextInput {
    /// An empty input. `label` names it for screen readers; `placeholder`
    /// shows while it is empty.
    pub fn new(
        label: impl Into<String>,
        placeholder: impl Into<String>,
        style: TextStyle,
        colors: InputColors,
    ) -> Self {
        let padding = 7.0;
        Self {
            field: TextField::new("", style, false).with_padding(padding),
            label: label.into(),
            placeholder: Label::new(placeholder, style.color(colors.placeholder)),
            on_change: Box::new(|_| {}),
            on_submit: Box::new(|_| {}),
            passes: Vec::new(),
            colors,
            height: (style.size * style.line_height + 2.0 * padding as f32).ceil(),
        }
    }

    /// Called with the new text after every change.
    pub fn on_change(mut self, action: impl FnMut(&str) + 'static) -> Self {
        self.on_change = Box::new(action);
        self
    }

    /// Called with the text on Enter.
    pub fn on_submit(mut self, action: impl FnMut(&str) + 'static) -> Self {
        self.on_submit = Box::new(action);
        self
    }

    /// Leaves `keys` to the ancestors instead of editing with them.
    pub fn passing(mut self, keys: impl IntoIterator<Item = Key>) -> Self {
        self.passes.extend(keys);
        self
    }

    /// The text.
    pub fn text(&self) -> String {
        self.field.text()
    }

    /// Replaces the text (no change callback).
    pub fn set_text(&mut self, text: &str) {
        self.field.editor().set_text(text);
    }

    /// The field inside.
    pub fn field(&mut self) -> &mut TextField {
        &mut self.field
    }
}

impl Widget for TextInput {
    fn style(&self) -> Style {
        Style {
            size: taffy::Size {
                width: percent(1.0),
                height: length(self.height),
            },
            flex_shrink: 1.0,
            ..Style::default()
        }
    }

    fn paint(&mut self, cx: &mut Cx<'_>, scene: &mut Scene) {
        let bounds = cx.bounds();
        let shape = RoundedRect::from_rect(bounds, 8.0);
        scene.fill(&shape, self.colors.background);
        if cx.is_focused() {
            scene.stroke(&shape, 2.0, self.colors.accent);
        } else {
            scene.stroke(&shape, 1.0, self.colors.border);
        }
        if self.field.text().is_empty() && !self.field.editor().is_composing() {
            let origin = Point::new(bounds.x0 + 7.0, bounds.y0 + 7.0);
            self.placeholder.block(cx, None).paint(scene, origin, true);
        }
        let accent = self.colors.accent;
        self.field.paint(cx.fonts(), scene, bounds, accent);
    }

    fn event(&mut self, cx: &mut Cx<'_>, input: &Input) -> bool {
        if let Input::Key { key, .. } = input
            && self.passes.contains(key)
        {
            return false;
        }
        let bounds = cx.bounds();
        let response = {
            let (fonts, clipboard) = cx.fonts_and_clipboard();
            self.field
                .input_with_clipboard(fonts, input, bounds, clipboard)
        };
        if response.changed {
            let text = self.field.text();
            (self.on_change)(&text);
        }
        if response.submitted {
            let text = self.field.text();
            (self.on_submit)(&text);
        }
        if response.blurred {
            cx.resign_focus();
        }
        if response.repaint || response.changed {
            cx.request_paint();
        }
        response.handled
    }

    fn focusable(&self) -> bool {
        true
    }

    fn focus_changed(&mut self, cx: &mut Cx<'_>, focused: bool) {
        if focused {
            self.field.focus();
        } else {
            self.field.blur(cx.fonts());
        }
        cx.request_paint();
    }

    fn ime_area(&mut self, cx: &mut Cx<'_>) -> Option<Rect> {
        let bounds = cx.bounds();
        self.field.ime_area(cx.fonts(), bounds)
    }

    fn wake_at(&self) -> Option<Instant> {
        self.field.wake_at()
    }

    fn wake(&mut self, cx: &mut Cx<'_>, _now: Instant) {
        cx.request_paint();
    }

    fn accessibility(&mut self, cx: &mut Cx<'_>) -> Option<Node> {
        let bounds = cx.bounds();
        let base = cx.id().access_base();
        let label = self.label.clone();
        let (fonts, tree) = cx.fonts_and_access();
        if let Some(tree) = tree {
            self.field.access(fonts, tree, base, bounds, &label);
        }
        cx.add_access_child(NodeId(base));
        None
    }

    fn access_action(&mut self, cx: &mut Cx<'_>, request: &ActionRequest) -> bool {
        let base = cx.id().access_base();
        let changed = self.field.access_action(cx.fonts(), request, base);
        if changed {
            let text = self.field.text();
            (self.on_change)(&text);
            cx.request_paint();
        }
        changed
    }
}
