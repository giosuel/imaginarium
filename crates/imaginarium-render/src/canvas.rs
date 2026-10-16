// SPDX-License-Identifier: AGPL-3.0-or-later

//! The canvas compositor (`docs/design/rendering.md`): items in document
//! space, rasterized into cached 256-pixel tiles per zoom level, so a frame
//! re-renders only tiles whose items changed and panning re-renders none.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use vello_common::kurbo::{Affine, Point, Rect, Size, Vec2};
use vello_common::peniko::Color;

use crate::scene::{Scene, TextureKey};
use crate::tile_workers::{Done, Job, TileWorkers};

/// Tile edge in device pixels (ADR-0003).
pub const TILE: u16 = 256;
const TILE_F: f64 = TILE as f64;
/// The grid index's cell, in document units.
const CELL: f64 = 512.0;
/// A level switch waits for this pause in zooming…
const ZOOM_PAUSE: Duration = Duration::from_millis(150);

/// Time per frame for tile renders (`rendering.md` §6) at most: with the
/// rest of a small window's frame (about 4 ms on the gate laptop), a frame
/// stays well within 16 ms.
const RENDER_BUDGET: Duration = Duration::from_millis(6);

/// What a whole frame on the UI thread may take, once the rest of the frame
/// is known ([`Canvas::note_rest_of_frame`]): tile renders get what the rest
/// leaves of it. Under a 60 Hz refresh, with room for misestimates.
const FRAME_TARGET: Duration = Duration::from_millis(12);

/// Tile renders' time per frame at least, however long the rest takes.
const MIN_RENDER_BUDGET: Duration = Duration::from_millis(1);

/// A tile's fixed cost (a GPU submission, the renderer's setup), in items:
/// on the gate laptop a tile costs about as much as 50 items in it, and an
/// item about 12 µs, in panning and zooming alike.
const TILE_UNITS: usize = 50;

/// A tile's fixed cost on the UI thread when workers render it: building
/// its scene and handing it over, then uploading the pixels.
const WORKER_TILE_UNITS: usize = 8;

/// Tiles the workers may have at once, per thread: enough to keep them
/// busy until the next frame, few enough that the queue follows what is
/// on screen (8 shapes tiles are about 3 ms of a thread's work).
const FLIGHTS_PER_THREAD: usize = 8;

/// With workers, how long a frame waits at most for the tiles it handed
/// over that nothing can stand in for: a worker renders one in about a
/// millisecond, so they show in this frame instead of blank until the
/// next. Within [`FRAME_TARGET`].
const BLANK_WAIT: Duration = Duration::from_millis(4);

/// The time per unit before any tile work was measured.
const INITIAL_UNIT_COST: Duration = Duration::from_micros(12);

/// With workers: tiles uploaded per frame at most, whatever their measured
/// cost says (4 MiB of pixels). A burst costs more per tile than the
/// average: in the gate's run 3 on an Intel GPU, a frame's 57 uploads took
/// 0.23 ms each, other frames' 0.08 ms.
const MAX_UPLOADS: usize = 16;

/// With workers: the time to upload a tile before any upload was measured.
const INITIAL_UPLOAD_COST: Duration = Duration::from_micros(100);

/// A time sample counts as this many times the current estimate at most:
/// one frame that waited for the driver (or for a tile page) must not
/// stall the tile work of the frames after it.
const MAX_SAMPLE_RATIO: u32 = 4;

/// An item covering more grid cells than this is kept in a list of its
/// own instead of the grid (a huge frame, or bounds from a bug).
const MAX_ITEM_CELLS: i64 = 4_096;

/// Levels further from 0 aren't drawn: 2^±32 device pixels per document
/// unit, far beyond any zoom (and where tile coordinates would overflow).
const MAX_LEVEL: i32 = 64;

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
/// magnified and at most √2 too fine (within ±64).
pub fn level_for(scale: f64) -> i32 {
    ((2.0 * scale.max(1e-6).log2() - 1e-9).ceil() as i32).clamp(-MAX_LEVEL, MAX_LEVEL)
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
    /// `None`: nothing is in it, so there is nothing to draw.
    texture: Option<TextureKey>,
    /// Its items changed since it was rendered.
    stale: bool,
    /// The frame it was last drawn in (for eviction).
    used: u64,
}

/// What shows in place of a tile that isn't rendered yet.
#[derive(Debug)]
enum StandIn {
    /// Its stale copy.
    Stale,
    /// A coarser level's tiles over it, all of it.
    Coarser(Vec<TileKey>),
    /// A finer level's tiles in it, all of it.
    Finer(Vec<TileKey>),
    /// Another level's tiles over part of it: the rest is blank.
    Partial(Vec<TileKey>),
}

impl StandIn {
    /// How much it is worth rendering the tile now instead (lower first;
    /// 0 is a tile with no stand-in, or one covering only part of it).
    fn rank(&self) -> u8 {
        match self {
            Self::Partial(_) => 0,
            Self::Stale => 1,
            Self::Finer(_) => 2,
            Self::Coarser(_) => 3,
        }
    }

    /// The tiles it draws for `key`.
    fn tiles(self, key: TileKey) -> Vec<TileKey> {
        match self {
            Self::Stale => vec![key],
            Self::Coarser(tiles) | Self::Finer(tiles) | Self::Partial(tiles) => tiles,
        }
    }
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
    /// Of `owed`: tiles with no stand-in, or one covering only part of
    /// them, left blank (in part) this frame.
    pub blank: usize,
    /// Items painted into tiles this frame.
    pub items_painted: usize,
    /// Tiles in the cache.
    pub cached: usize,
    /// Tiles evicted this frame.
    pub evicted: usize,
    /// Building the tiles' scenes (painting items into them).
    pub build: Duration,
    /// With workers: tiles that came back and are uploaded with this frame.
    pub arrived: usize,
    /// With workers: tiles still with them after this frame, or back and
    /// waiting to be uploaded (some of `owed`).
    pub in_flight: usize,
    /// With workers: tiles back from them that wait for a later frame's
    /// uploads (some of `in_flight`).
    pub parked: usize,
    /// With workers: their time for the tiles that `arrived`.
    pub worker_time: Duration,
    /// With workers: tiles they failed to render, left empty.
    pub failed: usize,
    /// With workers: waiting for tiles nothing stood in for (part of the
    /// frame, like `build`).
    pub waited: Duration,
}

/// The tile workers a canvas hands its tiles to, and the tiles they have.
#[derive(Debug)]
struct Workers {
    pool: TileWorkers,
    reply: mpsc::Sender<Done>,
    done: mpsc::Receiver<Done>,
    /// Tiles being rendered, or back and waiting in `parked`.
    flights: HashMap<TileKey, Flight>,
    /// Tiles back from the workers that a frame had no time to upload, in
    /// the order they came.
    parked: Vec<Done>,
    next_tag: u64,
}

/// A tile being rendered by a worker.
#[derive(Debug)]
struct Flight {
    tag: u64,
    /// Its items changed since it was handed over: it comes back stale.
    stale: bool,
}

impl Workers {
    /// Keeps a tile that came back for a frame to upload, unless it is an
    /// earlier render's or no longer wanted.
    fn park(&mut self, done: Done) {
        if self
            .flights
            .get(&done.key)
            .is_some_and(|flight| flight.tag == done.tag)
        {
            self.parked.push(done);
        }
    }
}

/// Items on an infinite canvas, drawn through a tile cache.
#[derive(Debug)]
pub struct Canvas {
    items: HashMap<ItemId, Item>,
    grid: HashMap<(i32, i32), Vec<ItemId>>,
    /// Items too big for the grid, checked everywhere.
    large: Vec<ItemId>,
    tiles: HashMap<TileKey, Tile>,
    /// Cached tiles per level, so an invalidation looks up the tiles over
    /// its area level by level instead of going through all of them.
    levels: BTreeMap<i32, usize>,
    frame: u64,
    /// Tile renders per frame at most; the rest are owed. A cap: the time
    /// budget usually decides first.
    budget: usize,
    /// Time per frame for tile renders: painting their items and the
    /// renderer's work on them, as estimated from earlier frames.
    render_budget: Duration,
    /// The estimated time per unit of tile work (an item painted, or a
    /// tile's fixed cost of [`TILE_UNITS`]), from [`Canvas::note_render_time`].
    unit_cost: Duration,
    /// The last frame's tile work, until the renderer's time for it comes:
    /// building time and units.
    pending: Option<(Duration, usize)>,
    /// With workers: the estimated time to upload a tile, from
    /// [`Canvas::note_render_time`].
    upload_cost: Duration,
    /// With workers: the tiles the last frame uploaded, until the
    /// renderer's time for them comes.
    last_uploads: usize,
    /// The last frame's time apart from tile work, if told.
    rest: Option<Duration>,
    /// The last frame's own tile work on the UI thread (building and
    /// waiting), which [`Canvas::note_rest_of_frame`]'s time includes.
    last_tile_work: Duration,
    /// How long a frame waits at most for tiles nothing stands in for.
    blank_wait: Duration,
    /// A tile's fixed cost, in units.
    tile_units: usize,
    /// Renders tiles off the UI thread, if set.
    workers: Option<Workers>,
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
    /// An empty canvas: tile renders take at most 6 ms per frame (and at
    /// most 64 tiles), 1,000 tiles are cached (256 MB).
    pub fn new() -> Self {
        Self {
            items: HashMap::new(),
            grid: HashMap::new(),
            large: Vec::new(),
            tiles: HashMap::new(),
            levels: BTreeMap::new(),
            frame: 0,
            budget: 64,
            render_budget: RENDER_BUDGET,
            unit_cost: INITIAL_UNIT_COST,
            pending: None,
            upload_cost: INITIAL_UPLOAD_COST,
            last_uploads: 0,
            rest: None,
            last_tile_work: Duration::ZERO,
            blank_wait: BLANK_WAIT,
            tile_units: TILE_UNITS,
            workers: None,
            capacity: 1_000,
            level: None,
            last_scale: 0.0,
            scale_changed: None,
            last_rendered: Vec::new(),
            show_updates: false,
        }
    }

