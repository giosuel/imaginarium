// SPDX-License-Identifier: AGPL-3.0-or-later

//! The test content: a board of cards with text, panned with the wheel or
//! by dragging, zoomed with Ctrl + wheel. Double-click a card to edit its
//! text in place, at any zoom.

use std::collections::HashMap;
use std::time::Instant;

use accesskit::{Node, NodeId, Role};
use imaginarium_render::Scene;
use imaginarium_render::kurbo::{Affine, Point, Rect, RoundedRect, Size, Vec2};
use imaginarium_render::peniko::Color;
use imaginarium_text::{Fonts, TextBlock, TextStyle};
use imaginarium_ui::{AccessTree, Input, Key, TextField, access_rect};

use crate::content;

const CARD: Size = Size::new(180.0, 112.0);
const GAP: f64 = 28.0;
const PADDING: f64 = 14.0;
const MIN_ZOOM: f64 = 0.05;
const MAX_ZOOM: f64 = 8.0;
/// Below this zoom, cards are drawn as plain tiles without text.
const TEXT_ZOOM: f64 = 0.3;

pub(crate) const INK: Color = Color::from_rgb8(0x1f, 0x23, 0x30);
pub(crate) const PAPER: Color = Color::from_rgb8(0xf6, 0xf5, 0xf1);
const GRID: Color = Color::from_rgb8(0xe0, 0xde, 0xd6);
pub(crate) const ACCENT: Color = Color::from_rgb8(0x3b, 0x5b, 0xdb);
const TINTS: [Color; 6] = [
    Color::from_rgb8(0xfd, 0xe6, 0x8a),
    Color::from_rgb8(0xb9, 0xe6, 0xd3),
    Color::from_rgb8(0xc7, 0xd7, 0xfa),
    Color::from_rgb8(0xf9, 0xc9, 0xd4),
    Color::from_rgb8(0xe0, 0xd2, 0xf7),
    Color::from_rgb8(0xff, 0xd8, 0xb5),
];

/// Node ids: cards from here, the edited card's field after them.
const CARD_NODES: u64 = 10_000;
const EDIT_NODES: u64 = 1_000_000;

fn title_style() -> TextStyle {
    TextStyle::new(14.0).weight(650.0).color(INK)
}

fn body_style() -> TextStyle {
    TextStyle::new(12.5)
        .color(INK.with_alpha(0.75))
        .line_height(1.35)
}

/// A card's text, laid out.
struct CardText {
    title: TextBlock,
    body: TextBlock,
}

/// The board, its camera and the card being edited.
pub(crate) struct Board {
    cards: usize,
    columns: usize,
    titles: Vec<String>,
    bodies: Vec<String>,
    /// Lowercase title and body per card, for searching.
    haystacks: Vec<String>,
    /// Laid-out text of cards that were on screen.
    texts: HashMap<usize, CardText>,
    /// Layouts built in the last paint (cache misses), for the report.
    pub(crate) last_layouts: usize,
    /// The world point at the top left of the viewport.
    origin: Point,
    zoom: f64,
    hovered: Option<usize>,
    dragging: Option<Point>,
    /// The card whose body is being edited, and its field.
    editing: Option<(usize, TextField)>,
    /// Cards whose title matches the search; `None`: no search.
    matches: Option<Vec<bool>>,
    /// The viewport (window coordinates) from the last paint.
    viewport: Rect,
}

impl Board {
    /// A board of `cards` cards, laid out in a roughly 16:10 grid.
    pub(crate) fn new(cards: usize) -> Self {
        let columns = ((cards as f64 * 1.6).sqrt().ceil() as usize).max(1);
        let mut board = Self {
            cards,
            columns,
            titles: (0..cards).map(content::title).collect(),
            bodies: (0..cards).map(content::body).collect(),
            haystacks: Vec::new(),
            texts: HashMap::new(),
            last_layouts: 0,
            origin: Point::new(-40.0, -40.0),
            zoom: 1.0,
            hovered: None,
            dragging: None,
            editing: None,
            matches: None,
            viewport: Rect::ZERO,
        };
        board.haystacks = (0..cards).map(|index| board.haystack(index)).collect();
        board
    }

