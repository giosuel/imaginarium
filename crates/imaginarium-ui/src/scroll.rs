// SPDX-License-Identifier: AGPL-3.0-or-later

//! Scrolling: [`Scroll`], a container whose content moves under it, and
//! [`VirtualList`], a scrolled list that only has widgets for the rows in
//! view (the palette's results, recent files, a long menu).

use std::rc::Rc;

use accesskit::{Node, Role};
use imaginarium_render::Scene;
use imaginarium_render::kurbo::{Point, Rect, RoundedRect, Size, Vec2};
use taffy::prelude::{length, percent};
use taffy::{Display, Overflow, Style};

use crate::access::rect as access_rect;
use crate::theme::{Token, theme};
use crate::tree::{Cx, Ui, Widget, WidgetId};
use crate::window::{Input, Key};

/// The scroll bar's width.
const BAR: f64 = 6.0;
/// The strip along the right edge where the pointer grabs the bar.
const TRACK: f64 = 12.0;
/// The shortest the thumb gets.
const MIN_THUMB: f64 = 24.0;

/// A container that scrolls its content vertically: the wheel and touchpad
/// (the scroll goes on to an outer container at either end), Page Up and
/// Page Down, a thin scroll bar to drag or click beside, and the focus,
/// which it keeps in view. Its content is its child, laid out at its own
/// height.
pub struct Scroll {
    style: Style,
    offset: f64,
    /// A thumb drag: where the pointer and the offset were at the press.
    drag: Option<(f64, f64)>,
    over_bar: bool,
    label: Option<String>,
}

impl Default for Scroll {
    fn default() -> Self {
        Self::new()
    }
}

impl Scroll {
    /// A scroll container that fills the width it is given; set its height
    /// (or let a parent decide) with [`Scroll::height`] or
    /// [`Scroll::style`].
    pub fn new() -> Self {
        Self {
            style: Style {
                // Block layout: the content keeps its own height.
                display: Display::Block,
                overflow: taffy::Point {
                    x: Overflow::Clip,
                    y: Overflow::Scroll,
                },
                scrollbar_width: 0.0,
                size: taffy::Size {
                    width: percent(1.0),
                    height: taffy::Dimension::auto(),
                },
                min_size: taffy::Size {
                    width: length(0.0),
                    height: length(0.0),
                },
                ..Style::default()
            },
            offset: 0.0,
            drag: None,
            over_bar: false,
            label: None,
        }
    }

    /// A fixed height.
    pub fn height(mut self, height: f32) -> Self {
        self.style.size.height = length(height);
        self
    }

    /// Takes a share of the free space along the parent's direction.
    pub fn grow(mut self, grow: f32) -> Self {
        self.style.flex_grow = grow;
        self.style.flex_basis = length(0.0);
        self
    }

    /// Any other style change.
    pub fn style(mut self, change: impl FnOnce(&mut Style)) -> Self {
        change(&mut self.style);
        self
    }

    /// Names it for screen readers.
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// How far the content is scrolled, in logical pixels.
    pub fn offset(&self) -> f64 {
        self.offset
    }

    /// Scrolls to `offset` (clamped at the next layout).
    pub fn set_offset(&mut self, offset: f64) {
        self.offset = offset.max(0.0);
    }

    fn max_offset(bounds: Rect, content: Size) -> f64 {
        (content.height - bounds.height()).max(0.0)
    }

    /// Moves to `offset`, clamped; `true` if it moved.
    fn scroll_to(&mut self, cx: &mut Cx<'_>, offset: f64) -> bool {
        let clamped = offset.clamp(0.0, Self::max_offset(cx.bounds(), cx.content_size()));
        if (clamped - self.offset).abs() < 0.01 {
            return false;
        }
        self.offset = clamped;
        cx.request_place();
        true
    }

    /// The thumb, when the content is taller than the box.
    fn thumb(&self, bounds: Rect, content: Size) -> Option<Rect> {
        let max = Self::max_offset(bounds, content);
        if max <= 0.0 {
            return None;
        }
        let height = bounds.height();
        let length = (height * height / content.height)
            .max(MIN_THUMB)
            .min(height);
        let top = bounds.y0 + (height - length) * (self.offset / max).clamp(0.0, 1.0);
        let x = bounds.x1 - BAR - 3.0;
        Some(Rect::new(x, top, x + BAR, top + length))
    }
}

