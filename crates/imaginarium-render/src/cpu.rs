// SPDX-License-Identifier: AGPL-3.0-or-later

//! The CPU backend: `vello_cpu`.

use vello_common::kurbo::Affine;
use vello_common::peniko::Color;
use vello_cpu::{
    Level, Pixmap, RasterizerSettings, RenderContext, RenderMode, RenderSettings, Resources,
};

use std::collections::HashMap;
use std::sync::Arc;

use crate::scene::{Scene, Target, TextureKey, TextureWork, Textures, replay};

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

impl CpuMode {
    fn settings(self) -> (RenderSettings, RenderMode) {
        match self {
            Self::Deterministic => (
                RenderSettings {
                    level: Level::baseline(),
                    num_threads: 0,
                },
                RenderMode::OptimizeQuality,
            ),
            Self::Fast => (
                RenderSettings {
                    num_threads: 0,
                    ..RenderSettings::default()
                },
                RenderMode::OptimizeSpeed,
            ),
        }
    }

    /// A drawing context for `width × height` in this mode, on the calling
    /// thread.
    pub(crate) fn context(self, width: u16, height: u16) -> RenderContext {
        RenderContext::new_with(width, height, self.settings().0)
    }
}

/// Draws `scene` with `context` (reset first, at its size) into a new
/// pixmap, cleared to `background`.
pub(crate) fn draw(
    context: &mut RenderContext,
    resources: &mut Resources,
    mode: CpuMode,
    scene: &Scene,
    textures: Textures<'_>,
    background: Color,
) -> Pixmap {
    context.reset();
    replay(
        scene,
        &mut Target {
            context: &mut *context,
            resources: &mut *resources,
            glyph_atlas: false,
            bitmap_fonts: None,
            transform: Affine::IDENTITY,
            textures,
            missing: 0,
        },
        Affine::IDENTITY,
    );
    context.flush();
    let mut pixmap = Pixmap::new(context.width(), context.height());
    context.render_with(
        &mut pixmap,
        resources,
        RasterizerSettings {
            render_mode: mode.settings().1,
            target_init: vello_cpu::TargetInit::Clear(background),
            ..RasterizerSettings::default()
        },
    );
    pixmap
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

    /// Draws `scene` into a new pixmap, cleared to `background`.
    fn draw(&mut self, scene: &Scene, width: u16, height: u16, background: Color) -> Pixmap {
        let mut context = self.mode.context(width, height);
        draw(
            &mut context,
            &mut self.resources,
            self.mode,
            scene,
            Textures::Pixmaps(&self.textures),
            background,
        )
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
                TextureWork::Upload { key, pixmap } => {
                    self.textures.insert(*key, Arc::clone(&pixmap.0));
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
