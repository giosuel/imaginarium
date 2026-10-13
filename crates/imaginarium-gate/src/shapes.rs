// SPDX-License-Identifier: AGPL-3.0-or-later

//! 10,000 shapes for criteria 1 and 2: rectangles, rounded rectangles,
//! ellipses, triangles and stars in a loose grid, each draggable.

use imaginarium_render::Scene;
use imaginarium_render::kurbo::{BezPath, Ellipse, Point, Rect, RoundedRect, Vec2};
use imaginarium_render::peniko::Color;

const STROKE: f64 = 2.0;
const PITCH: f64 = 64.0;
const INK: Color = Color::from_rgb8(0x1f, 0x23, 0x30);
const FILLS: [Color; 8] = [
    Color::from_rgb8(0xf4, 0x7c, 0x7c),
    Color::from_rgb8(0xf7, 0xc8, 0x5c),
    Color::from_rgb8(0x8f, 0xd1, 0x9e),
    Color::from_rgb8(0x6c, 0xb4, 0xee),
    Color::from_rgb8(0xb3, 0x92, 0xf0),
    Color::from_rgb8(0xf2, 0x9e, 0xc8),
    Color::from_rgb8(0x5f, 0xc8, 0xc0),
    Color::from_rgb8(0xa8, 0xb4, 0xc4),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Rect,
    Rounded,
    Ellipse,
    Triangle,
    Star,
}

/// One shape: its kind, box and fill.
#[derive(Clone, Debug)]
pub(crate) struct Shape {
    kind: Kind,
    pub(crate) rect: Rect,
    fill: Color,
    pub(crate) z: u64,
}

impl Shape {
    /// The area it paints, stroke included.
    pub(crate) fn bounds(&self) -> Rect {
        self.rect.inflate(STROKE, STROKE)
    }

    /// Moves it by `delta` document units.
    pub(crate) fn translate(&mut self, delta: Vec2) {
        self.rect = self.rect + delta;
    }

    /// Draws it in document coordinates. At small scales (`level_scale`,
    /// device pixels per unit) the outline is left out.
    pub(crate) fn paint(&self, scene: &mut Scene, level_scale: f64) {
        let outline = level_scale >= 0.25;
        match self.kind {
            Kind::Rect => {
                scene.fill(&self.rect, self.fill);
                if outline {
                    scene.stroke(&self.rect, STROKE, INK);
                }
            }
            Kind::Rounded => {
                let shape = RoundedRect::from_rect(self.rect, 10.0);
                scene.fill(&shape, self.fill);
                if outline {
                    scene.stroke(&shape, STROKE, INK);
                }
            }
            Kind::Ellipse => {
                let shape = Ellipse::from_rect(self.rect);
                scene.fill(&shape, self.fill);
                if outline {
                    scene.stroke(&shape, STROKE, INK);
                }
            }
            Kind::Triangle => {
                let r = self.rect;
                let mut path = BezPath::new();
                path.move_to((r.center().x, r.y0));
                path.line_to((r.x1, r.y1));
                path.line_to((r.x0, r.y1));
                path.close_path();
                scene.fill(&path, self.fill);
                if outline {
                    scene.stroke(&path, STROKE, INK);
                }
            }
            Kind::Star => {
                let r = self.rect;
                let center = r.center();
                let (outer, inner) = (
                    r.width().min(r.height()) / 2.0,
                    r.width().min(r.height()) / 4.5,
                );
                let mut path = BezPath::new();
                for i in 0..10 {
                    let angle = std::f64::consts::PI * (f64::from(i) / 5.0 - 0.5);
                    let radius = if i % 2 == 0 { outer } else { inner };
                    let point = Point::new(
                        center.x + radius * angle.cos(),
                        center.y + radius * angle.sin(),
                    );
                    if i == 0 {
                        path.move_to(point);
                    } else {
                        path.line_to(point);
                    }
                }
                path.close_path();
                scene.fill(&path, self.fill);
                if outline {
                    scene.stroke(&path, STROKE, INK);
                }
            }
        }
    }
}

/// `count` shapes in a loose grid of `columns`.
pub(crate) fn generate(count: usize) -> Vec<Shape> {
    let columns = ((count as f64 * 1.6).sqrt().ceil() as usize).max(1);
    (0..count)
        .map(|i| {
            let mut state = (i as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ 0x5851_f42d_4c95_7f2d;
            let mut next = || {
                state ^= state >> 33;
                state = state.wrapping_mul(0xff51_afd7_ed55_8ccd);
                state ^= state >> 29;
                state
            };
            let column = (i % columns) as f64;
            let row = (i / columns) as f64;
            let jitter = Vec2::new((next() % 13) as f64 - 6.0, (next() % 13) as f64 - 6.0);
            let size = 30.0 + (next() % 22) as f64;
            let origin = Point::new(column * PITCH, row * PITCH) + jitter;
            let kind = match next() % 5 {
                0 => Kind::Rect,
                1 => Kind::Rounded,
                2 => Kind::Ellipse,
                3 => Kind::Triangle,
                _ => Kind::Star,
            };
            Shape {
                kind,
                rect: Rect::from_origin_size(
                    origin,
                    (size, size * (0.75 + (next() % 50) as f64 / 100.0)),
                ),
                fill: FILLS[(next() % FILLS.len() as u64) as usize],
                z: i as u64,
            }
        })
        .collect()
}
