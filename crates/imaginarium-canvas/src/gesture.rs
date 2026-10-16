// SPDX-License-Identifier: AGPL-3.0-or-later

//! Handles and gestures (`canvas.md` §5, §6): the selection's box, its
//! handles, and what moving, resizing and rotating do to each node's box.

use imaginarium_model::{Geometry, NodeId};
use imaginarium_render::kurbo::{Affine, Point, Rect, Size, Vec2};

/// Which side of the box a resize handle moves, per axis.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Side {
    /// The left or top edge.
    Start,
    /// Neither: an edge handle on the other axis.
    Middle,
    /// The right or bottom edge.
    End,
}

/// A handle of the selection's box.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Handle {
    /// One of the 8 resize handles.
    Resize { x: Side, y: Side },
    /// The rotation handle above the top edge.
    Rotate,
    /// The corner radius of a single rectangle.
    Radius,
}

/// Handles are drawn this big, in logical pixels.
pub(crate) const HANDLE_SIZE: f64 = 9.0;
/// The rotation handle's diameter, and its distance above the top edge.
pub(crate) const ROTATE_SIZE: f64 = 10.0;
pub(crate) const ROTATE_GAP: f64 = 20.0;
/// The radius handle's diameter.
pub(crate) const RADIUS_SIZE: f64 = 8.0;
/// How far from a handle's center a press still takes it.
pub(crate) const HANDLE_REACH: f64 = 7.0;

/// The box the handles go around: one node's turned box, or the
/// axis-aligned box around several.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct SelectionBox {
    /// Box space (`(0, 0)` to `size`) to document space.
    pub world: Affine,
    pub size: Size,
}

impl SelectionBox {
    /// The center in document space.
    pub(crate) fn center(&self) -> Point {
        self.world * Point::new(self.size.width / 2.0, self.size.height / 2.0)
    }
}

/// A selected node as a gesture started: its box, and its parent's space.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Start {
    pub node: NodeId,
    pub geometry: Geometry,
    /// The parent's box space to document space.
    pub parent: Affine,
}

impl Start {
    /// The node's box space to document space at the start.
    fn world(&self) -> Affine {
        let g = self.geometry;
        self.parent * crate::items::local_transform(g.x, g.y, g.w.max(0.0), g.h.max(0.0), g.r)
    }

    /// The geometry whose box has `center` (document space), `size` and
    /// `rotation`, in the parent's space.
    fn placed(&self, center: Point, size: Size, rotation: f64) -> Geometry {
        let local = self.parent.inverse() * center;
        Geometry {
            x: local.x - size.width / 2.0,
            y: local.y - size.height / 2.0,
            w: size.width,
            h: size.height,
            r: rotation,
        }
    }
}

/// The screen positions of the box's handles. `camera` maps document
/// space to the window; `radius` places the radius handle of a rounded
/// rectangle (its radius in points), if there is one.
pub(crate) fn handles(
    selection: &SelectionBox,
    camera: Affine,
    radius: Option<f64>,
) -> Vec<(Handle, Point)> {
    let screen = camera * selection.world;
    let (w, h) = (selection.size.width, selection.size.height);
    let mut out = Vec::with_capacity(10);
    for (y, at_y) in [(Side::Start, 0.0), (Side::Middle, h / 2.0), (Side::End, h)] {
        for (x, at_x) in [(Side::Start, 0.0), (Side::Middle, w / 2.0), (Side::End, w)] {
            if x == Side::Middle && y == Side::Middle {
                continue;
            }
            out.push((Handle::Resize { x, y }, screen * Point::new(at_x, at_y)));
        }
    }
    // Above the top edge's middle, a fixed distance on screen, along the
    // box's up direction.
    let top = screen * Point::new(w / 2.0, 0.0);
    let up = screen * Point::new(w / 2.0, -1.0) - top;
    let up = if up.hypot() > 0.0 {
        up / up.hypot()
    } else {
        Vec2::new(0.0, -1.0)
    };
    out.push((Handle::Rotate, top + up * ROTATE_GAP));
    if let Some(radius) = radius {
        // Inside the top left corner, at least a few pixels in.
        let zoom = (camera * Point::new(1.0, 0.0) - camera * Point::ZERO)
            .hypot()
            .max(1e-9);
        let along = (radius.max(0.0) * zoom).max(12.0) / zoom;
        let along = along.min(w / 2.0).min(h / 2.0);
        out.push((Handle::Radius, screen * Point::new(along, along)));
    }
    out
}

