// SPDX-License-Identifier: AGPL-3.0-or-later

//! The CPU backend: `vello_cpu`.

use vello_common::kurbo::Affine;
use vello_common::peniko::Color;
use vello_cpu::{
    Level, Pixmap, RasterizerSettings, RenderContext, RenderMode, RenderSettings, Resources,
};

use std::collections::HashMap;
use std::sync::Arc;

use crate::scene::{Scene, Target, TextureKey, TextureWork, replay};

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
    /// Textures (canvas tiles) kept across frames.
    textures: HashMap<TextureKey, Arc<Pixmap>>,
}

impl CpuRenderer {
    /// A renderer in `mode`.
    pub fn new(mode: CpuMode) -> Self {
        Self {
            mode,
            resources: Resources::new(),
            textures: HashMap::new(),
        }
    }

    fn settings(&self) -> (RenderSettings, RenderMode) {
        match self.mode {
            CpuMode::Deterministic => (
                RenderSettings {
                    level: Level::baseline(),
                    num_threads: 0,
                },
                RenderMode::OptimizeQuality,
            ),
            CpuMode::Fast => (RenderSettings::default(), RenderMode::OptimizeSpeed),
        }
    }

    /// Draws `scene` into a new pixmap, cleared to `background`.
    fn draw(&mut self, scene: &Scene, width: u16, height: u16, background: Color) -> Pixmap {
        let (settings, render_mode) = self.settings();
        let mut context = RenderContext::new_with(width, height, settings);
        replay(
            scene,
            &mut Target {
                context: &mut context,
                resources: &mut self.resources,
                glyph_atlas: false,
                pixmaps: Some(&self.textures),
            },
            Affine::IDENTITY,
        );
        context.flush();
        let mut pixmap = Pixmap::new(width, height);
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

    /// Renders `scene` into a new `width × height` pixmap (premultiplied
    /// RGBA8), cleared to `background` first. Textures the scene renders
    /// are kept for later scenes.
    pub fn render(&mut self, scene: &Scene, width: u16, height: u16, background: Color) -> Pixmap {
        for work in scene.texture_work() {
            match work {
                TextureWork::Free(key) => {
                    self.textures.remove(key);
                }
                TextureWork::Render {
                    key,
                    width,
                    height,
                    scene,
                } => {
                    let pixmap =
                        self.draw(scene, (*width).max(1), (*height).max(1), Color::TRANSPARENT);
                    self.textures.insert(*key, Arc::new(pixmap));
                }
            }
        }
        self.draw(scene, width, height, background)
    }

    /// How many textures are kept.
    pub fn texture_count(&self) -> usize {
        self.textures.len()
    }
}