    /// Sets the tile renders per frame at most and the cache size, in
    /// tiles.
    pub fn with_limits(mut self, renders_per_frame: usize, cached_tiles: usize) -> Self {
        self.budget = renders_per_frame.max(1);
        self.capacity = cached_tiles.max(1);
        self
    }

    /// Renders tiles on `workers` instead of in the frame: a frame paints a
    /// tile's items and hands it over, and shows stand-ins until it comes
    /// back (usually by the next frame) to be uploaded. Rendering tiles
    /// then takes nothing from the frame; uploading them takes the frame's
    /// tile time (half of it before building tiles, the rest after) and 16
    /// tiles at most, and the others wait for the next.
    /// Without workers (none could start), tiles render in the frame.
    pub fn with_workers(mut self, workers: TileWorkers) -> Self {
        if workers.threads() > 0 {
            let (reply, done) = mpsc::channel();
            self.workers = Some(Workers {
                pool: workers,
                reply,
                done,
                flights: HashMap::new(),
                parked: Vec::new(),
                next_tag: 0,
            });
            self.tile_units = WORKER_TILE_UNITS;
        }
        self
    }

    /// Sets how long a frame waits at most for the tiles it hands to the
    /// workers that nothing can stand in for (default 4 ms, and never past
    /// what the rest of the frame leaves of 12 ms). Zero shows them blank
    /// until a later frame.
    pub fn with_blank_wait(mut self, wait: Duration) -> Self {
        self.blank_wait = wait;
        self
    }

    /// Tiles the workers are rendering, or rendered and a frame is yet to
    /// upload.
    pub fn in_flight(&self) -> usize {
        self.workers
            .as_ref()
            .map_or(0, |workers| workers.flights.len())
    }

    /// Sets the time per frame for tile renders (default 6 ms). At least
    /// one tile renders per frame, whatever it costs.
    pub fn with_render_budget(mut self, budget: Duration) -> Self {
        self.render_budget = budget;
        self
    }

    /// Tells the canvas how long the renderer took for the tile renders of
    /// the frame it drew last ([`crate::FrameTiming::textures`]; with
    /// workers, uploading the tiles that came back), so it can keep later
    /// frames' tile work within the time budget. Without it, the canvas
    /// assumes a mid-range laptop.
    pub fn note_render_time(&mut self, renderer: Duration) {
        let uploads = std::mem::take(&mut self.last_uploads);
        if self.workers.is_some() && uploads > 0 {
            // The renderer's time includes making tile pages and driver
            // stalls: one slow frame must not stop the uploads of the next.
            let sample = (renderer / uploads as u32).min(self.upload_cost * MAX_SAMPLE_RATIO);
            self.upload_cost = (self.upload_cost + sample) / 2;
        }
        let Some((build, units)) = self.pending.take() else {
            return;
        };
        if units == 0 {
            return;
        }
        // With workers the renderer's time is the uploads, measured apart.
        let work = if self.workers.is_some() {
            build
        } else {
            build + renderer
        };
        let sample = (work / units as u32).min(self.unit_cost * MAX_SAMPLE_RATIO);
        // Halfway to the new sample: quick to follow a change of content,
        // steady across frames of similar tiles.
        self.unit_cost = (self.unit_cost + sample) / 2;
    }

    /// Tells the canvas how long the frame it drew last took on the UI
    /// thread (painting, encoding and submitting it), so that later
    /// frames' tile work fits in what the rest of a frame leaves: a big
    /// window's frame leaves less. The canvas's own share of it (painting
    /// tiles' items, waiting for workers) doesn't count.
    pub fn note_rest_of_frame(&mut self, frame: Duration) {
        self.rest = Some(frame.saturating_sub(self.last_tile_work));
    }

    /// The time for this frame's tile renders.
    fn frame_render_budget(&self) -> Duration {
        match self.rest {
            Some(rest) => self
                .render_budget
                .min(FRAME_TARGET.saturating_sub(rest).max(MIN_RENDER_BUDGET)),
            None => self.render_budget,
        }
    }

    /// The number of items.
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Whether there are no items.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// The grid cells `bounds` touches, as a range: (x0, y0, x1, y1),
    /// inclusive.
    fn cell_range(bounds: Rect) -> (i32, i32, i32, i32) {
        (
            (bounds.x0 / CELL).floor() as i32,
            (bounds.y0 / CELL).floor() as i32,
            (bounds.x1 / CELL).floor() as i32,
            (bounds.y1 / CELL).floor() as i32,
        )
    }

    fn cell_count((x0, y0, x1, y1): (i32, i32, i32, i32)) -> i64 {
        (i64::from(x1) - i64::from(x0) + 1)
            .max(0)
            .saturating_mul((i64::from(y1) - i64::from(y0) + 1).max(0))
    }

    fn cells(bounds: Rect) -> impl Iterator<Item = (i32, i32)> {
        let (x0, y0, x1, y1) = Self::cell_range(bounds);
        (y0..=y1).flat_map(move |y| (x0..=x1).map(move |x| (x, y)))
    }

    /// Whether an item with these bounds goes in the large list.
    fn is_large(bounds: Rect) -> bool {
        Self::cell_count(Self::cell_range(bounds)) > MAX_ITEM_CELLS
    }

    /// The ids in the grid cells `area` touches (with repeats for items in
    /// several), and the large items: by cell when there are fewer cells
    /// than the grid has, else by going through the grid.
    fn candidates(&self, area: Rect) -> Vec<ItemId> {
        let range = Self::cell_range(area);
        let mut ids = self.large.clone();
        if Self::cell_count(range) <= self.grid.len() as i64 {
            for cell in Self::cells(area) {
                ids.extend(self.grid.get(&cell).into_iter().flatten());
            }
        } else {
            let (x0, y0, x1, y1) = range;
            for (&(x, y), cell) in &self.grid {
                if (x0..=x1).contains(&x) && (y0..=y1).contains(&y) {
                    ids.extend(cell);
                }
            }
        }
        ids
    }

    fn unlink(&mut self, id: ItemId, bounds: Rect) {
        if Self::is_large(bounds) {
            self.large.retain(|&other| other != id);
            return;
        }
        for cell in Self::cells(bounds) {
            if let Some(ids) = self.grid.get_mut(&cell) {
                ids.retain(|&other| other != id);
                if ids.is_empty() {
                    self.grid.remove(&cell);
                }
            }
        }
    }

    /// Adds or changes an item: its stacking order `z` (higher on top) and
    /// the document rectangle it paints in. Its old and new areas are
    /// redrawn. An item whose bounds aren't finite isn't kept.
    pub fn upsert(&mut self, id: ItemId, z: u64, bounds: Rect) {
        if let Some(old) = self.items.remove(&id) {
            self.unlink(id, old.bounds);
            self.invalidate(old.bounds);
        }
        if !(bounds.x0.is_finite()
            && bounds.y0.is_finite()
            && bounds.x1.is_finite()
            && bounds.y1.is_finite())
        {
            return;
        }
        if Self::is_large(bounds) {
            self.large.push(id);
        } else {
            for cell in Self::cells(bounds) {
                self.grid.entry(cell).or_default().push(id);
            }
        }
        self.items.insert(id, Item { z, bounds });
        self.invalidate(bounds);
    }

    /// Like [`Canvas::upsert`], but only the new area is redrawn: for an
    /// item whose old area was invalidated while the content didn't paint
    /// it there (a dragged shape is lifted out of the tiles and drawn live
    /// meanwhile). Tiles rendered since then are right without it, and
    /// the others are stale already.
    pub fn upsert_drawn_elsewhere(&mut self, id: ItemId, z: u64, bounds: Rect) {
        let Some(old) = self.items.remove(&id) else {
            self.upsert(id, z, bounds);
            return;
        };
        self.unlink(id, old.bounds);
        self.upsert(id, z, bounds);
    }

    /// Removes an item; its area is redrawn.
    pub fn remove(&mut self, id: ItemId) {
        if let Some(item) = self.items.remove(&id) {
            self.unlink(id, item.bounds);
            self.invalidate(item.bounds);
        }
    }

    /// Frees every texture the canvas holds, through `scene`'s texture
    /// work, and forgets its tiles: call it before dropping a canvas
    /// whose renderer lives on (the renderer can't know the textures are
    /// unused). The items stay.
    pub fn release(&mut self, scene: &mut Scene) {
        for (_, tile) in self.tiles.drain() {
            if let Some(texture) = tile.texture {
                scene.free_texture(texture);
            }
        }
        self.levels.clear();
        if let Some(workers) = &mut self.workers {
            workers.flights.clear();
            workers.parked.clear();
        }
        self.level = None;
    }

    fn put_tile(&mut self, key: TileKey, tile: Tile) {
        if self.tiles.insert(key, tile).is_none() {
            *self.levels.entry(key.level).or_default() += 1;
        }
    }

    fn take_tile(&mut self, key: TileKey) -> Option<Tile> {
        let tile = self.tiles.remove(&key)?;
        if let Some(count) = self.levels.get_mut(&key.level) {
            *count -= 1;
            if *count == 0 {
                self.levels.remove(&key.level);
            }
        }
        Some(tile)
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
            .get(&cell)
            .into_iter()
            .flatten()
            .chain(&self.large)
            .filter_map(|id| Some((*id, self.items.get(id)?)))
            .filter(|(_, item)| item.bounds.contains(point))
            .max_by_key(|(id, item)| (item.z, *id))
            .map(|(id, _)| id)
    }