/// The box after dragging `handle` to `pointer` (document space): the
/// opposite side stays; `keep_aspect` scales corners evenly. Returns the
/// new rectangle in the starting box's space.
pub(crate) fn resized(
    selection: &SelectionBox,
    handle: (Side, Side),
    pointer: Point,
    keep_aspect: bool,
) -> Rect {
    let local = selection.world.inverse() * pointer;
    let (w, h) = (selection.size.width, selection.size.height);
    let (sx, sy) = handle;
    let mut x0 = if sx == Side::Start { local.x } else { 0.0 };
    let mut x1 = if sx == Side::End { local.x } else { w };
    let mut y0 = if sy == Side::Start { local.y } else { 0.0 };
    let mut y1 = if sy == Side::End { local.y } else { h };
    if keep_aspect && sx != Side::Middle && sy != Side::Middle && w > 0.0 && h > 0.0 {
        let scale = ((x1 - x0) / w).abs().max(((y1 - y0) / h).abs());
        let (nw, nh) = (w * scale, h * scale);
        let sign = |a: f64, b: f64| if b >= a { 1.0 } else { -1.0 };
        if sx == Side::Start {
            x0 = x1 - nw * sign(x0, x1);
        } else {
            x1 = x0 + nw * sign(x0, x1);
        }
        if sy == Side::Start {
            y0 = y1 - nh * sign(y0, y1);
        } else {
            y1 = y0 + nh * sign(y0, y1);
        }
    }
    Rect::new(x0, y0, x1, y1).abs()
}

/// Each node's box after its selection box became `to` (in the starting
/// box's space): positions and sizes scale with it, rotations stay.
pub(crate) fn resize_all(
    selection: &SelectionBox,
    starts: &[Start],
    to: Rect,
) -> Vec<(NodeId, Geometry)> {
    let (w, h) = (selection.size.width, selection.size.height);
    let sx = if w > 0.0 { to.width() / w } else { 1.0 };
    let sy = if h > 0.0 { to.height() / h } else { 1.0 };
    let map = Affine::translate(to.origin().to_vec2()) * Affine::scale_non_uniform(sx, sy);
    let inverse = selection.world.inverse();
    starts
        .iter()
        .map(|start| {
            let g = start.geometry;
            let center_doc = start.world() * Point::new(g.w.max(0.0) / 2.0, g.h.max(0.0) / 2.0);
            let center = selection.world * (map * (inverse * center_doc));
            // The node's axes in the selection box: a turned node in a
            // straight box scales by the box's scale along its own axes.
            let angle = (g.r - selection_rotation(selection, start)).to_radians();
            let (sin, cos) = angle.sin_cos();
            let scale_w = ((sx * cos).powi(2) + (sy * sin).powi(2)).sqrt();
            let scale_h = ((sx * sin).powi(2) + (sy * cos).powi(2)).sqrt();
            let size = Size::new((g.w * scale_w).max(0.0), (g.h * scale_h).max(0.0));
            (start.node, start.placed(center, size, g.r))
        })
        .collect()
}

/// The rotation of the selection box relative to a node's parent, in
/// degrees: how far the box's axes are turned from the parent's.
fn selection_rotation(selection: &SelectionBox, start: &Start) -> f64 {
    let in_parent = start.parent.inverse() * selection.world;
    let x = in_parent * Point::new(1.0, 0.0) - in_parent * Point::ZERO;
    x.y.atan2(x.x).to_degrees()
}

/// Each node's box after turning the selection by `degrees` around
/// `center` (document space).
pub(crate) fn rotate_all(starts: &[Start], center: Point, degrees: f64) -> Vec<(NodeId, Geometry)> {
    let turn = Affine::rotate_about(degrees.to_radians(), center);
    starts
        .iter()
        .map(|start| {
            let g = start.geometry;
            let size = Size::new(g.w.max(0.0), g.h.max(0.0));
            let center_doc = start.world() * Point::new(size.width / 2.0, size.height / 2.0);
            let rotation = normalize_degrees(g.r + degrees);
            (start.node, start.placed(turn * center_doc, size, rotation))
        })
        .collect()
}

/// An angle in (-180, 180].
pub(crate) fn normalize_degrees(degrees: f64) -> f64 {
    let turned = degrees.rem_euclid(360.0);
    if turned > 180.0 {
        turned - 360.0
    } else {
        turned
    }
}

/// The angle of `point` around `center`, in degrees, clockwise from the
/// right (screen y points down).
pub(crate) fn angle(center: Point, point: Point) -> f64 {
    let d = point - center;
    d.y.atan2(d.x).to_degrees()
}

