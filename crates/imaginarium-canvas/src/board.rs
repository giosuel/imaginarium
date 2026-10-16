// SPDX-License-Identifier: AGPL-3.0-or-later

//! The board widget: a document's nodes on the tile canvas, with the
//! board's background, panning and zooming, and what is drawn on top of
//! the tiles (frame names).

use std::collections::HashMap;
use std::rc::Rc;
use std::time::Instant;

use accesskit::{Action, ActionRequest, Node, Role};
use imaginarium_model::{
    Actor, ChangeSet, Document, NodeId, NodeKind, Result, Transaction, UndoScope,
};
use imaginarium_reactive::{Signal, signal};
use imaginarium_render::canvas::{Canvas, CanvasStats, Viewport};
use imaginarium_render::kurbo::{Affine, Point, Rect, Stroke, Vec2};
use imaginarium_render::{Scene, TileWorkers};
use imaginarium_text::{Fonts, TextBlock, TextStyle};
use imaginarium_ui::{
    ACCESS_LOCAL_IDS, Cx, Input, Key, Theme, Token, Widget, access_rect, mix, theme,
};

use crate::hit;
use crate::items::{BoardColors, Items};
use crate::paint::ItemPainter;

/// What the pointer is doing since its press.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Press {
    /// Pressed on a node, which is now selected.
    Node,
    /// Dragging a marquee from `start` (document space); `additive` keeps
    /// the selection it started with.
    Marquee {
        start: Point,
        current: Point,
        additive: bool,
    },
}

/// The most items listed for screen readers: the visible ones, up to this.
const MAX_ACCESS_ITEMS: usize = 2_000;

/// The zoom's limits.
pub const MIN_ZOOM: f64 = 0.05;
/// The zoom's limits.
pub const MAX_ZOOM: f64 = 8.0;

/// The board's dots, every this many points at 100 %.
const DOT_PITCH: f64 = 24.0;

/// A frame's name sits this far above it, in logical pixels.
const NAME_GAP: f64 = 8.0;

/// A board: the document it shows (ADR-0020: the open document lives on
/// the UI thread, here) and its view.
pub struct BoardCanvas {
    document: Document,
    items: Option<Items>,
    canvas: Canvas,
    origin: Point,
    zoom: f64,
    /// The theme the items were built in; a switch rebuilds them.
    theme: Option<Rc<Theme>>,
    /// Frames drawn so far: the first is drawn without tiles.
    painted: u64,
    /// Tiles are owed: wake to draw them.
    owed_at: Option<Instant>,
    /// Frame names laid out, by frame and text.
    names: HashMap<NodeId, (String, TextBlock)>,
    /// Changes not on the canvas yet (laying out text needs the fonts,
    /// which painting and input bring).
    pending: Vec<ChangeSet>,
    stats: CanvasStats,
    /// The selected nodes, in the order they were selected.
    selection: Signal<Vec<NodeId>>,
    /// The node under the pointer.
    hovered: Option<NodeId>,
    /// The group a double-click went into: its children select one by one.
    entered: Option<NodeId>,
    press: Option<Press>,
    /// Where frame names were drawn (window space), front last.
    name_rects: Vec<(NodeId, Rect)>,
    /// The nodes listed for screen readers, by their local access id - 1.
    access_nodes: Vec<NodeId>,
    /// The board's box in the last layout.
    bounds: Rect,
}

impl BoardCanvas {
    /// A board showing `document`, at 100 % with its origin at the top left.
    pub fn new(document: Document) -> Self {
        Self {
            document,
            items: None,
            canvas: Canvas::new(),
            origin: Point::ZERO,
            zoom: 1.0,
            theme: None,
            painted: 0,
            owed_at: None,
            names: HashMap::new(),
            pending: Vec::new(),
            stats: CanvasStats::default(),
            selection: signal(Vec::new()),
            hovered: None,
            entered: None,
            press: None,
            name_rects: Vec::new(),
            access_nodes: Vec::new(),
            bounds: Rect::ZERO,
        }
    }

    /// The selection: the selected nodes, in the order they were selected.
    /// Panels and commands read it; it stays valid as nodes go.
    pub fn selection(&self) -> Signal<Vec<NodeId>> {
        self.selection
    }

