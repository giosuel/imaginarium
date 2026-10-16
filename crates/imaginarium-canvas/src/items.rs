// SPDX-License-Identifier: AGPL-3.0-or-later

//! Nodes to canvas items (`canvas.md` §2): one item per visible node that
//! paints something, with its world transform, z key, look and bounds.

use std::collections::HashMap;
use std::rc::Rc;

use imaginarium_model::view::NodeView;
use imaginarium_model::{DocumentView, NodeId, NodeKind, Parent};
use imaginarium_render::canvas::ItemId;
use imaginarium_render::kurbo::{Affine, Point, Rect, Shape, Size, Vec2};
use imaginarium_render::peniko::Color;
use imaginarium_text::{Align, Fonts, TextBlock, TextStyle};
use imaginarium_ui::ThemeKind;

use crate::shapes::{Outline, shape_path};
use crate::style::{self, ShapeStyle, TextLook};

/// The gap between neighbouring z keys after a full walk, so a node
/// inserted between two others gets a key between theirs.
pub(crate) const Z_GAP: u64 = 1 << 20;

/// The space between a shape's edge and its label, in points.
const LABEL_PADDING: f64 = 8.0;

/// The colors a board takes from its theme: frames without a style of
/// their own, and text without a color.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BoardColors {
    /// Which theme, for the palette's versions.
    pub theme: ThemeKind,
    /// Text without a color of its own.
    pub text: Color,
    /// A frame's background.
    pub frame_bg: Color,
    /// A frame's border.
    pub frame_border: Color,
}

impl BoardColors {
    /// The colors of `theme` (the mockups' board colors, `shell.md` §7).
    pub fn of(theme: &imaginarium_ui::Theme) -> Self {
        let kind = theme.kind();
        let (frame_bg, frame_border) = match kind {
            ThemeKind::Dark => (
                Color::from_rgb8(0x1A, 0x1A, 0x1E),
                Color::from_rgb8(0x33, 0x33, 0x3B),
            ),
            ThemeKind::Light => (
                Color::from_rgb8(0xFF, 0xFF, 0xFF),
                Color::from_rgb8(0xD9, 0xD9, 0xDE),
            ),
        };
        Self {
            theme: kind,
            text: theme.color(imaginarium_ui::Token::Text),
            frame_bg,
            frame_border,
        }
    }
}

/// A frame around an item: its children are clipped to its box.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Clip {
    /// The frame's box space to document space.
    pub world: Affine,
    /// The frame's box, in its box space.
    pub rect: Rect,
}

/// Laid-out text in an item, at `origin` in box space.
#[derive(Clone)]
pub(crate) struct ItemText {
    pub block: Rc<TextBlock>,
    pub origin: Point,
    /// The area its lines line up in.
    pub extent: Size,
    pub look: TextLook,
}

/// What an item paints.
#[derive(Clone)]
pub(crate) enum Look {
    /// A shape: its outline, style and label.
    Shape {
        outline: Rc<Outline>,
        style: ShapeStyle,
        label: Option<ItemText>,
    },
    /// A text box.
    Text {
        text: Option<ItemText>,
        opacity: f32,
    },
    /// A frame's background and border (its children are items of their
    /// own, clipped to it).
    Frame { style: ShapeStyle },
    /// A kind this version doesn't draw yet: a dashed box (schema rule 4).
    Placeholder,
}

/// A node on the canvas.
#[derive(Clone)]
pub(crate) struct Item {
    pub id: ItemId,
    pub z: u64,
    /// Box space (`(0, 0)` to `size`) to document space.
    pub world: Affine,
    pub size: Size,
    pub look: Look,
    /// What its pixels can touch, in document space.
    pub bounds: Rect,
    /// The frames it is inside, outermost first.
    pub clips: Rc<[Clip]>,
    /// The frame's name, for frames (drawn on top of the tiles).
    pub name: Option<String>,
}

/// Every item of a board, by node.
#[derive(Default)]
pub(crate) struct Items {
    pub by_node: HashMap<NodeId, Item>,
    pub by_item: HashMap<ItemId, NodeId>,
    /// Item ids are handed out once and never reused.
    next_item: u64,
}

