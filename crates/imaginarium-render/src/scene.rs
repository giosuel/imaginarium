// SPDX-License-Identifier: AGPL-3.0-or-later

//! A recorded frame: drawing commands that every backend replays.

use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use vello_common::kurbo::{Affine, BezPath, Rect, Shape, Stroke};
use vello_common::paint::{Image, ImageSource};
use vello_common::peniko::{Color, FontData, ImageSampler};
use vello_cpu::{Glyph, Pixmap};

/// Curves are flattened to this tolerance (in the scene's units) when a
/// shape is turned into a path.
const PATH_TOLERANCE: f64 = 0.1;

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Command {
    Transform(Affine),
    FillRect {
        rect: Rect,
        color: Color,
    },
    Fill {
        path: BezPath,
        color: Color,
    },
    Stroke {
        path: BezPath,
        style: Stroke,
        color: Color,
    },
    PushClip(BezPath),
    PushOpacity(f32),
    Pop,
    Glyphs {
        font: FontData,
        size: f32,
        hint: bool,
        color: Color,
        /// Into [`Scene::coords`].
        coords: Range<usize>,
        /// Into [`Scene::glyphs`].
        glyphs: Range<usize>,
    },
    /// Draws texture `key`'s `width × height` texels into `dest`.
    DrawTexture {
        key: TextureKey,
        width: u16,
        height: u16,
        dest: Rect,
    },
}

/// A texture the renderer keeps across frames for its owner (a canvas
/// tile): rendered with [`Scene::render_texture`], drawn with
/// [`Scene::draw_texture`], released with [`Scene::free_texture`].
///
/// Keys are unique in the process ([`TextureKey::unique`]): one renderer
/// holds the textures of every owner (all canvases of a window), so keys
/// picked per owner would collide.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TextureKey(u64);

impl TextureKey {
    /// A key no other texture has.
    pub fn unique() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }

    pub(crate) fn id(self) -> u64 {
        self.0
    }
}

/// Work on textures, done before the scene is drawn, in order.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum TextureWork {
    /// Renders `scene` into texture `key` (`width × height`, transparent
    /// first), creating or resizing it.
    Render {
        key: TextureKey,
        width: u16,
        height: u16,
        scene: Scene,
    },
    /// Releases texture `key`.
    Free(TextureKey),
}

/// The attributes shared by a run of glyphs (see [`Scene::glyphs`]).
#[derive(Clone, Copy, Debug)]
pub struct GlyphRun<'a> {
    /// The font file and the face in it.
    pub font: &'a FontData,
    /// The font size, in the scene's units.
    pub size: f32,
    /// Variable-font coordinates (weight, optical size, …), normalized to
    /// −1…1 in 2.14 fixed point, as the font's axes are ordered.
    pub coords: &'a [i16],
    /// Snap outlines to the pixel grid vertically. Crisper UI text, but
    /// only for text drawn without zoom or rotation: animated zoom with
    /// hinting makes glyphs jump.
    pub hint: bool,
    /// The fill color.
    pub color: Color,
}

/// Drawing commands for one frame (or one tile, or one widget), in the order
/// they paint. Built once, replayed by the CPU and the GPU backend alike, so
/// a golden image made on the CPU tests the scene code the GPU runs.
///
/// Coordinates are in device pixels after the current transform
/// ([`Scene::set_transform`]).
///
/// ```
/// use imaginarium_render::{Scene, kurbo::{Circle, Rect}, peniko::color::palette::css};
///
/// let mut scene = Scene::new();
/// scene.fill(&Rect::new(10.0, 10.0, 90.0, 50.0), css::TEAL);
/// scene.stroke(&Circle::new((50.0, 50.0), 20.0), 2.0, css::BLACK);
/// assert_eq!(scene.len(), 2);
/// ```
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Scene {
    commands: Vec<Command>,
    /// The glyphs of all runs, one allocation per scene rather than per
    /// run.
    glyphs: Vec<Glyph>,
    coords: Vec<i16>,
    /// Texture renders and releases, done before the frame.
    textures: Vec<TextureWork>,
    open_layers: u32,
    /// The transform in effect at the end, restored after [`Scene::append`].
    current: Affine,
}