impl Widget for Scroll {
    fn style(&self) -> Style {
        self.style.clone()
    }

    fn clips(&self) -> bool {
        true
    }

    fn scroll_offset(&self) -> Vec2 {
        Vec2::new(0.0, self.offset)
    }

    fn claims(&self, bounds: Rect, content: Size, point: Point) -> bool {
        Self::max_offset(bounds, content) > 0.0 && point.x >= bounds.x1 - TRACK
    }

    fn paints_over(&self) -> bool {
        true
    }

    fn paint_over(&mut self, cx: &mut Cx<'_>, scene: &mut Scene) {
        // Content that shrank since the last scroll: stay in range.
        let max = Self::max_offset(cx.bounds(), cx.content_size());
        if self.offset > max {
            self.offset = max;
            cx.request_place();
        }
        if let Some(thumb) = self.thumb(cx.bounds(), cx.content_size()) {
            let strong = self.drag.is_some() || self.over_bar;
            let color =
                theme()
                    .color(Token::TextSubtle)
                    .with_alpha(if strong { 0.75 } else { 0.4 });
            scene.fill(&RoundedRect::from_rect(thumb, BAR / 2.0), color);
        }
    }

    fn event(&mut self, cx: &mut Cx<'_>, input: &Input) -> bool {
        let bounds = cx.bounds();
        match input {
            // Ctrl + wheel zooms, for someone else.
            Input::Scroll {
                delta, modifiers, ..
            } if !modifiers.command() => self.scroll_to(cx, self.offset - delta.y),
            Input::PointerButton {
                primary: true,
                pressed: true,
                position,
                ..
            } if position.x >= bounds.x1 - TRACK => {
                match self.thumb(bounds, cx.content_size()) {
                    Some(thumb) if position.y >= thumb.y0 && position.y <= thumb.y1 => {
                        self.drag = Some((position.y, self.offset));
                        cx.request_paint();
                    }
                    // Beside the thumb: a page toward the click.
                    Some(thumb) => {
                        let page = bounds.height() * 0.9;
                        let toward = if position.y < thumb.y0 { -page } else { page };
                        self.scroll_to(cx, self.offset + toward);
                    }
                    None => return false,
                }
                true
            }
            Input::PointerButton { pressed: false, .. } if self.drag.is_some() => {
                self.drag = None;
                cx.request_paint();
                true
            }
            Input::PointerMoved(position) => {
                if let Some((start, from)) = self.drag {
                    let content = cx.content_size();
                    if let Some(thumb) = self.thumb(bounds, content) {
                        let travel = (bounds.height() - thumb.height()).max(1.0);
                        let per_pixel = Self::max_offset(bounds, content) / travel;
                        self.scroll_to(cx, from + (position.y - start) * per_pixel);
                    }
                    return true;
                }
                let over = position.x >= bounds.x1 - TRACK && bounds.contains(*position);
                if over != self.over_bar {
                    self.over_bar = over;
                    cx.request_paint();
                }
                false
            }
            Input::PointerLeft if self.over_bar => {
                self.over_bar = false;
                cx.request_paint();
                false
            }
            // Keys that the focused widget inside didn't use.
            Input::Key {
                key, pressed: true, ..
            } => {
                let page = bounds.height() * 0.9;
                match key {
                    Key::PageDown => self.scroll_to(cx, self.offset + page),
                    Key::PageUp => self.scroll_to(cx, self.offset - page),
                    Key::Home => self.scroll_to(cx, 0.0),
                    Key::End => self.scroll_to(cx, f64::MAX),
                    _ => false,
                }
            }
            _ => false,
        }
    }

    fn reveal(&mut self, cx: &mut Cx<'_>, target: Rect) -> bool {
        let bounds = cx.bounds();
        let mut offset = self.offset;
        if target.y1 > bounds.y1 {
            offset += target.y1 - bounds.y1;
        }
        // The top wins when the target is taller than the box.
        if target.y0 - (offset - self.offset) < bounds.y0 {
            offset = self.offset - (bounds.y0 - target.y0);
        }
        self.scroll_to(cx, offset)
    }

