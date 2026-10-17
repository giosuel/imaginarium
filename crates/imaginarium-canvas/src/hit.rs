// SPDX-License-Identifier: AGPL-3.0-or-later

//! Hit testing (`canvas.md` §4): which node is under a point, and which
//! lie inside a marquee.

use std::collections::{HashMap, HashSet};

use imaginarium_model::{DocumentView, NodeId, NodeKind, Parent};
use imaginarium_render::canvas::ItemId;
use imaginarium_render::kurbo::{BezPath, ParamCurveNearest, Point, Rect, Shape};

use crate::items::{Item, Items, Look};

/// Pointer tolerance in screen pixels: thin lines stay easy to hit.
pub(crate) const TOLERANCE_PIXELS: f64 = 4.0;

/// Whether `point` (box space) is within `reach` of the outline.
fn near_outline(path: &BezPath, point: Point, reach: f64) -> bool {
    let reach_sq = reach * reach;
    path.segments()
        .any(|segment| segment.nearest(point, 1e-3).distance_sq <= reach_sq)
}

/// Whether `point` (document space) is on the item, `tolerance` in
/// document units around strokes.
fn on_item(item: &Item, point: Point, tolerance: f64) -> bool {
    // A frame's children can't be hit where the frame clips them.
    if !item
        .clips
        .iter()
        .all(|clip| clip.rect.contains(clip.world.inverse() * point))
    {
        return false;
    }
    let local = item.world.inverse() * point;
    let rect = Rect::from_origin_size(Point::ZERO, item.size);
    match &item.look {
        Look::Shape {
            outline,
            style,
            label,
        } => {
            let filled = outline.closed && style.fill.is_some();
            let stroke = if style.stroke.is_some() {
                style.stroke_width / 2.0
            } else {
                0.0
            };
            (filled && outline.path.contains(local))
                || near_outline(&outline.path, local, stroke + tolerance)
                || label.as_ref().is_some_and(|label| {
                    Rect::from_origin_size(label.origin, label.extent).contains(local)
                })
        }
        Look::Text { text, .. } => {
            rect.inflate(tolerance, tolerance).contains(local)
                || text.as_ref().is_some_and(|text| {
                    Rect::from_origin_size(text.origin, text.extent).contains(local)
                })
        }
        // Inside a frame is empty board (a marquee starts there); its
        // border and its name select it.
        Look::Frame { style } => near_outline(
            &rect.to_path(0.1),
            local,
            style.stroke_width / 2.0 + tolerance,
        ),
        Look::Placeholder => rect.contains(local),
    }
}

/// Whether `point` (document space) is inside a frame's box, where its
/// clips let it show.
fn inside_frame(item: &Item, point: Point) -> bool {
    item.clips
        .iter()
        .all(|clip| clip.rect.contains(clip.world.inverse() * point))
        && Rect::from_origin_size(Point::ZERO, item.size).contains(item.world.inverse() * point)
}

/// Whether `node` lies somewhere below `ancestor`.
pub(crate) fn is_inside(view: &DocumentView, node: NodeId, ancestor: NodeId) -> bool {
    let mut next = view.node(node).map(|node| node.parent());
    while let Some(Parent::Node(parent)) = next {
        if parent == ancestor {
            return true;
        }
        next = view.node(parent).map(|node| node.parent());
    }
    false
}

/// Whether the node or one of its ancestors is locked (schema §5).
pub(crate) fn locked(view: &DocumentView, node: NodeId) -> bool {
    let mut next = Some(node);
    while let Some(id) = next {
        let Some(node) = view.node(id) else {
            return false;
        };
        if node.locked() {
            return true;
        }
        next = match node.parent() {
            Parent::Root => None,
            Parent::Node(parent) => Some(parent),
        };
    }
    false
}

/// The groups that are open: the one a double-click went into, and its
/// ancestors.
pub(crate) fn open_groups(view: &DocumentView, entered: Option<NodeId>) -> HashSet<NodeId> {
    let mut open: HashSet<NodeId> = HashSet::new();
    let mut next = entered;
    while let Some(id) = next {
        open.insert(id);
        next = match view.node(id).map(|node| node.parent()) {
            Some(Parent::Node(parent)) => Some(parent),
            _ => None,
        };
    }
    open
}

