# Rendering: display lists, tiles, compositor, backends

- Status: **Draft** (2026-10-12, M2.2). Accepted once the M2 gate passes.
- Implements ADR-0003 (own render stack) for the UI toolkit of ADR-0007.
  Versions and crate choices: [`m2-ui-stack.md`](m2-ui-stack.md).
- Crate: `imaginarium-render` (toolkit layer). It knows nothing about the
  document model: the canvas crate (M3) turns nodes into items, and the M2
  gate app builds items from the test fixtures.

## 1. Goals

From ADR-0007's gate and PLAN §10:

1. 10,000 shapes pan and zoom at 60 fps or more on an integrated GPU,
   p99 frame time under 16 ms.
2. Dragging re-renders only the tiles the change touched, and this is
   countable (the gate asserts it).
3. Panning re-rasterizes nothing that is already cached.
4. Nothing is drawn when nothing changed (idle ≈ 0 % CPU).
5. The same scenes render on the CPU, deterministically, for golden-image
   tests in CI (which has no GPU) and for export.

## 2. Coordinate spaces

| Space | Unit | Used for |
|---|---|---|
| **Document** | points (`f64`) | items, their bounds, the spatial index |
| **Level** | device pixels at a zoom level's scale | tiles: tile `(x, y)` covers level pixels `[256x, 256x+256) × [256y, 256y+256)` |
| **Screen** | device pixels of the window | the compositor |

The viewport maps document to screen: `screen = scale · (doc − origin)`,
where `scale` = zoom × the window's scale factor (HiDPI). Tiles are
rasterized at a **level** scale close to `scale`, and the compositor draws
them scaled by `scale / level_scale`.

## 3. Items and display lists

The renderer holds a set of **items**, each with:

- an `ItemId` (chosen by the caller, e.g. a node id);
- a **z-order key** (`u64`, sparse, so the canvas can insert between
  neighbours; a re-key is an update of the items involved);
- a **display list**: draw operations in document coordinates, shared as
  `Arc<[DrawOp]>` so building scenes never copies them;
- **bounds**: the document-space rectangle its pixels can touch,
  computed from the operations (path bounds, plus half the stroke width,
  plus the miter limit for sharp joins). The renderer relies on it: a
  display list never paints outside its bounds.

`DrawOp` (M2): `Fill` and `Stroke` of a `kurbo` path with a `peniko`
brush and a transform, `PushClip`/`PopClip`, `PushLayer`/`PopLayer`
(opacity and blend mode). Glyph runs come with M2.3, images with M4. The
types are Vello's own (`kurbo` 0.13, `peniko` 0.6), so a backend encodes
them directly.

A **uniform grid** in document space (cell size configurable, 512 pt by
default) indexes items by bounds, for "which items touch this tile". An
item lives in every cell its bounds cover. That is simple, fast for
boards (items are small relative to cells), and update-friendly (no
rebalancing). Very large items, such as frame backgrounds, cover many
cells; if that shows up in profiles, they move to a separate list.

## 4. Tiles and levels

- **Tile size:** 256 × 256 device pixels (ADR-0003).
- **Levels:** half-octave steps, `level_scale(L) = 2^(L/2)`, L an integer
  (L = 0 is 1:1). For a viewport scale `s`, the level is the smallest L
  with `level_scale(L) ≥ s`, so tiles are shown at a factor in
  `(1/√2, 1]`: never magnified, at most √2 smaller. Memory cost: at most
  2× the screen's pixels per cached screenful, compared with 4× for
  octave steps.
- **Key:** `TileKey { level, x, y }`.
- **Seams:** every tile renders all geometry that touches it, clipped to
  its square, at an integer pixel offset. Antialiasing is then identical
  on both sides of a tile edge, and adjacent tiles fit without seams.
  The compositor draws tiles at whole device pixels when the viewport
  scale equals the level scale (the usual case outside a zoom gesture),
  so panning never resamples.

## 5. Invalidation

A change to an item (insert, update, remove, re-key) dirties its **old
bounds ∪ new bounds**, grown by one device pixel for antialiasing. For
every level that has cached tiles, the tiles intersecting that region are
marked **stale**. Nothing else is touched, so:

