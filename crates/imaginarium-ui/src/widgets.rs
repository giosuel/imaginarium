// SPDX-License-Identifier: AGPL-3.0-or-later

//! The basic widgets: containers, labels, buttons and text inputs.

use std::rc::Rc;
use std::time::Instant;

use accesskit::{Action, ActionRequest, Node, NodeId, Role};
use imaginarium_render::Scene;
use imaginarium_render::kurbo::{
    Affine, BezPath, Cap, Join, Point, Rect, RoundedRect, Size, Stroke, Vec2,
};
use imaginarium_render::peniko::Color;
use imaginarium_text::{Fonts, TextBlock, TextStyle};
use taffy::prelude::{FlexDirection, length, percent};
use taffy::{AlignItems, AvailableSpace, JustifyContent, Style};

use crate::access::rect as access_rect;
use crate::text_field::TextField;
use crate::theme::{Paint, theme};
use crate::tree::{Cx, Ui, Widget, WidgetId};
use crate::window::{Input, Key};

/// A box that lays out its children in a row or column (CSS flexbox), with
/// an optional background, border and rounded corners.
pub struct Flex {
    style: Style,
    background: Option<Paint>,
    border: Option<(f64, Paint)>,
    radius: f64,
    shadow: bool,
    clips: bool,
    pass_through: bool,
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
            pass_through: false,
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

    /// A background: a theme token or a fixed color.
    pub fn background(mut self, paint: impl Into<Paint>) -> Self {
        self.background = Some(paint.into());
        self
    }

    /// A border inside the edges.
    pub fn border(mut self, width: f64, paint: impl Into<Paint>) -> Self {
        self.border = Some((width, paint.into()));
        self
    }

    /// Rounded corners.
    pub fn radius(mut self, radius: f64) -> Self {
        self.radius = radius;
        self
    }

    /// A soft drop shadow (for panels and popups), as strong as the theme
    /// says.
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

    /// Lets the pointer through to what is below, except over its
    /// children: an overlay that places panels over the board.
    pub fn pass_through(mut self) -> Self {
        self.pass_through = true;
        self
    }

    /// Its layout, to change after it was added (through
    /// [`Ui::widget_mut`]; the tree lays it out again).
    pub fn style_mut(&mut self) -> &mut Style {
        &mut self.style
    }
}

/// A panel's shadow under `bounds`, as strong as `theme` says: layers of
/// black from the widest to the tightest.
fn paint_shadow(scene: &mut Scene, bounds: Rect, radius: f64, theme: &crate::theme::Theme) {
    for layer in theme.shadow {
        let spread = layer.spread;
        let shadow = RoundedRect::from_rect(
            bounds.inflate(spread, spread) + Vec2::new(0.0, spread / 2.0),
            radius + spread,
        );
        scene.fill(&shadow, Color::BLACK.with_alpha(layer.alpha));
    }
}

impl Widget for Flex {
    fn style(&self) -> Style {
        self.style.clone()
    }

    fn passes_pointer(&self) -> bool {
        self.pass_through
    }

