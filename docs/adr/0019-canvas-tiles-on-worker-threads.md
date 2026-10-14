# 0019. Canvas tiles render on CPU worker threads

- Status: Accepted (owner, 2026-10-14: remove the stalls and render tiles
  on worker threads, both in M2.8)
- Date: 2026-10-14

## Context
The gate's M2.8 run on the owner's laptop (Intel UHD Graphics, TGL GT1;
a 3456×2160 window, about 7.5 megapixels) held 60 Hz while panning and
dragging. Two problems remained (`docs/design/m2-gate.md`, "M2.8 run"):

- **Slow fill.** After a board switch, or zooming out to a level not
  cached, the screen filled with tiles over about 30 frames (0.5 s):
  the middle first, then ring by ring, with blank areas meanwhile.
- **Stalls.** Now and then a single submission of 3–4 tile renders took
  58–117 ms, although the GPU's work on them took under 1 ms.

Tiles were rendered on the UI thread, in the frame, within a time budget
(6 ms, less when the rest of the frame needs it). A tile cost about
1.8 ms there:

- about 0.5 ms painting its items into a scene;
- about 1.3 ms of Vello's work: `vello_gpu` flattens paths and builds its
  strips on the CPU too, and each tile was a GPU submission of its own.

So 1.4 to 3 tiles rendered per frame, while a fullscreen view needs up
to 228. The laptop has 16 hardware threads, and the UI thread used one.

`docs/design/rendering.md` §9 planned this ("`vello_cpu` can already use
several threads"), and ADR-0003 puts background work on a pool.

## Decision
1. **Canvas tiles render on tile worker threads, with `vello_cpu`**
   (`TileWorkers`, `Canvas::with_workers`). By default there are half as
   many threads as cores, at most 8.
   - The UI thread paints a tile's items into a scene and hands it over.
   - Frames show stand-ins until the tile's pixels come back, usually by
     the next frame, and upload them then (`Scene::upload_texture`).
   - For tiles nothing can stand in for, the frame that hands them over
     waits up to 4 ms for them instead (amended 2026-10-14, after the
     owner's second run).
   - The GPU still composites the tiles and draws everything else.
2. **On the GPU, uploaded tiles share pages**: 2048² textures holding 64
   tiles each, so a frame draws a page's tiles with one binding. Pages
   are made ahead of need, and cleared, on a thread of their own, as
   soon as the canvas hands tiles to the workers.
3. **The pool is the canvas's own.** It isn't the general rayon pool of
   ADR-0003, which doesn't exist yet. Tile threads live as long as the
   window, and each keeps its own `vello_cpu` context and glyph cache.
   The queue is first in, first out, with at most 8 tiles per thread at
   once (4 until the owner's second run), so it follows what is on
   screen.
4. **Without workers, tiles render in the frame on the GPU**, as before.
   This applies when none could start, or later in a browser build.

## Consequences
- **The UI thread only paints.** Its share of a tile is painting the
  items (0.1–0.5 ms on the VM) and the upload. The rendering, 0.5–1.6 ms
  per tile on the VM, runs on the workers, several tiles at once.
- **Two renderers.** Tiles are rendered by `vello_cpu` and the rest of
  the frame by `vello_gpu`. Both are Vello's sparse-strip renderers with
  one scene format, and the tests compare them. Anything tiles draw must
  work in `vello_cpu`: images and effects come to both.
- **Tiles arrive a frame later.** A tile shows from the frame after the
  one that painted its items. Until then, a stand-in shows: a stale copy
  or another level. For an item being moved, the drag lift already draws
  it live. Where there is no stand-in, the frame waits for the tile, up
  to 4 ms.
- **Memory.** Tile pages take GPU memory in 16 MB steps. Each worker
  keeps its own glyph cache.
- **CPU use.** While tiles fill, the workers use their threads. When
  nothing is owed they wait, so idle stays at zero frames and zero CPU
  (criterion 6).
- **Watch:** the UI thread's painting, which now limits the fill (cards'
  text layout most of all), and the frames' GPU time at 4K on the
  integrated GPU.

## Alternatives considered
- **`vello_gpu` on worker threads**, each with its own renderer and
  submitting its tiles:
  - every worker compiles Vello's pipelines and keeps a glyph atlas;
  - wgpu serializes submissions from several threads;
  - the integrated GPU, the slowest part at 4K (9 ms of GPU work per
    frame), would get the tiles' work too.
- **A bigger budget while the view is still.** It fills faster, but with
  long frames just when the user may start moving, and it still uses one
  core.
- **Rayon**, ADR-0003's pool. Per-thread renderer state needs another
  crate (`thread_local`), and on a shared pool tiles would wait behind
  other background work. Rayon isn't in the tree yet; the general pool
  can still use it.
