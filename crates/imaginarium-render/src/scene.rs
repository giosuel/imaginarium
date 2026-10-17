// SPDX-License-Identifier: AGPL-3.0-or-later

//! A recorded frame: drawing commands that every backend replays.

use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use vello_common::geometry::RectU16;
use vello_common::kurbo::{Affine, BezPath, Rect, Shape, Stroke};
use vello_common::paint::{Image, ImageSource};
use vello_common::peniko::{Color, FontData, ImageQuality, ImageSampler};
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
    /// A soft shadow: a rounded rectangle blurred by a Gaussian.
    Shadow {
        rect: Rect,
        radius: f32,
        std_dev: f32,
        color: Color,
    },
    /// A clip to an axis-aligned rectangle (in the current transform).
    PushClipRect(Rect),
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
    /// Makes `pixmap` texture `key`'s pixels, creating or resizing it.
    Upload {
        key: TextureKey,
        pixmap: SharedPixmap,
    },
    /// Releases texture `key`.
    Free(TextureKey),
    /// About this many uploads of new canvas tiles are coming in the next
    /// frames ([`Scene::expect_uploads`]).
    Expect(usize),
}

/// A pixmap a scene shares; scenes compare it by identity.
#[derive(Clone, Debug)]
pub(crate) struct SharedPixmap(pub(crate) Arc<Pixmap>);

impl PartialEq for SharedPixmap {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
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
    /// Under every transform set while [`Scene::paint_with`] runs.
    base: Affine,
    /// The layers open when [`Scene::paint_with`] began, which a pop in it
    /// can't end.
    floor: u32,
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
        self.base = Affine::IDENTITY;
        self.floor = 0;
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
        self.current = self.base * transform;
        self.commands.push(Command::Transform(self.current));
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

    /// A soft shadow: `rect` with corners of `radius`, blurred by a
    /// Gaussian of standard deviation `std_dev` (half a CSS blur radius),
    /// in `color`.
    pub fn shadow(&mut self, rect: Rect, radius: f64, std_dev: f64, color: impl Into<Color>) {
        self.commands.push(Command::Shadow {
            rect,
            radius: radius as f32,
            std_dev: std_dev.max(0.0) as f32,
            color: color.into(),
        });
    }

    /// Strokes the outline of `shape`, `width` wide.
    pub fn stroke(&mut self, shape: &impl Shape, width: f64, color: impl Into<Color>) {
        self.commands.push(Command::Stroke {
            path: shape.to_path(PATH_TOLERANCE),
            style: Stroke::new(width),
            color: color.into(),
        });
    }