    fn paint(&mut self, cx: &mut Cx<'_>, scene: &mut Scene) {
        let bounds = cx.bounds();
        let theme = theme();
        if self.shadow {
            paint_shadow(scene, bounds, self.radius, &theme);
        }
        let shape = RoundedRect::from_rect(bounds, self.radius);
        if let Some(background) = self.background {
            scene.fill(&shape, background.resolve(&theme));
        }
        if let Some((width, paint)) = self.border {
            let color = paint.resolve(&theme);
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
///
/// It is drawn in [`Token::Text`](crate::Token::Text) unless
/// [`Label::color`] says otherwise; the style's own color is not used.
pub struct Label {
    source: Source,
    style: TextStyle,
    color: Paint,
    /// The text, the width and the color it was laid out for.
    block: Option<(String, Option<f32>, Color, TextBlock)>,
    layout: Style,
    /// Never wraps (a status line); otherwise it wraps down to its
    /// longest word when its box gets narrow, as text in CSS does.
    single_line: bool,
    /// Stretches drawn in another weight and color (a search's matches),
    /// as byte ranges.
    highlights: Vec<std::ops::Range<usize>>,
    highlight: (f32, Paint),
}

impl Label {
    /// A label showing `text`.
    pub fn new(text: impl Into<String>, style: TextStyle) -> Self {
        Self {
            source: Source::Fixed(text.into()),
            style,
            color: Paint::Token(crate::theme::Token::Text),
            block: None,
            layout: Style::default(),
            single_line: false,
            highlights: Vec::new(),
            highlight: (600.0, Paint::Token(crate::theme::Token::AccentInk)),
        }
    }

    /// Changes the highlighted byte ranges after it was added (the
    /// palette's rows, reused for each query).
    pub fn set_highlights(&mut self, ranges: Vec<std::ops::Range<usize>>) {
        if self.highlights != ranges {
            self.highlights = ranges;
            self.block = None;
        }
    }

    /// Draws the byte `ranges` of its text in `weight` and `paint` (the
    /// letters a search matched).
    pub fn highlight(
        mut self,
        ranges: Vec<std::ops::Range<usize>>,
        weight: f32,
        paint: impl Into<Paint>,
    ) -> Self {
        self.highlights = ranges;
        self.highlight = (weight, paint.into());
        self
    }

    /// Keeps the text on one line, however narrow the box (its parent
    /// clips what doesn't fit).
    pub fn single_line(mut self) -> Self {
        self.single_line = true;
        self
    }

    /// See [`Label::single_line`], for labels made by [`label`]. Change it
    /// through [`Ui::widget_mut`], which lays the label out again.
    pub fn set_single_line(&mut self, single_line: bool) {
        self.single_line = single_line;
        self.block = None;
    }

    /// Draws the text in `paint` (a token follows the theme).
    pub fn color(mut self, paint: impl Into<Paint>) -> Self {
        self.color = paint.into();
        self
    }

    /// Changes the color.
    pub fn set_color(&mut self, paint: impl Into<Paint>) {
        self.color = paint.into();
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

    /// The color the text was last shaped in.
    #[cfg(test)]
    pub(crate) fn shaped_color(&self) -> Option<Color> {
        self.block.as_ref().map(|(_, _, color, _)| *color)
    }

    fn text(&self) -> String {
        match &self.source {
            Source::Fixed(text) => text.clone(),
            Source::Dynamic(text) => imaginarium_reactive::untrack(|| text()),
        }
    }

    /// The text laid out for `width`: shaped once per text, re-broken
    /// (cheap) when only the width changes, as Taffy asks for several.
    fn block(&mut self, cx: &mut Cx<'_>, width: Option<f32>) -> &TextBlock {
        let text = self.text();
        let color = self.color.current();
        match &mut self.block {
            Some((shaped, laid_for, shaped_color, block))
                if *shaped == text && *shaped_color == color =>
            {
                if *laid_for != width {
                    block.set_width(width);
                    *laid_for = width;
                }
            }
            _ => {
                let style = self.style.color(color);
                let block = if self.highlights.is_empty() {
                    TextBlock::new(cx.fonts(), &text, &style, width)
                } else {
                    let (weight, paint) = self.highlight;
                    let color = Some(paint.current());
                    let spans: Vec<imaginarium_text::Span> = self
                        .highlights
                        .iter()
                        .map(|range| imaginarium_text::Span {
                            range: range.clone(),
                            weight: Some(weight),
                            color,
                        })
                        .collect();
                    TextBlock::with_spans(cx.fonts(), &text, &style, &spans, width)
                };
                self.block = Some((text, width, color, block));
            }
        }
        match &self.block {
            Some((_, _, _, block)) => block,
            None => unreachable_block(),
        }
    }
}

/// Only reachable if `block` above stopped filling the cache.
#[cold]
fn unreachable_block() -> ! {
    panic!("Label::block always fills its cache")
}

/// Adds a label in `color` whose text `text` computes from signals; it
/// re-lays out when they change.
pub fn label(
    ui: &mut Ui,
    style: TextStyle,
    color: impl Into<Paint>,
    text: impl Fn() -> String + 'static,
) -> WidgetId {
    let text: Rc<dyn Fn() -> String> = Rc::new(text);
    let id = ui.add(Label {
        source: Source::Dynamic(text.clone()),
        style,
        color: color.into(),
        block: None,
        layout: Style::default(),
        single_line: false,
        highlights: Vec::new(),
        highlight: (600.0, Paint::Token(crate::theme::Token::AccentInk)),
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
        // Asked for its smallest width, text breaks at every chance, so
        // its longest word decides (as in CSS); otherwise it fills the
        // width offered, or stays on one line.
        let width = if self.single_line {
            None
        } else {
            known.width.or(match available.width {
                AvailableSpace::Definite(width) => Some(width),
                AvailableSpace::MinContent => Some(0.0),
                AvailableSpace::MaxContent => None,
            })
        };
        let size = self.block(cx, width).size();
        Size::new(size.width.ceil(), size.height.ceil())
    }

    fn paint(&mut self, cx: &mut Cx<'_>, scene: &mut Scene) {
        let bounds = cx.bounds();
        let width = (!self.single_line).then_some(bounds.width() as f32 + 0.5);
        let block = self.block(cx, width);
        block.paint(scene, bounds.origin(), true);
    }

    fn accessibility(&mut self, cx: &mut Cx<'_>) -> Option<Node> {
        let mut node = Node::new(Role::Label);
        node.set_value(self.text());
        node.set_bounds(access_rect(cx.bounds()));
        Some(node)
    }
}

/// Colors of a button: theme tokens or fixed colors.
#[derive(Clone, Copy, Debug)]
pub struct ButtonColors {
    /// The resting background.
    pub background: Paint,
    /// The background under the pointer.
    pub hover: Paint,
    /// The background while pressed.
    pub pressed: Paint,
    /// The text.
    pub text: Paint,
    /// The focus ring.
    pub focus: Paint,
    /// A 1 px outline, if any (secondary buttons).
    pub border: Option<Paint>,
}

impl ButtonColors {
    /// A primary button: the accent fill (Save…, Present).
    pub fn primary() -> Self {
        use crate::theme::Token;
        Self {
            background: Token::Accent.into(),
            hover: Token::AccentHover.into(),
            pressed: Token::AccentPressed.into(),
            text: Token::OnAccent.into(),
            focus: Token::AccentInk.into(),
            border: None,
        }
    }

    /// A secondary button: outlined on the panel (a dialog's Cancel).
    pub fn secondary() -> Self {
        Self {
            border: Some(crate::theme::Token::ControlBorder.into()),
            ..Self::quiet()
        }
    }

    /// A destructive action without a fill: red text (Don't save, Delete).
    pub fn danger() -> Self {
        Self {
            text: crate::theme::Token::Danger.into(),
            ..Self::quiet()
        }
    }

    /// A quiet button: no fill until the pointer is over it (tool bars,
    /// panels).
    pub fn quiet() -> Self {
        use crate::theme::Token;
        Self {
            background: Token::PanelBg.into(),
            hover: Token::ControlHover.into(),
            pressed: Token::ControlActive.into(),
            text: Token::Text.into(),
            focus: Token::AccentInk.into(),
            border: None,
        }
    }
}

/// A button: click, Enter or Space runs its action.
pub struct Button {
    label: Label,
    action: Box<dyn FnMut()>,
    colors: ButtonColors,
    pressed: bool,
    /// The background's tone: 0 at rest, 1 hovered, 2 pressed.
    tone: crate::animation::Transition,
    padding: (f64, f64),
    min_height: f64,
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
            label: Label::new(text, style).color(colors.text),
            action: Box::new(action),
            colors,
            pressed: false,
            tone: crate::animation::Transition::new(0.0),
            padding: (12.0, 6.0),
            min_height: 0.0,
        }
    }

    /// Space around the text: horizontal and vertical.
    pub fn padding(mut self, horizontal: f64, vertical: f64) -> Self {
        self.padding = (horizontal, vertical);
        self
    }

    /// At least this tall (the text stays centered), e.g. 36 for dialog
    /// buttons.
    pub fn min_height(mut self, height: f64) -> Self {
        self.min_height = height;
        self
    }
}

#[cfg(test)]
impl Button {
    /// The color the label was last shaped in.
    pub(crate) fn label_color(&self) -> Option<Color> {
        self.label.shaped_color()
    }
}

impl Button {
    /// Heads the background for its state.
    fn retone(&mut self, hovered: bool) {
        use crate::animation::{Easing, durations};
        let target = if self.pressed {
            2.0
        } else if hovered {
            1.0
        } else {
            0.0
        };
        self.tone.to(target, durations::HOVER, Easing::EaseOut);
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
            (text.height + 2.0 * self.padding.1)
                .ceil()
                .max(self.min_height),
        )
    }

    fn paint(&mut self, cx: &mut Cx<'_>, scene: &mut Scene) {
        let bounds = cx.bounds();
        let theme = theme();
        // The background fades between rest, hover and press.
        let tone = self.tone.value();
        let (rest, hover, pressed) = (
            self.colors.background.resolve(&theme),
            self.colors.hover.resolve(&theme),
            self.colors.pressed.resolve(&theme),
        );
        let background = if tone <= 1.0 {
            crate::theme::mix(rest, hover, tone as f32)
        } else {
            crate::theme::mix(hover, pressed, (tone - 1.0) as f32)
        };
        let radius = theme.control_radius;
        let shape = RoundedRect::from_rect(bounds, radius);
        scene.fill(&shape, background);
        if let Some(border) = self.colors.border {
            let inset = RoundedRect::from_rect(bounds.inset(-0.5), radius - 0.5);
            scene.stroke(&inset, 1.0, border.resolve(&theme));
        }
        // The ring shows where the keyboard is, not where a click landed.
        if cx.is_focus_visible() {
            scene.stroke(
                &RoundedRect::from_rect(bounds.inflate(1.5, 1.5), radius + 1.5),
                2.0,
                self.colors.focus.resolve(&theme),
            );
        }
        let (px, _) = self.padding;
        let text = self.label.block(cx, None).size();
        let origin = Point::new(bounds.x0 + px, bounds.center().y - text.height / 2.0);
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
                self.retone(cx.is_hovered() && inside);
                cx.request_paint();
                true
            }
            // Once per press, not again while the key repeats.
            Input::Key {
                key,
                pressed: true,
                repeat,
                ..
            } if *key == Key::Enter || *key == Key::Character(" ".into()) => {
                if !repeat {
                    (self.action)();
                }
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

    fn hover_changed(&mut self, cx: &mut Cx<'_>, hovered: bool) {
        self.retone(hovered);
        cx.request_paint();
    }

    fn wake_at(&self) -> Option<Instant> {
        self.tone.wake_at()
    }

    fn wake(&mut self, cx: &mut Cx<'_>, _now: Instant) {
        self.tone.advance();
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

/// A button holding other widgets, laid out as a [`Flex`] row: the search
/// field with its key, a button with an icon and a label. Click, Enter or
/// Space runs its action; it takes the pointer over its whole box, so the
/// widgets in it only show.
///
/// ```
/// # use imaginarium_ui::{ButtonBox, ButtonColors, Icon, Ui, Token};
/// let mut ui = Ui::new();
/// let icon = ui.add(Icon::named("search", 16.0, Token::TextMuted));
/// let search = ui.add_with_children(
///     ButtonBox::new("Search commands and settings", ButtonColors::quiet(), || {})
///         .size(176.0, 44.0),
///     &[icon],
/// );
/// # let _ = search;
/// ```
pub struct ButtonBox {
    style: Style,
    colors: ButtonColors,
    radius: f64,
    shadow: bool,
    label: String,
    tooltip: Option<(String, crate::layers::Placement)>,
    focus_on_click: bool,
    selected: Option<Flag>,
    role: Role,
    /// Can't be used now: no action, not focusable, greyed by its owner.
    disabled: bool,
    action: Box<dyn FnMut()>,
    pressed: bool,
    /// The background's tone: 0 at rest, 1 hovered, 2 pressed.
    tone: crate::animation::Transition,
}

impl ButtonBox {
    /// A button named `label` (for screen readers) that runs `action`; its
    /// children are centered in a row, 8 apart.
    pub fn new(
        label: impl Into<String>,
        colors: ButtonColors,
        action: impl FnMut() + 'static,
    ) -> Self {
        Self {
            style: Style {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::CENTER,
                justify_content: JustifyContent::CENTER,
                gap: taffy::Size {
                    width: length(8.0),
                    height: length(0.0),
                },
                flex_shrink: 0.0,
                ..Style::default()
            },
            colors,
            radius: 8.0,
            shadow: false,
            label: label.into(),
            tooltip: None,
            focus_on_click: true,
            selected: None,
            role: Role::Button,
            disabled: false,
            action: Box::new(action),
            pressed: false,
            tone: crate::animation::Transition::new(0.0),
        }
    }

    /// A fixed size.
    pub fn size(mut self, width: f64, height: f64) -> Self {
        self.style.size = taffy::Size {
            width: length(width as f32),
            height: length(height as f32),
        };
        self
    }

    /// A fixed height; the width follows the children.
    pub fn height(mut self, height: f64) -> Self {
        self.style.size.height = length(height as f32);
        self
    }

    /// Space around the children: left, right.
    pub fn padding(mut self, left: f64, right: f64) -> Self {
        self.style.padding.left = length(left as f32);
        self.style.padding.right = length(right as f32);
        self
    }

    /// Children start at the left (the search field's text) instead of
    /// being centered.
    pub fn start(mut self) -> Self {
        self.style.justify_content = JustifyContent::FLEX_START;
        self
    }

    /// Space between the children.
    pub fn gap(mut self, gap: f64) -> Self {
        self.style.gap.width = length(gap as f32);
        self
    }

    /// The corner radius (8 by default; 12 for a button that is a panel).
    pub fn radius(mut self, radius: f64) -> Self {
        self.radius = radius;
        self
    }

    /// A panel's shadow under it (a button floating over the board).
    pub fn shadow(mut self) -> Self {
        self.shadow = true;
        self
    }

    /// A tooltip shown when the pointer rests on it, on the `placement`
    /// side.
    pub fn tooltip(
        mut self,
        tooltip: impl Into<String>,
        placement: crate::layers::Placement,
    ) -> Self {
        self.tooltip = Some((tooltip.into(), placement));
        self
    }

    /// A click leaves the focus where it was ([`Widget::focus_on_click`]).
    pub fn keep_focus(mut self) -> Self {
        self.focus_on_click = false;
        self
    }

    /// Can't be used now (a menu's row that doesn't apply): it does
    /// nothing, the keyboard skips it, and screen readers hear so.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// What screen readers call it ([`Role::Button`] unless a menu's
    /// row: [`Role::MenuItem`]).
    pub fn role(mut self, role: Role) -> Self {
        self.role = role;
        self
    }

    /// Filled with [`Token::ControlActive`](crate::Token::ControlActive)
    /// while `selected` is true (the palette's active row). It may read
    /// signals: [`ButtonBox::add_to`] repaints when they change.
    pub fn selected(mut self, selected: impl Fn() -> bool + 'static) -> Self {
        self.selected = Some(Rc::new(selected));
        self
    }

    /// Adds the button with `children` to `ui`, repainting it when the
    /// signals its selected state reads change.
    pub fn add_to(self, ui: &mut Ui, children: &[WidgetId]) -> WidgetId {
        let selected = self.selected.clone();
        let id = ui.add_with_children(self, children);
        if let Some(selected) = selected {
            ui.watch(id, false, move || {
                selected();
            });
        }
        id
    }

    fn retone(&mut self, hovered: bool) {
        use crate::animation::{Easing, durations};
        let target = if self.pressed {
            2.0
        } else if hovered {
            1.0
        } else {
            0.0
        };
        self.tone.to(target, durations::HOVER, Easing::EaseOut);
    }
}

impl Widget for ButtonBox {
    fn style(&self) -> Style {
        self.style.clone()
    }

    fn claims(&self, _bounds: Rect, _content: Size, _point: Point) -> bool {
        true
    }

    fn paint(&mut self, cx: &mut Cx<'_>, scene: &mut Scene) {
        let bounds = cx.bounds();
        let theme = theme();
        let shape = RoundedRect::from_rect(bounds, self.radius);
        if self.shadow {
            paint_shadow(scene, bounds, self.radius, &theme);
        }
        let tone = self.tone.value();
        let selected = self.selected.as_ref().is_some_and(|selected| selected());
        let (rest, hover, pressed) = if selected {
            let active = theme.color(crate::theme::Token::ControlActive);
            (active, active, active)
        } else {
            (
                self.colors.background.resolve(&theme),
                self.colors.hover.resolve(&theme),
                self.colors.pressed.resolve(&theme),
            )
        };
        let background = if tone <= 1.0 {
            crate::theme::mix(rest, hover, tone as f32)
        } else {
            crate::theme::mix(hover, pressed, (tone - 1.0) as f32)
        };
        scene.fill(&shape, background);
        if let Some(border) = self.colors.border {
            let inset = RoundedRect::from_rect(bounds.inset(-0.5), self.radius - 0.5);
            scene.stroke(&inset, 1.0, border.resolve(&theme));
        }
        if cx.is_focus_visible() {
            scene.stroke(
                &RoundedRect::from_rect(bounds.inflate(1.5, 1.5), self.radius + 1.5),
                2.0,
                self.colors.focus.resolve(&theme),
            );
        }
    }

    fn event(&mut self, cx: &mut Cx<'_>, input: &Input) -> bool {
        if self.disabled {
            return matches!(input, Input::PointerButton { primary: true, .. });
        }
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
                self.retone(cx.is_hovered() && inside);
                cx.request_paint();
                true
            }
            Input::Key {
                key,
                pressed: true,
                repeat,
                ..
            } if *key == Key::Enter || *key == Key::Character(" ".into()) => {
                if !repeat {
                    (self.action)();
                }
                true
            }
            _ => false,
        }
    }

    fn focusable(&self) -> bool {
        !self.disabled
    }

    fn focus_on_click(&self) -> bool {
        self.focus_on_click
    }

    fn capture_lost(&mut self, cx: &mut Cx<'_>) {
        // The release went elsewhere (the window lost the focus).
        self.pressed = false;
        self.retone(false);
        cx.request_paint();
    }

    fn focus_changed(&mut self, cx: &mut Cx<'_>, _focused: bool) {
        cx.request_paint();
    }

    fn hover_changed(&mut self, cx: &mut Cx<'_>, hovered: bool) {
        self.retone(hovered);
        cx.request_paint();
    }

    fn tooltip(&self) -> Option<String> {
        self.tooltip.as_ref().map(|(text, _)| text.clone())
    }

    fn tooltip_placement(&self) -> crate::layers::Placement {
        self.tooltip
            .as_ref()
            .map_or(crate::layers::Placement::Below, |(_, placement)| *placement)
    }

    fn wake_at(&self) -> Option<Instant> {
        self.tone.wake_at()
    }

    fn wake(&mut self, cx: &mut Cx<'_>, _now: Instant) {
        self.tone.advance();
        cx.request_paint();
    }

    fn accessibility(&mut self, cx: &mut Cx<'_>) -> Option<Node> {
        let mut node = Node::new(self.role);
        if self.selected.as_ref().is_some_and(|selected| selected()) {
            node.set_selected(true);
        }
        node.set_label(self.label.clone());
        node.set_bounds(access_rect(cx.bounds()));
        if self.disabled {
            node.set_disabled();
        } else {
            node.add_action(Action::Click);
        }
        Some(node)
    }

    fn access_action(&mut self, _cx: &mut Cx<'_>, request: &ActionRequest) -> bool {
        if request.action == Action::Click && !self.disabled {
            (self.action)();
            true
        } else {
            false
        }
    }
}

/// A state an [`IconButton`] reads from signals (selected, enabled).
type Flag = Rc<dyn Fn() -> bool>;

/// A square button showing an icon: tool bars, zoom controls. Its label
/// names it for screen readers and, with its shortcut, in a tooltip
/// ("Rectangle · R"). Quiet: no fill until hovered; a selected button (the
/// active tool) is filled with [`Token::AccentSoft`] and its icon in [`Token::AccentInk`];
/// a disabled one is drawn in [`Token::Disabled`] and does nothing.
///
/// [`Token::AccentSoft`]: crate::theme::Token::AccentSoft
/// [`Token::AccentInk`]: crate::theme::Token::AccentInk
/// [`Token::Disabled`]: crate::theme::Token::Disabled
///
/// ```
/// # use imaginarium_ui::{IconButton, Ui};
/// # use imaginarium_reactive::signal;
/// let mut ui = Ui::new();
/// let can_redo = signal(false);
/// let redo = IconButton::new("redo-2", "Redo", || {})
///     .shortcut("Ctrl Shift Z")
///     .enabled(move || can_redo.get())
///     .add_to(&mut ui);
/// # let _ = redo;
/// ```
pub struct IconButton {
    paths: Vec<BezPath>,
    icon_size: f64,
    size: (f64, f64),
    radius: f64,
    label: String,
    shortcut: Option<String>,
    action: Box<dyn FnMut()>,
    selected: Option<Flag>,
    enabled: Option<Flag>,
    tooltip_placement: crate::layers::Placement,
    pressed: bool,
    /// The background's tone: 0 at rest, 1 hovered, 2 pressed.
    tone: crate::animation::Transition,
}

impl IconButton {
    /// A 36×36 button with the vendored Lucide icon `icon` at 18 px (the
    /// mockups' zoom and undo controls), named `label`, that runs `action`.
    /// An unknown icon name panics in debug builds, as [`Icon::named`] does.
    pub fn new(icon: &str, label: impl Into<String>, action: impl FnMut() + 'static) -> Self {
        let paths = crate::icons::icon_paths(icon);
        debug_assert!(paths.is_some(), "the icon `{icon}` isn't vendored");
        Self::with_paths(paths.unwrap_or(&[]), label, action)
    }

    /// A button with an icon from its paths' `d` strings, for icons that
    /// aren't Lucide's (the tool bar's diamond and triangle).
    pub fn with_paths(
        paths: &[&str],
        label: impl Into<String>,
        action: impl FnMut() + 'static,
    ) -> Self {
        Self {
            paths: paths
                .iter()
                .filter_map(|data| BezPath::from_svg(data).ok())
                .collect(),
            icon_size: 18.0,
            size: (36.0, 36.0),
            radius: 8.0,
            label: label.into(),
            shortcut: None,
            action: Box::new(action),
            selected: None,
            enabled: None,
            tooltip_placement: crate::layers::Placement::Below,
            pressed: false,
            tone: crate::animation::Transition::new(0.0),
        }
    }

    /// The button's size and corner radius, and the icon's size: 40×40,
    /// radius 10 and 20 for the tool bar.
    pub fn size(mut self, width: f64, height: f64, radius: f64, icon: f64) -> Self {
        self.size = (width, height);
        self.radius = radius;
        self.icon_size = icon;
        self
    }

    /// The shortcut the tooltip shows after the label, as the user types
    /// it ("Shift T").
    pub fn shortcut(mut self, shortcut: impl Into<String>) -> Self {
        self.shortcut = Some(shortcut.into());
        self
    }

    /// Shown as selected while `selected` is true (the active tool). It
    /// may read signals: [`IconButton::add_to`] repaints when they change.
    pub fn selected(mut self, selected: impl Fn() -> bool + 'static) -> Self {
        self.selected = Some(Rc::new(selected));
        self
    }

    /// Usable only while `enabled` is true (redo with something to redo).
    /// It may read signals, as [`IconButton::selected`] does.
    pub fn enabled(mut self, enabled: impl Fn() -> bool + 'static) -> Self {
        self.enabled = Some(Rc::new(enabled));
        self
    }

    /// Which side the tooltip shows on (below by default).
    pub fn tooltip_placement(mut self, placement: crate::layers::Placement) -> Self {
        self.tooltip_placement = placement;
        self
    }

    /// Changes the tooltip's side after it was added (a tool bar moved to
    /// another edge).
    pub fn set_tooltip_placement(&mut self, placement: crate::layers::Placement) {
        self.tooltip_placement = placement;
    }

    /// Adds the button to `ui`, repainting it when the signals its
    /// selected and enabled states read change.
    pub fn add_to(self, ui: &mut Ui) -> WidgetId {
        let flags: Vec<Flag> = [self.selected.clone(), self.enabled.clone()]
            .into_iter()
            .flatten()
            .collect();
        let id = ui.add(self);
        if !flags.is_empty() {
            ui.watch(id, false, move || {
                for flag in &flags {
                    flag();
                }
            });
        }
        id
    }

    fn is_selected(&self) -> bool {
        self.selected.as_ref().is_some_and(|selected| selected())
    }

    fn is_enabled(&self) -> bool {
        self.enabled.as_ref().is_none_or(|enabled| enabled())
    }

    /// Heads the background for its state.
    fn retone(&mut self, hovered: bool) {
        use crate::animation::{Easing, durations};
        let target = if !self.is_enabled() {
            0.0
        } else if self.pressed {
            2.0
        } else if hovered {
            1.0
        } else {
            0.0
        };
        self.tone.to(target, durations::HOVER, Easing::EaseOut);
    }

    fn run(&mut self) {
        if self.is_enabled() {
            (self.action)();
        }
    }
}

impl Widget for IconButton {
    fn style(&self) -> Style {
        Style {
            size: taffy::Size {
                width: length(self.size.0 as f32),
                height: length(self.size.1 as f32),
            },
            flex_shrink: 0.0,
            ..Style::default()
        }
    }

    fn paint(&mut self, cx: &mut Cx<'_>, scene: &mut Scene) {
        use crate::theme::{Token, mix};
        let bounds = cx.bounds();
        let theme = theme();
        let (selected, enabled) = (self.is_selected(), self.is_enabled());
        // The selected tint is the resting fill; hover and press darken
        // from there, as on a quiet button from nothing.
        let rest = if selected {
            theme.color(Token::AccentSoft)
        } else {
            theme.color(Token::ControlHover).with_alpha(0.0)
        };
        let (hover, pressed) = if selected {
            (rest, rest)
        } else {
            (
                theme.color(Token::ControlHover),
                theme.color(Token::ControlActive),
            )
        };
        // A button that just became unusable (undo with nothing left)
        // shows no hover, whatever its tone was heading for.
        let tone = if enabled { self.tone.value() } else { 0.0 };
        let background = if tone <= 1.0 {
            mix(rest, hover, tone as f32)
        } else {
            mix(hover, pressed, (tone - 1.0) as f32)
        };
        let shape = RoundedRect::from_rect(bounds, self.radius);
        scene.fill(&shape, background);
        if cx.is_focus_visible() {
            scene.stroke(
                &RoundedRect::from_rect(bounds.inflate(1.5, 1.5), self.radius + 1.5),
                2.0,
                theme.color(Token::AccentInk),
            );
        }
        let color = theme.color(if !enabled {
            Token::Disabled
        } else if selected {
            Token::AccentInk
        } else {
            Token::TextSecondary
        });
        let scale = self.icon_size / 24.0;
        let corner = bounds.center() - Vec2::splat(self.icon_size / 2.0);
        let transform = Affine::translate(corner.to_vec2()) * Affine::scale(scale);
        let stroke = Stroke::new(1.75 * scale)
            .with_caps(Cap::Round)
            .with_join(Join::Round);
        for path in &self.paths {
            scene.stroke_with(&(transform * path.clone()), &stroke, color);
        }
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
                        self.run();
                    }
                    self.pressed = false;
                }
                self.retone(cx.is_hovered() && inside);
                cx.request_paint();
                true
            }
            Input::Key {
                key,
                pressed: true,
                repeat,
                ..
            } if *key == Key::Enter || *key == Key::Character(" ".into()) => {
                if !repeat {
                    self.run();
                }
                true
            }
            _ => false,
        }
    }

    fn focusable(&self) -> bool {
        self.is_enabled()
    }

    fn focus_on_click(&self) -> bool {
        false
    }

    fn capture_lost(&mut self, cx: &mut Cx<'_>) {
        // The release went elsewhere (the window lost the focus).
        self.pressed = false;
        self.retone(false);
        cx.request_paint();
    }

    fn focus_changed(&mut self, cx: &mut Cx<'_>, _focused: bool) {
        cx.request_paint();
    }

    fn hover_changed(&mut self, cx: &mut Cx<'_>, hovered: bool) {
        self.retone(hovered);
        cx.request_paint();
    }

    fn tooltip(&self) -> Option<String> {
        Some(match &self.shortcut {
            Some(shortcut) => format!("{} · {shortcut}", self.label),
            None => self.label.clone(),
        })
    }

    fn tooltip_placement(&self) -> crate::layers::Placement {
        self.tooltip_placement
    }

    fn wake_at(&self) -> Option<Instant> {
        self.tone.wake_at()
    }

    fn wake(&mut self, cx: &mut Cx<'_>, _now: Instant) {
        self.tone.advance();
        cx.request_paint();
    }

    fn accessibility(&mut self, cx: &mut Cx<'_>) -> Option<Node> {
        let mut node = Node::new(Role::Button);
        node.set_label(self.label.clone());
        if let Some(shortcut) = &self.shortcut {
            node.set_keyboard_shortcut(shortcut.clone());
        }
        node.set_bounds(access_rect(cx.bounds()));
        if self.selected.is_some() {
            node.set_toggled(if self.is_selected() {
                accesskit::Toggled::True
            } else {
                accesskit::Toggled::False
            });
        }
        if self.is_enabled() {
            node.add_action(Action::Click);
        } else {
            node.set_disabled();
        }
        Some(node)
    }

    fn access_action(&mut self, _cx: &mut Cx<'_>, request: &ActionRequest) -> bool {
        if request.action == Action::Click && self.is_enabled() {
            self.run();
            true
        } else {
            false
        }
    }
}

