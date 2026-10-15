// SPDX-License-Identifier: AGPL-3.0-or-later

//! The CPU backend: `vello_cpu`.

use vello_common::kurbo::Affine;
use vello_common::peniko::Color;
use vello_cpu::{
    Level, Pixmap, RasterizerSettings, RenderContext, RenderMode, RenderSettings, Resources,
};

use crate::scene::{Scene, replay};

/// How the CPU backend trades speed against reproducibility.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CpuMode {
    /// The same bytes on every machine: no SIMD, the `f32` pipeline. For
    /// golden-image tests.
    #[default]
    Deterministic,
    /// The fastest settings this CPU supports, for drawing on screen.
    Fast,
}

/// Renders scenes into pixmaps on the CPU (`vello_cpu`): the fallback when
/// there is no usable GPU, headless export, and golden-image tests (CI has
/// no GPU).
pub struct CpuRenderer {
    mode: CpuMode,
    resources: Resources,
}

impl CpuRenderer {
    /// A renderer in `mode`.
    pub fn new(mode: CpuMode) -> Self {
        Self {
            mode,
            resources: Resources::new(),
        }
    }

    /// Renders `scene` into a new `width × height` pixmap (premultiplied
    /// RGBA8), cleared to `background` first.
    pub fn render(&mut self, scene: &Scene, width: u16, height: u16, background: Color) -> Pixmap {
        let settings = match self.mode {
            CpuMode::Deterministic => RenderSettings {
                level: Level::baseline(),
                num_threads: 0,
            },
            CpuMode::Fast => RenderSettings::default(),
        };
        let mut context = RenderContext::new_with(width, height, settings);
        replay(scene, &mut context, Affine::IDENTITY);
        context.flush();
        let mut pixmap = Pixmap::new(width, height);
        let render_mode = match self.mode {
            CpuMode::Deterministic => RenderMode::OptimizeQuality,
            CpuMode::Fast => RenderMode::OptimizeSpeed,
        };
        context.render_with(
            &mut pixmap,
            &mut self.resources,
            RasterizerSettings {
                render_mode,
                target_init: vello_cpu::TargetInit::Clear(background),
                ..RasterizerSettings::default()
            },
        );
        pixmap
    }
}
