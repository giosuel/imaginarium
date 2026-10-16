// SPDX-License-Identifier: AGPL-3.0-or-later

//! What the board's commands do (M3.3): delete, duplicate, select all,
//! order, group and ungroup, lock, and the view's zoom. The commands
//! (`commands.rs`) check their arguments and call these.

use std::collections::HashSet;

use imaginarium_model::{Actor, Geometry, NodeId, NodeKind, Parent, Result, Value};
use imaginarium_render::kurbo::{Point, Rect};

use super::BoardCanvas;
use crate::copy::NodeCopy;
use crate::hit;
use crate::items::local_transform;

/// Duplicates sit this far down and right of their originals, in points.
pub(crate) const DUPLICATE_OFFSET: f64 = 16.0;

/// The zoom steps of zoom in and out.
const ZOOM_STEP: f64 = 1.25;

/// Room around what "zoom to fit" shows, in logical pixels.
const FIT_MARGIN: f64 = 48.0;

/// Where a node stands among its siblings: its parent, and its index.
fn place_of(board: &BoardCanvas, node: NodeId) -> Option<(Parent, usize)> {
    let view = board.document.view();
    let parent = view.node(node)?.parent();
    let index = view.children(parent).position(|child| child == node)?;
    Some((parent, index))
}

impl BoardCanvas {
    /// The selected nodes a command acts on: not locked, not inside
    /// another selected node, back to front.
    pub(crate) fn selected_units(&self) -> Vec<NodeId> {
        let view = self.document.view();
        let selection = self.selection.get_untracked();
        let chosen: HashSet<NodeId> = selection.iter().copied().collect();
        let mut units: Vec<NodeId> = selection
            .into_iter()
            .filter(|node| view.node(*node).is_some() && !hit::locked(view, *node))
            .filter(|node| {
                !chosen
                    .iter()
                    .any(|other| hit::is_inside(view, *node, *other))
            })
            .collect();
        let order: Vec<NodeId> = view.walk();
        units.sort_by_key(|node| order.iter().position(|id| id == node));
        units
    }

    /// Deletes the selected nodes. `false` if there were none.
    pub(crate) fn delete_selection(&mut self) -> Result<bool> {
        let units = self.selected_units();
        if units.is_empty() {
            return Ok(false);
        }
        self.transact(&Actor::User, |txn| {
            for node in &units {
                txn.delete_node(*node)?;
            }
            Ok(())
        })?;
        self.selection.set(Vec::new());
        Ok(true)
    }

    /// Duplicates the selected nodes, each above its original and moved
    /// by `offset` on the board, and selects the copies.
    pub(crate) fn duplicate_selection(&mut self, offset: (f64, f64)) -> Result<Vec<NodeId>> {
        let units = self.selected_units();
        let view = self.document.view();
        let copies: Vec<(NodeCopy, Parent, usize, (f64, f64))> = units
            .iter()
            .filter_map(|node| {
                let copy = NodeCopy::read(view, *node)?;
                let (parent, index) = place_of(self, *node)?;
                // The offset in the parent's space (a turned frame or group).
                let world = self
                    .items
                    .as_ref()
                    .and_then(|items| items.parent_world(view, *node))
                    .unwrap_or_default();
                let inverse = world.inverse();
                let delta = inverse * Point::new(offset.0, offset.1) - inverse * Point::ZERO;
                Some((copy, parent, index, (delta.x, delta.y)))
            })
            .collect();
        if copies.is_empty() {
            return Ok(Vec::new());
        }
        let created = self.transact(&Actor::User, |txn| {
            let mut created = Vec::new();
            // Last first, so the earlier indexes still hold.
            for (copy, parent, index, delta) in copies.iter().rev() {
                created.push(copy.write(txn, *parent, Some(index + 1), *delta)?);
            }
            created.reverse();
            Ok(created)
        })?;
        self.selection.set(created.clone());
        Ok(created)
    }

    /// Selects every node a click could select, at the current level (in
    /// an entered group, its children).
    pub(crate) fn select_all(&mut self) -> bool {
        let order = self.tab_order();
        let changed = self.selection.get_untracked() != order;
        self.selection.set_if_changed(order);
        changed
    }