/// An on/off switch (a boolean setting): click, Enter or Space runs its
/// action, which flips what `on` reads.
///
/// ```
/// # use imaginarium_ui::{Switch, Ui};
/// # use imaginarium_reactive::signal;
/// let mut ui = Ui::new();
/// let snap = signal(true);
/// let switch = Switch::new("Snap to whole pixels", move || snap.get(), move || {
///     snap.update(|on| *on = !*on)
/// })
/// .add_to(&mut ui);
/// # let _ = switch;
/// ```
pub struct Switch {
    label: String,
    on: Flag,
    action: Box<dyn FnMut()>,
    pressed: bool,
}

impl Switch {
    /// A 34×20 switch named `label`, showing `on`, that runs `action`.
    pub fn new(
        label: impl Into<String>,
        on: impl Fn() -> bool + 'static,
        action: impl FnMut() + 'static,
    ) -> Self {
        Self {
            label: label.into(),
            on: Rc::new(on),
            action: Box::new(action),
            pressed: false,
        }
    }

    /// Adds the switch to `ui`, repainting it when what `on` reads
    /// changes.
    pub fn add_to(self, ui: &mut Ui) -> WidgetId {
        let on = self.on.clone();
        let id = ui.add(self);
        ui.watch(id, false, move || {
            on();
        });
        id
    }
}