    fn accessibility(&mut self, cx: &mut Cx<'_>) -> Option<Node> {
        let mut node = Node::new(Role::ScrollView);
        if let Some(label) = &self.label {
            node.set_label(label.clone());
        }
        node.set_bounds(access_rect(cx.bounds()));
        node.set_scroll_y(self.offset);
        node.set_scroll_y_min(0.0);
        node.set_scroll_y_max(Self::max_offset(cx.bounds(), cx.content_size()));
        Some(node)
    }
}

/// Builds the row at an index.
pub type RowBuilder = Box<dyn FnMut(&mut Ui, usize) -> WidgetId>;

/// A list of `count` rows of one height in a [`Scroll`], where only the
/// rows in view (and a few around them) exist as widgets: the tree builds
/// them during layout with `build` and removes them when they scroll away,
/// so 10,000 rows cost what 20 do. Add it with [`Ui::add_virtual_list`].
pub struct VirtualList {
    pub(crate) row_height: f64,
    pub(crate) overscan: usize,
    pub(crate) count: Rc<dyn Fn() -> usize>,
    pub(crate) build: RowBuilder,
    pub(crate) label: String,
}

impl VirtualList {
    /// A list of rows `row_height` tall. `count` says how many there are;
    /// read inside it, signals make the list update when they change.
    /// `build` makes the row at an index (a widget without a parent).
    pub fn new(
        row_height: f64,
        count: impl Fn() -> usize + 'static,
        build: impl FnMut(&mut Ui, usize) -> WidgetId + 'static,
    ) -> Self {
        Self {
            row_height,
            overscan: 4,
            count: Rc::new(count),
            build: Box::new(build),
            label: String::new(),
        }
    }

    /// Rows kept beyond each edge of the view (default 4), so a quick
    /// scroll doesn't show empty space.
    pub fn overscan(mut self, rows: usize) -> Self {
        self.overscan = rows;
        self
    }

    /// Names the list for screen readers.
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = label.into();
        self
    }
}

/// The list's full height, holding the rows at their places: a spacer as
/// tall as the rows above the first built one, then the built rows, in
/// flow. (Absolutely placed rows would be simpler, but Taffy keeps a cached
/// list of out-of-flow children that went stale as rows came and went.)
pub(crate) struct ListBody {
    pub(crate) height: f64,
    pub(crate) count: usize,
    pub(crate) label: String,
}

impl Widget for ListBody {
    fn style(&self) -> Style {
        Style {
            display: Display::Flex,
            flex_direction: taffy::FlexDirection::Column,
            size: taffy::Size {
                width: percent(1.0),
                height: length(self.height as f32),
            },
            ..Style::default()
        }
    }

    fn accessibility(&mut self, cx: &mut Cx<'_>) -> Option<Node> {
        let mut node = Node::new(Role::List);
        if !self.label.is_empty() {
            node.set_label(self.label.clone());
        }
        node.set_size_of_set(self.count);
        node.set_bounds(access_rect(cx.bounds()));
        Some(node)
    }
}

/// One row's place in a [`VirtualList`].
pub(crate) struct RowSlot {
    pub(crate) index: usize,
    pub(crate) count: usize,
    pub(crate) height: f64,
}

impl Widget for RowSlot {
    fn style(&self) -> Style {
        Style {
            size: taffy::Size {
                width: percent(1.0),
                height: length(self.height as f32),
            },
            flex_shrink: 0.0,
            display: Display::Flex,
            flex_direction: taffy::FlexDirection::Column,
            ..Style::default()
        }
    }

    fn accessibility(&mut self, cx: &mut Cx<'_>) -> Option<Node> {
        // Screen readers learn the full length even though most rows
        // don't exist yet.
        let mut node = Node::new(Role::ListItem);
        node.set_position_in_set(self.index + 1);
        node.set_size_of_set(self.count);
        node.set_bounds(access_rect(cx.bounds()));
        Some(node)
    }
}

/// The space above a [`VirtualList`]'s first built row.
pub(crate) struct ListSpacer {
    pub(crate) height: f64,
}

impl Widget for ListSpacer {
    fn style(&self) -> Style {
        Style {
            size: taffy::Size {
                width: percent(1.0),
                height: length(self.height as f32),
            },
            flex_shrink: 0.0,
            ..Style::default()
        }
    }
}
