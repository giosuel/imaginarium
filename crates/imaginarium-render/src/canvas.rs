// SPDX-License-Identifier: AGPL-3.0-or-later

//! The canvas compositor (`docs/design/rendering.md`): items in document
//! space, rasterized into cached 256-pixel tiles per zoom level, so a frame
//! re-renders only tiles whose items changed and panning re-renders none.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use vello_common::kurbo::{Affine, Point, Rect, Size, Vec2};
use vello_common::peniko::Color;

use crate::scene::{Scene, TextureKey};

/// Tile edge in device pixels (ADR-0003).
pub const TILE: u16 = 256;
const TILE_F: f64 = TILE as f64;
/// The grid index's cell, in document units.
const CELL: f64 = 512.0;
/// A level switch waits for this pause in zooming…
const ZOOM_PAUSE: Duration = Duration::from_millis(150);

/// An item on the canvas, chosen by the caller (a node id).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ItemId(pub u64);

/// What the canvas shows: it paints items into tiles when asked.
pub trait CanvasContent {
    /// Draws item `id` into `scene` (document coordinates; the canvas sets
    /// the transform). `level_scale` is device pixels per document unit of
    /// the tile being drawn, for level of detail. It must not paint outside
    /// the bounds the item was given.
    fn paint_item(&mut self, id: ItemId, scene: &mut Scene, level_scale: f64);
}

/// Which part of the document is shown, and where.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Viewport {
    /// The document point at the top left of `screen`.
    pub origin: Point,
    /// Logical pixels per document unit.
    pub zoom: f64,
    /// Where the canvas is drawn, in the scene's logical pixels.
    pub screen: Rect,
    /// Device pixels per logical pixel (the window's scale factor).
    pub scale_factor: f64,
}

impl Viewport {
    /// Device pixels per document unit.
    pub fn device_scale(&self) -> f64 {
        self.zoom * self.scale_factor
    }

    /// The document rectangle in view.
    pub fn visible(&self) -> Rect {
        Rect::from_origin_size(self.origin, self.screen.size() / self.zoom)
    }

    /// A document point in the scene's logical pixels.
    pub fn to_screen(&self, point: Point) -> Point {
        self.screen.origin() + (point - self.origin) * self.zoom
    }

    /// A point in the scene's logical pixels in document units.
    pub fn to_document(&self, point: Point) -> Point {
        self.origin + (point - self.screen.origin()) / self.zoom
    }
}

/// A tile: level `level`, column `x`, row `y`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TileKey {
    /// The zoom level: device pixels per document unit = 2^(level/2).
    pub level: i32,
    /// The column.
    pub x: i32,
    /// The row.
    pub y: i32,
}

/// Device pixels per document unit at `level`.
pub fn level_scale(level: i32) -> f64 {
    2f64.powf(f64::from(level) / 2.0)
}

/// The smallest level whose scale is at least `scale`: tiles are never
/// magnified and at most √2 too fine.
pub fn level_for(scale: f64) -> i32 {
    (2.0 * scale.max(1e-6).log2() - 1e-9).ceil() as i32
}

impl TileKey {
    /// The tile's document rectangle.
    pub fn bounds(&self) -> Rect {
        let scale = level_scale(self.level);
        Rect::new(
            f64::from(self.x) * TILE_F / scale,
            f64::from(self.y) * TILE_F / scale,
            f64::from(self.x + 1) * TILE_F / scale,
            f64::from(self.y + 1) * TILE_F / scale,
        )
    }
}

/// The tiles of `level` touching document rectangle `area`.
fn tiles_in(level: i32, area: Rect) -> (i32, i32, i32, i32) {
    let scale = level_scale(level) / TILE_F;
    (
        (area.x0 * scale).floor() as i32,
        (area.y0 * scale).floor() as i32,
        (area.x1 * scale).ceil() as i32,
        (area.y1 * scale).ceil() as i32,
    )
}

#[derive(Debug)]
struct Item {
    z: u64,
    bounds: Rect,
}

#[derive(Debug)]
struct Tile {
    texture: TextureKey,
    /// Its items changed since it was rendered.
    stale: bool,
    /// The frame it was last drawn in (for eviction).
    used: u64,
}

