// SPDX-License-Identifier: AGPL-3.0-or-later

//! Painting items into tiles (`CanvasContent`), in document space.

use std::collections::HashMap;

use imaginarium_model::NodeId;
use imaginarium_render::Scene;
use imaginarium_render::canvas::{CanvasContent, ItemId};
use imaginarium_render::kurbo::{Affine, Cap, Join, Point, Rect, RoundedRect, Stroke};

use crate::items::{Clip, Item, ItemText, Items, Look};

/// Below this many device pixels per point, shapes are filled boxes: the
/// same picture at a fraction of the cost, far zoomed out.
const BOX_SCALE: f64 = 0.125;

/// Below this size in device pixels, text is a faint bar instead of
/// glyphs.
const MIN_TEXT_PIXELS: f64 = 2.0;

/// Paints a board's items into tiles, leaving out those drawn elsewhere
/// (dragged ones, drawn live on top).
pub(crate) struct ItemPainter<'a> {
    pub items: &'a HashMap<NodeId, Item>,
    pub by_item: &'a HashMap<ItemId, NodeId>,
    /// Drawn live, not in the tiles.
    pub lifted: &'a [NodeId],
}

impl<'a> ItemPainter<'a> {
    pub(crate) fn new(items: &'a Items, lifted: &'a [NodeId]) -> Self {
        Self {
            items: &items.by_node,
            by_item: &items.by_item,
            lifted,
        }
    }
}

impl CanvasContent for ItemPainter<'_> {
    fn paint_item(&mut self, id: ItemId, scene: &mut Scene, level_scale: f64) {
        let Some(node) = self.by_item.get(&id) else {
            return;
        };
        if self.lifted.contains(node) {
            return;
        }
        if let Some(item) = self.items.get(node) {
            paint(item, scene, level_scale);
        }
    }
}

/// Paints one item in document space, inside its frames' clips.
pub(crate) fn paint(item: &Item, scene: &mut Scene, level_scale: f64) {
    clipped(scene, &item.clips, Affine::IDENTITY, &mut |scene, base| {
        scene.paint_with(base.inverse() * item.world, |scene| {
            paint_look(item, scene, level_scale);
        });
    });
}

/// Runs `paint` inside every clip, outermost first. `base` is the
/// transform the scene is in.
fn clipped(
    scene: &mut Scene,
    clips: &[Clip],
    base: Affine,
    paint: &mut dyn FnMut(&mut Scene, Affine),
) {
    match clips.split_first() {
        None => paint(scene, base),
        Some((clip, rest)) => {
            scene.paint_with(base.inverse() * clip.world, |scene| {
                scene.push_clip(&clip.rect);
                clipped(scene, rest, clip.world, paint);
                scene.pop();
            });
        }
    }
}

fn outline_stroke(width: f64) -> Stroke {
    Stroke::new(width)
        .with_join(Join::Round)
        .with_caps(Cap::Round)
}

/// Paints the item in its box space.
fn paint_look(item: &Item, scene: &mut Scene, level_scale: f64) {
    let size = item.size;
    match &item.look {
        Look::Shape {
            outline,
            style,
            label,
        } => {
            let layer = style.opacity < 1.0;
            if layer {
                scene.push_opacity(style.opacity);
            }
            if level_scale < BOX_SCALE {
                if let Some(color) = style.fill.or(style.stroke) {
                    scene.fill(&Rect::from_origin_size(Point::ZERO, size), color);
                }
            } else {
                if outline.closed
                    && let Some(fill) = style.fill
                {
                    scene.fill(&outline.path, fill);
                }
                if let Some(stroke) = style.stroke
                    && style.stroke_width > 0.0
                {
                    scene.stroke_with(&outline.path, &outline_stroke(style.stroke_width), stroke);
                }
                if let Some(label) = label {
                    paint_text(label, scene, level_scale);
                }
            }
            if layer {
                scene.pop();
            }
        }
        Look::Text { text, opacity } => {
            let layer = *opacity < 1.0;
            if layer {
                scene.push_opacity(*opacity);
            }
            if let Some(text) = text {
                paint_text(text, scene, level_scale);
            }
            if layer {
                scene.pop();
            }
        }
        Look::Frame { style } => {
            let rect = Rect::from_origin_size(Point::ZERO, size);
            let shape =
                RoundedRect::from_rect(rect, style.radius.min(size.width.min(size.height) / 2.0));
            if let Some(fill) = style.fill {
                scene.fill(&shape, fill);
            }
            if let Some(stroke) = style.stroke
                && style.stroke_width > 0.0
            {
                scene.stroke_with(&shape, &outline_stroke(style.stroke_width), stroke);
            }
        }
        Look::Placeholder => {
            let rect = Rect::from_origin_size(Point::ZERO, size);
            let dashed = Stroke::new(1.0).with_dashes(0.0, [4.0, 4.0]);
            scene.stroke_with(&rect, &dashed, crate::style::INK.with_alpha(0.6));
        }
    }
}

fn paint_text(text: &ItemText, scene: &mut Scene, level_scale: f64) {
    if f64::from(text.look.size) * level_scale < MIN_TEXT_PIXELS {
        let laid = text.block.size();
        let rect = Rect::from_origin_size(
            text.origin
                + imaginarium_render::kurbo::Vec2::new((text.extent.width - laid.width) / 2.0, 0.0),
            laid,
        );
        scene.fill(&rect, text.look.color.with_alpha(0.25));
        return;
    }
    text.block.paint(scene, text.origin, false);
}