    /// Every item whose bounds overlap `area` (document units), back to
    /// front: the candidates of a marquee.
    pub fn items_overlapping(&self, area: Rect) -> Vec<ItemId> {
        self.items_in(area)
    }

    /// Marks the tiles over `area` (document units) for re-rendering:
    /// content there changed without an item changing (a style change).
    pub fn invalidate(&mut self, area: Rect) {
        // One device pixel more, for antialiasing, at each level.
        let touches = |key: &TileKey| {
            let grow = 1.0 / level_scale(key.level);
            key.bounds().intersect(area.inflate(grow, grow)).area() > 0.0
                || area.inflate(grow, grow).contains(key.bounds().origin())
        };
        // The tiles over `area`, level by level (half a pixel more, for
        // rounding), unless that is more than the cache holds: an edit
        // touches a few tiles, the cache has up to a thousand.
        let mut ranges = Vec::with_capacity(self.levels.len());
        let mut keys = 0i64;
        for &level in self.levels.keys() {
            let grow = 1.5 / level_scale(level);
            let (x0, y0, x1, y1) = tiles_in(level, area.inflate(grow, grow));
            keys = keys.saturating_add(
                (i64::from(x1) - i64::from(x0))
                    .max(0)
                    .saturating_mul((i64::from(y1) - i64::from(y0)).max(0)),
            );
            ranges.push((level, x0, y0, x1, y1));
        }
        if keys <= self.tiles.len() as i64 {
            for (level, x0, y0, x1, y1) in ranges {
                for y in y0..y1 {
                    for x in x0..x1 {
                        let key = TileKey { level, x, y };
                        if let Some(tile) = self.tiles.get_mut(&key)
                            && touches(&key)
                        {
                            tile.stale = true;
                        }
                    }
                }
            }
        } else {
            for (key, tile) in &mut self.tiles {
                if touches(key) {
                    tile.stale = true;
                }
            }
        }
        // At most a few dozen.
        for (key, flight) in self.flights_mut() {
            if touches(key) {
                flight.stale = true;
            }
        }
    }

    /// Marks every tile for re-rendering.
    pub fn invalidate_all(&mut self) {
        for tile in self.tiles.values_mut() {
            tile.stale = true;
        }
        for (_, flight) in self.flights_mut() {
            flight.stale = true;
        }
    }

    fn flights_mut(&mut self) -> impl Iterator<Item = (&TileKey, &mut Flight)> {
        self.workers
            .iter_mut()
            .flat_map(|workers| workers.flights.iter_mut())
    }

    /// Takes the tiles the workers finished, to upload as many as a frame
    /// has time for ([`Canvas::upload_parked`]).
    fn receive(&mut self) {
        let Some(workers) = &mut self.workers else {
            return;
        };
        while let Ok(done) = workers.done.try_recv() {
            workers.park(done);
        }
    }

    /// How many tiles uploading takes about `time`: at least one, and at
    /// most [`MAX_UPLOADS`].
    fn uploads_within(&self, time: Duration) -> usize {
        let fits = time.as_nanos() / self.upload_cost.as_nanos().max(1);
        usize::try_from(fits)
            .unwrap_or(usize::MAX)
            .clamp(1, MAX_UPLOADS)
    }

    /// Uploads `count` of the parked tiles at most with this frame's
    /// scene: those of `level` first, then the others in the order they
    /// came.
    fn upload_parked(
        &mut self,
        level: i32,
        count: usize,
        scene: &mut Scene,
        stats: &mut CanvasStats,
    ) {
        let Some(workers) = &mut self.workers else {
            return;
        };
        let mut parked = std::mem::take(&mut workers.parked);
        parked.sort_by_key(|done| done.key.level != level);
        let start = stats.arrived;
        let mut rest = Vec::new();
        for done in parked {
            if stats.arrived - start < count {
                self.accept(done, scene, stats);
            } else {
                rest.push(done);
            }
        }
        if let Some(workers) = &mut self.workers {
            workers.parked = rest;
        }
    }

    /// Waits until `deadline` at most for the tiles in `awaited` to come
    /// back from the workers and uploads them. Each upload's estimated time
    /// comes off the wait, since the frame spends it too. The others that
    /// come meanwhile are parked.
    fn wait_for(
        &mut self,
        awaited: &mut HashSet<TileKey>,
        mut deadline: Instant,
        scene: &mut Scene,
        stats: &mut CanvasStats,
    ) {
        while !awaited.is_empty() {
            // The frame's uploads are capped, blank tiles' too: the rest
            // show blank for one more frame.
            if stats.arrived >= MAX_UPLOADS {
                return;
            }
            let left = deadline.saturating_duration_since(Instant::now());
            let Some(done) = self
                .workers
                .as_ref()
                .and_then(|workers| workers.done.recv_timeout(left).ok())
            else {
                return;
            };
            let key = done.key;
            if !awaited.contains(&key) {
                if let Some(workers) = &mut self.workers {
                    workers.park(done);
                }
                continue;
            }
            let uploads = stats.arrived;
            if self.accept(done, scene, stats) {
                awaited.remove(&key);
            }
            if stats.arrived > uploads {
                deadline = deadline
                    .checked_sub(self.upload_cost)
                    .unwrap_or_else(Instant::now);
            }
        }
    }

    /// Takes a tile back from the workers and uploads it, unless it is an
    /// earlier render's (`false`).
    fn accept(&mut self, done: Done, scene: &mut Scene, stats: &mut CanvasStats) -> bool {
        let Some(workers) = &mut self.workers else {
            return false;
        };
        let Some(flight) = workers.flights.remove(&done.key) else {
            return false;
        };
        if flight.tag != done.tag {
            workers.flights.insert(done.key, flight);
            return false;
        }
        stats.worker_time += done.took;
        let old = self.tiles.get(&done.key).and_then(|tile| tile.texture);
        let texture = match done.pixmap {
            Some(pixmap) => {
                let texture = old.unwrap_or_else(TextureKey::unique);
                scene.upload_texture(texture, pixmap);
                stats.arrived += 1;
                self.last_rendered.push(done.key);
                Some(texture)
            }
            // Empty rather than tried again in every frame; a change
            // to its items tries again.
            None => {
                if let Some(old) = old {
                    scene.free_texture(old);
                }
                stats.failed += 1;
                None
            }
        };
        // A stale one renders again, also when this render failed: its
        // items changed, and that is the retry the failure waits for.
        self.put_tile(
            done.key,
            Tile {
                texture,
                stale: flight.stale,
                used: self.frame,
            },
        );
        true
    }

    /// The items touching `area`, bottom first.
    /// Whether any item touches `area`.
    fn any_item_in(&self, area: Rect) -> bool {
        self.candidates(area).iter().any(|id| {
            self.items
                .get(id)
                .is_some_and(|item| item.bounds.intersect(area).area() > 0.0)
        })
    }