/// The transform from a node's box to its parent's space: to its corner,
/// then rotated around its center (schema-v1.md §5.2).
pub(crate) fn local_transform(x: f64, y: f64, w: f64, h: f64, degrees: f64) -> Affine {
    let mut local = Affine::translate(Vec2::new(x, y));
    if degrees != 0.0 && degrees.is_finite() {
        local *= Affine::rotate_about(degrees.to_radians(), Point::new(w / 2.0, h / 2.0));
    }
    local
}

impl Items {
    /// The item id of `node`, kept for the node's life on this canvas.
    fn item_id(&mut self, node: NodeId) -> ItemId {
        if let Some(item) = self.by_node.get(&node) {
            return item.id;
        }
        self.next_item += 1;
        let id = ItemId(self.next_item);
        self.by_item.insert(id, node);
        id
    }

    /// Builds every item of `view`, in z order with `Z_GAP` between keys.
    pub(crate) fn build(view: &DocumentView, fonts: &mut Fonts, colors: &BoardColors) -> Self {
        let mut items = Self::default();
        let mut z = 0;
        let mut walker = Walker {
            view,
            fonts,
            colors,
            items: &mut items,
            z: &mut z,
        };
        walker.children(Parent::Root, Affine::IDENTITY, &Rc::from([]));
        items
    }
}

struct Walker<'a> {
    view: &'a DocumentView,
    fonts: &'a mut Fonts,
    colors: &'a BoardColors,
    items: &'a mut Items,
    z: &'a mut u64,
}