impl Widget for Switch {
    fn style(&self) -> Style {
        Style {
            size: taffy::Size {
                width: length(34.0),
                height: length(20.0),
            },
            flex_shrink: 0.0,
            ..Style::default()
        }
    }

    fn paint(&mut self, cx: &mut Cx<'_>, scene: &mut Scene) {
        use crate::theme::Token;
        let bounds = cx.bounds();
        let theme = theme();
        let on = (self.on)();
        let track = RoundedRect::from_rect(bounds, bounds.height() / 2.0);
        let (fill, knob) = if on {
            (theme.color(Token::Accent), theme.color(Token::OnAccent))
        } else {
            (
                theme.color(Token::ControlBorder),
                theme.color(Token::TextMuted),
            )
        };
        scene.fill(&track, fill);
        let radius = bounds.height() / 2.0 - 2.0;
        let x = if on {
            bounds.x1 - 2.0 - radius
        } else {
            bounds.x0 + 2.0 + radius
        };
        scene.fill(
            &imaginarium_render::kurbo::Circle::new((x, bounds.center().y), radius),
            knob,
        );
        if cx.is_focus_visible() {
            scene.stroke(
                &RoundedRect::from_rect(bounds.inflate(2.0, 2.0), bounds.height() / 2.0 + 2.0),
                2.0,
                theme.color(Token::AccentInk),
            );
        }
    }