/// What a click on `node` selects: groups are selected as a whole, the
/// outermost one not entered (`entered`: the group a double-click went
/// into, and so its ancestors).
pub(crate) fn selectable(view: &DocumentView, node: NodeId, entered: Option<NodeId>) -> NodeId {
    selectable_in(view, node, &open_groups(view, entered))
}

/// [`selectable`] with the open groups worked out once.
pub(crate) fn selectable_in(view: &DocumentView, node: NodeId, open: &HashSet<NodeId>) -> NodeId {
    let mut chosen = node;
    let mut next = view.node(node).map(|node| node.parent());
    while let Some(Parent::Node(parent)) = next {
        let Some(view_node) = view.node(parent) else {
            break;
        };
        if matches!(view_node.kind(), NodeKind::Group) && !open.contains(&parent) {
            chosen = parent;
        }
        next = Some(view_node.parent());
    }
    chosen
}

/// The topmost node under `point` (document space), from the canvas's
/// candidates (`front_to_back`), mapped to what a click selects. Locked
/// and hidden nodes are skipped.
pub(crate) fn hit(
    items: &Items,
    view: &DocumentView,
    front_to_back: &[ItemId],
    point: Point,
    tolerance: f64,
    entered: Option<NodeId>,
) -> Option<NodeId> {
    for id in front_to_back {
        let Some(&node) = items.by_item.get(id) else {
            continue;
        };
        let Some(item) = items.by_node.get(&node) else {
            continue;
        };
        if on_item(item, point, tolerance) && !locked(view, node) {
            return Some(selectable(view, node, entered));
        }
        // A frame's empty inside hides what is behind it: nothing is hit
        // there (a marquee starts).
        if matches!(item.look, Look::Frame { .. }) && inside_frame(item, point) {
            return None;
        }
    }
    None
}

/// The box a node covers in document space: its item's box, or for a
/// group the union of its descendants'.
pub(crate) fn node_box(items: &Items, view: &DocumentView, node: NodeId) -> Option<Rect> {
    if let Some(item) = items.by_node.get(&node) {
        return Some(
            item.world
                .transform_rect_bbox(Rect::from_origin_size(Point::ZERO, item.size)),
        );
    }
    let mut union: Option<Rect> = None;
    for child in view.children(Parent::Node(node)) {
        if let Some(rect) = node_box(items, view, child) {
            union = Some(union.map_or(rect, |union| union.union(rect)));
        }
    }
    union
}

/// The nodes a marquee over `area` selects: those whose box lies wholly
/// inside it, groups as a whole, in z order. A frame inside selects the
/// frame; the children of a frame that isn't are selected one by one.
pub(crate) fn hit_rect(
    items: &Items,
    view: &DocumentView,
    back_to_front: &[ItemId],
    area: Rect,
    entered: Option<NodeId>,
) -> Vec<NodeId> {
    let mut chosen = Vec::new();
    let mut seen = HashSet::new();
    let mut boxes: HashMap<NodeId, Option<Rect>> = HashMap::new();
    for id in back_to_front {
        let Some(&node) = items.by_item.get(id) else {
            continue;
        };
        if locked(view, node) {
            continue;
        }
        // The outermost enclosing frame inside the area takes it.
        let mut unit = selectable(view, node, entered);
        let mut next = view.node(unit).map(|node| node.parent());
        while let Some(Parent::Node(parent)) = next {
            let inside = *boxes
                .entry(parent)
                .or_insert_with(|| node_box(items, view, parent));
            if view
                .node(parent)
                .is_some_and(|node| matches!(node.kind(), NodeKind::Frame))
                && inside.is_some_and(|rect| area.contains_rect(rect))
            {
                unit = selectable(view, parent, entered);
            }
            next = view.node(parent).map(|node| node.parent());
        }
        if !seen.insert(unit) {
            continue;
        }
        let rect = *boxes
            .entry(unit)
            .or_insert_with(|| node_box(items, view, unit));
        if rect.is_some_and(|rect| area.contains_rect(rect)) {
            chosen.push(unit);
        }
    }
    chosen
}