impl Walker<'_> {
    fn children(&mut self, parent: Parent, world: Affine, clips: &Rc<[Clip]>) {
        let children: Vec<NodeId> = self.view.children(parent).collect();
        for child in children {
            self.node(child, world, clips);
        }
    }

    fn node(&mut self, id: NodeId, parent_world: Affine, clips: &Rc<[Clip]>) {
        let Some(node) = self.view.node(id) else {
            return;
        };
        if node.hidden() {
            return;
        }
        let geometry = node.geometry().unwrap_or_default();
        let size = Size::new(geometry.w.max(0.0), geometry.h.max(0.0));
        let world = parent_world
            * local_transform(geometry.x, geometry.y, size.width, size.height, geometry.r);
        match node.kind() {
            // Groups paint nothing themselves; their children are placed in
            // the group's space.
            NodeKind::Group | NodeKind::Page | NodeKind::Slide => {
                self.children(Parent::Node(id), world, clips)
            }
            NodeKind::Frame => {
                let style = frame_style(self.view, id, self.colors);
                let rect = Rect::from_origin_size(Point::ZERO, size);
                let bounds = world.transform_rect_bbox(rect).inflate(
                    style.stroke_width / 2.0 + 1.0,
                    style.stroke_width / 2.0 + 1.0,
                );
                let name = node
                    .name()
                    .map(str::to_owned)
                    .or_else(|| Some("Frame".to_owned()));
                self.push(id, world, size, Look::Frame { style }, bounds, clips, name);
                let mut inner: Vec<Clip> = clips.to_vec();
                inner.push(Clip { world, rect });
                self.children(Parent::Node(id), world, &Rc::from(inner));
            }
            NodeKind::Shape => {
                let style = style::shape_style(self.view, id, self.colors.theme);
                let outline = shape_path(node, size, style.radius);
                let label = self.text(node, size, Align::Center, style::LABEL_SIZE, 500.0, 1.3);
                let reach = if style.stroke.is_some() {
                    style.stroke_width / 2.0 + 1.0
                } else {
                    1.0
                };
                let mut bounds = (world * outline.path.clone())
                    .bounding_box()
                    .inflate(reach, reach);
                if let Some(label) = &label {
                    bounds = bounds.union(text_bounds(world, label));
                }
                let look = Look::Shape {
                    outline: Rc::new(outline),
                    style,
                    label,
                };
                self.push(id, world, size, look, bounds, clips, None);
            }
            NodeKind::Text => {
                let text = self.text(node, size, Align::Start, style::TEXT_SIZE, 400.0, 1.5);
                let mut bounds = world
                    .transform_rect_bbox(Rect::from_origin_size(Point::ZERO, size))
                    .inflate(1.0, 1.0);
                if let Some(text) = &text {
                    bounds = bounds.union(text_bounds(world, text));
                }
                let opacity = style::shape_style(self.view, id, self.colors.theme).opacity;
                self.push(
                    id,
                    world,
                    size,
                    Look::Text { text, opacity },
                    bounds,
                    clips,
                    None,
                );
            }
            // Images, connectors and paint layers come in later milestones;
            // newer kinds are leaf boxes (schema rule 4).
            _ => {
                let bounds = world
                    .transform_rect_bbox(Rect::from_origin_size(Point::ZERO, size))
                    .inflate(2.0, 2.0);
                self.push(id, world, size, Look::Placeholder, bounds, clips, None);
            }
        }
    }

    /// The node's text laid out in its box: wrapped at the box's width,
    /// at the top for text boxes (`Align::Start`), in the middle for
    /// labels (`Align::Center`).
    fn text(
        &mut self,
        node: &NodeView,
        size: Size,
        align: Align,
        default_size: f32,
        weight: f32,
        line_height: f32,
    ) -> Option<ItemText> {
        let text = node.text()?.to_plain_string();
        let text = text.strip_suffix('\n').unwrap_or(&text);
        if text.trim().is_empty() {
            return None;
        }
        let look = style::text_look(
            self.view,
            node.id(),
            self.colors.theme,
            self.colors.text,
            default_size,
            weight,
        );
        let padding = if align == Align::Center {
            LABEL_PADDING
        } else {
            0.0
        };
        let width = (size.width - 2.0 * padding).max(1.0) as f32;
        let text_style = TextStyle::new(look.size)
            .weight(look.weight)
            .color(look.color)
            .line_height(line_height)
            .align(align);
        let block = TextBlock::new(self.fonts, text, &text_style, Some(width));
        let laid = block.size();
        // Lines line up within the wrapping width, so the block spans it.
        let extent = Size::new(laid.width.max(f64::from(width)), laid.height);
        let origin = if align == Align::Center {
            // A label's middle: the box's, or a triangle's centroid.
            let triangle = node
                .prop("shape")
                .and_then(imaginarium_model::Value::as_str)
                == Some("triangle");
            let middle = if triangle {
                size.height * 2.0 / 3.0
            } else {
                size.height / 2.0
            };
            Point::new(
                padding,
                (middle - laid.height / 2.0)
                    .max(0.0)
                    .min((size.height - laid.height).max(0.0)),
            )
        } else {
            Point::ZERO
        };
        Some(ItemText {
            block: Rc::new(block),
            origin,
            extent,
            look,
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn push(
        &mut self,
        node: NodeId,
        world: Affine,
        size: Size,
        look: Look,
        bounds: Rect,
        clips: &Rc<[Clip]>,
        name: Option<String>,
    ) {
        // Clipped by its frames: it can't paint outside them.
        let bounds = clips.iter().fold(bounds, |bounds, clip| {
            bounds.intersect(clip.world.transform_rect_bbox(clip.rect).inflate(1.0, 1.0))
        });
        *self.z += Z_GAP;
        let id = self.items.item_id(node);
        self.items.by_node.insert(
            node,
            Item {
                id,
                z: *self.z,
                world,
                size,
                look,
                bounds,
                clips: clips.clone(),
                name,
            },
        );
    }
}

/// Where an item's text can paint, in document space.
fn text_bounds(world: Affine, text: &ItemText) -> Rect {
    let rect = Rect::from_origin_size(text.origin, text.extent);
    world.transform_rect_bbox(rect).inflate(2.0, 2.0)
}

/// A frame's style: its own, else the theme's frame colors, a 1 pt border
/// and 4 pt corners.
fn frame_style(view: &DocumentView, node: NodeId, colors: &BoardColors) -> ShapeStyle {
    let own = style::shape_style(view, node, colors.theme);
    let has = |prop: &str| view.style_prop(node, prop).is_some();
    ShapeStyle {
        fill: if has("fill") {
            own.fill
        } else {
            Some(colors.frame_bg)
        },
        stroke: if has("stroke") {
            own.stroke
        } else {
            Some(colors.frame_border)
        },
        stroke_width: if has("stroke_w") {
            own.stroke_width
        } else {
            1.0
        },
        opacity: own.opacity,
        radius: if has("radius") { own.radius } else { 4.0 },
    }
}