    /// Selects `nodes` (those that exist and are visible).
    pub fn select(&mut self, nodes: &[NodeId]) {
        let view = self.document.view();
        let mut chosen: Vec<NodeId> = Vec::new();
        for node in nodes {
            if view.node(*node).is_some() && !chosen.contains(node) {
                chosen.push(*node);
            }
        }
        self.selection.set_if_changed(chosen);
    }

    /// Runs `edit` as one transaction of `actor` and puts its changes on
    /// the board (`Document::transact`). Every edit of a shown document
    /// goes through here, or its changes through [`BoardCanvas::apply`].
    pub fn transact<R>(
        &mut self,
        actor: &Actor,
        edit: impl FnOnce(&mut Transaction<'_>) -> Result<R>,
    ) -> Result<R> {
        let (result, changes) = self.document.transact(actor, edit)?;
        self.apply(changes);
        Ok(result)
    }

    /// Undoes the last step of `scope`'s stack. `false` if there was none.
    pub fn undo(&mut self, scope: UndoScope, by: &Actor) -> Result<bool> {
        let changes = self.document.undo(scope, by)?;
        let undone = changes.is_some();
        if let Some(changes) = changes {
            self.apply(changes);
        }
        Ok(undone)
    }

    /// Redoes the last undone step of `scope`'s stack. `false` if there was
    /// none.
    pub fn redo(&mut self, scope: UndoScope, by: &Actor) -> Result<bool> {
        let changes = self.document.redo(scope, by)?;
        let redone = changes.is_some();
        if let Some(changes) = changes {
            self.apply(changes);
        }
        Ok(redone)
    }

    /// Applies updates from other peers (`Document::apply_remote`).
    pub fn apply_remote(&mut self, updates: &[&[u8]]) -> Result<()> {
        let changes = self.document.apply_remote(updates)?;
        self.apply(changes);
        Ok(())
    }

    /// Puts changes the document already contains on the board: only the
    /// tiles they touch are drawn again. They take effect at the next
    /// paint or input.
    pub fn apply(&mut self, changes: ChangeSet) {
        if !changes.is_empty() {
            self.pending.push(changes);
        }
    }

    #[cfg(test)]
    pub(crate) fn sync_for_tests(&mut self, fonts: &mut Fonts) {
        self.sync(fonts);
    }

    #[cfg(test)]
    pub(crate) fn items_for_tests(&self) -> &Items {
        self.items.as_ref().expect("synced")
    }

    #[cfg(test)]
    pub(crate) fn items_mut_for_tests(&mut self) -> &mut Items {
        self.items.as_mut().expect("synced")
    }

    #[cfg(test)]
    pub(crate) fn document_mut_for_tests(&mut self) -> &mut Document {
        &mut self.document
    }

    /// Brings the items up to date with the document: built in the
    /// current theme, with every pending change applied.
    fn sync(&mut self, fonts: &mut Fonts) {
        self.ensure_items(fonts);
        if self.pending.is_empty() {
            return;
        }
        let colors = BoardColors::of(&theme());
        let Some(items) = &mut self.items else {
            return;
        };
        for changes in std::mem::take(&mut self.pending) {
            let update = items.apply(self.document.view(), &changes, fonts, &colors);
            for id in &update.removed {
                self.canvas.remove(*id);
            }
            for node in &update.upserted {
                if let Some(item) = items.by_node.get(node) {
                    self.canvas.upsert(item.id, item.z, item.bounds);
                }
            }
            for node in &changes.deleted {
                self.names.remove(node);
            }
        }
        // Gone or hidden nodes leave the selection and the hover.
        let still =
            |node: &NodeId| {
                items.is_visible(*node)
                    || self.document.view().node(*node).is_some_and(|_| {
                        hit::node_box(items, self.document.view(), *node).is_some()
                    })
            };
        let selection = self.selection.get_untracked();
        if !selection.iter().all(still) {
            self.selection
                .set(selection.into_iter().filter(still).collect());
        }
        if self.hovered.is_some_and(|node| !still(&node)) {
            self.hovered = None;
        }
        if self
            .entered
            .is_some_and(|node| self.document.view().node(node).is_none())
        {
            self.entered = None;
        }
    }

    /// Renders tiles on `workers` instead of in the frame (ADR-0019).
    pub fn with_workers(mut self, workers: TileWorkers) -> Self {
        self.canvas = self.canvas.with_workers(workers);
        self
    }

    /// The document shown.
    pub fn document(&self) -> &Document {
        &self.document
    }

    /// The document point at the board's top left corner.
    pub fn origin(&self) -> Point {
        self.origin
    }

    /// Logical pixels per point.
    pub fn zoom(&self) -> f64 {
        self.zoom
    }

    /// Shows the board from `origin` at `zoom` (clamped to the limits).
    pub fn set_view(&mut self, origin: Point, zoom: f64) {
        self.origin = origin;
        self.zoom = zoom.clamp(MIN_ZOOM, MAX_ZOOM);
    }

    /// What the last frame's tiles cost.
    pub fn stats(&self) -> &CanvasStats {
        &self.stats
    }

    /// The document point under `screen` (logical pixels in the window),
    /// for a board whose box is `bounds`.
    pub fn to_document(&self, bounds: Rect, screen: Point) -> Point {
        self.origin + (screen - bounds.origin()) / self.zoom
    }

    /// Zooms by `factor` keeping the document point under `anchor` in place.
    pub fn zoom_about(&mut self, bounds: Rect, factor: f64, anchor: Point) {
        let before = self.to_document(bounds, anchor);
        self.zoom = (self.zoom * factor).clamp(MIN_ZOOM, MAX_ZOOM);
        let after = self.to_document(bounds, anchor);
        self.origin += before - after;
    }

    fn viewport(&self, bounds: Rect) -> Viewport {
        Viewport {
            origin: self.origin,
            zoom: self.zoom,
            screen: bounds,
            scale_factor: imaginarium_ui::scale_factor(),
        }
    }

    /// The camera: document points to the window's logical pixels.
    fn camera(&self, bounds: Rect) -> Affine {
        Affine::translate(bounds.origin().to_vec2())
            * Affine::scale(self.zoom)
            * Affine::translate(-self.origin.to_vec2())
    }

    /// Builds the items when there are none, or in a new theme.
    fn ensure_items(&mut self, fonts: &mut Fonts) {
        let current = theme();
        let stale = self
            .theme
            .as_ref()
            .is_none_or(|built| !Rc::ptr_eq(built, &current));
        if !stale && self.items.is_some() {
            return;
        }
        let colors = BoardColors::of(&current);
        let items = Items::build(self.document.view(), fonts, &colors);
        // Replace the canvas's items: the old ones go, the new ones come.
        if let Some(old) = &self.items {
            for item in old.by_node.values() {
                if !items.by_item.contains_key(&item.id) {
                    self.canvas.remove(item.id);
                }
            }
        }
        for item in items.by_node.values() {
            self.canvas.upsert(item.id, item.z, item.bounds);
        }
        if self.items.is_some() {
            self.canvas.invalidate_all();
        }
        self.names.clear();
        // A build reads the whole view, pending changes included.
        self.pending.clear();
        self.items = Some(items);
        self.theme = Some(current);
    }

    /// Paints the board into `bounds` (window coordinates) as its widget
    /// does: the background, the tiles (directly in the first frame) and
    /// the frame names. For the widget, exports and tests.
    pub fn paint_into(&mut self, fonts: &mut Fonts, scene: &mut Scene, bounds: Rect, now: Instant) {
        self.sync(fonts);
        scene.push_clip(&bounds);
        scene.fill(&bounds, theme().color(Token::BoardBg));
        self.paint_dots(scene, bounds);
        let viewport = self.viewport(bounds);
        let mut stats = CanvasStats::default();
        if let Some(items) = &self.items {
            let mut painter = ItemPainter::new(items, &[]);
            // Rendering every visible tile would delay the first frame:
            // draw it directly, tiles from the next one.
            stats = if self.painted == 0 {
                self.canvas.draw_direct(scene, &viewport, &mut painter)
            } else {
                self.canvas.frame(scene, &viewport, now, &mut painter)
            };
        }
        let owed = stats.owed > 0 || self.painted == 0;
        self.painted += 1;
        self.stats = stats;
        self.owed_at = owed.then_some(now);
        self.paint_names(fonts, scene, bounds);
        self.paint_selection(scene, bounds);
        scene.pop();
        self.bounds = bounds;
    }

    /// The color of selection outlines and handles: the accent, lighter
    /// in the dark theme (the mockups).
    fn selection_color() -> imaginarium_render::peniko::Color {
        let theme = theme();
        let accent = theme.color(Token::Accent);
        match theme.kind() {
            imaginarium_ui::ThemeKind::Dark => {
                mix(accent, imaginarium_render::peniko::Color::WHITE, 0.35)
            }
            imaginarium_ui::ThemeKind::Light => accent,
        }
    }

    /// The outline of a node's box in window space: its item's turned box,
    /// a group's bounding box.
    fn node_outline(
        &self,
        node: NodeId,
        bounds: Rect,
    ) -> Option<imaginarium_render::kurbo::BezPath> {
        let items = self.items.as_ref()?;
        let camera = self.camera(bounds);
        if let Some(item) = items.by_node.get(&node) {
            let rect = Rect::from_origin_size(Point::ZERO, item.size);
            return Some(
                camera * item.world * imaginarium_render::kurbo::Shape::to_path(&rect, 0.1),
            );
        }
        let rect = hit::node_box(items, self.document.view(), node)?;
        Some(camera * imaginarium_render::kurbo::Shape::to_path(&rect, 0.1))
    }

    /// The hover outline, the selection's outlines and the marquee, on top
    /// of the tiles.
    fn paint_selection(&self, scene: &mut Scene, bounds: Rect) {
        let color = Self::selection_color();
        let selection = self.selection.get_untracked();
        if let Some(hovered) = self.hovered
            && !selection.contains(&hovered)
            && self.press.is_none()
            && let Some(outline) = self.node_outline(hovered, bounds)
        {
            scene.stroke_with(&outline, &Stroke::new(1.5), color.with_alpha(0.6));
        }
        for node in &selection {
            if let Some(outline) = self.node_outline(*node, bounds) {
                scene.stroke_with(&outline, &Stroke::new(1.5), color);
            }
        }
        if let Some(Press::Marquee { start, current, .. }) = self.press {
            let camera = self.camera(bounds);
            let area = Rect::from_points(camera * start, camera * current);
            scene.fill(&area, color.with_alpha(0.08));
            scene.stroke_with(&area, &Stroke::new(1.0), color.with_alpha(0.8));
        }
    }

    /// What the pointer at `screen` (window space) would select.
    fn hit_at(&self, screen: Point) -> Option<NodeId> {
        let items = self.items.as_ref()?;
        let view = self.document.view();
        // A frame's name, drawn on top, selects the frame.
        if let Some((frame, _)) = self
            .name_rects
            .iter()
            .rev()
            .find(|(_, rect)| rect.contains(screen))
        {
            return (!hit::locked(view, *frame))
                .then(|| hit::selectable(view, *frame, self.entered));
        }
        let point = self.to_document(self.bounds, screen);
        let tolerance = hit::TOLERANCE_PIXELS / self.zoom;
        // Items within the tolerance count: a thin line's bounds are
        // narrower than it.
        let near = Rect::from_center_size(point, (2.0 * tolerance, 2.0 * tolerance));
        let mut candidates = self.canvas.items_overlapping(near);
        candidates.reverse();
        hit::hit(items, view, &candidates, point, tolerance, self.entered)
    }

    /// The nodes a marquee from `start` to `end` (document space) selects.
    fn marquee_hits(&self, start: Point, end: Point) -> Vec<NodeId> {
        let Some(items) = &self.items else {
            return Vec::new();
        };
        let area = Rect::from_points(start, end);
        let candidates = self.canvas.items_overlapping(area);
        hit::hit_rect(items, self.document.view(), &candidates, area, self.entered)
    }

    /// The nodes Tab steps through: what a click on each item would
    /// select, in z order.
    fn tab_order(&self) -> Vec<NodeId> {
        let Some(items) = &self.items else {
            return Vec::new();
        };
        let view = self.document.view();
        let mut order: Vec<NodeId> = Vec::new();
        for node in view.walk() {
            if items.by_node.contains_key(&node) && !hit::locked(view, node) {
                let unit = hit::selectable(view, node, self.entered);
                if !order.contains(&unit) {
                    order.push(unit);
                }
            }
        }
        order
    }

    fn pointer(&mut self, cx: &mut Cx<'_>, input: &Input) -> bool {
        match input {
            Input::PointerButton {
                primary: true,
                pressed: true,
                position,
                clicks,
                modifiers,
            } => {
                cx.request_focus();
                cx.capture_pointer();
                let hit = self.hit_at(*position);
                let mut selection = self.selection.get_untracked();
                match hit {
                    // A double-click on a group goes into it.
                    Some(node)
                        if *clicks >= 2
                            && self
                                .document
                                .view()
                                .node(node)
                                .is_some_and(|node| matches!(node.kind(), NodeKind::Group)) =>
                    {
                        self.entered = Some(node);
                        let inner = self.hit_at(*position).unwrap_or(node);
                        self.select(&[inner]);
                        self.press = Some(Press::Node);
                    }
                    Some(node) => {
                        if modifiers.shift {
                            if let Some(at) =
                                selection.iter().position(|selected| *selected == node)
                            {
                                selection.remove(at);
                            } else {
                                selection.push(node);
                            }
                            self.selection.set(selection);
                        } else if !selection.contains(&node) {
                            self.selection.set(vec![node]);
                        }
                        self.press = Some(Press::Node);
                    }
                    None => {
                        // Empty board: out of an entered group, and a marquee.
                        if self.entered.take().is_some() || !modifiers.shift {
                            self.selection.set_if_changed(Vec::new());
                        }
                        let start = self.to_document(cx.bounds(), *position);
                        self.press = Some(Press::Marquee {
                            start,
                            current: start,
                            additive: modifiers.shift,
                        });
                    }
                }
                cx.request_paint();
                true
            }
            Input::PointerMoved(position) => {
                if let Some(Press::Marquee {
                    start, additive, ..
                }) = self.press
                {
                    let current = self.to_document(cx.bounds(), *position);
                    self.press = Some(Press::Marquee {
                        start,
                        current,
                        additive,
                    });
                    cx.request_paint();
                    return true;
                }
                let hovered = self.hit_at(*position);
                if hovered != self.hovered {
                    self.hovered = hovered;
                    cx.request_paint();
                }
                false
            }
            Input::PointerButton {
                primary: true,
                pressed: false,
                ..
            } => {
                cx.release_pointer();
                if let Some(Press::Marquee {
                    start,
                    current,
                    additive,
                }) = self.press.take()
                {
                    let found = self.marquee_hits(start, current);
                    let mut selection = if additive {
                        self.selection.get_untracked()
                    } else {
                        Vec::new()
                    };
                    for node in found {
                        if !selection.contains(&node) {
                            selection.push(node);
                        }
                    }
                    self.selection.set_if_changed(selection);
                }
                self.press = None;
                cx.request_paint();
                true
            }
            Input::PointerLeft => {
                if self.hovered.take().is_some() {
                    cx.request_paint();
                }
                false
            }
            _ => false,
        }
    }

    fn key(&mut self, cx: &mut Cx<'_>, input: &Input) -> bool {
        let Input::Key {
            key,
            pressed: true,
            modifiers,
            ..
        } = input
        else {
            return false;
        };
        match key {
            Key::Escape => {
                let had = !self.selection.get_untracked().is_empty() || self.entered.is_some();
                // Out of an entered group, with the group selected.
                match self.entered.take() {
                    Some(group) => self.select(&[group]),
                    None => {
                        self.selection.set_if_changed(Vec::new());
                    }
                }
                cx.request_paint();
                had
            }
            Key::Tab if !modifiers.command() => {
                let order = self.tab_order();
                if order.is_empty() {
                    return false;
                }
                let current = self.selection.get_untracked().last().copied();
                let at = current.and_then(|node| order.iter().position(|unit| *unit == node));
                let next = match (at, modifiers.shift) {
                    (Some(at), false) => (at + 1) % order.len(),
                    (Some(at), true) => (at + order.len() - 1) % order.len(),
                    (None, false) => 0,
                    (None, true) => order.len() - 1,
                };
                self.selection.set(vec![order[next]]);
                cx.request_paint();
                true
            }
            _ => false,
        }
    }

    fn paint_dots(&self, scene: &mut Scene, bounds: Rect) {
        let mut pitch = DOT_PITCH;
        while pitch * self.zoom < 12.0 {
            pitch *= 4.0;
        }
        let color = theme().color(Token::BoardDot);
        let radius = 1.2;
        let start = self.origin;
        let first = Point::new(
            (start.x / pitch).ceil() * pitch,
            (start.y / pitch).ceil() * pitch,
        );
        let step = pitch * self.zoom;
        let mut y = bounds.y0 + (first.y - start.y) * self.zoom;
        while y < bounds.y1 {
            let mut x = bounds.x0 + (first.x - start.x) * self.zoom;
            while x < bounds.x1 {
                scene.fill(
                    &imaginarium_render::kurbo::Circle::new((x, y), radius),
                    color,
                );
                x += step;
            }
            y += step;
        }
    }

    /// Frame names, above their frames at the interface's size.
    fn paint_names(&mut self, fonts: &mut Fonts, scene: &mut Scene, bounds: Rect) {
        let Some(items) = &self.items else {
            return;
        };
        let camera = self.camera(bounds);
        let color = theme().color(Token::TextMuted);
        let style = TextStyle::new(13.0).weight(500.0).color(color);
        self.name_rects.clear();
        let mut frames: Vec<(u64, NodeId)> = items
            .frames
            .iter()
            .filter_map(|node| Some((items.by_node.get(node)?.z, *node)))
            .collect();
        frames.sort_unstable();
        for (_, node) in &frames {
            let Some(item) = items.by_node.get(node) else {
                continue;
            };
            let Some(name) = &item.name else {
                continue;
            };
            let corner = camera * (item.world * Point::ZERO);
            if !bounds.inflate(0.0, 40.0).contains(corner)
                && !item.bounds.overlaps(self.viewport(bounds).visible())
            {
                continue;
            }
            let fresh = self.names.get(node).is_none_or(|(text, _)| text != name);
            if fresh {
                let block = TextBlock::new(fonts, name, &style, None);
                self.names.insert(*node, (name.clone(), block));
            }
            if let Some((_, block)) = self.names.get(node) {
                let origin = corner - Vec2::new(0.0, block.size().height + NAME_GAP);
                block.paint(scene, origin, true);
                self.name_rects
                    .push((*node, Rect::from_origin_size(origin, block.size())));
            }
        }
    }
}

impl Widget for BoardCanvas {
    fn style(&self) -> taffy::Style {
        taffy::Style {
            flex_grow: 1.0,
            size: taffy::Size {
                width: taffy::prelude::percent(1.0),
                height: taffy::prelude::percent(1.0),
            },
            ..taffy::Style::default()
        }
    }