    fn haystack(&self, index: usize) -> String {
        format!("{}\n{}", self.titles[index], self.bodies[index]).to_lowercase()
    }

    /// The card count.
    pub(crate) fn cards(&self) -> usize {
        self.cards
    }

    /// The zoom factor.
    pub(crate) fn zoom(&self) -> f64 {
        self.zoom
    }

    fn card_rect(&self, index: usize) -> Rect {
        card_rect(self.columns, index)
    }

    fn body_rect(&self, index: usize) -> Rect {
        body_rect(card_rect(self.columns, index))
    }

    /// World coordinates to window coordinates.
    fn camera(&self) -> Affine {
        Affine::translate(self.viewport.origin().to_vec2())
            * Affine::scale(self.zoom)
            * Affine::translate(-self.origin.to_vec2())
    }

    fn to_world(&self, screen: Point) -> Point {
        self.origin + (screen - self.viewport.origin()) / self.zoom
    }

    fn card_at(&self, screen: Point) -> Option<usize> {
        if !self.viewport.contains(screen) {
            return None;
        }
        let world = self.to_world(screen);
        if world.x < 0.0 || world.y < 0.0 {
            return None;
        }
        let column = (world.x / (CARD.width + GAP)) as usize;
        let row = (world.y / (CARD.height + GAP)) as usize;
        let index = row * self.columns + column;
        (column < self.columns && index < self.cards && self.card_rect(index).contains(world))
            .then_some(index)
    }

    /// Moves the camera by `delta` screen pixels (content follows).
    pub(crate) fn pan(&mut self, delta: Vec2) {
        self.origin -= delta / self.zoom;
    }

    /// Zooms by `factor`, keeping the world point under `anchor` in place.
    pub(crate) fn zoom_about(&mut self, factor: f64, anchor: Point) {
        let before = self.to_world(anchor);
        self.zoom = (self.zoom * factor).clamp(MIN_ZOOM, MAX_ZOOM);
        self.origin = before - (anchor - self.viewport.origin()) / self.zoom;
    }

    /// Highlights cards whose title contains `query` (case-insensitive);
    /// an empty query clears it. Returns the number of matches.
    pub(crate) fn search(&mut self, query: &str) -> Option<usize> {
        let query = query.trim().to_lowercase();
        if query.is_empty() {
            self.matches = None;
            return None;
        }
        let matches: Vec<bool> = self
            .haystacks
            .iter()
            .map(|haystack| haystack.contains(&query))
            .collect();
        let count = matches.iter().filter(|&&m| m).count();
        self.matches = Some(matches);
        Some(count)
    }

    /// Ends editing, keeping the edited text.
    pub(crate) fn stop_editing(&mut self, fonts: &mut Fonts) {
        if let Some((index, mut field)) = self.editing.take() {
            field.blur(fonts);
            self.bodies[index] = field.text();
            self.haystacks[index] = self.haystack(index);
            self.texts.remove(&index);
        }
    }

    fn start_editing(&mut self, fonts: &mut Fonts, index: usize) {
        self.stop_editing(fonts);
        let mut field = TextField::new(&self.bodies[index], body_style(), true).with_padding(2.0);
        field.focus();
        self.texts.remove(&index);
        self.editing = Some((index, field));
    }

    /// An input in window coordinates, in world coordinates.
    fn world_input(&self, input: &Input) -> Input {
        match input {
            Input::PointerMoved(position) => Input::PointerMoved(self.to_world(*position)),
            Input::PointerButton {
                primary,
                pressed,
                position,
                clicks,
                modifiers,
            } => Input::PointerButton {
                primary: *primary,
                pressed: *pressed,
                position: self.to_world(*position),
                clicks: *clicks,
                modifiers: *modifiers,
            },
            other => other.clone(),
        }
    }