    fn event(&mut self, cx: &mut Cx<'_>, input: &Input) -> bool {
        match input {
            Input::PointerButton {
                primary: true,
                pressed,
                position,
                ..
            } => {
                if *pressed {
                    self.pressed = true;
                } else if std::mem::take(&mut self.pressed) && cx.bounds().contains(*position) {
                    (self.action)();
                    cx.request_paint();
                }
                true
            }
            Input::Key {
                key,
                pressed: true,
                repeat: false,
                ..
            } if *key == Key::Enter || *key == Key::Character(" ".into()) => {
                (self.action)();
                cx.request_paint();
                true
            }
            _ => false,
        }
    }

    fn capture_lost(&mut self, _cx: &mut Cx<'_>) {
        self.pressed = false;
    }

    fn focusable(&self) -> bool {
        true
    }

    fn focus_changed(&mut self, cx: &mut Cx<'_>, _focused: bool) {
        cx.request_paint();
    }

    fn accessibility(&mut self, cx: &mut Cx<'_>) -> Option<Node> {
        let mut node = Node::new(Role::Switch);
        node.set_label(self.label.clone());
        node.set_toggled(if (self.on)() {
            accesskit::Toggled::True
        } else {
            accesskit::Toggled::False
        });
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

/// How a [`Swatch`] shows its color.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SwatchShape {
    /// A rounded square (a color grid).
    Square,
    /// A filled dot (the toolbar's fill button).
    Dot,
    /// A ring (the toolbar's outline button).
    Ring,
}

/// A color to pick or a color shown: a rounded square in a color grid, or
/// the dot and ring on the contextual toolbar. `None` is "no color", drawn
/// as a red slash. Click, Enter or Space runs its action when it has one.
pub struct Swatch {
    color: Option<Color>,
    label: String,
    shape: SwatchShape,
    size: f64,
    selected: bool,
    action: Option<Box<dyn FnMut()>>,
    pressed: bool,
}

impl Swatch {
    /// A 28 px square of `color` named `label` ("Indigo") that runs
    /// `action`.
    pub fn new(
        color: Option<Color>,
        label: impl Into<String>,
        action: impl FnMut() + 'static,
    ) -> Self {
        Self {
            color,
            label: label.into(),
            shape: SwatchShape::Square,
            size: 28.0,
            selected: false,
            action: Some(Box::new(action)),
            pressed: false,
        }
    }