/// What a frame did, for the gate report and the debug overlay.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CanvasStats {
    /// The level drawn.
    pub level: i32,
    /// Tiles covering the viewport.
    pub visible: usize,
    /// Tiles rendered this frame (stale or new).
    pub rendered: usize,
    /// Tiles drawn from the cache unchanged.
    pub reused: usize,
    /// Visible tiles shown by a stand-in (a stale copy or another level),
    /// to be rendered in a following frame.
    pub owed: usize,
    /// Items painted into tiles this frame.
    pub items_painted: usize,
    /// Tiles in the cache.
    pub cached: usize,
    /// Tiles evicted this frame.
    pub evicted: usize,
    /// Building the tiles' scenes (painting items into them).
    pub build: Duration,
}

/// Items on an infinite canvas, drawn through a tile cache.
#[derive(Debug)]
pub struct Canvas {
    items: HashMap<ItemId, Item>,
    grid: HashMap<(i32, i32), Vec<ItemId>>,
    tiles: HashMap<TileKey, Tile>,
    next_texture: u64,
    frame: u64,
    /// Tiles rendered per frame at most; the rest are owed.
    budget: usize,
    /// Tiles kept at most (256 KiB each).
    capacity: usize,
    level: Option<i32>,
    last_scale: f64,
    scale_changed: Option<Instant>,
    /// Tile renders of the last frame, for the overlay.
    last_rendered: Vec<TileKey>,
    /// Tint tiles that were rendered this frame (shows invalidation).
    pub show_updates: bool,
}

impl Default for Canvas {
    fn default() -> Self {
        Self::new()
    }
}

impl Canvas {
    /// An empty canvas, with a budget of 24 tile renders per frame and
    /// 1,000 cached tiles (256 MB).
    pub fn new() -> Self {
        Self {
            items: HashMap::new(),
            grid: HashMap::new(),
            tiles: HashMap::new(),
            next_texture: 1 << 40,
            frame: 0,
            budget: 24,
            capacity: 1_000,
            level: None,
            last_scale: 0.0,
            scale_changed: None,
            last_rendered: Vec::new(),
            show_updates: false,
        }
    }

    /// Sets the tile renders per frame and the cache size, in tiles.
    pub fn with_limits(mut self, renders_per_frame: usize, cached_tiles: usize) -> Self {
        self.budget = renders_per_frame.max(1);
        self.capacity = cached_tiles.max(1);
        self
    }

    /// The number of items.
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Whether there are no items.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    fn cells(bounds: Rect) -> impl Iterator<Item = (i32, i32)> {
        let x0 = (bounds.x0 / CELL).floor() as i32;
        let y0 = (bounds.y0 / CELL).floor() as i32;
        let x1 = (bounds.x1 / CELL).floor() as i32;
        let y1 = (bounds.y1 / CELL).floor() as i32;
        (y0..=y1).flat_map(move |y| (x0..=x1).map(move |x| (x, y)))
    }

    /// Adds or changes an item: its stacking order `z` (higher on top) and
    /// the document rectangle it paints in. Its old and new areas are
    /// redrawn.
    pub fn upsert(&mut self, id: ItemId, z: u64, bounds: Rect) {
        if let Some(old) = self.items.get(&id) {
            let old_bounds = old.bounds;
            for cell in Self::cells(old_bounds) {
                if let Some(ids) = self.grid.get_mut(&cell) {
                    ids.retain(|&other| other != id);
                }
            }
            self.invalidate(old_bounds);
        }
        for cell in Self::cells(bounds) {
            self.grid.entry(cell).or_default().push(id);
        }
        self.items.insert(id, Item { z, bounds });
        self.invalidate(bounds);
    }

    /// Removes an item; its area is redrawn.
    pub fn remove(&mut self, id: ItemId) {
        if let Some(item) = self.items.remove(&id) {
            for cell in Self::cells(item.bounds) {
                if let Some(ids) = self.grid.get_mut(&cell) {
                    ids.retain(|&other| other != id);
                }
            }
            self.invalidate(item.bounds);
        }
    }

    /// An item's bounds.
    pub fn bounds(&self, id: ItemId) -> Option<Rect> {
        self.items.get(&id).map(|item| item.bounds)
    }

    /// The topmost item whose bounds contain `point` (document units).
    pub fn item_at(&self, point: Point) -> Option<ItemId> {
        let cell = (
            (point.x / CELL).floor() as i32,
            (point.y / CELL).floor() as i32,
        );
        self.grid
            .get(&cell)?
            .iter()
            .filter_map(|id| Some((*id, self.items.get(id)?)))
            .filter(|(_, item)| item.bounds.contains(point))
            .max_by_key(|(id, item)| (item.z, *id))
            .map(|(id, _)| id)
    }

