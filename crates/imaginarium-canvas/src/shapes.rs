// SPDX-License-Identifier: AGPL-3.0-or-later

//! Shape outlines (ADR-0021): every preset becomes a path in its box's
//! coordinates, `(0, 0)` to `(w, h)`, which painting, hit testing and
//! bounds then share.

use imaginarium_model::Value;
use imaginarium_model::view::NodeView;
use imaginarium_render::kurbo::{
    BezPath, Ellipse, Point, Rect, RoundedRect, RoundedRectRadii, Shape, Size,
};

/// Curve accuracy of the outlines, in points.
const TOLERANCE: f64 = 0.05;

/// A shape's outline, and whether it is closed (filled) or a line.
#[derive(Clone, Debug, PartialEq)]
pub struct Outline {
    /// The path in box coordinates.
    pub path: BezPath,
    /// Closed outlines are filled; lines are only stroked.
    pub closed: bool,
}

/// The outline of a `shape` node of `size` (schema-v1.md §5.6). `radius`
/// is the style's corner radius, used by `rect` when the node has no
/// `radii` of its own. Unknown presets, and `path` until the path tools
/// come, are rectangles (schema rule 4).
pub fn shape_path(node: &NodeView, size: Size, radius: f64) -> Outline {
    let preset = node.prop("shape").and_then(Value::as_str).unwrap_or("rect");
    let (w, h) = (size.width.max(0.0), size.height.max(0.0));
    let polygon = |points: &[(f64, f64)]| {
        let mut path = BezPath::new();
        for (index, &(x, y)) in points.iter().enumerate() {
            if index == 0 {
                path.move_to((x, y));
            } else {
                path.line_to((x, y));
            }
        }
        path.close_path();
        Outline { path, closed: true }
    };
    match preset {
        "ellipse" => Outline {
            path: Ellipse::from_rect(Rect::new(0.0, 0.0, w, h)).to_path(TOLERANCE),
            closed: true,
        },
        "triangle" => polygon(&[(w / 2.0, 0.0), (w, h), (0.0, h)]),
        "diamond" => polygon(&[(w / 2.0, 0.0), (w, h / 2.0), (w / 2.0, h), (0.0, h / 2.0)]),
        "line" => {
            let flip = node.prop("flip").and_then(Value::as_str).unwrap_or("");
            let (flip_h, flip_v) = (flip.contains('h'), flip.contains('v'));
            let x = |at_start: bool| if at_start != flip_h { 0.0 } else { w };
            let y = |at_start: bool| if at_start != flip_v { 0.0 } else { h };
            let mut path = BezPath::new();
            path.move_to(Point::new(x(true), y(true)));
            path.line_to(Point::new(x(false), y(false)));
            Outline {
                path,
                closed: false,
            }
        }
        _ => rect_path(node, w, h, radius),
    }
}

fn rect_path(node: &NodeView, w: f64, h: f64, radius: f64) -> Outline {
    let rect = Rect::new(0.0, 0.0, w, h);
    // Four radii of the node's own, else the style's for every corner.
    let radii = node
        .prop("radii")
        .and_then(|value| match value {
            Value::List(list) if list.len() == 4 => {
                let mut radii = [0.0; 4];
                for (radius, value) in radii.iter_mut().zip(list.iter()) {
                    *radius = value.as_number().filter(|r| r.is_finite())?;
                }
                Some(radii)
            }
            _ => None,
        })
        .unwrap_or([radius; 4]);
    let limit = w.min(h) / 2.0;
    let [tl, tr, br, bl] = radii.map(|radius| radius.clamp(0.0, limit));
    let path = if [tl, tr, br, bl].iter().all(|radius| *radius <= 0.0) {
        rect.to_path(TOLERANCE)
    } else {
        RoundedRect::from_rect(rect, RoundedRectRadii::new(tl, tr, br, bl)).to_path(TOLERANCE)
    };
    Outline { path, closed: true }
}