    /// A swatch that only shows a color (inside a button that acts).
    pub fn shown(color: Option<Color>, shape: SwatchShape, size: f64) -> Self {
        Self {
            color,
            label: String::new(),
            shape,
            size,
            selected: false,
            action: None,
            pressed: false,
        }
    }

    /// Ringed in the accent: the color the selection has now.
    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    fn run(&mut self) -> bool {
        match &mut self.action {
            Some(action) => {
                action();
                true
            }
            None => false,
        }
    }
}

impl Widget for Swatch {
    fn style(&self) -> Style {
        Style {
            size: taffy::Size {
                width: length(self.size as f32),
                height: length(self.size as f32),
            },
            flex_shrink: 0.0,
            ..Style::default()
        }
    }

    fn paint(&mut self, cx: &mut Cx<'_>, scene: &mut Scene) {
        use crate::theme::Token;
        let bounds = cx.bounds();
        let theme = theme();
        let edge = match theme.kind() {
            crate::theme::ThemeKind::Dark => Color::WHITE.with_alpha(0.28),
            crate::theme::ThemeKind::Light => Color::BLACK.with_alpha(0.14),
        };
        let center = bounds.center();
        match self.shape {
            SwatchShape::Square => {
                let shape = RoundedRect::from_rect(bounds, 8.0);
                scene.fill(&shape, self.color.unwrap_or(theme.color(Token::PanelBg)));
                scene.stroke(&RoundedRect::from_rect(bounds.inset(-0.5), 7.5), 1.0, edge);
                if self.selected {
                    scene.stroke(
                        &RoundedRect::from_rect(bounds.inflate(3.0, 3.0), 11.0),
                        2.0,
                        theme.color(Token::AccentInk),
                    );
                }
            }
            SwatchShape::Dot => {
                let dot = imaginarium_render::kurbo::Circle::new(center, self.size / 2.0);
                scene.fill(&dot, self.color.unwrap_or(theme.color(Token::PanelBg)));
                scene.stroke(
                    &imaginarium_render::kurbo::Circle::new(center, self.size / 2.0 - 0.5),
                    1.0,
                    edge,
                );
            }
            SwatchShape::Ring => {
                let ring = imaginarium_render::kurbo::Circle::new(center, self.size / 2.0 - 1.5);
                scene.stroke(&ring, 3.0, self.color.unwrap_or(edge));
            }
        }
        if self.color.is_none() {
            // No color: a red slash across.
            let inset = bounds.inset(-self.size * 0.2);
            let mut slash = imaginarium_render::kurbo::BezPath::new();
            slash.move_to((inset.x0, inset.y1));
            slash.line_to((inset.x1, inset.y0));
            scene.stroke_with(
                &slash,
                &Stroke::new(2.0).with_caps(Cap::Round),
                theme.color(Token::Danger),
            );
        }
        if cx.is_focus_visible() {
            scene.stroke(
                &RoundedRect::from_rect(bounds.inflate(2.0, 2.0), 10.0),
                2.0,
                theme.color(Token::AccentInk),
            );
        }
    }