    /// Handles input; `true` if the picture changed.
    pub(crate) fn input(&mut self, fonts: &mut Fonts, input: &Input) -> bool {
        // The edited card's field sees input first, in world coordinates.
        if let Some((index, mut field)) = self.editing.take() {
            let bounds = self.body_rect(index);
            let response = field.input(fonts, &self.world_input(input), bounds);
            self.editing = Some((index, field));
            if response.blurred {
                self.stop_editing(fonts);
            }
            if response.handled {
                return true;
            }
            if response.blurred && matches!(input, Input::PointerButton { .. }) {
                // The press that ended editing may start a pan or another
                // edit: fall through.
                self.handle(fonts, input);
                return true;
            }
        }
        self.handle(fonts, input)
    }

    fn handle(&mut self, fonts: &mut Fonts, input: &Input) -> bool {
        match input {
            Input::PointerMoved(position) => {
                if let Some(last) = self.dragging {
                    self.pan(*position - last);
                    self.dragging = Some(*position);
                    return true;
                }
                self.hover(self.card_at(*position))
            }
            Input::PointerLeft => self.hover(None),
            Input::PointerButton {
                primary: true,
                pressed: true,
                position,
                clicks,
                ..
            } => {
                let card = self.card_at(*position);
                if let (Some(index), 2) = (card, clicks) {
                    self.dragging = None;
                    self.start_editing(fonts, index);
                    // Place the caret where the double click was.
                    let bounds = self.body_rect(index);
                    let world = self.to_world(*position);
                    if let Some((_, field)) = &mut self.editing {
                        let click = Input::PointerButton {
                            primary: true,
                            pressed: true,
                            position: Point::new(
                                world.x.clamp(bounds.x0, bounds.x1),
                                world.y.clamp(bounds.y0, bounds.y1),
                            ),
                            clicks: 1,
                            modifiers: Default::default(),
                        };
                        field.input(fonts, &click, bounds);
                    }
                    return true;
                }
                if self.viewport.contains(*position) {
                    self.dragging = Some(*position);
                }
                false
            }
            Input::PointerButton {
                primary: true,
                pressed: false,
                ..
            } => {
                self.dragging = None;
                false
            }
            Input::Scroll {
                delta,
                position,
                modifiers,
            } if modifiers.command() => {
                self.zoom_about((delta.y * 0.004).exp(), *position);
                self.hover(self.card_at(*position));
                true
            }
            Input::Scroll {
                delta, position, ..
            } => {
                self.pan(*delta);
                self.hover(self.card_at(*position));
                true
            }
            Input::Key {
                key: Key::Character(key),
                pressed: true,
                modifiers,
                ..
            } if key == "0" && modifiers.command() => {
                self.origin = Point::new(-40.0, -40.0);
                self.zoom = 1.0;
                true
            }
            _ => false,
        }
    }

    fn hover(&mut self, card: Option<usize>) -> bool {
        let changed = self.hovered != card;
        self.hovered = card;
        changed
    }

    /// The caret blink of the edited card.
    pub(crate) fn wake_at(&self) -> Option<Instant> {
        self.editing.as_ref().and_then(|(_, field)| field.wake_at())
    }

    /// Where the input method's window goes while a card is edited.
    pub(crate) fn ime_area(&mut self, fonts: &mut Fonts) -> Option<Rect> {
        let camera = self.camera();
        let columns = self.columns;
        let (index, field) = self.editing.as_mut()?;
        let body = body_rect(card_rect(columns, *index));
        let area = field.ime_area(fonts, body)?;
        Some(camera.transform_rect_bbox(area))
    }