    /// Strokes the outline of `shape` with a full stroke style (caps,
    /// joins, dashes), as line icons need.
    pub fn stroke_with(&mut self, shape: &impl Shape, style: &Stroke, color: impl Into<Color>) {
        self.commands.push(Command::Stroke {
            path: shape.to_path(PATH_TOLERANCE),
            style: style.clone(),
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

    /// Makes `pixmap` texture `key`'s pixels (premultiplied, as Vello
    /// renders them), before this scene is drawn: content rendered
    /// elsewhere, such as canvas tiles rendered on worker threads. The
    /// texture stays until [`Scene::free_texture`].
    pub fn upload_texture(&mut self, key: TextureKey, pixmap: Arc<Pixmap>) {
        self.textures.push(TextureWork::Upload {
            key,
            pixmap: SharedPixmap(pixmap),
        });
    }

    /// Tells the renderer that about `count` uploads of new canvas tiles
    /// (256² pixmaps, for textures that have none yet) are coming in the
    /// next frames, so it can make room for them ahead: on the GPU, that
    /// room takes a while to make. Say it again each frame with the number
    /// still to come; it doesn't add up.
    pub fn expect_uploads(&mut self, count: usize) {
        self.textures.push(TextureWork::Expect(count));
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

    /// Texture uploads queued by this scene.
    pub fn texture_uploads(&self) -> usize {
        self.textures
            .iter()
            .filter(|work| matches!(work, TextureWork::Upload { .. }))
            .count()
    }

    pub(crate) fn texture_work(&self) -> &[TextureWork] {
        &self.textures
    }

    /// Moves the texture work queued in this scene to the end of `to`'s.
    ///
    /// For scenes kept and appended again in later frames (a widget's
    /// cached drawing): the work must be done once, in the frame that
    /// first draws the scene. Appended again with its work, the scene
    /// would render its textures again in every frame.
    pub fn move_texture_work(&mut self, to: &mut Scene) {
        to.textures.append(&mut self.textures);
    }

    /// Clips what follows to `shape`, until the matching [`Scene::pop`].
    ///
    /// The clip applies to each thing drawn, not to a layer of them: it
    /// costs no extra pass on the GPU, and a rectangle on whole pixels
    /// costs nothing at all.
    pub fn push_clip(&mut self, shape: &impl Shape) {
        self.open_layers += 1;
        self.commands.push(match shape.as_rect() {
            Some(rect) => Command::PushClipRect(rect),
            None => Command::PushClip(shape.to_path(PATH_TOLERANCE)),
        });
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
        if self.open_layers > self.floor {
            self.open_layers -= 1;
            self.commands.push(Command::Pop);
        }
    }

    /// Appends `other`'s commands, drawn with `transform` applied after
    /// their own (for composing widgets or tiles).
    pub fn append(&mut self, other: &Scene, transform: Affine) {
        let transform = self.base * transform;
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

    /// Lets `paint` draw into this scene with `transform` under all it
    /// draws, as if it drew a scene of its own that is then appended with
    /// `transform` ([`Scene::append`]), without the copy: its transforms
    /// apply after `transform`, its pops can't end layers opened before,
    /// and layers it leaves open close at its end.
    ///
    /// ```
    /// use imaginarium_render::{Scene, kurbo::{Affine, Rect}, peniko::Color};
    ///
    /// let mut tile = Scene::new();
    /// tile.paint_with(Affine::scale(2.0), |item| {
    ///     item.push_clip(&Rect::new(0.0, 0.0, 10.0, 10.0));
    ///     item.fill(&Rect::new(1.0, 1.0, 4.0, 4.0), Color::BLACK);
    /// });
    /// let mut item = Scene::new();
    /// item.push_clip(&Rect::new(0.0, 0.0, 10.0, 10.0));
    /// item.fill(&Rect::new(1.0, 1.0, 4.0, 4.0), Color::BLACK);
    /// let mut appended = Scene::new();
    /// appended.append(&item, Affine::scale(2.0));
    /// assert_eq!(tile, appended);
    /// ```
    pub fn paint_with<R>(&mut self, transform: Affine, paint: impl FnOnce(&mut Self) -> R) -> R {
        let (base, floor, current) = (self.base, self.floor, self.current);
        self.base = base * transform;
        self.floor = self.open_layers;
        self.set_transform(Affine::IDENTITY);
        let painted = paint(self);
        while self.open_layers > self.floor {
            self.pop();
        }
        self.base = base;
        self.floor = floor;
        self.current = current;
        self.commands.push(Command::Transform(current));
        painted
    }

    pub(crate) fn commands(&self) -> &[Command] {
        &self.commands
    }
}

/// What both Vello renderers can draw.
pub(crate) trait Sink {
    fn set_transform(&mut self, transform: Affine);
    fn set_color(&mut self, color: Color);
    fn fill_rect(&mut self, rect: &Rect);
    fn fill_blurred(&mut self, rect: &Rect, radius: f32, std_dev: f32);
    fn fill_path(&mut self, path: &BezPath);
    fn stroke_path(&mut self, path: &BezPath, style: &Stroke);
    fn push_clip_rect(&mut self, rect: &Rect);
    fn push_clip(&mut self, path: &BezPath);
    fn pop_clip(&mut self);
    fn push_opacity(&mut self, opacity: f32);
    fn pop_layer(&mut self);
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
    /// The GPU's fonts with bitmap glyphs, which it can only draw from
    /// its glyph atlas; `None` on the CPU, which draws them directly.
    pub(crate) bitmap_fonts: Option<&'a mut crate::gpu::BitmapFonts>,
    /// The transform in effect (for glyphs' size on the device).
    pub(crate) transform: Affine,
    /// The textures the scene may draw.
    pub(crate) textures: Textures<'a>,
    /// Draws of textures that don't exist (skipped).
    pub(crate) missing: u32,
}

/// Where a texture's texels are on the GPU: a texture bound for the frame,
/// and the region of it (all of it, or a slot of a tile page).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Placed {
    pub(crate) id: vello_common::TextureId,
    pub(crate) region: RectU16,
}

/// The textures a backend can draw.
pub(crate) enum Textures<'a> {
    /// None (inside a texture's own content).
    None,
    /// The CPU backend's pixmaps.
    Pixmaps(&'a HashMap<TextureKey, Arc<Pixmap>>),
    /// The GPU's, bound at render time. Drawing a texture without a
    /// binding would fail the whole frame.
    Bound(&'a HashMap<TextureKey, Placed>),
}

/// Replays `scene` into a backend, with `base` applied before every
/// transform (the window's scale factor, say).
pub(crate) fn replay(scene: &Scene, sink: &mut impl Sink, base: Affine) {
    sink.set_transform(base);
    // What each open push was, for its pop: clips and layers pop apart.
    let mut open: Vec<bool> = Vec::new();
    let pop = |sink: &mut _, layer| {
        if layer {
            Sink::pop_layer(sink);
        } else {
            Sink::pop_clip(sink);
        }
    };
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
            Command::Shadow {
                rect,
                radius,
                std_dev,
                color,
            } => {
                sink.set_color(*color);
                sink.fill_blurred(rect, *radius, *std_dev);
            }
            Command::Stroke { path, style, color } => {
                sink.set_color(*color);
                sink.stroke_path(path, style);
            }
            Command::PushClipRect(rect) => {
                sink.push_clip_rect(rect);
                open.push(false);
            }
            Command::PushClip(path) => {
                sink.push_clip(path);
                open.push(false);
            }
            Command::PushOpacity(opacity) => {
                sink.push_opacity(*opacity);
                open.push(true);
            }
            Command::Pop => {
                if let Some(layer) = open.pop() {
                    pop(sink, layer);
                }
            }
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
    while let Some(layer) = open.pop() {
        pop(sink, layer);
    }
}

/// The largest glyph (pixels per em) vello_gpu 0.3 keeps in its glyph atlas
/// (glifo's default `max_cached_font_size`, not configurable there).
const MAX_ATLAS_GLYPH_SIZE: f32 = 128.0;

/// A font size in device pixels under `transform`.
fn device_size(size: f32, transform: Affine) -> f32 {
    let [a, b, c, d, _, _] = transform.as_coeffs();
    size * (a * d - b * c).abs().sqrt() as f32
}

/// How to sample a texture whose texels map to device pixels by
/// `texels`: one texel per pixel on whole pixels needs no filtering, and the
/// nearest texel is the same as filtering there. On the GPU that is one
/// load per pixel instead of four (canvas tiles cover the window).
fn sampling(texels: Affine) -> ImageQuality {
    let [a, b, c, d, x, y] = texels.as_coeffs();
    let near = |value: f64, to: f64| (value - to).abs() < 1e-6;
    if near(a, 1.0)
        && near(d, 1.0)
        && near(b, 0.0)
        && near(c, 0.0)
        && near(x, x.round())
        && near(y, y.round())
    {
        ImageQuality::Low
    } else {
        ImageQuality::Medium
    }
}

macro_rules! impl_sink {
    ($context:ty, $resources:ty) => {
        impl Sink for Target<'_, $context, $resources> {
            fn set_transform(&mut self, transform: Affine) {
                self.transform = transform;
                self.context.set_transform(transform);
            }
            fn set_color(&mut self, color: Color) {
                self.context.set_paint(color);
            }
            fn fill_rect(&mut self, rect: &Rect) {
                self.context.fill_rect(rect);
            }
            fn fill_blurred(&mut self, rect: &Rect, radius: f32, std_dev: f32) {
                self.context
                    .fill_blurred_rounded_rect(rect, radius, std_dev, false);
            }
            fn fill_path(&mut self, path: &BezPath) {
                self.context.fill_path(path);
            }
            fn stroke_path(&mut self, path: &BezPath, style: &Stroke) {
                self.context.set_stroke(style.clone());
                self.context.stroke_path(path);
            }
            fn push_clip_rect(&mut self, rect: &Rect) {
                self.context.push_clip_rect(rect);
            }
            fn push_clip(&mut self, path: &BezPath) {
                self.context.push_clip_path(path);
            }
            fn pop_clip(&mut self) {
                self.context.pop_clip();
            }
            fn push_opacity(&mut self, opacity: f32) {
                self.context.push_opacity_layer(opacity);
            }
            fn pop_layer(&mut self) {
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
                // vello_gpu draws bitmap glyphs (color emoji) only from its
                // glyph atlas, and panics on any other. The atlas takes
                // glyphs up to MAX_ATLAS_GLYPH_SIZE pixels per em: bigger ones
                // are cached at that size and scaled up. (Emoji bitmaps are
                // about 109 pixels per em, so that loses nothing.) The
                // 0.1 % stretch keeps the scale out of the font size, into
                // which glifo folds uniform scales before it checks the
                // limit.
                let bitmaps = self
                    .bitmap_fonts
                    .as_deref_mut()
                    .is_some_and(|fonts| fonts.has_bitmaps(font));
                let run = self
                    .context
                    .glyph_run(self.resources, font)
                    .hint(hint)
                    .normalized_coords(coords)
                    .atlas_cache(self.glyph_atlas || bitmaps);
                let run = if bitmaps && device_size(size, self.transform) > MAX_ATLAS_GLYPH_SIZE {
                    let scale = f64::from(size / MAX_ATLAS_GLYPH_SIZE);
                    run.font_size(MAX_ATLAS_GLYPH_SIZE)
                        .glyph_transform(Affine::scale_non_uniform(scale, scale * 1.001))
                } else {
                    run.font_size(size)
                };
                // A glyph the backend can't draw otherwise is skipped; the
                // rest of the run still draws.
                let _ = run.fill_glyphs(glyphs.iter().copied());
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
                    Textures::Bound(bound) => match bound.get(&key) {
                        Some(placed) => {
                            ImageSource::external_texture(placed.id, placed.region, true)
                        }
                        None => {
                            self.missing += 1;
                            return;
                        }
                    },
                    Textures::None => {
                        self.missing += 1;
                        return;
                    }
                };
                // Texels to the destination rectangle.
                let texels = Affine::translate(dest.origin().to_vec2())
                    * Affine::scale_non_uniform(
                        dest.width() / f64::from(width),
                        dest.height() / f64::from(height),
                    );
                self.context.set_paint(Image {
                    image,
                    sampler: ImageSampler {
                        quality: sampling(self.transform * texels),
                        ..ImageSampler::default()
                    },
                });
                self.context.set_paint_transform(texels);
                self.context.fill_rect(dest);
                self.context.reset_paint_transform();
            }
        }
    };
}

impl_sink!(vello_cpu::RenderContext, vello_cpu::Resources);
impl_sink!(vello_gpu::Scene, vello_gpu::Resources);

#[cfg(test)]
mod tests {
    use super::*;

    /// An item that sets its own transform, draws glyphs, pops more than
    /// it pushed and leaves a layer open.
    fn item(scene: &mut Scene, font: &FontData) {
        scene.pop();
        scene.push_opacity(0.5);
        scene.fill(&Rect::new(0.0, 0.0, 4.0, 4.0), Color::BLACK);
        scene.set_transform(Affine::translate((3.0, 1.0)));
        scene.push_clip(&Rect::new(0.0, 0.0, 2.0, 2.0));
        scene.pop();
        scene.glyphs(
            GlyphRun {
                font,
                size: 12.0,
                coords: &[1, 2],
                hint: false,
                color: Color::WHITE,
            },
            [Glyph {
                id: 7,
                x: 1.0,
                y: 2.0,
            }],
        );
    }

    /// A scene with a layer open and a transform of its own.
    fn started() -> Scene {
        let mut scene = Scene::new();
        scene.push_clip(&Rect::new(0.0, 0.0, 100.0, 100.0));
        scene.set_transform(Affine::rotate(0.1));
        scene.fill(&Rect::new(0.0, 0.0, 1.0, 1.0), Color::BLACK);
        scene
    }

    #[test]
    fn painting_in_place_is_appending_without_the_copy() {
        let font = FontData::new(vello_common::peniko::Blob::new(Arc::new([0u8; 4])), 0);
        let transform = Affine::scale(2.0) * Affine::translate((5.0, 0.0));
        let mut drawn = Scene::new();
        item(&mut drawn, &font);
        let mut appended = started();
        appended.append(&drawn, transform);
        appended.append(&drawn, transform);
        let mut painted = started();
        painted.paint_with(transform, |scene| item(scene, &font));
        painted.paint_with(transform, |scene| item(scene, &font));
        assert_eq!(painted, appended);
        assert_eq!(painted.open_layers, 1, "the outer clip is still open");
        // Nested, the inner transform comes after the outer.
        let mut nested = started();
        nested.paint_with(Affine::scale(2.0), |scene| {
            scene.paint_with(Affine::translate((5.0, 0.0)), |scene| item(scene, &font));
        });
        let mut inner = Scene::new();
        inner.append(&drawn, Affine::translate((5.0, 0.0)));
        let mut twice = started();
        twice.append(&inner, Affine::scale(2.0));
        assert_eq!(nested, twice);
    }
}