    fn event(&mut self, cx: &mut Cx<'_>, input: &Input) -> bool {
        if self.action.is_none() {
            return false;
        }
        match input {
            Input::PointerButton {
                primary: true,
                pressed,
                position,
                ..
            } => {
                if *pressed {
                    self.pressed = true;
                } else if std::mem::take(&mut self.pressed) && cx.bounds().contains(*position) {
                    self.run();
                }
                true
            }
            // Once per press; a held key's repeats are taken, not passed on.
            Input::Key {
                key,
                pressed: true,
                repeat,
                ..
            } if *key == Key::Enter || *key == Key::Character(" ".into()) => {
                if *repeat {
                    true
                } else {
                    self.run()
                }
            }
            _ => false,
        }
    }

    fn capture_lost(&mut self, _cx: &mut Cx<'_>) {
        self.pressed = false;
    }

    fn focusable(&self) -> bool {
        self.action.is_some()
    }

    fn focus_changed(&mut self, cx: &mut Cx<'_>, _focused: bool) {
        cx.request_paint();
    }

    fn tooltip(&self) -> Option<String> {
        (self.action.is_some() && !self.label.is_empty()).then(|| self.label.clone())
    }

    fn accessibility(&mut self, cx: &mut Cx<'_>) -> Option<Node> {
        // A shown color says nothing of its own: its button names it.
        self.action.as_ref()?;
        let mut node = Node::new(Role::Button);
        node.set_label(self.label.clone());
        if self.selected {
            node.set_selected(true);
        }
        node.set_bounds(access_rect(cx.bounds()));
        node.add_action(Action::Click);
        Some(node)
    }

    fn access_action(&mut self, _cx: &mut Cx<'_>, request: &ActionRequest) -> bool {
        request.action == Action::Click && self.run()
    }
}

/// A value on a track (corner radius, opacity): drag the knob or click the
/// track; ←→ step it by a twentieth. It shows its own value while
/// dragged and hands the value on when the press ends, so a drag is one
/// change, not one per pointer move.
pub struct Slider {
    label: String,
    range: (f64, f64),
    value: Rc<dyn Fn() -> f64>,
    on_change: Box<dyn FnMut(f64)>,
    /// The value while dragging, before it is handed on.
    dragging: Option<f64>,
}

impl Slider {
    /// A slider named `label` over `min..=max`, showing `value`, that
    /// calls `on_change` with a new value.
    pub fn new(
        label: impl Into<String>,
        min: f64,
        max: f64,
        value: impl Fn() -> f64 + 'static,
        on_change: impl FnMut(f64) + 'static,
    ) -> Self {
        Self {
            label: label.into(),
            range: (min, max.max(min + f64::EPSILON)),
            value: Rc::new(value),
            on_change: Box::new(on_change),
            dragging: None,
        }
    }

    /// Adds it to `ui`, repainting it when what `value` reads changes.
    pub fn add_to(self, ui: &mut Ui) -> WidgetId {
        let value = self.value.clone();
        let id = ui.add(self);
        ui.watch(id, false, move || {
            value();
        });
        id
    }

    fn shown(&self) -> f64 {
        let (min, max) = self.range;
        self.dragging
            .unwrap_or_else(|| (self.value)())
            .clamp(min, max)
    }

    /// The value at window x `x` on a track in `bounds`.
    fn at(&self, bounds: Rect, x: f64) -> f64 {
        let (min, max) = self.range;
        let inner = bounds.inset(-8.0);
        let t = ((x - inner.x0) / inner.width().max(1.0)).clamp(0.0, 1.0);
        min + t * (max - min)
    }
}

impl Widget for Slider {
    fn style(&self) -> Style {
        Style {
            size: taffy::Size {
                width: taffy::Dimension::auto(),
                height: length(16.0),
            },
            flex_grow: 1.0,
            min_size: taffy::Size {
                width: length(48.0),
                height: length(16.0),
            },
            ..Style::default()
        }
    }

    fn paint(&mut self, cx: &mut Cx<'_>, scene: &mut Scene) {
        use crate::theme::Token;
        let bounds = cx.bounds();
        let theme = theme();
        let (min, max) = self.range;
        let t = (self.shown() - min) / (max - min);
        let inner = bounds.inset(-8.0);
        let y = bounds.center().y;
        let x = inner.x0 + t * inner.width();
        let track = RoundedRect::new(inner.x0, y - 2.0, inner.x1, y + 2.0, 2.0);
        scene.fill(&track, theme.color(Token::ControlBorder));
        let filled = RoundedRect::new(inner.x0, y - 2.0, x, y + 2.0, 2.0);
        scene.fill(&filled, theme.color(Token::Accent));
        let knob = imaginarium_render::kurbo::Circle::new((x, y), 7.25);
        scene.fill(&knob, theme.color(Token::PanelBg));
        scene.stroke(&knob, 1.5, theme.color(Token::Accent));
        if cx.is_focus_visible() {
            scene.stroke(
                &imaginarium_render::kurbo::Circle::new((x, y), 10.5),
                2.0,
                theme.color(Token::AccentInk),
            );
        }
    }

    fn event(&mut self, cx: &mut Cx<'_>, input: &Input) -> bool {
        let bounds = cx.bounds();
        match input {
            Input::PointerButton {
                primary: true,
                pressed: true,
                position,
                ..
            } => {
                self.dragging = Some(self.at(bounds, position.x));
                cx.request_paint();
                true
            }
            Input::PointerMoved(position) if self.dragging.is_some() => {
                self.dragging = Some(self.at(bounds, position.x));
                cx.request_paint();
                true
            }
            Input::PointerButton {
                primary: true,
                pressed: false,
                position,
                ..
            } => {
                if self.dragging.take().is_some() {
                    let value = self.at(bounds, position.x);
                    (self.on_change)(value);
                    cx.request_paint();
                }
                true
            }
            Input::Key {
                key, pressed: true, ..
            } if *key == Key::ArrowLeft || *key == Key::ArrowRight => {
                let (min, max) = self.range;
                let step = (max - min) / 20.0;
                let by = if *key == Key::ArrowLeft { -step } else { step };
                let value = (self.shown() + by).clamp(min, max);
                (self.on_change)(value);
                cx.request_paint();
                true
            }
            _ => false,
        }
    }

