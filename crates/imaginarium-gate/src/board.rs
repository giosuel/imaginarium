// SPDX-License-Identifier: AGPL-3.0-or-later

//! The test content: a board of cards with a toolbar, panned with the wheel
//! or by dragging, zoomed with Ctrl + wheel. Until text arrives (M2.3) the
//! cards show placeholder bars where their text will be.

use imaginarium_render::Scene;
use imaginarium_render::kurbo::{Affine, Circle, Point, Rect, RoundedRect, Size, Vec2};
use imaginarium_render::peniko::Color;
use imaginarium_ui::{Input, Key};

const CARD: Size = Size::new(180.0, 112.0);
const GAP: f64 = 28.0;
const TOOLBAR: f64 = 44.0;
const MIN_ZOOM: f64 = 0.05;
const MAX_ZOOM: f64 = 8.0;

const INK: Color = Color::from_rgb8(0x1f, 0x23, 0x30);
const PAPER: Color = Color::from_rgb8(0xf6, 0xf5, 0xf1);
const GRID: Color = Color::from_rgb8(0xe4, 0xe2, 0xdb);
const CHROME: Color = Color::from_rgb8(0xff, 0xff, 0xff);
const CHROME_EDGE: Color = Color::from_rgb8(0xd9, 0xd6, 0xcc);
const ACCENT: Color = Color::from_rgb8(0x3b, 0x5b, 0xdb);
const TINTS: [Color; 6] = [
    Color::from_rgb8(0xfd, 0xe6, 0x8a),
    Color::from_rgb8(0xb9, 0xe6, 0xd3),
    Color::from_rgb8(0xc7, 0xd7, 0xfa),
    Color::from_rgb8(0xf9, 0xc9, 0xd4),
    Color::from_rgb8(0xe0, 0xd2, 0xf7),
    Color::from_rgb8(0xff, 0xd8, 0xb5),
];

/// The board and its camera.
pub(crate) struct Board {
    cards: usize,
    columns: usize,
    /// The world point at the top left of the viewport.
    origin: Point,
    zoom: f64,
    hovered: Option<usize>,
    pointer: Point,
    dragging: Option<Point>,
}

impl Board {
    /// A board of `cards` cards, laid out in a roughly 16:10 grid.
    pub(crate) fn new(cards: usize) -> Self {
        let columns = ((cards as f64 * 1.6).sqrt().ceil() as usize).max(1);
        Self {
            cards,
            columns,
            origin: Point::new(-40.0, -40.0 - TOOLBAR),
            zoom: 1.0,
            hovered: None,
            pointer: Point::ZERO,
            dragging: None,
        }
    }

    /// The card count.
    pub(crate) fn cards(&self) -> usize {
        self.cards
    }

    fn card_rect(&self, index: usize) -> Rect {
        let column = (index % self.columns) as f64;
        let row = (index / self.columns) as f64;
        let origin = Point::new(column * (CARD.width + GAP), row * (CARD.height + GAP));
        Rect::from_origin_size(origin, CARD)
    }

    fn to_world(&self, screen: Point) -> Point {
        self.origin + screen.to_vec2() / self.zoom
    }