impl Scene {
    /// An empty scene.
    pub fn new() -> Self {
        Self::default()
    }

    /// Removes everything, keeping the allocation for the next frame.
    pub fn clear(&mut self) {
        self.commands.clear();
        self.glyphs.clear();
        self.coords.clear();
        self.textures.clear();
        self.open_layers = 0;
        self.current = Affine::IDENTITY;
    }

    /// The number of drawing commands.
    pub fn len(&self) -> usize {
        self.commands.len()
    }

    /// Whether nothing was drawn.
    pub fn is_empty(&self) -> bool {
        self.commands.is_empty()
    }

    /// Sets the transform for the commands that follow.
    pub fn set_transform(&mut self, transform: Affine) {
        self.current = transform;
        self.commands.push(Command::Transform(transform));
    }

    /// Fills `shape` with `color`.
    pub fn fill(&mut self, shape: &impl Shape, color: impl Into<Color>) {
        let color = color.into();
        match shape.as_rect() {
            Some(rect) => self.commands.push(Command::FillRect { rect, color }),
            None => self.commands.push(Command::Fill {
                path: shape.to_path(PATH_TOLERANCE),
                color,
            }),
        }
    }

    /// Strokes the outline of `shape`, `width` wide.
    pub fn stroke(&mut self, shape: &impl Shape, width: f64, color: impl Into<Color>) {
        self.commands.push(Command::Stroke {
            path: shape.to_path(PATH_TOLERANCE),
            style: Stroke::new(width),
            color: color.into(),
        });
    }

    /// Fills a run of glyphs. Positions are in the scene's units: `x` along
    /// the line, `y` the baseline (down is positive).
    pub fn glyphs(&mut self, run: GlyphRun<'_>, glyphs: impl IntoIterator<Item = Glyph>) {
        let first_glyph = self.glyphs.len();
        self.glyphs.extend(glyphs);
        if self.glyphs.len() == first_glyph {
            return;
        }
        let first_coord = self.coords.len();
        self.coords.extend_from_slice(run.coords);
        self.commands.push(Command::Glyphs {
            font: run.font.clone(),
            size: run.size,
            hint: run.hint,
            color: run.color,
            coords: first_coord..self.coords.len(),
            glyphs: first_glyph..self.glyphs.len(),
        });
    }

    /// Renders `content` into texture `key` (`width × height` pixels,
    /// transparent before), before this scene is drawn. The texture stays
    /// until [`Scene::free_texture`]; rendering again replaces its pixels.
    pub fn render_texture(&mut self, key: TextureKey, width: u16, height: u16, content: Scene) {
        self.textures.push(TextureWork::Render {
            key,
            width,
            height,
            scene: content,
        });
    }

    /// Draws texture `key` (`width × height` texels) into `dest`, in the
    /// current transform. A texture drawn at 1:1 on whole device pixels is
    /// copied exactly.
    pub fn draw_texture(&mut self, key: TextureKey, width: u16, height: u16, dest: Rect) {
        self.commands.push(Command::DrawTexture {
            key,
            width,
            height,
            dest,
        });
    }

    /// Releases texture `key` after this frame's work.
    pub fn free_texture(&mut self, key: TextureKey) {
        self.textures.push(TextureWork::Free(key));
    }

    /// Texture renders queued by this scene.
    pub fn texture_renders(&self) -> usize {
        self.textures
            .iter()
            .filter(|work| matches!(work, TextureWork::Render { .. }))
            .count()
    }

    pub(crate) fn texture_work(&self) -> &[TextureWork] {
        &self.textures
    }

    /// Clips what follows to `shape`, until the matching [`Scene::pop`].
    pub fn push_clip(&mut self, shape: &impl Shape) {
        self.open_layers += 1;
        self.commands
            .push(Command::PushClip(shape.to_path(PATH_TOLERANCE)));
    }

    /// Draws what follows with `opacity` (0 to 1), until the matching
    /// [`Scene::pop`].
    pub fn push_opacity(&mut self, opacity: f32) {
        self.open_layers += 1;
        self.commands
            .push(Command::PushOpacity(opacity.clamp(0.0, 1.0)));
    }