    fn items_in(&self, area: Rect) -> Vec<ItemId> {
        let mut seen = HashSet::new();
        let mut found: Vec<(u64, ItemId)> = Vec::new();
        for id in self.candidates(area) {
            if seen.insert(id)
                && let Some(item) = self.items.get(&id)
                && item.bounds.intersect(area).area() > 0.0
            {
                found.push((item.z, id));
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
        let finite = |v: f64| v.is_finite();
        let screen = viewport.screen;
        if !(viewport.zoom > 0.0
            && finite(viewport.zoom)
            && viewport.scale_factor > 0.0
            && finite(viewport.scale_factor)
            && finite(viewport.origin.x)
            && finite(viewport.origin.y)
            && finite(screen.x0)
            && finite(screen.y0)
            && finite(screen.x1)
            && finite(screen.y1))
        {
            return CanvasStats::default(); // nothing sensible to show
        }
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
        // Uploads first, as they show finished work, in half the frame's
        // tile time at most, so that tiles still get built.
        let budget = self.frame_render_budget();
        self.receive();
        self.upload_parked(level, self.uploads_within(budget / 2), scene, &mut stats);
        let uploading = self.upload_cost * stats.arrived as u32;
        let build_start = Instant::now();
        let mut draws: Vec<(TileKey, Option<TextureKey>)> = Vec::with_capacity(keys.len());
        let mut invalid: Vec<TileKey> = Vec::new();
        for key in keys {
            match self.tiles.get_mut(&key) {
                Some(tile) if !tile.stale => {
                    tile.used = self.frame;
                    draws.push((key, tile.texture));
                    stats.reused += 1;
                }
                _ => invalid.push(key),
            }
        }
        // Tiles with nothing in them need no texture: valid at once, for
        // free (zoomed out, most of the screen can be empty).
        invalid.retain(|key| {
            if self.any_item_in(key.bounds()) {
                return true;
            }
            if let Some(texture) = self.tiles.get(key).and_then(|tile| tile.texture) {
                scene.free_texture(texture);
            }
            // A render on its way is out of date: its items are gone.
            if let Some(workers) = &mut self.workers {
                workers.flights.remove(key);
            }
            self.put_tile(
                *key,
                Tile {
                    texture: None,
                    stale: false,
                    used: self.frame,
                },
            );
            false
        });
        // What the budget leaves over shows a stand-in, so render first
        // where there would be none or only part of one (blank), then where
        // it would be wrong (a stale copy), then where it is blurry
        // (another level). Nearest the middle first within each.
        let mut ranked: Vec<(u8, TileKey)> = invalid
            .into_iter()
            .map(|key| {
                (
                    self.stand_in(key).map_or(0, |stand_in| stand_in.rank()),
                    key,
                )
            })
            .collect();
        ranked.sort_by_key(|(rank, _)| *rank);
        let mut owed: Vec<TileKey> = Vec::new();
        let mut units = 0;
        // Once a tile doesn't fit, the rest wait too (and their items
        // aren't looked up).
        let render_budget = budget.saturating_sub(uploading);
        let mut spent = false;
        // Handed to the workers with nothing to stand in for all of them.
        let mut blanks: HashSet<TileKey> = HashSet::new();
        for (rank, key) in ranked {
            // A tile the workers have shows a stand-in until it is back;
            // when they have enough to do, the rest wait for the next frame.
            let waiting = self.workers.as_ref().is_some_and(|workers| {
                workers.flights.contains_key(&key)
                    || workers.flights.len() >= workers.pool.threads() * FLIGHTS_PER_THREAD
            });
            if waiting || spent || stats.rendered >= self.budget {
                owed.push(key);
                continue;
            }
            let items = self.items_in(key.bounds());
            let cost = self.tile_units + items.len();
            // The estimate decides, and the clock when it is wrong.
            if stats.rendered > 0
                && (self.unit_cost * (units + cost) as u32 > render_budget
                    || build_start.elapsed() >= render_budget)
            {
                spent = true;
                owed.push(key);
                continue;
            }
            units += cost;
            let mut tile_scene = Scene::new();
            // Document → level pixels → this tile's pixels.
            let to_tile = Affine::translate(Vec2::new(
                -f64::from(key.x) * TILE_F,
                -f64::from(key.y) * TILE_F,
            )) * Affine::scale(level_scale);
            for id in items {
                tile_scene.paint_with(to_tile, |scene| {
                    content.paint_item(id, scene, level_scale);
                });
                stats.items_painted += 1;
            }
            stats.rendered += 1;
            if let Some(workers) = &mut self.workers {
                let tag = workers.next_tag;
                workers.next_tag += 1;
                workers.pool.submit(Job {
                    key,
                    tag,
                    scene: tile_scene,
                    size: TILE,
                    reply: workers.reply.clone(),
                });
                workers.flights.insert(key, Flight { tag, stale: false });
                if rank == 0 {
                    blanks.insert(key);
                }
                owed.push(key);
                continue;
            }
            let texture = self
                .tiles
                .get(&key)
                .and_then(|tile| tile.texture)
                .unwrap_or_else(TextureKey::unique);
            scene.render_texture(texture, TILE, TILE, tile_scene);
            self.put_tile(
                key,
                Tile {
                    texture: Some(texture),
                    stale: false,
                    used: self.frame,
                },
            );
            draws.push((key, Some(texture)));
            self.last_rendered.push(key);
        }
        stats.build = build_start.elapsed();
        if !blanks.is_empty() {
            // They would be blank until a later frame: wait a little for
            // them instead, within what the rest of the frame leaves.
            let left = self.rest.map_or(self.blank_wait, |rest| {
                self.blank_wait
                    .min(FRAME_TARGET.saturating_sub(rest + uploading + stats.build))
            });
            let waiting = Instant::now();
            if !left.is_zero() {
                self.wait_for(&mut blanks, waiting + left, scene, &mut stats);
            }
            stats.waited = waiting.elapsed();
        }
        // Time left over: more of the tiles that came back.
        let used = self.upload_cost * stats.arrived as u32 + stats.build + stats.waited;
        if budget > used && stats.arrived < MAX_UPLOADS {
            let more = self
                .uploads_within(budget - used)
                .min(MAX_UPLOADS - stats.arrived);
            self.upload_parked(level, more, scene, &mut stats);
        }
        // Tiles back by now are drawn as cached.
        owed.retain(|key| {
            let out = self
                .workers
                .as_ref()
                .is_some_and(|workers| workers.flights.contains_key(key));
            match self.tiles.get(key) {
                Some(tile) if !out && !tile.stale => {
                    draws.push((*key, tile.texture));
                    false
                }
                _ => true,
            }
        });
        self.last_tile_work = stats.build + stats.waited;
        self.last_uploads = stats.arrived;
        stats.owed = owed.len();
        self.pending = (stats.rendered > 0).then_some((stats.build, units));
        if self.workers.is_some() {
            // Each needs a texture once it is back: on the GPU, room for
            // them is made meanwhile.
            let new = owed
                .iter()
                .filter(|key| self.tiles.get(*key).and_then(|tile| tile.texture).is_none())
                .count();
            if new > 0 {
                scene.expect_uploads(new);
            }
        }

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
        // Stand-ins first, clipped to the tile they stand in for. They
        // count as drawn, so this frame's eviction keeps them.
        for key in &owed {
            let shown = match self.stand_in(*key) {
                None => {
                    stats.blank += 1;
                    continue;
                }
                Some(stand_in) => {
                    if matches!(stand_in, StandIn::Partial(_)) {
                        stats.blank += 1;
                    }
                    stand_in.tiles(*key)
                }
            };
            scene.push_clip(&dest(key));
            for other in shown {
                if let Some(tile) = self.tiles.get_mut(&other) {
                    tile.used = self.frame;
                    if let Some(texture) = tile.texture {
                        scene.draw_texture(texture, TILE, TILE, dest(&other));
                    }
                }
            }
            scene.pop();
        }
        for (key, texture) in &draws {
            if let Some(texture) = texture {
                scene.draw_texture(*texture, TILE, TILE, dest(key));
            }
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
        stats.in_flight = self.in_flight();
        stats.parked = self
            .workers
            .as_ref()
            .map_or(0, |workers| workers.parked.len());
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
            scene.paint_with(camera, |scene| content.paint_item(id, scene, scale));
            stats.items_painted += 1;
        }
        scene.set_transform(Affine::IDENTITY);
        scene.pop();
        stats.build = build.elapsed();
        stats
    }

    /// What can show in place of `key` until it is rendered: its stale
    /// copy, or the nearest level whose cached tiles cover it, coarser
    /// first (zooming in) and then finer (zooming out). Levels are half
    /// octaves apart, so a tile can straddle up to four of the next
    /// level's, coarser or finer: a stand-in needs all of them. Failing
    /// that, the nearest level with any of them covers part of it.
    fn stand_in(&self, key: TileKey) -> Option<StandIn> {
        if self.tiles.contains_key(&key) {
            return Some(StandIn::Stale);
        }
        // Inset a little, so tiles that only touch it don't count.
        let inside = key.bounds().inset(-1e-6);
        let coarser = (key.level - 6..key.level).rev().map(|level| (level, true));
        let finer = (key.level + 1..=key.level + 4).map(|level| (level, false));
        let mut partial = Vec::new();
        for (level, is_coarser) in coarser.chain(finer) {
            let (x0, y0, x1, y1) = tiles_in(level, inside);
            let mut cached = Vec::new();
            let mut all = true;
            for y in y0..y1 {
                for x in x0..x1 {
                    let tile = TileKey { level, x, y };
                    if self.tiles.contains_key(&tile) {
                        cached.push(tile);
                    } else {
                        all = false;
                    }
                }
            }
            // An empty range (from coordinates that overflowed) covers
            // nothing.
            if all && !cached.is_empty() {
                return Some(if is_coarser {
                    StandIn::Coarser(cached)
                } else {
                    StandIn::Finer(cached)
                });
            }
            if partial.is_empty() {
                partial = cached;
            }
        }
        (!partial.is_empty()).then_some(StandIn::Partial(partial))
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
            if let Some(tile) = self.take_tile(key) {
                if let Some(texture) = tile.texture {
                    scene.free_texture(texture);
                }
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

    #[test]
    fn items_overlapping_lists_the_items_over_an_area_back_to_front() {
        let mut canvas = Canvas::new();
        canvas.upsert(ItemId(1), 10, Rect::new(0.0, 0.0, 100.0, 100.0));
        canvas.upsert(ItemId(2), 30, Rect::new(50.0, 50.0, 150.0, 150.0));
        canvas.upsert(ItemId(3), 20, Rect::new(-1e6, -1e6, 1e6, 1e6));
        assert_eq!(
            canvas.items_overlapping(Rect::new(90.0, 90.0, 95.0, 95.0)),
            vec![ItemId(1), ItemId(3), ItemId(2)]
        );
        assert_eq!(
            canvas.items_overlapping(Rect::new(5.0, 5.0, 6.0, 6.0)),
            vec![ItemId(1), ItemId(3)]
        );
    }
    use crate::CpuMode;
    use crate::scene::{Command, TextureWork};

    /// A time budget that never runs out (tests of other limits).
    const UNLIMITED: Duration = Duration::from_secs(3_600);

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
        let mut canvas = Canvas::new()
            .with_limits(1_000, 10_000)
            .with_render_budget(UNLIMITED);
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

    /// The frame that rendered the most tiles of a few. Besides the
    /// estimate, the clock stops a frame's tile renders, so a busy machine
    /// (a CI runner) renders fewer: never more. The most is what the
    /// estimate allows.
    fn most_rendered(mut frame: impl FnMut() -> CanvasStats) -> CanvasStats {
        (0..10)
            .map(|_| frame())
            .max_by_key(|stats| stats.rendered)
            .unwrap_or_default()
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

    /// Textures a scene both draws and frees (the renderer frees before it
    /// draws).
    fn drawn_and_freed(scene: &Scene) -> Vec<TextureKey> {
        let freed: HashSet<TextureKey> = scene
            .texture_work()
            .iter()
            .filter_map(|work| match work {
                TextureWork::Free(key) => Some(*key),
                TextureWork::Render { .. }
                | TextureWork::Upload { .. }
                | TextureWork::Expect(_) => None,
            })
            .collect();
        scene
            .commands()
            .iter()
            .filter_map(|command| match command {
                Command::DrawTexture { key, .. } if freed.contains(key) => Some(*key),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_stand_in_is_not_evicted_in_the_frame_it_is_drawn() {
        let (mut canvas, mut boxes) = board(2_000);
        let now = Instant::now();
        // 5 × 4 tiles, all cached.
        canvas.frame(
            &mut Scene::new(),
            &viewport(Point::ZERO, 1.0),
            now,
            &mut boxes,
        );
        assert_eq!(canvas.tiles.len(), 20);
        // The corner tile goes stale; then a pan brings in a column of new
        // tiles, but only one can render: the stale tile is owed, shown by
        // its stale copy, and the cache is over capacity.
        canvas.invalidate(Rect::new(10.0, 10.0, 20.0, 20.0));
        canvas.budget = 1;
        canvas.capacity = 20;
        let mut scene = Scene::new();
        let stats = canvas.frame(
            &mut scene,
            &viewport(Point::new(128.0, 0.0), 1.0),
            now,
            &mut boxes,
        );
        assert!(stats.owed > 0, "{stats:?}");
        assert_eq!(drawn_and_freed(&scene), Vec::new());
    }

    #[test]
    fn the_time_budget_follows_what_tiles_cost() {
        let (canvas, mut boxes) = board(2_000);
        let mut canvas = canvas.with_render_budget(RENDER_BUDGET);
        let now = Instant::now();
        let view = viewport(Point::ZERO, 1.0);
        // The first guess: a mid-range laptop.
        let first = most_rendered(|| {
            canvas.invalidate_all();
            canvas.frame(&mut Scene::new(), &view, now, &mut boxes)
        });
        assert!(first.rendered > 1 && first.owed > 0, "{first:?}");
        // The renderer took 60 ms for them: far fewer next time.
        canvas.note_render_time(Duration::from_millis(60));
        let slow = canvas.frame(&mut Scene::new(), &view, now, &mut boxes);
        assert!(
            slow.rendered >= 1 && slow.rendered < first.rendered,
            "{slow:?}"
        );
        // Cheap renders: more per frame again.
        for _ in 0..4 {
            canvas.note_render_time(Duration::ZERO);
            canvas.invalidate_all();
            canvas.frame(&mut Scene::new(), &view, now, &mut boxes);
        }
        let fast = most_rendered(|| {
            canvas.invalidate_all();
            canvas.frame(&mut Scene::new(), &view, now, &mut boxes)
        });
        assert!(fast.rendered > slow.rendered, "{fast:?}");
    }

    #[test]
    fn tile_work_gets_what_the_rest_of_the_frame_leaves() {
        let (canvas, mut boxes) = board(2_000);
        let mut canvas = canvas.with_render_budget(RENDER_BUDGET);
        let now = Instant::now();
        let view = viewport(Point::ZERO, 1.0);
        let mut rendered = |rest: Option<Duration>| {
            most_rendered(|| {
                if let Some(rest) = rest {
                    // The frame took this besides the canvas's own part.
                    canvas.note_rest_of_frame(rest + canvas.last_tile_work);
                }
                canvas.invalidate_all();
                canvas.frame(&mut Scene::new(), &view, now, &mut boxes)
            })
            .rendered
        };
        let unknown = rendered(None);
        // A light frame leaves the whole budget.
        assert_eq!(rendered(Some(Duration::from_millis(2))), unknown);
        // A heavy one leaves less: fewer tiles.
        let heavy = rendered(Some(Duration::from_millis(10)));
        assert!(heavy >= 1 && heavy < unknown, "{heavy} of {unknown}");
        // One over the whole frame still renders a tile.
        assert!(rendered(Some(Duration::from_millis(30))) >= 1);
    }

    #[test]
    fn zooming_out_shows_finer_tiles_and_renders_blank_areas_first() {
        let (canvas, mut boxes) = board(5_000);
        let mut canvas = canvas.with_limits(1, 10_000);
        let start = Instant::now();
        let near = viewport(Point::ZERO, 1.0);
        while canvas
            .frame(&mut Scene::new(), &near, start, &mut boxes)
            .owed
            > 0
        {}
        let finer: HashSet<TextureKey> = canvas
            .tiles
            .values()
            .filter_map(|tile| tile.texture)
            .collect();
        // Zoomed out past half: a coarser level at once, one tile render
        // per frame.
        let later = start + Duration::from_secs(1);
        let far = viewport(Point::ZERO, 0.45);
        let mut scene = Scene::new();
        let stats = canvas.frame(&mut scene, &far, later, &mut boxes);
        assert!(
            stats.level < 0 && stats.rendered == 1 && stats.owed > 0,
            "{stats:?}"
        );
        // The rendered tile is one nothing stood in for all of: not within
        // the area the finer tiles cover.
        let rendered = canvas.last_rendered[0].bounds();
        assert!(!near.visible().contains_rect(rendered), "{rendered:?}");
        // The rest of the old view shows the finer tiles.
        let drawn = scene
            .commands()
            .iter()
            .filter(|command| {
                matches!(command, Command::DrawTexture { key, .. } if finer.contains(key))
            })
            .count();
        assert!(drawn >= 4, "{drawn} finer tiles drawn");
    }

    /// The tiles `key`'s stand-in draws, if any.
    fn shown_for(canvas: &Canvas, key: TileKey) -> Vec<TileKey> {
        canvas
            .stand_in(key)
            .map_or_else(Vec::new, |stand_in| stand_in.tiles(key))
    }

    /// Whether `tiles` cover all of `key` (sampled at 9 × 9 points).
    fn covered(key: TileKey, tiles: &[TileKey]) -> bool {
        let b = key.bounds();
        (0..9).all(|i| {
            (0..9).all(|j| {
                let p = Point::new(
                    b.x0 + b.width() * (f64::from(i) + 0.5) / 9.0,
                    b.y0 + b.height() * (f64::from(j) + 0.5) / 9.0,
                );
                tiles.iter().any(|tile| tile.bounds().contains(p))
            })
        })
    }

    #[test]
    fn stand_ins_cover_the_whole_tile_zooming_in_and_out() {
        let (canvas, mut boxes) = board(5_000);
        let mut canvas = canvas.with_limits(1, 10_000);
        let start = Instant::now();
        let settle = |canvas: &mut Canvas, boxes: &mut Boxes, view: &Viewport, at: Instant| {
            while canvas.frame(&mut Scene::new(), view, at, boxes).owed > 0 {}
        };
        // Levels are half octaves apart, so a tile straddles the next
        // level's tiles both ways.
        // In by one level, out by three, in by three.
        for (zoom, from) in [(1.1, 1.0), (0.3, 1.0), (1.1, 0.45)] {
            canvas.invalidate_all();
            canvas.tiles.clear();
            // Big enough for a whole tile of the coarsest level inside.
            let at = |zoom| Viewport {
                screen: Rect::new(0.0, 0.0, 2560.0, 1600.0),
                ..viewport(Point::new(300.0, 200.0), zoom)
            };
            let first = at(from);
            settle(&mut canvas, &mut boxes, &first, start);
            let later = start + Duration::from_secs(1);
            let view = at(zoom);
            let stats = canvas.frame(&mut Scene::new(), &view, later, &mut boxes);
            assert!(stats.owed > 0, "{zoom}: {stats:?}");
            let (x0, y0, x1, y1) = tiles_in(stats.level, view.visible());
            let mut checked = 0;
            for y in y0..y1 {
                for x in x0..x1 {
                    let key = TileKey {
                        level: stats.level,
                        x,
                        y,
                    };
                    if canvas.tiles.get(&key).is_some_and(|tile| !tile.stale) {
                        continue;
                    }
                    // Within the old view (the rest is newly in view).
                    if !first.visible().contains_rect(key.bounds()) {
                        continue;
                    }
                    checked += 1;
                    let shown = shown_for(&canvas, key);
                    assert!(covered(key, &shown), "{zoom}: {key:?} shows only {shown:?}");
                }
            }
            assert!(checked > 0, "{zoom}");
        }
    }

    #[test]
    fn empty_tiles_need_no_texture() {
        let mut canvas = Canvas::new().with_render_budget(UNLIMITED);
        let mut boxes = Boxes {
            bounds: HashMap::new(),
            painted: Vec::new(),
        };
        let bounds = Rect::new(10.0, 10.0, 60.0, 60.0);
        canvas.upsert(ItemId(1), 1, bounds);
        boxes.bounds.insert(ItemId(1), bounds);
        let now = Instant::now();
        let view = viewport(Point::ZERO, 1.0);
        let mut scene = Scene::new();
        let stats = canvas.frame(&mut scene, &view, now, &mut boxes);
        assert_eq!((stats.rendered, stats.owed), (1, 0), "{stats:?}");
        assert_eq!(scene.texture_renders(), 1);
        assert_eq!(
            canvas.tiles.len(),
            stats.visible,
            "the empty ones are valid too"
        );
        // Without the item, its tile's texture goes and nothing renders.
        canvas.remove(ItemId(1));
        boxes.bounds.remove(&ItemId(1));
        let mut scene = Scene::new();
        let stats = canvas.frame(&mut scene, &view, now, &mut boxes);
        assert_eq!(stats.rendered, 0);
        assert_eq!(scene.texture_renders(), 0);
        assert!(
            scene
                .texture_work()
                .iter()
                .any(|work| matches!(work, TextureWork::Free(_)))
        );
    }

    #[test]
    fn the_cache_evicts_the_least_recently_drawn_tiles() {
        let mut canvas = Canvas::new()
            .with_limits(1_000, 40)
            .with_render_budget(UNLIMITED);
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

    /// The items a tile shows: what it would paint, by id and bounds.
    fn content_of(canvas: &Canvas, key: TileKey) -> Vec<(ItemId, [i64; 4])> {
        let mut items: Vec<(ItemId, [i64; 4])> = canvas
            .items_in(key.bounds())
            .into_iter()
            .filter_map(|id| {
                let b = canvas.items.get(&id)?.bounds;
                Some((id, [b.x0, b.y0, b.x1, b.y1].map(|v| (v * 1000.0) as i64)))
            })
            .collect();
        items.sort();
        items
    }

    /// A frame with workers, keeping track of which textures exist in
    /// `alive`: every texture it draws must have been uploaded by then.
    fn tracked_frame(
        canvas: &mut Canvas,
        view: &Viewport,
        boxes: &mut Boxes,
        alive: &mut HashSet<TextureKey>,
    ) -> CanvasStats {
        let mut scene = Scene::new();
        let stats = canvas.frame(&mut scene, view, Instant::now(), boxes);
        assert_eq!(scene.texture_renders(), 0, "nothing renders in the frame");
        for work in scene.texture_work() {
            match work {
                TextureWork::Upload { key, .. } => {
                    alive.insert(*key);
                }
                TextureWork::Free(key) => {
                    alive.remove(key);
                }
                TextureWork::Render { .. } | TextureWork::Expect(_) => {}
            }
        }
        for command in scene.commands() {
            if let Command::DrawTexture { key, .. } = command {
                assert!(alive.contains(key), "{key:?} is drawn but wasn't uploaded");
            }
        }
        stats
    }

    /// Frames until no tile is owed (the workers have sent them all back),
    /// as [`tracked_frame`]s. Returns the frames' stats.
    fn frames_until_settled(
        canvas: &mut Canvas,
        view: &Viewport,
        boxes: &mut Boxes,
        alive: &mut HashSet<TextureKey>,
    ) -> Vec<CanvasStats> {
        let started = Instant::now();
        let mut frames = Vec::new();
        loop {
            let stats = tracked_frame(canvas, view, boxes, alive);
            let settled = stats.owed == 0;
            frames.push(stats);
            if settled {
                return frames;
            }
            assert!(
                started.elapsed() < Duration::from_secs(60),
                "the workers never finished: {:?}",
                frames.last()
            );
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn workers_render_the_tiles_and_frames_upload_them() {
        let (canvas, mut boxes) = board(2_000);
        let mut canvas = canvas.with_workers(TileWorkers::new(2, CpuMode::Deterministic));
        let view = viewport(Point::ZERO, 1.0);
        let mut alive = HashSet::new();
        let frames = frames_until_settled(&mut canvas, &view, &mut boxes, &mut alive);
        let first = &frames[0];
        // Some may be back within the frame's wait for blank tiles.
        assert!(
            first.rendered > 0 && first.arrived + first.in_flight == first.rendered,
            "{first:?}"
        );
        assert_eq!(first.reused, 0);
        let arrived: usize = frames.iter().map(|stats| stats.arrived).sum();
        let handed: usize = frames.iter().map(|stats| stats.rendered).sum();
        assert_eq!(arrived, handed);
        assert_eq!(canvas.in_flight(), 0);
        // Nothing changed: nothing more to do, all from the cache.
        let again = canvas.frame(&mut Scene::new(), &view, Instant::now(), &mut boxes);
        assert_eq!((again.rendered, again.owed), (0, 0));
        assert_eq!(again.reused, first.visible);
    }

    #[test]
    fn frames_say_how_many_tiles_the_workers_will_send() {
        let (canvas, mut boxes) = board(2_000);
        let mut canvas = canvas
            .with_workers(TileWorkers::new(2, CpuMode::Deterministic))
            .with_blank_wait(Duration::ZERO);
        let view = viewport(Point::ZERO, 1.0);
        let expected = |scene: &Scene| {
            scene
                .texture_work()
                .iter()
                .filter_map(|work| match work {
                    TextureWork::Expect(count) => Some(*count),
                    _ => None,
                })
                .collect::<Vec<_>>()
        };
        let mut scene = Scene::new();
        let first = canvas.frame(&mut scene, &view, Instant::now(), &mut boxes);
        // Nothing is cached yet: every owed tile is blank and needs room.
        assert!(first.owed > 0);
        assert_eq!(first.blank, first.owed);
        assert_eq!(expected(&scene), vec![first.owed]);
        assert_eq!(first.arrived, 0);
        frames_until_settled(&mut canvas, &view, &mut boxes, &mut HashSet::new());
        let mut scene = Scene::new();
        let settled = canvas.frame(&mut scene, &view, Instant::now(), &mut boxes);
        assert_eq!((settled.owed, settled.blank), (0, 0));
        assert!(expected(&scene).is_empty());
    }

    #[test]
    fn a_frame_waits_for_the_tiles_nothing_stands_in_for() {
        let (canvas, mut boxes) = board(2_000);
        // Enough threads for every visible tile at once.
        let mut canvas = canvas
            .with_workers(TileWorkers::new(4, CpuMode::Deterministic))
            .with_blank_wait(Duration::from_secs(60))
            .with_render_budget(UNLIMITED);
        // 12 tiles: within a frame's uploads.
        let view = Viewport {
            screen: Rect::new(0.0, 0.0, 1024.0, 768.0),
            ..viewport(Point::ZERO, 1.0)
        };
        let mut scene = Scene::new();
        let first = canvas.frame(&mut scene, &view, Instant::now(), &mut boxes);
        // Everything came back within the frame and is drawn from it.
        assert!(first.rendered > 0);
        assert_eq!(
            (first.arrived, first.rendered, first.owed, first.blank),
            (first.visible, first.visible, 0, 0)
        );
        assert!(first.waited > Duration::ZERO);
        let uploaded: HashSet<TextureKey> = scene
            .texture_work()
            .iter()
            .filter_map(|work| match work {
                TextureWork::Upload { key, .. } => Some(*key),
                _ => None,
            })
            .collect();
        let drawn: Vec<TextureKey> = scene
            .commands()
            .iter()
            .filter_map(|command| match command {
                Command::DrawTexture { key, .. } => Some(*key),
                _ => None,
            })
            .collect();
        assert_eq!(drawn.len(), first.rendered);
        assert!(drawn.iter().all(|key| uploaded.contains(key)));
        // Told how long the frame took, the canvas leaves out its own part.
        canvas.note_rest_of_frame(first.build + first.waited + Duration::from_millis(3));
        assert_eq!(canvas.rest, Some(Duration::from_millis(3)));
    }

    /// Content that paints like [`Boxes`] and takes `delay` per item.
    struct SlowBoxes {
        boxes: Boxes,
        delay: Duration,
    }

    impl CanvasContent for SlowBoxes {
        fn paint_item(&mut self, id: ItemId, scene: &mut Scene, level_scale: f64) {
            std::thread::sleep(self.delay);
            self.boxes.paint_item(id, scene, level_scale);
        }
    }

    /// A 2560 × 1600 window: 70 tiles at 100 %.
    fn big_viewport() -> Viewport {
        Viewport {
            screen: Rect::new(0.0, 0.0, 2560.0, 1600.0),
            ..viewport(Point::ZERO, 1.0)
        }
    }

    #[test]
    fn frames_upload_a_few_tiles_each_and_the_others_wait() {
        let (canvas, mut boxes) = board(5_000);
        let mut canvas = canvas
            .with_workers(TileWorkers::new(4, CpuMode::Deterministic))
            .with_blank_wait(Duration::ZERO);
        let view = big_viewport();
        let mut alive = HashSet::new();
        let first = tracked_frame(&mut canvas, &view, &mut boxes, &mut alive);
        assert!(first.rendered > MAX_UPLOADS, "{first:?}");
        // Every tile is back by now; a frame takes what it has time for.
        std::thread::sleep(Duration::from_millis(200));
        let next = tracked_frame(&mut canvas, &view, &mut boxes, &mut alive);
        assert!(next.arrived >= 1 && next.arrived <= MAX_UPLOADS, "{next:?}");
        assert!(next.parked > 0 && next.parked <= next.in_flight, "{next:?}");
        let frames = frames_until_settled(&mut canvas, &view, &mut boxes, &mut alive);
        assert!(frames.iter().all(|stats| stats.arrived <= MAX_UPLOADS));
        assert_eq!(canvas.in_flight(), 0);
    }

    #[test]
    fn uploads_follow_what_they_cost() {
        let (canvas, mut boxes) = board(5_000);
        let mut canvas = canvas
            .with_workers(TileWorkers::new(4, CpuMode::Deterministic))
            .with_blank_wait(Duration::ZERO)
            .with_render_budget(RENDER_BUDGET);
        let view = big_viewport();
        let now = Instant::now();
        canvas.frame(&mut Scene::new(), &view, now, &mut boxes);
        std::thread::sleep(Duration::from_millis(200));
        let cheap = canvas.frame(&mut Scene::new(), &view, now, &mut boxes);
        assert!(cheap.arrived > 4, "{cheap:?}");
        // From now on the renderer takes 5 ms for each: within a few
        // frames (one slow sample alone counts four times the estimate at
        // most), about one per frame.
        let mut arrived = cheap.arrived;
        let mut frames = Vec::new();
        for _ in 0..8 {
            canvas.note_render_time(Duration::from_millis(5) * arrived as u32);
            std::thread::sleep(Duration::from_millis(50));
            let stats = canvas.frame(&mut Scene::new(), &view, now, &mut boxes);
            arrived = stats.arrived;
            frames.push(stats);
        }
        let last = &frames[frames.len() - 1];
        assert!(last.arrived >= 1 && last.arrived <= 2, "{frames:?}");
    }

    #[test]
    fn the_clock_stops_building_when_the_estimate_is_wrong() {
        let (canvas, boxes) = board(5_000);
        let mut canvas = canvas
            .with_workers(TileWorkers::new(2, CpuMode::Deterministic))
            .with_blank_wait(Duration::ZERO)
            .with_render_budget(RENDER_BUDGET);
        // Ten times the first guess: 0.2 ms per item, so a full tile (12
        // items) takes 2.4 ms or more, where 25 seemed to fit in 6 ms.
        let mut slow = SlowBoxes {
            boxes,
            delay: Duration::from_micros(200),
        };
        let stats = canvas.frame(
            &mut Scene::new(),
            &big_viewport(),
            Instant::now(),
            &mut slow,
        );
        assert!(stats.rendered >= 1 && stats.rendered <= 3, "{stats:?}");
    }

    #[test]
    fn a_tile_whose_items_change_on_its_way_renders_again() {
        let (canvas, mut boxes) = board(2_000);
        let mut canvas = canvas.with_workers(TileWorkers::new(1, CpuMode::Deterministic));
        let view = viewport(Point::ZERO, 1.0);
        let mut alive = HashSet::new();
        frames_until_settled(&mut canvas, &view, &mut boxes, &mut alive);
        // Item 0 moves (tile (0, 0)), and moves again while the worker has
        // the tile: the first render comes back stale and goes again.
        let moved = Rect::new(10.0, 10.0, 110.0, 80.0);
        canvas.upsert(ItemId(0), 0, moved);
        boxes.bounds.insert(ItemId(0), moved);
        let handed = canvas.frame(&mut Scene::new(), &view, Instant::now(), &mut boxes);
        assert_eq!((handed.rendered, handed.in_flight), (1, 1));
        let again = Rect::new(20.0, 10.0, 120.0, 80.0);
        canvas.upsert(ItemId(0), 0, again);
        boxes.bounds.insert(ItemId(0), again);
        let frames = frames_until_settled(&mut canvas, &view, &mut boxes, &mut alive);
        let handed: usize = frames.iter().map(|stats| stats.rendered).sum();
        assert_eq!(handed, 1, "{frames:?}");
        assert!(canvas.tiles.values().all(|tile| !tile.stale));
    }

    proptest::proptest! {
        #![proptest_config(proptest::prelude::ProptestConfig::with_cases(64))]

        /// No change ever leaves a valid tile showing stale content: after
        /// any mix of inserts, moves, removals and frames at any viewport,
        /// every tile not marked stale shows what it would show if it were
        /// rendered now.
        #[test]
        fn valid_tiles_never_show_stale_content(
            ops in proptest::collection::vec(
                (0u8..4, 0u64..40, -300.0f64..900.0, -300.0f64..900.0, 4.0f64..200.0, 0.25f64..3.0),
                1..60,
            )
        ) {
            let mut canvas = Canvas::new().with_limits(64, 10_000).with_render_budget(UNLIMITED);
            let mut boxes = Boxes { bounds: HashMap::new(), painted: Vec::new() };
            let mut rendered: HashMap<TileKey, Vec<(ItemId, [i64; 4])>> = HashMap::new();
            let now = Instant::now();
            for (step, (op, id, x, y, size, zoom)) in ops.into_iter().enumerate() {
                let id = ItemId(id);
                match op {
                    0 | 1 => {
                        let bounds = Rect::new(x, y, x + size, y + size * 0.7);
                        canvas.upsert(id, id.0, bounds);
                        boxes.bounds.insert(id, bounds);
                    }
                    2 => {
                        canvas.remove(id);
                        boxes.bounds.remove(&id);
                    }
                    _ => {
                        let view = Viewport {
                            origin: Point::new(x, y),
                            zoom,
                            screen: Rect::new(0.0, 0.0, 800.0, 600.0),
                            scale_factor: 1.0,
                        };
                        // Frames far apart, so the zoom pause never holds a level.
                        let at = now + Duration::from_secs(step as u64);
                        let stats = canvas.frame(&mut Scene::new(), &view, at, &mut boxes);
                        for key in &canvas.last_rendered {
                            rendered.insert(*key, content_of(&canvas, *key));
                        }
                        proptest::prop_assert_eq!(stats.owed, 0);
                    }
                }
                for (key, tile) in &canvas.tiles {
                    if !tile.stale {
                        // A tile without a texture shows nothing.
                        let shown = match tile.texture {
                            Some(_) => rendered.get(key).cloned().unwrap_or_default(),
                            None => Vec::new(),
                        };
                        proptest::prop_assert_eq!(&shown, &content_of(&canvas, *key), "tile {:?} is valid but stale", key);
                    }
                }
            }
        }

        /// Every texture a frame draws exists when it is drawn: the frame's
        /// texture work (renders, then releases) has been done, as a
        /// renderer does it, and two canvases share the renderer. Small
        /// limits make eviction and stand-ins (stale copies, coarser
        /// levels) common.
        #[test]
        fn every_drawn_texture_exists(
            ops in proptest::collection::vec(
                (0u8..5, 0u64..60, -600.0f64..900.0, -600.0f64..900.0, 0.2f64..3.0, 0u64..400),
                1..100,
            )
        ) {
            let mut canvases = [Canvas::new().with_limits(3, 12), Canvas::new().with_limits(3, 12)];
            let mut boxes = [
                Boxes { bounds: HashMap::new(), painted: Vec::new() },
                Boxes { bounds: HashMap::new(), painted: Vec::new() },
            ];
            let mut alive: HashSet<TextureKey> = HashSet::new();
            let mut at = Instant::now();
            for (op, id, x, y, zoom, millis) in ops {
                let which = (id % 2) as usize;
                let item = ItemId(id);
                match op {
                    0 | 1 => {
                        // Revisiting the same few places keeps stale
                        // copies and coarser levels around as stand-ins.
                        let bounds = Rect::new(x, y, x + 80.0, y + 60.0);
                        canvases[which].upsert(item, id, bounds);
                        boxes[which].bounds.insert(item, bounds);
                    }
                    2 => {
                        canvases[which].remove(item);
                        boxes[which].bounds.remove(&item);
                    }
                    _ => {
                        let view = Viewport {
                            origin: Point::new((x / 300.0).round() * 300.0, (y / 300.0).round() * 300.0),
                            zoom,
                            screen: Rect::new(0.0, 0.0, 800.0, 600.0),
                            scale_factor: 1.0,
                        };
                        // Sometimes within the zoom pause, so a held level
                        // or a new one leaves tiles owed.
                        at += Duration::from_millis(millis);
                        let mut scene = Scene::new();
                        canvases[which].frame(&mut scene, &view, at, &mut boxes[which]);
                        for work in scene.texture_work() {
                            match work {
                                TextureWork::Render { key, .. } | TextureWork::Upload { key, .. } => {
                                    alive.insert(*key);
                                }
                                TextureWork::Free(key) => {
                                    alive.remove(key);
                                }
                                TextureWork::Expect(_) => {}
                            }
                        }
                        for command in scene.commands() {
                            if let Command::DrawTexture { key, .. } = command {
                                proptest::prop_assert!(alive.contains(key), "{:?} is drawn but doesn't exist", key);
                            }
                        }
                    }
                }
            }
        }

        /// Panning never invalidates a tile.
        #[test]
        fn panning_invalidates_nothing(dx in -2_000.0f64..2_000.0, dy in -2_000.0f64..2_000.0) {
            let (mut canvas, mut boxes) = board(500);
            let now = Instant::now();
            canvas.frame(&mut Scene::new(), &viewport(Point::ZERO, 1.0), now, &mut boxes);
            canvas.frame(&mut Scene::new(), &viewport(Point::new(dx, dy), 1.0), now, &mut boxes);
            proptest::prop_assert!(canvas.tiles.values().all(|tile| !tile.stale));
        }
    }

    /// A canvas with `count` cached tiles over levels -2 to 3 around the
    /// origin, without rendering them.
    fn cached(count: usize) -> Canvas {
        let mut canvas = Canvas::new().with_limits(64, 100_000);
        let mut placed = 0;
        'levels: for level in -2..=3 {
            for y in -30..30 {
                for x in -30..30 {
                    if placed == count {
                        break 'levels;
                    }
                    canvas.put_tile(
                        TileKey { level, x, y },
                        Tile {
                            texture: Some(TextureKey::unique()),
                            stale: false,
                            used: 0,
                        },
                    );
                    placed += 1;
                }
            }
        }
        canvas
    }

    #[test]
    fn edits_cost_the_tiles_they_touch_not_the_cache() {
        // 5,000 cached tiles: scanning them for each of the 40,000
        // invalidations of 20,000 moves takes seconds even in release.
        let mut canvas = cached(5_000);
        let start = Instant::now();
        for i in 0..20_000u64 {
            let x = (i % 100) as f64 * 30.0 - 1_500.0;
            let y = (i / 100) as f64 * 15.0 - 1_500.0;
            canvas.upsert(ItemId(i % 5_000), i, Rect::new(x, y, x + 20.0, y + 10.0));
        }
        assert!(
            start.elapsed() < Duration::from_secs(2),
            "{:?}",
            start.elapsed()
        );
    }

    #[test]
    fn a_frame_uploads_sixteen_tiles_at_most_also_while_waiting_for_blank_ones() {
        let (canvas, mut boxes) = board(5_000);
        let mut canvas = canvas
            .with_workers(TileWorkers::new(4, CpuMode::Deterministic))
            .with_blank_wait(Duration::from_secs(60))
            .with_render_budget(UNLIMITED);
        let first = canvas.frame(
            &mut Scene::new(),
            &big_viewport(),
            Instant::now(),
            &mut boxes,
        );
        assert!(first.rendered > MAX_UPLOADS, "{first:?}");
        assert!(first.arrived <= MAX_UPLOADS, "{first:?}");
    }

    #[test]
    fn one_slow_upload_does_not_stall_the_next_frames() {
        let mut canvas = Canvas::new().with_workers(TileWorkers::new(1, CpuMode::Deterministic));
        // A frame uploaded one tile and waited 100 ms for a tile page.
        canvas.last_uploads = 1;
        canvas.note_render_time(Duration::from_millis(100));
        assert!(
            canvas.upload_cost <= INITIAL_UPLOAD_COST * 3,
            "{:?}",
            canvas.upload_cost
        );
        assert!(canvas.uploads_within(RENDER_BUDGET / 2) > 1);
    }

    #[test]
    fn a_failed_render_of_a_changed_tile_is_tried_again() {
        let mut canvas = Canvas::new().with_workers(TileWorkers::new(1, CpuMode::Deterministic));
        let key = TileKey {
            level: 0,
            x: 0,
            y: 0,
        };
        if let Some(workers) = &mut canvas.workers {
            workers.flights.insert(
                key,
                Flight {
                    tag: 7,
                    stale: true,
                },
            );
        }
        let done = Done {
            key,
            tag: 7,
            pixmap: None,
            took: Duration::ZERO,
        };
        assert!(canvas.accept(done, &mut Scene::new(), &mut CanvasStats::default()));
        assert!(
            canvas.tiles[&key].stale,
            "its items changed while it was out"
        );
    }

    #[test]
    fn nonsense_viewports_and_bounds_draw_nothing_and_do_not_hang() {
        let (mut canvas, mut boxes) = board(100);
        for zoom in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            let stats = canvas.frame(
                &mut Scene::new(),
                &viewport(Point::ZERO, zoom),
                Instant::now(),
                &mut boxes,
            );
            assert_eq!(stats.visible, 0, "{zoom}");
        }
        let start = Instant::now();
        // A huge item is kept apart from the grid, and found everywhere.
        let huge = Rect::new(-1e12, -1e12, 1e12, 1e12);
        canvas.upsert(ItemId(1_000), 1_000, huge);
        boxes.bounds.insert(ItemId(1_000), huge);
        assert_eq!(canvas.item_at(Point::new(5e11, -5e11)), Some(ItemId(1_000)));
        canvas.frame(
            &mut Scene::new(),
            &viewport(Point::ZERO, 1.0),
            Instant::now(),
            &mut boxes,
        );
        canvas.remove(ItemId(1_000));
        // Bounds that aren't numbers aren't kept.
        canvas.upsert(ItemId(1_001), 1, Rect::new(0.0, 0.0, f64::INFINITY, 1.0));
        canvas.upsert(ItemId(1_002), 1, Rect::new(f64::NAN, 0.0, 1.0, 1.0));
        assert!(canvas.bounds(ItemId(1_001)).is_none() && canvas.bounds(ItemId(1_002)).is_none());
        // Zoomed far out, a tile spans many grid cells.
        canvas.frame(
            &mut Scene::new(),
            &viewport(Point::ZERO, 1e-4),
            Instant::now(),
            &mut boxes,
        );
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "{:?}",
            start.elapsed()
        );
    }

    #[test]
    fn an_item_drawn_elsewhere_redraws_only_its_new_place() {
        let (mut canvas, mut boxes) = board(2_000);
        let view = viewport(Point::ZERO, 1.0);
        canvas.frame(&mut Scene::new(), &view, Instant::now(), &mut boxes);
        // Item 0 is lifted: its tiles render once without it.
        let old = boxes.bounds[&ItemId(0)];
        canvas.invalidate(old);
        canvas.frame(&mut Scene::new(), &view, Instant::now(), &mut boxes);
        let new = old + Vec2::new(700.0, 400.0);
        canvas.upsert_drawn_elsewhere(ItemId(0), 0, new);
        boxes.bounds.insert(ItemId(0), new);
        let grow = 1.0;
        for (key, tile) in &canvas.tiles {
            let under_new = key.bounds().intersect(new.inflate(grow, grow)).area() > 0.0;
            assert_eq!(tile.stale, under_new, "{key:?}");
        }
        assert_eq!(canvas.bounds(ItemId(0)), Some(new));
        assert_eq!(canvas.item_at(new.center()), Some(ItemId(0)));
        assert_eq!(canvas.item_at(old.center()), None);
    }

    #[test]
    fn release_frees_every_texture() {
        let (mut canvas, mut boxes) = board(2_000);
        let mut made = HashSet::new();
        let mut scene = Scene::new();
        canvas.frame(
            &mut scene,
            &viewport(Point::ZERO, 1.0),
            Instant::now(),
            &mut boxes,
        );
        for work in scene.texture_work() {
            if let TextureWork::Render { key, .. } = work {
                made.insert(*key);
            }
        }
        assert!(!made.is_empty());
        let mut scene = Scene::new();
        canvas.release(&mut scene);
        let freed: HashSet<TextureKey> = scene
            .texture_work()
            .iter()
            .filter_map(|work| match work {
                TextureWork::Free(key) => Some(*key),
                _ => None,
            })
            .collect();
        assert_eq!(freed, made);
        assert_eq!(canvas.tiles.len(), 0);
    }

    proptest::proptest! {
        #![proptest_config(proptest::prelude::ProptestConfig::with_cases(256))]

        /// Looking up the tiles over an area level by level marks the same
        /// tiles as going through all of them.
        #[test]
        fn invalidation_by_lookup_matches_a_full_scan(
            count in 1usize..2_400,
            x in -3_000.0f64..3_000.0,
            y in -3_000.0f64..3_000.0,
            w in 0.0f64..5_000.0,
            h in 0.0f64..5_000.0,
        ) {
            let mut canvas = cached(count);
            let area = Rect::new(x, y, x + w, y + h);
            canvas.invalidate(area);
            for (key, tile) in &canvas.tiles {
                let grow = 1.0 / level_scale(key.level);
                let touches = key.bounds().intersect(area.inflate(grow, grow)).area() > 0.0
                    || area.inflate(grow, grow).contains(key.bounds().origin());
                proptest::prop_assert_eq!(tile.stale, touches, "{:?}", key);
            }
        }
    }

    proptest::proptest! {
        #![proptest_config(proptest::prelude::ProptestConfig::with_cases(48))]

        /// As [`valid_tiles_never_show_stale_content`], with tiles on
        /// worker threads: items change while tiles are out, tiles come
        /// back in later frames, and some become empty on their way.
        #[test]
        fn valid_tiles_never_show_stale_content_with_workers(
            threads in 1usize..3,
            ops in proptest::collection::vec(
                (0u8..5, 0u64..40, -300.0f64..900.0, -300.0f64..900.0, 4.0f64..200.0, 0.25f64..3.0),
                1..60,
            )
        ) {
            let mut canvas = Canvas::new()
                .with_limits(64, 10_000)
                .with_render_budget(UNLIMITED)
                .with_workers(TileWorkers::new(threads, CpuMode::Deterministic));
            let mut boxes = Boxes { bounds: HashMap::new(), painted: Vec::new() };
            let mut rendered: HashMap<TileKey, Vec<(ItemId, [i64; 4])>> = HashMap::new();
            let now = Instant::now();
            let mut last_view = viewport(Point::ZERO, 1.0);
            let check = |canvas: &Canvas, rendered: &HashMap<TileKey, Vec<(ItemId, [i64; 4])>>| {
                for (key, tile) in &canvas.tiles {
                    if !tile.stale {
                        let shown = match tile.texture {
                            Some(_) => rendered.get(key).cloned().unwrap_or_default(),
                            None => Vec::new(),
                        };
                        if shown != content_of(canvas, *key) {
                            return Err(format!("tile {key:?} is valid but stale"));
                        }
                    }
                }
                Ok(())
            };
            for (step, (op, id, x, y, size, zoom)) in ops.into_iter().enumerate() {
                let id = ItemId(id);
                match op {
                    0 | 1 => {
                        let bounds = Rect::new(x, y, x + size, y + size * 0.7);
                        canvas.upsert(id, id.0, bounds);
                        boxes.bounds.insert(id, bounds);
                    }
                    2 => {
                        canvas.remove(id);
                        boxes.bounds.remove(&id);
                    }
                    op => {
                        if op == 4 {
                            // Let the workers finish some.
                            std::thread::sleep(Duration::from_millis(2));
                        }
                        last_view = Viewport {
                            origin: Point::new(x, y),
                            zoom,
                            screen: Rect::new(0.0, 0.0, 800.0, 600.0),
                            scale_factor: 1.0,
                        };
                        let at = now + Duration::from_secs(step as u64);
                        canvas.frame(&mut Scene::new(), &last_view, at, &mut boxes);
                        for key in &canvas.last_rendered {
                            rendered.insert(*key, content_of(&canvas, *key));
                        }
                    }
                }
                proptest::prop_assert_eq!(check(&canvas, &rendered), Ok(()));
            }
            // Everything comes back in the end.
            let started = Instant::now();
            let at = now + Duration::from_secs(1_000);
            loop {
                let stats = canvas.frame(&mut Scene::new(), &last_view, at, &mut boxes);
                for key in &canvas.last_rendered {
                    rendered.insert(*key, content_of(&canvas, *key));
                }
                proptest::prop_assert_eq!(check(&canvas, &rendered), Ok(()));
                if stats.owed == 0 {
                    break;
                }
                proptest::prop_assert!(started.elapsed() < Duration::from_secs(60));
                std::thread::sleep(Duration::from_millis(1));
            }
        }
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
