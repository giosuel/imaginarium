# Rendering: display lists, tiles, compositor, backends

- Status: **Implemented** in M2.5 (designed 2026-10-12 in M2.2). Accepted once the M2 gate passes.
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

## 10. What M2.5 implemented

`imaginarium-render::canvas` (and the texture commands of `Scene`):

- **Items** are an id, a z-order and bounds; the display list comes from a
  `CanvasContent` that paints an item into a tile on demand, with the
  level's scale, so content can pick a level of detail (cards drop their
  text below 30 %) without keeping display lists for every item.
- The **grid index**, **levels**, **tile keys**, **invalidation**
  (old ∪ new bounds plus a device pixel), the **budget** (24 tile renders
  per frame, nearest the middle first), **stand-ins** (a stale copy, or the
  nearest coarser cached level, clipped), the **zoom pause** (150 ms, or
  at once when tiles would be magnified), **LRU eviction** (1,000 tiles)
  and the **counters** are as above.
- **Tiles are textures:** `Scene::render_texture` renders a tile's scene
  into a texture the backend keeps, `Scene::draw_texture` draws it,
  `Scene::free_texture` releases it. On the GPU each tile render is its own
  submission (Vello rewrites its buffers between renders) and tiles are
  bound as external textures of the frame; the CPU backend keeps pixmaps,
  so golden images cover it. A tile drawn at 1:1 on whole device pixels
  equals drawing its items directly (tested).
- **Overlays** (hover outlines, the edited card, selection outlines) are
  drawn live on top and never dirty tiles. A **drag lifts** the selected
  shapes out of the tiles: their tiles render once without them, the
  shapes are drawn live while they move, and they go back into the tiles
  when the drag ends.
- **Texture rules** (from the first gate run, M2.7):
  - Texture keys are unique in the process (`TextureKey::unique`): one
    renderer holds the textures of every canvas of a window.
  - Texture work is done once, in the frame that first draws a scene: the
    widget tree moves a repainted widget's work into the frame
    (`Scene::move_texture_work`) and caches only the drawing.
  - The window renderer does the work before it acquires the surface, so
    a skipped frame (outdated surface, timeout) loses none of it.
  - A stand-in counts as drawn, so that frame's eviction keeps it.
  - A draw of a texture that doesn't exist draws nothing and is counted
    (`FrameTiming::missing_textures`, the gate report's
    `missing_textures`), on both backends.
  - Property test: every texture a frame draws exists when the renderer
    draws it, with two canvases sharing one renderer.
- Not yet: rendering tiles on worker threads, prefetching the margin ring,
  and partial tile updates.

Measured in the gate app (llvmpipe VM, M2.5): with tiles, a pan frame
encodes in 1.4 ms instead of 4.7 ms for the 10,000 shapes, and 1.6 ms
instead of 4.4 ms for the text cards; a drag renders at most 2 tiles per
frame (criterion 2); with the drag lift, 2 tile renders in 240 drag
frames.