    /// Ends the latest [`Scene::push_clip`] or [`Scene::push_opacity`].
    /// Extra pops are ignored.
    pub fn pop(&mut self) {
        if self.open_layers > 0 {
            self.open_layers -= 1;
            self.commands.push(Command::Pop);
        }
    }

    /// Appends `other`'s commands, drawn with `transform` applied after
    /// their own (for composing widgets or tiles).
    pub fn append(&mut self, other: &Scene, transform: Affine) {
        // `other` starts with the identity transform.
        self.commands.push(Command::Transform(transform));
        let (glyph_base, coord_base) = (self.glyphs.len(), self.coords.len());
        self.glyphs.extend_from_slice(&other.glyphs);
        self.coords.extend_from_slice(&other.coords);
        self.textures.extend_from_slice(&other.textures);
        for command in &other.commands {
            self.commands.push(match command {
                Command::Transform(t) => Command::Transform(transform * *t),
                Command::Glyphs {
                    font,
                    size,
                    hint,
                    color,
                    coords,
                    glyphs,
                } => Command::Glyphs {
                    font: font.clone(),
                    size: *size,
                    hint: *hint,
                    color: *color,
                    coords: coords.start + coord_base..coords.end + coord_base,
                    glyphs: glyphs.start + glyph_base..glyphs.end + glyph_base,
                },
                other => other.clone(),
            });
        }
        // Layers `other` left open close at its end.
        for _ in 0..other.open_layers {
            self.commands.push(Command::Pop);
        }
        self.commands.push(Command::Transform(self.current));
    }

    pub(crate) fn commands(&self) -> &[Command] {
        &self.commands
    }

    pub(crate) fn open_layers(&self) -> u32 {
        self.open_layers
    }
}

/// What both Vello renderers can draw.
pub(crate) trait Sink {
    fn set_transform(&mut self, transform: Affine);
    fn set_color(&mut self, color: Color);
    fn fill_rect(&mut self, rect: &Rect);
    fn fill_path(&mut self, path: &BezPath);
    fn stroke_path(&mut self, path: &BezPath, style: &Stroke);
    fn push_clip(&mut self, path: &BezPath);
    fn push_opacity(&mut self, opacity: f32);
    fn pop(&mut self);
    fn fill_glyphs(
        &mut self,
        font: &FontData,
        size: f32,
        hint: bool,
        coords: &[i16],
        glyphs: &[Glyph],
    );
    fn draw_texture(&mut self, key: TextureKey, width: u16, height: u16, dest: &Rect);
}

/// A backend's drawing context with the resources its glyph caches live
/// in.
pub(crate) struct Target<'a, C, R> {
    pub(crate) context: &'a mut C,
    pub(crate) resources: &'a mut R,
    /// Draw glyphs from the backend's glyph atlas.
    pub(crate) glyph_atlas: bool,
    /// The textures the scene may draw.
    pub(crate) textures: Textures<'a>,
    /// Draws of textures that don't exist (skipped).
    pub(crate) missing: u32,
}

