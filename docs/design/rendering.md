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

Since M2.8, tiles render on **tile worker threads** with `vello_cpu`
(ADR-0019; `TileWorkers`, `Canvas::with_workers`):

1. The UI thread paints a tile's items into a scene, within the frame's
   budget, and hands it over. The content stays on the UI thread: it
   isn't shared between threads.
2. A worker renders the scene into a pixmap and sends it back.
3. Until then, frames show a stand-in. The frame after the tile comes
   back uploads it (`Scene::upload_texture`) and draws it.
4. A tile nothing can stand in for (newly in view, or every tile when a
   board opens) would show blank until then. For those, the frame that
   hands them over waits up to 4 ms, within what the rest of the frame
   leaves of 12 ms, and draws the ones back by then
   (`Canvas::with_blank_wait`).
5. Each thread has at most 8 tiles at once, so the queue follows what is
   on screen.
6. On the GPU, uploaded tiles share pages of 64. The canvas says how many
   new tiles are coming (`Scene::expect_uploads`), so pages are made on a
   thread of their own while the workers render.

Without workers, tiles render in the frame on the GPU, as in M2.5. That
also keeps a browser (wasm) build possible. Prefetching off-screen tiles
and other levels comes later, on the same workers.

## 10. What M2.5 implemented

`imaginarium-render::canvas` (and the texture commands of `Scene`):

- **Items** are an id, a z-order and bounds; the display list comes from a
  `CanvasContent` that paints an item into a tile on demand, with the
  level's scale, so content can pick a level of detail (cards drop their
  text below 30 %) without keeping display lists for every item.
- The **grid index**, **levels**, **tile keys**, **invalidation**
  (old ∪ new bounds plus a device pixel), the **budget**, **stand-ins**,
  the **zoom pause** (150 ms, or at once when tiles would be magnified),
  **LRU eviction** (1,000 tiles) and the **counters** are as above.
- **The budget is time** (since M2.7; 24 tiles per frame before): tile
  work gets 6 ms per frame, at least one tile, at most 64. The cost is
  estimated per unit, an item painted or a tile's fixed cost of 50 units
  (the gate laptop: about 12 µs per unit, in panning and zooming alike),
  and follows the renderer's measured time for the last frame's tiles
  (`Canvas::note_render_time`, from `FrameTiming::textures`). Once a tile
  doesn't fit, the rest wait. Since M2.8, tile work also leaves room for
  the rest of the frame (painting, encoding, submitting;
  `Canvas::note_rest_of_frame`, which leaves out the canvas's own part):
  it gets 12 ms minus that, between 1 and 6 ms. A present that blocks doesn't count (`FrameTiming::present`; on
  the VM's software GPU it waits 12–14 ms for the frame). A fullscreen frame on the gate laptop encoded for 4–5 ms, and the
  full 6 ms on top made the p99 17.5–17.8 ms.
- **Stand-ins:** a stale copy, the nearest coarser cached level (zooming
  in) or the nearest finer level's cached tiles (zooming out), clipped.
  Tiles with no stand-in render first, then stale ones, then those another
  level stands in for; nearest the middle first within each.
- **Empty tiles** need no texture: valid at once, nothing drawn, no budget.
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
- **Color emoji on the GPU:** vello_gpu 0.3 draws bitmap glyphs (CBDT,
  sbix, EBDT) only from its glyph atlas, up to 128 pixels per em, and
  panics on any other. Runs of fonts with bitmap tables always use the
  atlas on the GPU, and bigger runs are cached at 128 and scaled up (emoji
  bitmaps are 109 pixels per em). Still unsupported: rotated or skewed
  bitmap glyphs (the atlas refuses them), which nothing draws yet.
- Not yet: prefetching the margin ring, and partial tile updates.

Measured in the gate app (llvmpipe VM, M2.5): with tiles, a pan frame
encodes in 1.4 ms instead of 4.7 ms for the 10,000 shapes, and 1.6 ms
instead of 4.4 ms for the text cards; a drag renders at most 2 tiles per
frame (criterion 2); with the drag lift, 2 tile renders in 240 drag
frames.

**Since M2.8 (the canvas on the integrated GPU, from the gate's fourth
run):**
- **Clips aren't layers.** `Scene::push_clip` used Vello clip layers.
  Each one costs a layer draw, a composite and a clear on the GPU. The
  canvas had two over the whole viewport every frame, one per stand-in,
  and one per card.
  - Clips are now Vello's clip rectangles (free on whole pixels) and clip
    paths, which clip each thing drawn.
  - Only `push_opacity` makes a layer.
  - On the VM's llvmpipe at 1280×800 this halved the GPU's time per
    frame (shapes pan 25 → 12 ms, cards pan 26 → 14 ms).
- **Glyph atlas pages are 2048²** (Vello's default is 4096², 64 MB).
  - A page is created when a glyph first needs it, and wgpu clears the
    whole texture on its first use.
  - On the VM the first tile frame with text stalled in one submit for
    71 ms; with 16 MB pages it stalls for 23 ms, and peak memory drops by
    51 MB.
- **The window surface is opaque** where offered. With premultiplied
  alpha, a compositor blends (and maybe blurs) what is behind the window
  every frame.
- **The GPU's own time:**
  - With `GpuOptions::gpu_times`, timestamp queries around each tile
    render and the frame's render are read back a few frames later
    without blocking. `FrameTiming::gpu` reports tiles, scene and span by
    frame number.
  - `FrameTiming::textures_create` and `textures_submit` split the tile
    work's CPU time.
- **Tiles render on worker threads** (section 9, ADR-0019), from the
  gate's M2.8 run: a fullscreen board filled over about 30 frames,
  ring by ring, and the UI thread rendered 1.4–3 tiles per frame.
  - The workers have at most 4 tiles per thread at once; the rest wait
    for a later frame.
  - A tile whose items change while a worker has it comes back stale and
    goes again.
  - A tile a worker fails to render is left empty, not tried again in
    every frame.
  - The time budget now covers the UI thread's part: painting the items
    (a tile's fixed cost is 8 units instead of 50) and the uploads.
  - On the VM (4 threads, 1280×800) the shapes pan's tiles filled in
    47 ms instead of 196 ms.
- **Tile pages:** on the GPU, uploaded 256² textures share 2048² pages
  (64 tiles, 16 MB). A frame draws a page's tiles with one binding,
  instead of a draw and a bind group per tile. Pages are made ahead of
  need on a thread of their own and cleared there: a screenful at the
  first upload, then one in reserve.
- **Smaller GPU memory blocks** (wgpu's `MemoryUsage` hint: 8–64 MB
  rather than 128–256 MB). The laptop's Intel GPU stalled in single tile
  submissions for 58–117 ms. The likely cause is new memory blocks backed
  when a submission first uses them; that isn't proven.
- **Nearest sampling at 1:1:** a texture drawn one texel per pixel on
  whole pixels is sampled nearest. That gives the same pixels, with one
  load per pixel on the GPU instead of four.
- **Items paint straight into the tile's scene** (`Scene::paint_with`),
  without a scene of their own that is then copied. A dense zoomed-out
  tile's painting took 0.31 ms and now takes 0.21–0.23 ms on the VM.
- **The glyph atlas takes up to 32 pages** (Vello's limit of 8 assumed
  4096² pages). A full atlas would draw glyphs directly, which vello_gpu
  can't do for color emoji.
