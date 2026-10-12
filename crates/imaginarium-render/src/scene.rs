// SPDX-License-Identifier: AGPL-3.0-or-later

//! A recorded frame: drawing commands that every backend replays.

use vello_common::kurbo::{Affine, BezPath, Rect, Shape, Stroke};
use vello_common::peniko::Color;

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
        for command in &other.commands {
            self.commands.push(match command {
                Command::Transform(t) => Command::Transform(transform * *t),
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
        }
    }
    for _ in 0..scene.open_layers() {
        sink.pop();
    }
}

macro_rules! impl_sink {
    ($target:ty) => {
        impl Sink for $target {
            fn set_transform(&mut self, transform: Affine) {
                <$target>::set_transform(self, transform);
            }
            fn set_color(&mut self, color: Color) {
                self.set_paint(color);
            }
            fn fill_rect(&mut self, rect: &Rect) {
                <$target>::fill_rect(self, rect);
            }
            fn fill_path(&mut self, path: &BezPath) {
                <$target>::fill_path(self, path);
            }
            fn stroke_path(&mut self, path: &BezPath, style: &Stroke) {
                self.set_stroke(style.clone());
                <$target>::stroke_path(self, path);
            }
            fn push_clip(&mut self, path: &BezPath) {
                self.push_clip_layer(path);
            }
            fn push_opacity(&mut self, opacity: f32) {
                self.push_opacity_layer(opacity);
            }
            fn pop(&mut self) {
                self.pop_layer();
            }
        }
    };
}

impl_sink!(vello_cpu::RenderContext);
impl_sink!(vello_gpu::Scene);