/// The corner radius from dragging the radius handle to `pointer`
/// (document space), for a rectangle whose box is `selection`.
pub(crate) fn radius_at(selection: &SelectionBox, pointer: Point) -> f64 {
    let local = selection.world.inverse() * pointer;
    let limit = selection.size.width.min(selection.size.height) / 2.0;
    // Along the corner's diagonal.
    ((local.x + local.y) / 2.0).clamp(0.0, limit.max(0.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn straight(x: f64, y: f64, w: f64, h: f64) -> SelectionBox {
        SelectionBox {
            world: Affine::translate((x, y)),
            size: Size::new(w, h),
        }
    }

    fn start(geometry: Geometry) -> Start {
        Start {
            node: NodeId::Stored(1),
            geometry,
            parent: Affine::IDENTITY,
        }
    }

    #[test]
    fn a_corner_moves_and_the_opposite_one_stays() {
        let selection = straight(100.0, 100.0, 200.0, 100.0);
        let rect = resized(
            &selection,
            (Side::End, Side::End),
            Point::new(400.0, 250.0),
            false,
        );
        assert_eq!(rect, Rect::new(0.0, 0.0, 300.0, 150.0));
        let rect = resized(
            &selection,
            (Side::Start, Side::Middle),
            Point::new(50.0, 999.0),
            false,
        );
        assert_eq!(rect, Rect::new(-50.0, 0.0, 200.0, 100.0));
        // Past the opposite side: the box flips over instead of going negative.
        let rect = resized(
            &selection,
            (Side::End, Side::Middle),
            Point::new(50.0, 0.0),
            false,
        );
        assert_eq!(rect, Rect::new(-50.0, 0.0, 0.0, 100.0));
        // Shift keeps the shape.
        let rect = resized(
            &selection,
            (Side::End, Side::End),
            Point::new(500.0, 150.0),
            true,
        );
        assert_eq!(rect, Rect::new(0.0, 0.0, 400.0, 200.0));
    }

    #[test]
    fn resizing_a_turned_node_keeps_the_opposite_corner_in_place() {
        let g = Geometry {
            x: 0.0,
            y: 0.0,
            w: 100.0,
            h: 50.0,
            r: 30.0,
        };
        let node = start(g);
        let selection = SelectionBox {
            world: node.world(),
            size: Size::new(100.0, 50.0),
        };
        let anchor = selection.world * Point::ZERO;
        let rect = resized(
            &selection,
            (Side::End, Side::End),
            selection.world * Point::new(150.0, 80.0),
            false,
        );
        let [(_, placed)] = resize_all(&selection, &[node], rect)[..] else {
            panic!("one node")
        };
        assert!((placed.w - 150.0).abs() < 1e-9 && (placed.h - 80.0).abs() < 1e-9);
        assert_eq!(placed.r, 30.0);
        let moved = start(placed).world() * Point::ZERO;
        assert!((moved - anchor).hypot() < 1e-9, "{moved:?} vs {anchor:?}");
    }

    #[test]
    fn several_nodes_scale_with_their_box_and_turn_around_its_center() {
        let a = start(Geometry::new(0.0, 0.0, 100.0, 100.0));
        let b = start(Geometry::new(200.0, 0.0, 100.0, 100.0));
        let selection = straight(0.0, 0.0, 300.0, 100.0);
        let placed = resize_all(&selection, &[a, b], Rect::new(0.0, 0.0, 600.0, 100.0));
        assert_eq!(placed[0].1, Geometry::new(0.0, 0.0, 200.0, 100.0));
        assert_eq!(placed[1].1, Geometry::new(400.0, 0.0, 200.0, 100.0));
        let turned = rotate_all(&[a, b], selection.center(), 180.0);
        assert!((turned[0].1.x - 200.0).abs() < 1e-9 && (turned[0].1.y).abs() < 1e-9);
        assert_eq!(turned[0].1.r, 180.0);
        assert!((turned[1].1.x).abs() < 1e-9);
    }

    #[test]
    fn angles_stay_in_a_half_turn_either_way() {
        assert_eq!(normalize_degrees(190.0), -170.0);
        assert_eq!(normalize_degrees(-190.0), 170.0);
        assert_eq!(normalize_degrees(180.0), 180.0);
        assert_eq!(angle(Point::ZERO, Point::new(0.0, 10.0)), 90.0);
    }

    #[test]
    fn the_handles_surround_the_box_and_the_rotation_handle_sits_above() {
        let selection = straight(100.0, 100.0, 200.0, 100.0);
        let handles = handles(&selection, Affine::IDENTITY, Some(10.0));
        assert_eq!(handles.len(), 10);
        let rotate = handles
            .iter()
            .find(|(handle, _)| *handle == Handle::Rotate)
            .unwrap()
            .1;
        assert_eq!(rotate, Point::new(200.0, 80.0));
        let radius = handles
            .iter()
            .find(|(handle, _)| *handle == Handle::Radius)
            .unwrap()
            .1;
        assert_eq!(radius, Point::new(112.0, 112.0));
    }
}