    /// Moves the selected nodes in their parents' stacking order: to the
    /// front, the back, or one step (`step`: +1 forward, -1 backward).
    pub(crate) fn reorder_selection(&mut self, to: Order) -> Result<bool> {
        let units = self.selected_units();
        let places: Vec<(NodeId, Parent, usize, usize)> = units
            .iter()
            .filter_map(|node| {
                let (parent, index) = place_of(self, *node)?;
                let count = self.document.view().children(parent).count();
                Some((*node, parent, index, count))
            })
            .collect();
        let moves: Vec<(NodeId, Parent, Option<usize>)> = match to {
            Order::Front => places
                .iter()
                .filter(|(_, _, index, count)| index + 1 < *count)
                .map(|(node, parent, _, _)| (*node, *parent, None))
                .collect(),
            // Back to front reversed, so they keep their order at the back.
            Order::Back => places
                .iter()
                .rev()
                .filter(|(_, _, index, _)| *index > 0)
                .map(|(node, parent, _, _)| (*node, *parent, Some(0)))
                .collect(),
            Order::Forward => places
                .iter()
                .rev()
                .filter(|(_, _, index, count)| index + 1 < *count)
                .map(|(node, parent, index, _)| (*node, *parent, Some(index + 1)))
                .collect(),
            Order::Backward => places
                .iter()
                .filter(|(_, _, index, _)| *index > 0)
                .map(|(node, parent, index, _)| (*node, *parent, Some(index - 1)))
                .collect(),
        };
        if moves.is_empty() {
            return Ok(false);
        }
        self.transact(&Actor::User, |txn| {
            for (node, parent, index) in &moves {
                txn.move_node(*node, *parent, *index)?;
            }
            Ok(())
        })?;
        Ok(true)
    }

    /// Whether the selected nodes can be grouped: two or more, side by
    /// side under one parent.
    pub(crate) fn can_group(&self) -> bool {
        let units = self.selected_units();
        let view = self.document.view();
        let parents: HashSet<Parent> = units
            .iter()
            .filter_map(|node| Some(view.node(*node)?.parent()))
            .collect();
        units.len() >= 2 && parents.len() == 1
    }

    /// Groups the selected nodes (two or more under one parent) in a new
    /// group where the topmost was, and selects it.
    pub(crate) fn group_selection(&mut self) -> Result<Option<NodeId>> {
        if !self.can_group() {
            return Ok(None);
        }
        let units = self.selected_units();
        let view = self.document.view();
        let Some((parent, _)) = units.last().and_then(|node| place_of(self, *node)) else {
            return Ok(None);
        };
        // The group's box: around its children's turned boxes, in the
        // parent's space; the children keep their place on the board.
        let boxes: Vec<(NodeId, Geometry, Rect)> = units
            .iter()
            .filter_map(|node| {
                let g = view.node(*node)?.geometry().unwrap_or_default();
                let rect = local_transform(g.x, g.y, g.w.max(0.0), g.h.max(0.0), g.r)
                    .transform_rect_bbox(Rect::new(0.0, 0.0, g.w.max(0.0), g.h.max(0.0)));
                Some((*node, g, rect))
            })
            .collect();
        let Some(area) = boxes
            .iter()
            .map(|(_, _, rect)| *rect)
            .reduce(|a, b| a.union(b))
        else {
            return Ok(None);
        };
        // Just above the topmost: once its children move in, it stands
        // where they stood.
        let index = units
            .iter()
            .filter_map(|node| place_of(self, *node).map(|(_, index)| index))
            .max()
            .map(|top| top + 1);
        let group = self.transact(&Actor::User, |txn| {
            let group = txn.create_node(parent, index, &NodeKind::Group)?;
            txn.set_geometry(
                group,
                Geometry::new(area.x0, area.y0, area.width(), area.height()),
            )?;
            for (node, g, _) in &boxes {
                txn.move_node(*node, Parent::Node(group), None)?;
                txn.set_geometry(
                    *node,
                    Geometry {
                        x: g.x - area.x0,
                        y: g.y - area.y0,
                        ..*g
                    },
                )?;
            }
            Ok(group)
        })?;
        self.selection.set(vec![group]);
        Ok(Some(group))
    }