    fn capture_lost(&mut self, cx: &mut Cx<'_>) {
        // The press ended elsewhere: the value goes back.
        self.dragging = None;
        cx.request_paint();
    }

    fn focusable(&self) -> bool {
        true
    }

    fn focus_on_click(&self) -> bool {
        false
    }

    fn focus_changed(&mut self, cx: &mut Cx<'_>, _focused: bool) {
        cx.request_paint();
    }

    fn accessibility(&mut self, cx: &mut Cx<'_>) -> Option<Node> {
        let (min, max) = self.range;
        let mut node = Node::new(Role::Slider);
        node.set_label(self.label.clone());
        node.set_numeric_value(self.shown());
        node.set_min_numeric_value(min);
        node.set_max_numeric_value(max);
        node.set_bounds(access_rect(cx.bounds()));
        node.add_action(Action::SetValue);
        Some(node)
    }

    fn access_action(&mut self, _cx: &mut Cx<'_>, request: &ActionRequest) -> bool {
        match (request.action, &request.data) {
            (Action::SetValue, Some(accesskit::ActionData::NumericValue(value))) => {
                let (min, max) = self.range;
                (self.on_change)(value.clamp(min, max));
                true
            }
            _ => false,
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
    /// Drawn without its box and focus line (the palette's field).
    bare: bool,
    colors: InputColors,
    height: f32,
}

/// Colors of a text input: theme tokens or fixed colors.
#[derive(Clone, Copy, Debug)]
pub struct InputColors {
    /// The field's background.
    pub background: Paint,
    /// The border at rest.
    pub border: Paint,
    /// The border, caret and selection while focused.
    pub accent: Paint,
    /// The placeholder text.
    pub placeholder: Paint,
    /// The text.
    pub text: Paint,
}

impl InputColors {
    /// An input on a panel, following the theme.
    pub fn themed() -> Self {
        use crate::theme::Token;
        Self {
            background: Token::PanelBg.into(),
            border: Token::ControlBorder.into(),
            accent: Token::AccentInk.into(),
            placeholder: Token::TextSubtle.into(),
            text: Token::Text.into(),
        }
    }
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
            placeholder: Label::new(placeholder, style).color(colors.placeholder),
            on_change: Box::new(|_| {}),
            on_submit: Box::new(|_| {}),
            passes: Vec::new(),
            bare: false,
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

    /// Without its box and focus line: the field of a panel that is the
    /// box itself (the command palette).
    pub fn bare(mut self) -> Self {
        self.bare = true;
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

    /// Replaces the text (no change callback), ending a composition in
    /// progress.
    pub fn set_text(&mut self, fonts: &mut Fonts, text: &str) {
        self.field.editor().set_text(fonts, text);
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
        let theme = theme();
        if !self.bare {
            let shape = RoundedRect::from_rect(bounds, theme.control_radius);
            scene.fill(&shape, self.colors.background.resolve(&theme));
            if cx.is_focused() {
                scene.stroke(&shape, 2.0, self.colors.accent.resolve(&theme));
            } else {
                scene.stroke(&shape, 1.0, self.colors.border.resolve(&theme));
            }
        }
        self.field
            .editor()
            .set_color(self.colors.text.resolve(&theme));
        if self.field.text().is_empty() && !self.field.editor().is_composing() {
            let origin = Point::new(bounds.x0 + 7.0, bounds.y0 + 7.0);
            self.placeholder.block(cx, None).paint(scene, origin, true);
        }
        let accent = self.colors.accent.resolve(&theme);
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
        let response = self.field.access_action(cx.fonts(), request, base);
        if response.wants_focus {
            cx.request_focus();
        }
        if response.changed {
            let text = self.field.text();
            (self.on_change)(&text);
        }
        if response.repaint {
            cx.request_paint();
        }
        response.handled
    }
}

/// A line icon: SVG path data on Lucide's 24-unit grid, stroked with
/// round ends at the theme's icon weight and scaled to `size`.
pub struct Icon {
    paths: Vec<BezPath>,
    size: f64,
    paint: Paint,
    label: Option<String>,
}

impl Icon {
    /// An icon from its paths' `d` strings (circles and rectangles as paths,
    /// as `cargo xtask icons` writes them). A path that doesn't parse is
    /// left out.
    pub fn new(paths: &[&str], size: f64, paint: impl Into<Paint>) -> Self {
        Self {
            paths: paths
                .iter()
                .filter_map(|data| BezPath::from_svg(data).ok())
                .collect(),
            size,
            paint: paint.into(),
            label: None,
        }
    }

    /// A vendored Lucide icon by name (`"hand"`, `"trash"`; the list is
    /// `src/icons/icons.txt`), for names written in the code. An unknown
    /// name panics in debug builds and tests, so a typo shows at once, and
    /// draws nothing in a release build. For names that arrive at run time
    /// (add-ons, settings), use [`Icon::try_named`].
    pub fn named(name: &str, size: f64, paint: impl Into<Paint>) -> Self {
        let paths = crate::icons::icon_paths(name);
        debug_assert!(paths.is_some(), "the icon `{name}` isn't vendored");
        Self::new(paths.unwrap_or(&[]), size, paint)
    }

    /// The vendored icon `name`, or `None` when there is none.
    pub fn try_named(name: &str, size: f64, paint: impl Into<Paint>) -> Option<Self> {
        let paths = crate::icons::icon_paths(name)?;
        Some(Self::new(paths, size, paint))
    }

    /// Names it for screen readers (otherwise it is decoration).
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }
}

impl Widget for Icon {
    fn style(&self) -> Style {
        Style {
            size: taffy::Size {
                width: length(self.size as f32),
                height: length(self.size as f32),
            },
            flex_shrink: 0.0,
            ..Style::default()
        }
    }

    fn paint(&mut self, cx: &mut Cx<'_>, scene: &mut Scene) {
        let bounds = cx.bounds();
        let scale = self.size / 24.0;
        let transform = Affine::translate(bounds.origin().to_vec2()) * Affine::scale(scale);
        // 1.75 units of 24, as the mockups draw them, at any size.
        let stroke = Stroke::new(1.75 * scale)
            .with_caps(Cap::Round)
            .with_join(Join::Round);
        let color = self.paint.resolve(&theme());
        for path in &self.paths {
            scene.stroke_with(&(transform * path.clone()), &stroke, color);
        }
    }

    fn accessibility(&mut self, cx: &mut Cx<'_>) -> Option<Node> {
        let label = self.label.clone()?;
        let mut node = Node::new(Role::Image);
        node.set_label(label);
        node.set_bounds(access_rect(cx.bounds()));
        Some(node)
    }
}
