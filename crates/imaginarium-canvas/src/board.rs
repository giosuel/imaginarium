// SPDX-License-Identifier: AGPL-3.0-or-later

//! The board widget: a document's nodes on the tile canvas, with the
//! board's background, panning and zooming, and what is drawn on top of
//! the tiles (frame names).

use std::collections::HashMap;
use std::rc::Rc;
use std::time::Instant;

use imaginarium_model::{Actor, ChangeSet, Document, NodeId, Result, Transaction, UndoScope};
use imaginarium_render::canvas::{Canvas, CanvasStats, Viewport};
use imaginarium_render::kurbo::{Affine, Point, Rect, Vec2};
use imaginarium_render::{Scene, TileWorkers};
use imaginarium_text::{Fonts, TextBlock, TextStyle};
use imaginarium_ui::{Cx, Input, Theme, Token, Widget, theme};

use crate::items::{BoardColors, Items};
use crate::paint::ItemPainter;

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
        }
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
        scene.pop();
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
        for (node, item) in &items.by_node {
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

    fn event(&mut self, cx: &mut Cx<'_>, input: &Input) -> bool {
        // Input reads the items (hit testing): bring them up to date.
        self.sync(cx.fonts());
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
}
