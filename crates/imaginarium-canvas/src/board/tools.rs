// SPDX-License-Identifier: AGPL-3.0-or-later

//! Tools (`shell.md` §3.3, M3.3): select, the hand, and drawing shapes,
//! text and frames on the board. A shape tool draws one shape, then
//! Select returns.

use imaginarium_model::{Actor, Geometry, NodeId, NodeKind, Parent, Result, Value};
use imaginarium_render::kurbo::{Affine, Point, Rect};

use super::BoardCanvas;
use crate::hit;

/// A shape preset a tool draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Preset {
    /// `rect`.
    Rectangle,
    /// `ellipse`.
    Ellipse,
    /// `diamond`.
    Diamond,
    /// `triangle`.
    Triangle,
    /// `line`.
    Line,
}

impl Preset {
    /// The schema's preset name (`shape`).
    pub fn name(self) -> &'static str {
        match self {
            Preset::Rectangle => "rect",
            Preset::Ellipse => "ellipse",
            Preset::Diamond => "diamond",
            Preset::Triangle => "triangle",
            Preset::Line => "line",
        }
    }

    /// The preset with a schema name.
    pub fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "rect" => Preset::Rectangle,
            "ellipse" => Preset::Ellipse,
            "diamond" => Preset::Diamond,
            "triangle" => Preset::Triangle,
            "line" => Preset::Line,
            _ => return None,
        })
    }
}

/// What a press on the board does.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Tool {
    /// Select and move (the default).
    #[default]
    Select,
    /// Drag to pan.
    Hand,
    /// Draw a shape.
    Shape(Preset),
    /// Click to place a text box and type.
    Text,
    /// Draw a frame.
    Frame,
}

/// The size of a shape placed with a click instead of a drag, in points.
pub(crate) const CLICK_SIZE: (f64, f64) = (160.0, 96.0);

/// The size of a new text box, before its text grows it.
const TEXT_SIZE: (f64, f64) = (200.0, 24.0);

/// The size of a frame placed with a click.
const FRAME_SIZE: (f64, f64) = (480.0, 320.0);

impl BoardCanvas {
    /// The tool in use: a signal, so a tool bar can follow it.
    pub fn tool(&self) -> imaginarium_reactive::Signal<Tool> {
        self.tool
    }

    /// Takes up `tool` (the tool bar, a shortcut).
    pub fn set_tool(&mut self, tool: Tool) {
        self.tool.set_if_changed(tool);
        if tool != Tool::Select {
            self.hovered = None;
        }
    }

    /// The frame a new node at `point` (document space) goes into: the
    /// topmost visible, unlocked frame whose box holds it, or the board.
    /// Returns the parent and its box space.
    pub(crate) fn parent_at(&self, point: Point) -> (Parent, Affine) {
        let Some(items) = &self.items else {
            return (Parent::Root, Affine::IDENTITY);
        };
        let view = self.document.view();
        let mut candidates = self
            .canvas
            .items_overlapping(Rect::from_center_size(point, (1.0, 1.0)));
        candidates.reverse();
        for id in candidates {
            let Some(&node) = items.by_item.get(&id) else {
                continue;
            };
            let Some(item) = items.by_node.get(&node) else {
                continue;
            };
            let frame = view
                .node(node)
                .is_some_and(|node| matches!(node.kind(), NodeKind::Frame));
            let inside = Rect::from_origin_size(Point::ZERO, item.size)
                .contains(item.world.inverse() * point)
                && item
                    .clips
                    .iter()
                    .all(|clip| clip.rect.contains(clip.world.inverse() * point));
            if frame && inside && !hit::locked(view, node) {
                return (Parent::Node(node), item.world);
            }
        }
        (Parent::Root, Affine::IDENTITY)
    }

    /// Inserts a shape (`preset`) or a frame (`None`) with its box `area`
    /// in document space, into the frame there, and selects it. Returns
    /// its id.
    pub(crate) fn insert_box(&mut self, preset: Option<Preset>, area: Rect) -> Result<NodeId> {
        let (parent, space) = self.parent_at(area.center());
        let local = space.inverse().transform_rect_bbox(area);
        let id = self.transact(&Actor::User, |txn| {
            let kind = if preset.is_some() {
                NodeKind::Shape
            } else {
                NodeKind::Frame
            };
            let id = txn.create_node(parent, None, &kind)?;
            txn.set_geometry(
                id,
                Geometry::new(local.x0, local.y0, local.width(), local.height()),
            )?;
            if let Some(preset) = preset {
                txn.set_prop(id, "shape", Value::String(preset.name().into()))?;
            } else {
                txn.set_prop(id, "name", Value::String("Frame".into()))?;
            }
            Ok(id)
        })?;
        self.selection.set(vec![id]);
        Ok(id)
    }

    /// Inserts an empty text box with its top left at `at` (document
    /// space), selects it and asks to edit it.
    pub(crate) fn insert_text_at(&mut self, at: Point) -> Result<NodeId> {
        let (parent, space) = self.parent_at(at);
        let local = space.inverse() * at;
        let id = self.transact(&Actor::User, |txn| {
            let id = txn.create_node(parent, None, &NodeKind::Text)?;
            txn.set_geometry(
                id,
                Geometry::new(local.x, local.y, TEXT_SIZE.0, TEXT_SIZE.1),
            )?;
            Ok(id)
        })?;
        self.selection.set(vec![id]);
        self.pending_edit = Some(id);
        Ok(id)
    }

    /// Where a tool places a node it inserts without a drag: centered on
    /// `at`, at the tool's click size.
    pub(crate) fn click_area(tool: Tool, at: Point) -> Rect {
        let (w, h) = match tool {
            Tool::Frame => FRAME_SIZE,
            Tool::Shape(Preset::Line) => (CLICK_SIZE.0, 0.0),
            _ => CLICK_SIZE,
        };
        Rect::from_center_size(at, (w, h))
    }

    /// The middle of what the board shows, in document space.
    pub(crate) fn view_middle(&self) -> Point {
        self.to_document(self.bounds, self.bounds.center())
    }
}
