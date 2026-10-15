// SPDX-License-Identifier: AGPL-3.0-or-later

//! Rendering for the UI toolkit (ADR-0003, ADR-0007,
//! `docs/design/rendering.md`): every frame is one [`Scene`], drawn by
//! Vello on the GPU or the CPU.
//!
//! # Pieces
//! - [`Scene`]: recorded drawing commands (fills, strokes, clips, opacity
//!   layers, transforms, [`GlyphRun`]s of [`Glyph`]s) in device pixels.
//!   Widgets and, later, canvas tiles build scenes; both backends replay
//!   the same ones. Glyph outlines are cached by the backend across
//!   frames. Shaping and layout are `imaginarium-text`'s.
//! - [`WindowRenderer`]: `vello_gpu` on `wgpu`, drawing into a window
//!   surface. [`GpuOptions`] picks the GPU: the integrated one by default
//!   ([`Power::Low`]), the discrete one, or one by name. [`GpuInfo`]
//!   describes it for reports.
//! - [`OffscreenRenderer`]: the same on the GPU without a window, read back
//!   into a pixmap (screenshots, GPU-vs-CPU comparisons).
//! - [`CpuRenderer`]: `vello_cpu` into a pixmap. [`CpuMode::Deterministic`]
//!   gives the same bytes on every machine, for golden-image tests in CI,
//!   which has no GPU.
//! - [`kurbo`] (geometry) and [`peniko`] (colors, brushes): Vello's own
//!   types, re-exported, so callers use exactly the versions Vello uses.
//!
//! # Not here (yet)
//! - Display lists and the tile cache of the canvas (M2.5).
//! - Windows and input: `imaginarium-ui` creates the window and hands only
//!   its handle here, so this crate doesn't depend on `winit`.

pub mod canvas;
mod cpu;
mod error;
mod gpu;
mod scene;

pub use cpu::{CpuMode, CpuRenderer};
pub use error::RenderError;
pub use gpu::{
    FrameOutcome, FrameTiming, GpuInfo, GpuOptions, OffscreenRenderer, Power, WindowRenderer,
};
pub use scene::{GlyphRun, Scene, TextureKey};
pub use vello_common::{kurbo, peniko};
pub use vello_cpu::{Glyph, Pixmap};