    /// Marks the tiles over `area` (document units) for re-rendering:
    /// content there changed without an item changing (a style change).
    pub fn invalidate(&mut self, area: Rect) {
        // One device pixel more, for antialiasing, at each level.
        for (key, tile) in &mut self.tiles {
            let grow = 1.0 / level_scale(key.level);
            if key.bounds().intersect(area.inflate(grow, grow)).area() > 0.0
                || area.inflate(grow, grow).contains(key.bounds().origin())
            {
                tile.stale = true;
            }
        }
    }

    /// Marks every tile for re-rendering.
    pub fn invalidate_all(&mut self) {
        for tile in self.tiles.values_mut() {
            tile.stale = true;
        }
    }

    /// The items touching `area`, bottom first.
    fn items_in(&self, area: Rect) -> Vec<ItemId> {
        let mut seen = HashSet::new();
        let mut found: Vec<(u64, ItemId)> = Vec::new();
        for cell in Self::cells(area) {
            for id in self.grid.get(&cell).into_iter().flatten() {
                if seen.insert(*id)
                    && let Some(item) = self.items.get(id)
                    && item.bounds.intersect(area).area() > 0.0
                {
                    found.push((item.z, *id));
                }
            }
        }
        found.sort_unstable();
        found.into_iter().map(|(_, id)| id).collect()
    }

    fn choose_level(&mut self, scale: f64, now: Instant) -> i32 {
        if (scale - self.last_scale).abs() > 1e-9 {
            self.last_scale = scale;
            self.scale_changed = Some(now);
        }
        let target = level_for(scale);
        let level = match self.level {
            None => target,
            Some(current) if current == target => current,
            Some(current) => {
                let current_scale = level_scale(current);
                // Keep drawing the current level (scaled) during a zoom
                // gesture, unless it would be magnified or far too fine.
                let out_of_range = scale > current_scale * 1.0001 || scale < current_scale / 2.0;
                let paused = self
                    .scale_changed
                    .is_none_or(|changed| now.duration_since(changed) >= ZOOM_PAUSE);
                if out_of_range || paused {
                    target
                } else {
                    current
                }
            }
        };
        self.level = Some(level);
        level
    }

    /// When the canvas wants a frame without input: the end of a zoom
    /// pause (to switch levels).
    pub fn wake_at(&self, viewport: &Viewport) -> Option<Instant> {
        let level = self.level?;
        (level != level_for(viewport.device_scale()))
            .then(|| self.scale_changed.map(|changed| changed + ZOOM_PAUSE))
            .flatten()
    }

