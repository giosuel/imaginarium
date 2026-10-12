# 0003. Own render/text stack on wgpu + Vello + Parley

- Status: Proposed
- Date: 2026-10-12

## Context
Boards with 10k+ shapes, paginated documents with hundreds of pages, and
raster painting all need a renderer that we control end to end: tiling,
caching, invalidation and priorities. No GUI framework's built-in canvas
provides this.

## Decision
- **`tafel-render`** owns a retained per-node display list, a tile cache
  (256² tiles per zoom bucket) and a compositor. It exposes a backend trait
  with two implementations:
  - `GpuBackend`: Vello on wgpu (the default).
  - `CpuBackend`: vello_cpu or tiny-skia. Used for fallback, headless
    export and deterministic golden-image tests.
- **`tafel-text`** wraps Parley (layout), fontique (font discovery) and swash
  (shaping/rasterization) behind our own traits, with per-paragraph layout
  caches.
- Background work uses a rayon pool with two priorities (urgent and
  deferred). The core does not use an async runtime.

## Consequences
- We own a significant amount of rendering code. This is deliberate, because
  it is the product's core competency.
- Vello is pre-1.0, so we pin it and keep the CPU path working at all times.