    /// Draws the board into `viewport` (window coordinates).
    pub(crate) fn paint(&mut self, fonts: &mut Fonts, scene: &mut Scene, viewport: Rect) {
        self.viewport = viewport;
        self.last_layouts = 0;
        scene.fill(&viewport, PAPER);
        let view = Rect::from_points(
            self.to_world(viewport.origin()),
            self.to_world(Point::new(viewport.x1, viewport.y1)),
        );
        scene.push_clip(&viewport);
        scene.set_transform(self.camera());
        self.paint_grid(scene, view);
        self.paint_cards(fonts, scene, view);
        scene.set_transform(Affine::IDENTITY);
        scene.pop();
    }

    fn paint_grid(&self, scene: &mut Scene, view: Rect) {
        // Dots every 40 units, sparser when zoomed out so the count stays
        // bounded.
        let mut step = 40.0;
        while step * self.zoom < 16.0 {
            step *= 4.0;
        }
        let radius = 1.2 / self.zoom;
        let mut y = (view.y0 / step).floor() * step;
        while y <= view.y1 {
            let mut x = (view.x0 / step).floor() * step;
            while x <= view.x1 {
                scene.fill(
                    &Rect::new(x - radius, y - radius, x + radius, y + radius),
                    GRID,
                );
                x += step;
            }
            y += step;
        }
    }

    fn visible_cards(&self, view: Rect) -> impl Iterator<Item = usize> + use<> {
        let pitch = Vec2::new(CARD.width + GAP, CARD.height + GAP);
        let first_column = (view.x0 / pitch.x).floor().max(0.0) as usize;
        let last_column = ((view.x1 / pitch.x).ceil().max(0.0) as usize).min(self.columns);
        let first_row = (view.y0 / pitch.y).floor().max(0.0) as usize;
        let last_row = (view.y1 / pitch.y).ceil().max(0.0) as usize;
        let (columns, cards) = (self.columns, self.cards);
        (first_row..last_row)
            .flat_map(move |row| {
                (first_column..last_column).map(move |column| row * columns + column)
            })
            .filter(move |&index| index < cards)
    }

    fn paint_cards(&mut self, fonts: &mut Fonts, scene: &mut Scene, view: Rect) {
        let detailed = self.zoom >= TEXT_ZOOM;
        let editing = self.editing.as_ref().map(|(index, _)| *index);
        for index in self.visible_cards(view) {
            let rect = self.card_rect(index);
            let dimmed = self.matches.as_ref().is_some_and(|matches| !matches[index]);
            let tint = TINTS[index % TINTS.len()];
            let tint = if dimmed { tint.with_alpha(0.3) } else { tint };
            if !detailed {
                scene.fill(&rect, tint);
                continue;
            }
            let card = RoundedRect::from_rect(rect, 10.0);
            // A soft shadow, then the card.
            scene.fill(
                &RoundedRect::from_rect(rect + Vec2::new(0.0, 3.0), 10.0),
                INK.with_alpha(if dimmed { 0.03 } else { 0.08 }),
            );
            scene.fill(&card, tint);
            if dimmed {
                scene.push_opacity(0.35);
            }
            scene.push_clip(&card);
            if !self.texts.contains_key(&index) {
                self.last_layouts += 1;
                let width = (CARD.width - 2.0 * PADDING) as f32;
                let text = CardText {
                    // One line: long titles are cut at the card's edge.
                    title: TextBlock::new(fonts, &self.titles[index], &title_style(), None),
                    body: TextBlock::new(fonts, &self.bodies[index], &body_style(), Some(width)),
                };
                self.texts.insert(index, text);
            }
            if let Some(text) = self.texts.get(&index) {
                text.title
                    .paint(scene, Point::new(rect.x0 + PADDING, rect.y0 + 12.0), false);
                if editing != Some(index) {
                    scene.push_clip(&body_rect(rect));
                    text.body
                        .paint(scene, Point::new(rect.x0 + PADDING, rect.y0 + 38.0), false);
                    scene.pop();
                }
            }
            scene.pop();
            if dimmed {
                scene.pop();
            }
            if editing == Some(index) {
                scene.fill(&self.body_rect(index), Color::WHITE.with_alpha(0.55));
                scene.stroke(&card, 2.5 / self.zoom, ACCENT);
            } else if self.hovered == Some(index) {
                scene.stroke(&card, 2.0 / self.zoom, ACCENT.with_alpha(0.7));
            }
        }
        let columns = self.columns;
        if let Some((index, field)) = &mut self.editing {
            field.paint(fonts, scene, body_rect(card_rect(columns, *index)), ACCENT);
        }
    }