    /// Draws the canvas for `viewport` into `scene`: cached tiles, tiles
    /// rendered now (at most the budget), and stand-ins for the rest. If
    /// [`CanvasStats::owed`] isn't zero, draw another frame soon.
    pub fn frame(
        &mut self,
        scene: &mut Scene,
        viewport: &Viewport,
        now: Instant,
        content: &mut dyn CanvasContent,
    ) -> CanvasStats {
        self.frame += 1;
        let scale = viewport.device_scale();
        let level = self.choose_level(scale, now);
        let level_scale = level_scale(level);
        let visible = viewport.visible();
        let (x0, y0, x1, y1) = tiles_in(level, visible);
        let center = visible.center();
        let mut keys: Vec<TileKey> = (y0..y1)
            .flat_map(|y| (x0..x1).map(move |x| TileKey { level, x, y }))
            .collect();
        // Tiles nearest the middle first, when the budget runs out.
        keys.sort_by(|a, b| {
            let da = (a.bounds().center() - center).hypot2();
            let db = (b.bounds().center() - center).hypot2();
            da.total_cmp(&db)
        });
        let mut stats = CanvasStats {
            level,
            visible: keys.len(),
            ..CanvasStats::default()
        };
        self.last_rendered.clear();
        let build_start = Instant::now();
        let mut draws: Vec<(TileKey, TextureKey)> = Vec::with_capacity(keys.len());
        let mut owed: Vec<TileKey> = Vec::new();
        for key in keys {
            let valid = self.tiles.get(&key).is_some_and(|tile| !tile.stale);
            if valid {
                if let Some(tile) = self.tiles.get_mut(&key) {
                    tile.used = self.frame;
                    draws.push((key, tile.texture));
                    stats.reused += 1;
                }
                continue;
            }
            if stats.rendered >= self.budget {
                owed.push(key);
                continue;
            }
            let texture = match self.tiles.get(&key) {
                Some(tile) => tile.texture,
                None => {
                    self.next_texture += 1;
                    TextureKey(self.next_texture)
                }
            };
            let mut tile_scene = Scene::new();
            // Document → level pixels → this tile's pixels.
            let to_tile = Affine::translate(Vec2::new(
                -f64::from(key.x) * TILE_F,
                -f64::from(key.y) * TILE_F,
            )) * Affine::scale(level_scale);
            for id in self.items_in(key.bounds()) {
                let mut item_scene = Scene::new();
                content.paint_item(id, &mut item_scene, level_scale);
                tile_scene.append(&item_scene, to_tile);
                stats.items_painted += 1;
            }
            scene.render_texture(texture, TILE, TILE, tile_scene);
            self.tiles.insert(
                key,
                Tile {
                    texture,
                    stale: false,
                    used: self.frame,
                },
            );
            draws.push((key, texture));
            self.last_rendered.push(key);
            stats.rendered += 1;
        }
        stats.build = build_start.elapsed();
        stats.owed = owed.len();

        // Draw: device-pixel destinations, snapped, then in logical pixels.
        let to_device = |doc: Point| {
            let screen = viewport.to_screen(doc);
            Point::new(
                screen.x * viewport.scale_factor,
                screen.y * viewport.scale_factor,
            )
        };
        let dest = |key: &TileKey| {
            let bounds = key.bounds();
            let (a, b) = (
                to_device(bounds.origin()),
                to_device(Point::new(bounds.x1, bounds.y1)),
            );
            let snapped = Rect::new(a.x.round(), a.y.round(), b.x.round(), b.y.round());
            Rect::new(
                snapped.x0 / viewport.scale_factor,
                snapped.y0 / viewport.scale_factor,
                snapped.x1 / viewport.scale_factor,
                snapped.y1 / viewport.scale_factor,
            )
        };
        scene.push_clip(&viewport.screen);
        // Stand-ins first: a stale copy, or the nearest coarser level.
        for key in &owed {
            if let Some(tile) = self.tiles.get(key) {
                scene.draw_texture(tile.texture, TILE, TILE, dest(key));
                continue;
            }
            for coarser in (level - 6..level).rev() {
                let (cx, cy, _, _) = tiles_in(coarser, key.bounds().inset(-1e-6));
                let parent = TileKey {
                    level: coarser,
                    x: cx,
                    y: cy,
                };
                if let Some(tile) = self.tiles.get(&parent) {
                    scene.push_clip(&dest(key));
                    scene.draw_texture(tile.texture, TILE, TILE, dest(&parent));
                    scene.pop();
                    break;
                }
            }
        }
        for (key, texture) in &draws {
            scene.draw_texture(*texture, TILE, TILE, dest(key));
        }
        if self.show_updates {
            for key in &self.last_rendered {
                let area = dest(key);
                scene.fill(&area, Color::from_rgba8(255, 40, 40, 40));
                scene.stroke(&area.inset(-0.5), 1.0, Color::from_rgba8(220, 30, 30, 160));
            }
        }
        scene.pop();

        stats.evicted = self.evict(scene);
        stats.cached = self.tiles.len();
        stats
    }

    /// Draws the visible items directly, without tiles: every item is
    /// painted again each frame (for comparing with [`Canvas::frame`]).
    pub fn draw_direct(
        &mut self,
        scene: &mut Scene,
        viewport: &Viewport,
        content: &mut dyn CanvasContent,
    ) -> CanvasStats {
        let scale = viewport.device_scale();
        let camera = Affine::translate(viewport.screen.origin().to_vec2())
            * Affine::scale(viewport.zoom)
            * Affine::translate(-viewport.origin.to_vec2());
        let build = Instant::now();
        let mut stats = CanvasStats {
            level: level_for(scale),
            ..CanvasStats::default()
        };
        scene.push_clip(&viewport.screen);
        for id in self.items_in(viewport.visible()) {
            let mut item = Scene::new();
            content.paint_item(id, &mut item, scale);
            scene.append(&item, camera);
            stats.items_painted += 1;
        }
        scene.set_transform(Affine::IDENTITY);
        scene.pop();
        stats.build = build.elapsed();
        stats
    }