/// The textures a backend can draw.
pub(crate) enum Textures<'a> {
    /// None (inside a texture's own content).
    None,
    /// The CPU backend's pixmaps.
    Pixmaps(&'a HashMap<TextureKey, Arc<Pixmap>>),
    /// The GPU's, bound by key at render time. Drawing a key without a
    /// binding would fail the whole frame.
    Bound(&'a HashMap<TextureKey, crate::gpu::GpuTexture>),
}

/// Replays `scene` into a backend, with `base` applied before every
/// transform (the window's scale factor, say).
pub(crate) fn replay(scene: &Scene, sink: &mut impl Sink, base: Affine) {
    sink.set_transform(base);
    for command in scene.commands() {
        match command {
            Command::Transform(t) => sink.set_transform(base * *t),
            Command::FillRect { rect, color } => {
                sink.set_color(*color);
                sink.fill_rect(rect);
            }
            Command::Fill { path, color } => {
                sink.set_color(*color);
                sink.fill_path(path);
            }
            Command::Stroke { path, style, color } => {
                sink.set_color(*color);
                sink.stroke_path(path, style);
            }
            Command::PushClip(path) => sink.push_clip(path),
            Command::PushOpacity(opacity) => sink.push_opacity(*opacity),
            Command::Pop => sink.pop(),
            Command::DrawTexture {
                key,
                width,
                height,
                dest,
            } => sink.draw_texture(*key, *width, *height, dest),
            Command::Glyphs {
                font,
                size,
                hint,
                color,
                coords,
                glyphs,
            } => {
                sink.set_color(*color);
                sink.fill_glyphs(
                    font,
                    *size,
                    *hint,
                    &scene.coords[coords.clone()],
                    &scene.glyphs[glyphs.clone()],
                );
            }
        }
    }
    for _ in 0..scene.open_layers() {
        sink.pop();
    }
}

macro_rules! impl_sink {
    ($context:ty, $resources:ty) => {
        impl Sink for Target<'_, $context, $resources> {
            fn set_transform(&mut self, transform: Affine) {
                self.context.set_transform(transform);
            }
            fn set_color(&mut self, color: Color) {
                self.context.set_paint(color);
            }
            fn fill_rect(&mut self, rect: &Rect) {
                self.context.fill_rect(rect);
            }
            fn fill_path(&mut self, path: &BezPath) {
                self.context.fill_path(path);
            }
            fn stroke_path(&mut self, path: &BezPath, style: &Stroke) {
                self.context.set_stroke(style.clone());
                self.context.stroke_path(path);
            }
            fn push_clip(&mut self, path: &BezPath) {
                self.context.push_clip_layer(path);
            }
            fn push_opacity(&mut self, opacity: f32) {
                self.context.push_opacity_layer(opacity);
            }
            fn pop(&mut self) {
                self.context.pop_layer();
            }
            fn fill_glyphs(
                &mut self,
                font: &FontData,
                size: f32,
                hint: bool,
                coords: &[i16],
                glyphs: &[Glyph],
            ) {
                // A glyph the backend can't draw (a bitmap format it lacks)
                // is skipped; the rest of the run still draws.
                let _ = self
                    .context
                    .glyph_run(self.resources, font)
                    .font_size(size)
                    .hint(hint)
                    .normalized_coords(coords)
                    .atlas_cache(self.glyph_atlas)
                    .fill_glyphs(glyphs.iter().copied());
            }
            fn draw_texture(&mut self, key: TextureKey, width: u16, height: u16, dest: &Rect) {
                if width == 0 || height == 0 || dest.is_zero_area() {
                    return;
                }
                // A texture that was never rendered or is freed shows
                // nothing; the owner has a bug, but one frame of a missing
                // tile beats ending the program.
                let image = match self.textures {
                    Textures::Pixmaps(pixmaps) => match pixmaps.get(&key) {
                        Some(pixmap) => ImageSource::Pixmap(pixmap.clone()),
                        None => {
                            self.missing += 1;
                            return;
                        }
                    },
                    Textures::Bound(bound) if bound.contains_key(&key) => {
                        ImageSource::external_texture(
                            vello_common::TextureId(key.id()),
                            vello_common::geometry::RectU16 {
                                x0: 0,
                                y0: 0,
                                x1: width,
                                y1: height,
                            },
                            true,
                        )
                    }
                    Textures::Bound(_) | Textures::None => {
                        self.missing += 1;
                        return;
                    }
                };
                self.context.set_paint(Image {
                    image,
                    sampler: ImageSampler::default(),
                });
                // Texels to the destination rectangle.
                self.context.set_paint_transform(
                    Affine::translate(dest.origin().to_vec2())
                        * Affine::scale_non_uniform(
                            dest.width() / f64::from(width),
                            dest.height() / f64::from(height),
                        ),
                );
                self.context.fill_rect(dest);
                self.context.reset_paint_transform();
            }
        }
    };
}

impl_sink!(vello_cpu::RenderContext, vello_cpu::Resources);
impl_sink!(vello_gpu::Scene, vello_gpu::Resources);