    fn paint(&mut self, cx: &mut Cx<'_>, scene: &mut Scene) {
        let bounds = cx.bounds();
        self.paint_into(cx.fonts(), scene, bounds, Instant::now());
    }

    fn focusable(&self) -> bool {
        true
    }

    fn event(&mut self, cx: &mut Cx<'_>, input: &Input) -> bool {
        // Input reads the items (hit testing): bring them up to date.
        self.sync(cx.fonts());
        self.bounds = cx.bounds();
        if self.pointer(cx, input) || self.key(cx, input) {
            return true;
        }
        match input {
            Input::Scroll {
                delta,
                position,
                modifiers,
            } => {
                if modifiers.command() {
                    // Ctrl + wheel zooms around the pointer.
                    let factor = (delta.y / 240.0).exp();
                    self.zoom_about(cx.bounds(), factor, *position);
                } else {
                    self.origin -= *delta / self.zoom;
                }
                cx.request_paint();
                true
            }
            _ => false,
        }
    }

    fn wake_at(&self) -> Option<Instant> {
        let canvas = self.canvas.wake_at(&Viewport {
            origin: self.origin,
            zoom: self.zoom,
            screen: Rect::ZERO,
            scale_factor: imaginarium_ui::scale_factor(),
        });
        match (self.owed_at, canvas) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    fn wake(&mut self, cx: &mut Cx<'_>, _now: Instant) {
        self.owed_at = None;
        cx.request_paint();
    }

    fn capture_lost(&mut self, cx: &mut Cx<'_>) {
        if self.press.take().is_some() {
            cx.request_paint();
        }
    }

    fn accessibility(&mut self, cx: &mut Cx<'_>) -> Option<Node> {
        let bounds = cx.bounds();
        let mut board = Node::new(Role::Canvas);
        board.set_label("Board");
        board.set_bounds(access_rect(bounds));
        board.set_clips_children();
        self.access_nodes.clear();
        let Some(items) = &self.items else {
            return Some(board);
        };
        let visible = self.viewport(bounds).visible();
        let mut shown: Vec<(u64, NodeId)> = self
            .canvas
            .items_overlapping(visible)
            .into_iter()
            .filter_map(|id| {
                let node = *items.by_item.get(&id)?;
                Some((items.by_node.get(&node)?.z, node))
            })
            .collect();
        shown.sort_unstable();
        shown.truncate(MAX_ACCESS_ITEMS);
        let base = cx.id().access_base();
        let camera = self.camera(bounds);
        let selection = self.selection.get_untracked();
        let focused = cx.is_focused();
        let view = self.document.view();
        for (_, node_id) in shown {
            let (Some(item), Some(node)) = (items.by_node.get(&node_id), view.node(node_id)) else {
                continue;
            };
            if self.access_nodes.len() as u64 >= ACCESS_LOCAL_IDS {
                break;
            }
            self.access_nodes.push(node_id);
            let id = accesskit::NodeId(base + self.access_nodes.len() as u64);
            let role = match node.kind() {
                NodeKind::Text => Role::Paragraph,
                NodeKind::Frame => Role::Group,
                _ => Role::GraphicsObject,
            };
            let mut access = Node::new(role);
            let text = node
                .text()
                .map(|text| text.to_plain_string())
                .unwrap_or_default();
            let label = node
                .name()
                .map(str::to_owned)
                .or_else(|| (!text.trim().is_empty()).then(|| text.trim().to_owned()))
                .unwrap_or_else(|| describe(node));
            access.set_label(label);
            let rect = Rect::from_origin_size(Point::ZERO, item.size);
            access.set_bounds(access_rect(
                camera.transform_rect_bbox(item.world.transform_rect_bbox(rect)),
            ));
            access.add_action(Action::Focus);
            if selection.contains(&node_id) {
                access.set_selected(true);
            }
            if let Some(tree) = cx.access_tree() {
                tree.push(id, access);
                if focused && selection.last() == Some(&node_id) {
                    tree.set_focus(id);
                }
            }
            cx.add_access_child(id);
        }
        Some(board)
    }

    fn access_action(&mut self, cx: &mut Cx<'_>, request: &ActionRequest) -> bool {
        // Focusing an item's node selects it.
        let local = request.target_node.0.wrapping_sub(cx.id().access_base());
        if request.action != Action::Focus || local == 0 {
            return false;
        }
        let Some(node) = self.access_nodes.get(local as usize - 1).copied() else {
            return false;
        };
        self.select(&[node]);
        cx.request_focus();
        cx.request_paint();
        true
    }
}

/// What a node is, for screen readers when it has no name or text.
fn describe(node: &imaginarium_model::view::NodeView) -> String {
    match node.kind() {
        NodeKind::Shape => match node
            .prop("shape")
            .and_then(imaginarium_model::Value::as_str)
        {
            Some("ellipse") => "Ellipse",
            Some("line") => "Line",
            Some("triangle") => "Triangle",
            Some("diamond") => "Diamond",
            _ => "Rectangle",
        }
        .to_owned(),
        NodeKind::Text => "Text".to_owned(),
        NodeKind::Frame => "Frame".to_owned(),
        other => other.as_str().to_owned(),
    }
}