    /// Evicts the least recently drawn tiles over capacity (never one drawn
    /// this frame).
    fn evict(&mut self, scene: &mut Scene) -> usize {
        if self.tiles.len() <= self.capacity {
            return 0;
        }
        let mut old: Vec<(u64, TileKey)> = self
            .tiles
            .iter()
            .filter(|(_, tile)| tile.used < self.frame)
            .map(|(key, tile)| (tile.used, *key))
            .collect();
        old.sort_unstable();
        let excess = self.tiles.len() - self.capacity;
        let mut evicted = 0;
        for (_, key) in old.into_iter().take(excess) {
            if let Some(tile) = self.tiles.remove(&key) {
                scene.free_texture(tile.texture);
                evicted += 1;
            }
        }
        evicted
    }

    /// The size of a tile in document units at the current level.
    pub fn tile_size(&self) -> Option<Size> {
        let scale = level_scale(self.level?);
        Some(Size::new(TILE_F / scale, TILE_F / scale))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Content that paints each item's bounds and counts paints.
    struct Boxes {
        bounds: HashMap<ItemId, Rect>,
        painted: Vec<ItemId>,
    }

    impl CanvasContent for Boxes {
        fn paint_item(&mut self, id: ItemId, scene: &mut Scene, _level_scale: f64) {
            self.painted.push(id);
            if let Some(bounds) = self.bounds.get(&id) {
                scene.fill(bounds, Color::BLACK);
            }
        }
    }

    fn board(count: u64) -> (Canvas, Boxes) {
        let mut canvas = Canvas::new().with_limits(1_000, 10_000);
        let mut boxes = Boxes {
            bounds: HashMap::new(),
            painted: Vec::new(),
        };
        for i in 0..count {
            let x = (i % 100) as f64 * 120.0;
            let y = (i / 100) as f64 * 90.0;
            let bounds = Rect::new(x, y, x + 100.0, y + 70.0);
            canvas.upsert(ItemId(i), i, bounds);
            boxes.bounds.insert(ItemId(i), bounds);
        }
        (canvas, boxes)
    }

    fn viewport(origin: Point, zoom: f64) -> Viewport {
        Viewport {
            origin,
            zoom,
            screen: Rect::new(0.0, 0.0, 1280.0, 800.0),
            scale_factor: 1.0,
        }
    }

    #[test]
    fn levels_are_half_octaves_and_never_magnify() {
        assert_eq!(level_for(1.0), 0);
        assert_eq!(level_for(1.2), 1);
        assert_eq!(level_for(2.0), 2);
        assert_eq!(level_for(0.5), -2);
        for scale in [0.07, 0.3, 0.71, 1.0, 1.41, 1.5, 3.3, 8.0] {
            let level = level_scale(level_for(scale));
            assert!(
                level >= scale - 1e-9 && level < scale * 2f64.sqrt() + 1e-9,
                "{scale}"
            );
        }
    }

    #[test]
    fn a_second_frame_reuses_every_tile_and_panning_renders_only_new_ones() {
        let (mut canvas, mut boxes) = board(2_000);
        let now = Instant::now();
        let view = viewport(Point::ZERO, 1.0);
        let first = canvas.frame(&mut Scene::new(), &view, now, &mut boxes);
        assert_eq!(first.rendered, first.visible);
        assert!(first.visible >= 5 * 4);
        let second = canvas.frame(&mut Scene::new(), &view, now, &mut boxes);
        assert_eq!(second.rendered, 0);
        assert_eq!(second.reused, second.visible);
        // Pan by half a tile: only the newly exposed column and row render.
        let panned = viewport(Point::new(128.0, 0.0), 1.0);
        let pan = canvas.frame(&mut Scene::new(), &panned, now, &mut boxes);
        assert!(pan.rendered > 0 && pan.rendered <= 5, "{pan:?}");
    }

    #[test]
    fn moving_an_item_rerenders_only_the_tiles_it_touches() {
        let (mut canvas, mut boxes) = board(2_000);
        let now = Instant::now();
        let view = viewport(Point::ZERO, 1.0);
        canvas.frame(&mut Scene::new(), &view, now, &mut boxes);
        // Drag item 0 by 10 units: old and new bounds are in tile (0, 0).
        let moved = Rect::new(10.0, 10.0, 110.0, 80.0);
        canvas.upsert(ItemId(0), 0, moved);
        boxes.bounds.insert(ItemId(0), moved);
        boxes.painted.clear();
        let after = canvas.frame(&mut Scene::new(), &view, now, &mut boxes);
        assert_eq!(after.rendered, 1, "{after:?}");
        assert!(boxes.painted.contains(&ItemId(0)));
        // Across a tile edge (x = 256): both tiles render.
        let across = Rect::new(200.0, 10.0, 300.0, 80.0);
        canvas.upsert(ItemId(0), 0, across);
        boxes.bounds.insert(ItemId(0), across);
        let after = canvas.frame(&mut Scene::new(), &view, now, &mut boxes);
        assert_eq!(after.rendered, 2, "{after:?}");
    }

    #[test]
    fn the_budget_owes_tiles_and_stand_ins_fill_them() {
        let mut canvas = Canvas::new().with_limits(4, 1_000);
        let (_, mut boxes) = board(2_000);
        for (id, bounds) in &boxes.bounds {
            canvas.upsert(*id, id.0, *bounds);
        }
        let now = Instant::now();
        let view = viewport(Point::ZERO, 1.0);
        let first = canvas.frame(&mut Scene::new(), &view, now, &mut boxes);
        assert_eq!(first.rendered, 4);
        assert_eq!(first.owed, first.visible - 4);
        let mut frames = 1;
        loop {
            let stats = canvas.frame(&mut Scene::new(), &view, now, &mut boxes);
            frames += 1;
            if stats.owed == 0 {
                break;
            }
            assert!(frames < 100);
        }
        assert_eq!(frames, first.visible.div_ceil(4));
    }

    #[test]
    fn zooming_keeps_the_level_until_the_gesture_pauses() {
        let (mut canvas, mut boxes) = board(500);
        let start = Instant::now();
        let view = viewport(Point::ZERO, 1.0);
        assert_eq!(
            canvas
                .frame(&mut Scene::new(), &view, start, &mut boxes)
                .level,
            0
        );
        // Zooming out to 0.6 (level -1): level 0 is still fine, scaled down.
        let out = viewport(Point::ZERO, 0.6);
        let during = canvas.frame(
            &mut Scene::new(),
            &out,
            start + Duration::from_millis(20),
            &mut boxes,
        );
        assert_eq!(during.level, 0);
        assert!(canvas.wake_at(&out).is_some(), "a frame after the pause");
        let after = canvas.frame(
            &mut Scene::new(),
            &out,
            start + Duration::from_millis(400),
            &mut boxes,
        );
        assert_eq!(after.level, level_for(0.6));
        assert_eq!(after.level, -1);
        // Zooming in past the level: switch at once (never magnify).
        let near = viewport(Point::ZERO, 1.3);
        let zoomed = canvas.frame(
            &mut Scene::new(),
            &near,
            start + Duration::from_millis(410),
            &mut boxes,
        );
        assert_eq!(zoomed.level, level_for(1.3));
    }

    #[test]
    fn the_cache_evicts_the_least_recently_drawn_tiles() {
        let mut canvas = Canvas::new().with_limits(1_000, 40);
        let (_, mut boxes) = board(2_000);
        for (id, bounds) in &boxes.bounds {
            canvas.upsert(*id, id.0, *bounds);
        }
        let now = Instant::now();
        let mut scene = Scene::new();
        canvas.frame(&mut scene, &viewport(Point::ZERO, 1.0), now, &mut boxes);
        let far = canvas.frame(
            &mut Scene::new(),
            &viewport(Point::new(5_000.0, 3_000.0), 1.0),
            now,
            &mut boxes,
        );
        assert!(far.cached <= 40 || far.evicted == 0, "{far:?}");
        assert!(far.evicted > 0);
    }

    #[test]
    fn item_at_finds_the_topmost() {
        let mut canvas = Canvas::new();
        canvas.upsert(ItemId(1), 1, Rect::new(0.0, 0.0, 100.0, 100.0));
        canvas.upsert(ItemId(2), 5, Rect::new(50.0, 50.0, 150.0, 150.0));
        assert_eq!(canvas.item_at(Point::new(60.0, 60.0)), Some(ItemId(2)));
        assert_eq!(canvas.item_at(Point::new(10.0, 10.0)), Some(ItemId(1)));
        assert_eq!(canvas.item_at(Point::new(500.0, 10.0)), None);
    }
}