    /// Ungroups the selected groups: their children take their place, at
    /// the same spot on the board, and are selected.
    pub(crate) fn ungroup_selection(&mut self) -> Result<Vec<NodeId>> {
        let view = self.document.view();
        let groups: Vec<NodeId> = self
            .selected_units()
            .into_iter()
            .filter(|node| {
                view.node(*node)
                    .is_some_and(|node| matches!(node.kind(), NodeKind::Group))
            })
            .collect();
        if groups.is_empty() {
            return Ok(Vec::new());
        }
        let mut plans = Vec::new();
        for group in &groups {
            let Some((parent, index)) = place_of(self, *group) else {
                continue;
            };
            let g = view
                .node(*group)
                .and_then(|node| node.geometry())
                .unwrap_or_default();
            let space = local_transform(g.x, g.y, g.w.max(0.0), g.h.max(0.0), g.r);
            let children: Vec<(NodeId, Geometry)> = view
                .children(Parent::Node(*group))
                .filter_map(|child| {
                    let c = view.node(child)?.geometry().unwrap_or_default();
                    // The child's center in the parent's space; its turn
                    // adds the group's.
                    let center = space * Point::new(c.x + c.w / 2.0, c.y + c.h / 2.0);
                    Some((
                        child,
                        Geometry {
                            x: center.x - c.w / 2.0,
                            y: center.y - c.h / 2.0,
                            w: c.w,
                            h: c.h,
                            r: crate::gesture::normalize_degrees(c.r + g.r),
                        },
                    ))
                })
                .collect();
            plans.push((*group, parent, index, children));
        }
        let freed = self.transact(&Actor::User, |txn| {
            let mut freed = Vec::new();
            for (group, parent, index, children) in &plans {
                for (offset, (child, geometry)) in children.iter().enumerate() {
                    txn.move_node(*child, *parent, Some(index + offset))?;
                    txn.set_geometry(*child, *geometry)?;
                    freed.push(*child);
                }
                txn.delete_node(*group)?;
            }
            Ok(freed)
        })?;
        self.selection.set(freed.clone());
        Ok(freed)
    }

    /// Locks or unlocks the selected nodes (locked ones can't be selected
    /// by a click, so unlocking takes the selection as it is).
    pub(crate) fn set_locked(&mut self, locked: bool) -> Result<bool> {
        let view = self.document.view();
        let nodes: Vec<NodeId> = self
            .selection
            .get_untracked()
            .into_iter()
            .filter(|node| view.node(*node).is_some_and(|node| node.locked() != locked))
            .collect();
        if nodes.is_empty() {
            return Ok(false);
        }
        self.transact(&Actor::User, |txn| {
            for node in &nodes {
                let value = if locked {
                    Value::Bool(true)
                } else {
                    Value::Null
                };
                txn.set_prop(*node, "locked", value)?;
            }
            Ok(())
        })?;
        Ok(true)
    }

    /// Zooms by `factor` around the middle of the board's box.
    pub(crate) fn zoom_by(&mut self, factor: f64) {
        let middle = self.bounds.center();
        self.zoom_about(self.bounds, factor, middle);
    }

    /// Zooms in one step.
    pub(crate) fn zoom_in(&mut self) {
        self.zoom_by(ZOOM_STEP);
    }

    /// Zooms out one step.
    pub(crate) fn zoom_out(&mut self) {
        self.zoom_by(1.0 / ZOOM_STEP);
    }

    /// Shows the board at 100 %, keeping its middle in place.
    pub(crate) fn zoom_actual(&mut self) {
        self.zoom_by(1.0 / self.zoom);
    }

    /// Shows `area` (document space) whole, in the middle, at most at 100 %
    /// zoomed in further. `false` when there is nothing to show.
    pub(crate) fn zoom_to(&mut self, area: Rect) -> bool {
        let bounds = self.bounds;
        if area.width() <= 0.0 && area.height() <= 0.0
            || bounds.width() <= 0.0
            || bounds.height() <= 0.0
        {
            return false;
        }
        let room = (bounds.width() - 2.0 * FIT_MARGIN).max(1.0) / area.width().max(1.0);
        let room = room.min((bounds.height() - 2.0 * FIT_MARGIN).max(1.0) / area.height().max(1.0));
        let zoom = room.min(1.0).clamp(crate::MIN_ZOOM, crate::MAX_ZOOM);
        let middle = area.center();
        self.set_view(
            Point::new(
                middle.x - bounds.width() / 2.0 / zoom,
                middle.y - bounds.height() / 2.0 / zoom,
            ),
            zoom,
        );
        true
    }

    /// Shows every node.
    pub(crate) fn zoom_to_fit(&mut self) -> bool {
        let area = self.items.as_ref().and_then(|items| {
            items
                .by_node
                .values()
                .map(|item| item.bounds)
                .reduce(|a, b| a.union(b))
        });
        area.is_some_and(|area| self.zoom_to(area))
    }

    /// Shows the selection.
    pub(crate) fn zoom_to_selection(&mut self) -> bool {
        let Some(items) = &self.items else {
            return false;
        };
        let view = self.document.view();
        let area = self
            .selection
            .get_untracked()
            .iter()
            .filter_map(|node| hit::node_box(items, view, *node))
            .reduce(|a, b| a.union(b));
        area.is_some_and(|area| self.zoom_to(area))
    }
}

/// Where `board.order` moves the selection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Order {
    Front,
    Back,
    Forward,
    Backward,
}