- dragging a 100 pt shape at 1:1 dirties the 1 to 4 tiles under its old
  and new positions (criterion 2);
- panning dirties nothing; only tiles newly entering the viewport are
  missing (criterion 3).

Stale tiles keep their pixels, which the compositor may show for one more
frame while a non-urgent re-render waits (section 6). A dirty region is
also kept per level that has no cached tiles yet, so a level that is
built later doesn't need it.

Later optimization, not in M2: while an item is being dragged, draw it
live on top of the tiles (as an overlay) and bake it into the tiles only
when the drag ends. Then a drag re-renders only the tiles the item left.

## 6. Frames: what the compositor does

Per frame, for the current viewport:

1. **Visible tiles** at the current level, plus a one-tile margin ring
   (prefetch for panning).
2. For each visible tile:
   - **valid** in the cache: draw it;
   - **stale or missing**: rasterize it if the frame's **raster budget**
     allows (default 6 ms; the tiles nearest the viewport centre, or the
     pointer during a drag, go first). Otherwise draw a **stand-in**: the
     same area from another cached level, scaled (a stale copy of the
     tile itself counts), so the screen is never blank. Tiles still
     owed get another frame, immediately.
3. **Overlays** (selection handles, guides, the dragged item later) and
   the **panels** are drawn on top, every frame, directly into the final
   scene. They are cheap and change often.
4. If nothing changed and no tile is owed, **no frame is drawn**.

During a continuous zoom (a pinch or wheel zoom), the level doesn't change
on every frame: cached tiles are drawn scaled, and the level switches
when the gesture pauses (about 150 ms without zoom input) or the scale
leaves `[level_scale(L) / 2, level_scale(L) · 2]`. Then visible tiles of
the new level are rasterized within the budget, with the old ones as
stand-ins.

**Counters** per frame, for the gate report and Tracy: tiles rasterized,
tiles composited, stand-ins drawn, items encoded, raster time.

## 7. The tile cache

- Keyed by `TileKey`; each entry holds its pixels (a GPU texture or a CPU
  pixmap) and a **stale** flag.
- A **memory budget** (default 256 MB of tile pixels, about 1,000 tiles
  at 256 KiB each). A 4K screen needs about 135 visible tiles plus the
  margin ring, so the budget holds several screenfuls and other levels.
- **Eviction:** least recently composited first, never a tile that is
  visible this frame.

## 8. Backends

```text
trait Backend {
    type Tile;                                 // texture or pixmap
    fn render_tile(&mut self, ops: TileScene<'_>, into: &mut Self::Tile);
    fn present(&mut self, frame: Frame<'_, Self::Tile>);
}
```

- **`CpuBackend`** (`vello_cpu`, the `f32` pipeline for determinism): tile
  pixmaps, and a frame pixmap for tests and export.
- **`GpuBackend`** (`vello_gpu` on `wgpu`): tile textures. Whether a
  `vello_gpu` scene can draw our tile textures as images, or whether the
  compositor draws tiles with its own small `wgpu` pass and Vello draws
  the panels on top, is settled in part 2 of M2.2.
- A **recording backend** for tests records which items were encoded
  into which tiles, so invalidation and budget logic are tested without
  pixels.

## 9. Threads

M2 rasterizes on the UI thread within the per-frame budget. Prefetching
off-screen tiles and other levels moves to the worker pool later (PLAN
§4.3): tile rendering is independent per tile, and `vello_cpu` can
already use several threads. Nothing in the design assumes threads, which
also keeps a browser (wasm) build possible.

## 10. What M2.2 delivers, and in which order

1. **Part 1** (no rasterizer): items, display lists, bounds, the grid
   index, tile and level math, invalidation, the cache's bookkeeping and
   budget, frame planning (visible tiles, budget, stand-ins), counters, and
   the recording backend. Property tests: no change ever leaves a valid
   tile showing stale content; panning invalidates nothing; the plan
   covers the viewport. Benchmarks on `BoardFixture::LARGE`.
2. **Part 2** (from 2026-10-16, `vello_cpu`/`vello_gpu` 0.3): the CPU
   backend with golden images, then the GPU backend, then the first
   pictures of the 10k-shape board.