    fn card_at(&self, screen: Point) -> Option<usize> {
        if screen.y < TOOLBAR {
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

    /// Moves the camera by `delta` screen pixels (content follows the
    /// pointer).
    pub(crate) fn pan(&mut self, delta: Vec2) {
        self.origin -= delta / self.zoom;
    }

    /// Zooms by `factor`, keeping the world point under `anchor` in place.
    pub(crate) fn zoom_about(&mut self, factor: f64, anchor: Point) {
        let before = self.to_world(anchor);
        self.zoom = (self.zoom * factor).clamp(MIN_ZOOM, MAX_ZOOM);
        self.origin = before - anchor.to_vec2() / self.zoom;
    }

    /// Handles input; `true` if the picture changed.
    pub(crate) fn input(&mut self, input: &Input) -> bool {
        match input {
            Input::PointerMoved(position) => {
                self.pointer = *position;
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
                pressed,
                position,
                ..
            } => {
                self.dragging = pressed.then_some(*position);
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
                ..
            } if key == "0" => {
                *self = Self::new(self.cards);
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

    /// Draws the board for a viewport of `size` logical pixels.
    pub(crate) fn paint(&self, scene: &mut Scene, size: Size) {
        scene.fill(&size.to_rect(), PAPER);
        let camera = Affine::scale(self.zoom) * Affine::translate(-self.origin.to_vec2());
        let view = Rect::from_points(
            self.to_world(Point::ZERO),
            self.to_world(size.to_vec2().to_point()),
        );
        scene.push_clip(&Rect::new(0.0, TOOLBAR, size.width, size.height));
        scene.set_transform(camera);
        self.paint_grid(scene, view);
        self.paint_cards(scene, view);
        scene.set_transform(Affine::IDENTITY);
        scene.pop();
        Self::paint_toolbar(scene, size);
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

    fn paint_cards(&self, scene: &mut Scene, view: Rect) {
        let pitch = Vec2::new(CARD.width + GAP, CARD.height + GAP);
        let first_column = (view.x0 / pitch.x).floor().max(0.0) as usize;
        let last_column = ((view.x1 / pitch.x).ceil().max(0.0) as usize).min(self.columns);
        let first_row = (view.y0 / pitch.y).floor().max(0.0) as usize;
        let last_row = (view.y1 / pitch.y).ceil().max(0.0) as usize;
        let detailed = self.zoom >= 0.35;
        for row in first_row..last_row {
            for column in first_column..last_column {
                let index = row * self.columns + column;
                if index >= self.cards {
                    return;
                }
                let rect = self.card_rect(index);
                let tint = TINTS[index % TINTS.len()];
                if !detailed {
                    scene.fill(&rect, tint);
                    continue;
                }
                let card = RoundedRect::from_rect(rect, 10.0);
                // A soft shadow, then the card.
                scene.fill(
                    &RoundedRect::from_rect(rect + Vec2::new(0.0, 3.0), 10.0),
                    INK.with_alpha(0.08),
                );
                scene.fill(&card, tint);
                // Placeholder bars for a title and two lines of text.
                let inner = rect.inset(-16.0);
                let title = Rect::new(
                    inner.x0,
                    inner.y0,
                    inner.x0 + 72.0 + (index % 5) as f64 * 14.0,
                    inner.y0 + 12.0,
                );
                scene.fill(&RoundedRect::from_rect(title, 6.0), INK.with_alpha(0.55));
                for line in 0..2 {
                    let y = inner.y0 + 32.0 + line as f64 * 18.0;
                    let width = inner.width() - ((index * 7 + line * 31) % 50) as f64;
                    scene.fill(
                        &RoundedRect::new(inner.x0, y, inner.x0 + width, y + 8.0, 4.0),
                        INK.with_alpha(0.18),
                    );
                }
                scene.fill(
                    &Circle::new((inner.x1 - 6.0, inner.y1 - 6.0), 6.0),
                    INK.with_alpha(0.25),
                );
                if self.hovered == Some(index) {
                    scene.stroke(&card, 2.5 / self.zoom, ACCENT);
                }
            }
        }
    }

    fn paint_toolbar(scene: &mut Scene, size: Size) {
        scene.fill(&Rect::new(0.0, 0.0, size.width, TOOLBAR), CHROME);
        scene.fill(
            &Rect::new(0.0, TOOLBAR - 1.0, size.width, TOOLBAR),
            CHROME_EDGE,
        );
        // The logo, then tool buttons, then a search field on the right.
        scene.fill(&RoundedRect::new(12.0, 10.0, 36.0, 34.0, 7.0), ACCENT);
        scene.fill(&Circle::new((24.0, 22.0), 5.0), CHROME);
        for (i, active) in [true, false, false, false, false].into_iter().enumerate() {
            let x = 56.0 + i as f64 * 36.0;
            let button = RoundedRect::new(x, 8.0, x + 28.0, 36.0, 6.0);
            if active {
                scene.fill(&button, ACCENT.with_alpha(0.15));
            }
            scene.fill(
                &RoundedRect::new(x + 8.0, 16.0, x + 20.0, 28.0, 3.0),
                if active { ACCENT } else { INK.with_alpha(0.6) },
            );
        }
        let search = RoundedRect::new(
            (size.width - 260.0).max(240.0),
            8.0,
            (size.width - 12.0).max(480.0),
            36.0,
            8.0,
        );
        scene.fill(&search, PAPER);
        scene.stroke(&search, 1.0, CHROME_EDGE);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hit_testing_finds_the_card_under_the_pointer() {
        let board = Board::new(100);
        // The origin starts at (-40, -84): screen (40, 84) is world (0, 0).
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
        let mut board = Board::new(100);
        let anchor = Point::new(300.0, 200.0);
        let before = board.to_world(anchor);
        board.zoom_about(2.5, anchor);
        let after = board.to_world(anchor);
        assert!((before - after).hypot() < 1e-9);
    }
}