    /// Adds the visible cards (and the edited card's field) under a
    /// "Board" node.
    pub(crate) fn access(&mut self, fonts: &mut Fonts, tree: &mut AccessTree, id: NodeId) {
        let mut board = Node::new(Role::Canvas);
        board.set_label("Board");
        board.set_bounds(access_rect(self.viewport));
        let view = Rect::from_points(
            self.to_world(self.viewport.origin()),
            self.to_world(Point::new(self.viewport.x1, self.viewport.y1)),
        );
        let camera = self.camera();
        let mut children = Vec::new();
        for index in self.visible_cards(view) {
            let card_id = NodeId(CARD_NODES + index as u64);
            let mut card = Node::new(Role::Note);
            card.set_label(self.titles[index].clone());
            card.set_description(self.bodies[index].clone());
            card.set_bounds(access_rect(
                camera.transform_rect_bbox(self.card_rect(index)),
            ));
            let body = camera.transform_rect_bbox(self.body_rect(index));
            if let Some((editing, field)) = &mut self.editing
                && *editing == index
            {
                let field_id = field.access(fonts, tree, EDIT_NODES, body, "Card text");
                card.set_children(vec![field_id]);
            }
            tree.push(card_id, card);
            children.push(card_id);
        }
        board.set_children(children);
        tree.push(id, board);
    }
}

/// Card `index` in a grid of `columns`, in world coordinates.
fn card_rect(columns: usize, index: usize) -> Rect {
    let column = (index % columns) as f64;
    let row = (index / columns) as f64;
    let origin = Point::new(column * (CARD.width + GAP), row * (CARD.height + GAP));
    Rect::from_origin_size(origin, CARD)
}

/// Where a card's body text goes (and its editor while it is edited).
fn body_rect(card: Rect) -> Rect {
    Rect::new(
        card.x0 + PADDING - 2.0,
        card.y0 + 36.0,
        card.x1 - PADDING + 2.0,
        card.y1 - 8.0,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn board() -> Board {
        let mut board = Board::new(100);
        board.viewport = Rect::new(0.0, 44.0, 1280.0, 800.0);
        board
    }

    #[test]
    fn hit_testing_finds_the_card_under_the_pointer() {
        let board = board();
        // The origin starts at (-40, -40): screen (40, 84) is world (0, 0).
        assert_eq!(board.card_at(Point::new(41.0, 85.0)), Some(0));
        assert_eq!(
            board.card_at(Point::new(40.0 + CARD.width + 1.0, 90.0)),
            None,
            "the gap"
        );
        assert_eq!(board.card_at(Point::new(10.0, 10.0)), None, "the toolbar");
    }

    #[test]
    fn zooming_keeps_the_anchor_in_place() {
        let mut board = board();
        let anchor = Point::new(300.0, 200.0);
        let before = board.to_world(anchor);
        board.zoom_about(2.5, anchor);
        let after = board.to_world(anchor);
        assert!((before - after).hypot() < 1e-9);
    }

    #[test]
    fn search_matches_titles_and_bodies() {
        let mut board = board();
        assert_eq!(board.search(""), None);
        let count = board.search("planning").unwrap_or(0);
        assert!(count > 0 && count < 100);
        assert_eq!(board.search("zzz-not-there"), Some(0));
    }
}
