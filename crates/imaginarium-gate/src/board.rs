// SPDX-License-Identifier: AGPL-3.0-or-later

//! The test content: a board of 2,000 cards with text, or of 10,000
//! shapes, drawn through the canvas tile cache. Pan with the wheel or by
//! dragging, zoom with Ctrl + wheel. Double-click a card to edit its text
//! in place, at any zoom; drag shapes to move them.

use std::collections::HashMap;
use std::time::Instant;

use accesskit::{Node, NodeId, Role};
use imaginarium_render::Scene;
use imaginarium_render::canvas::{Canvas, CanvasContent, CanvasStats, ItemId, Viewport};
use imaginarium_render::kurbo::{Affine, Point, Rect, RoundedRect, Size, Vec2};
use imaginarium_render::peniko::Color;
use imaginarium_text::{Fonts, TextBlock, TextStyle};
use imaginarium_ui::{AccessTree, Clipboard, Input, Key, TextField, access_rect};

#[path = "board_edit.rs"]
mod edit;

use std::collections::BTreeSet;

use crate::content;
use crate::shapes::{self, Shape};

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

/// Which board is shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Mode {
    /// 2,000 cards with text (search, editing).
    Cards,
    /// 10,000 shapes (criteria 1 and 2).
    Shapes,
}

/// The board, its camera and the card being edited.
pub(crate) struct Board {
    mode: Mode,
    cards_canvas: Canvas,
    shapes_canvas: Canvas,
    shapes: Vec<Shape>,
    /// Draw through tiles (`false`: every item every frame, to compare).
    pub(crate) tiles: bool,
    /// What the canvas did in the last paint.
    pub(crate) last_canvas: CanvasStats,
    /// Visible tiles still to be rendered: another frame is needed.
    pub(crate) owed: bool,
    /// Frames painted so far: the first is drawn without tiles (cold
    /// start), the tiles fill in from the second.
    painted: u64,
    last_viewport: Option<Viewport>,
    hovered_shape: Option<usize>,
    selection: BTreeSet<usize>,
    /// Shapes being dragged, drawn live instead of in the tiles.
    lifted: Option<edit::Lift>,
    /// A rubber-band selection: where it began and where the pointer is.
    rubber: Option<(Point, Point)>,
    undo: Vec<edit::ShapeEdit>,
    redo: Vec<edit::ShapeEdit>,
    deleted: Vec<bool>,
    top_z: u64,
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
    pub(crate) fn new(cards: usize, shape_count: usize, tiles: bool) -> Self {
        let columns = ((cards as f64 * 1.6).sqrt().ceil() as usize).max(1);
        let mut cards_canvas = Canvas::new();
        for index in 0..cards {
            cards_canvas.upsert(
                ItemId(index as u64),
                index as u64,
                card_bounds(card_rect(columns, index)),
            );
        }
        let shapes = shapes::generate(shape_count);
        let mut shapes_canvas = Canvas::new();
        for (index, shape) in shapes.iter().enumerate() {
            shapes_canvas.upsert(ItemId(index as u64), shape.z, shape.bounds());
        }
        let mut board = Self {
            mode: Mode::Cards,
            cards_canvas,
            shapes_canvas,
            top_z: shapes.len() as u64,
            shapes,
            tiles,
            last_canvas: CanvasStats::default(),
            owed: false,
            painted: 0,
            last_viewport: None,
            hovered_shape: None,
            selection: BTreeSet::new(),
            lifted: None,
            rubber: None,
            undo: Vec::new(),
            redo: Vec::new(),
            deleted: vec![false; shape_count],
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

    /// The shape count.
    pub(crate) fn shape_count(&self) -> usize {
        self.shapes.len()
    }

    /// Which board is shown.
    pub(crate) fn mode(&self) -> Mode {
        self.mode
    }

    /// The renderer's time for the tile renders of the last frame, which
    /// keeps the shown board's tile work within its time budget.
    pub(crate) fn note_render_time(&mut self, renderer: std::time::Duration) {
        match self.mode {
            Mode::Cards => self.cards_canvas.note_render_time(renderer),
            Mode::Shapes => self.shapes_canvas.note_render_time(renderer),
        }
    }

    /// Shows the other board (each keeps its tiles).
    pub(crate) fn set_mode(&mut self, fonts: &mut Fonts, mode: Mode) {
        if mode != self.mode {
            self.stop_editing(fonts);
            self.mode = mode;
            self.hovered = None;
            self.hovered_shape = None;
        }
    }

    /// Tints tiles rendered in each frame (shows what a change redraws).
    pub(crate) fn toggle_tile_updates(&mut self) -> bool {
        let on = !self.cards_canvas.show_updates;
        self.cards_canvas.show_updates = on;
        self.shapes_canvas.show_updates = on;
        on
    }

    /// Starts dragging shape `index` alone (scripted drags).
    pub(crate) fn begin_drag(&mut self, index: usize) {
        if index < self.shapes.len() {
            self.select_only(index);
            let at = self.shapes[index].rect.center();
            self.lift(at);
        }
    }

    /// Moves the dragged shapes by `delta` document units.
    pub(crate) fn drag_by(&mut self, delta: Vec2) {
        if let Some(at) = self.lifted.as_ref().map(edit::Lift::at) {
            self.drag_to(at + delta);
        }
    }

    /// The shape nearest the middle of the view (scripted drags).
    pub(crate) fn shape_in_view(&self) -> Option<usize> {
        let center = self.to_world(self.viewport.center());
        let id = self.shapes_canvas.item_at(center).or_else(|| {
            // Between shapes: the nearest by grid search outwards.
            (1..20).find_map(|step| {
                let d = f64::from(step) * 16.0;
                [
                    Vec2::new(d, 0.0),
                    Vec2::new(0.0, d),
                    Vec2::new(-d, 0.0),
                    Vec2::new(0.0, -d),
                ]
                .into_iter()
                .find_map(|offset| self.shapes_canvas.item_at(center + offset))
            })
        })?;
        Some(id.0 as usize)
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

    /// Back to 100 % at the board's start.
    pub(crate) fn reset_view(&mut self) {
        self.origin = Point::new(-40.0, -40.0);
        self.zoom = 1.0;
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
            if self.matches.take().is_some() {
                self.cards_canvas.invalidate_all();
            }
            return None;
        }
        let matches: Vec<bool> = self
            .haystacks
            .iter()
            .map(|haystack| haystack.contains(&query))
            .collect();
        let count = matches.iter().filter(|&&m| m).count();
        self.matches = Some(matches);
        // Every card's look may have changed.
        self.cards_canvas.invalidate_all();
        Some(count)
    }

    /// Ends editing, keeping the edited text.
    pub(crate) fn stop_editing(&mut self, fonts: &mut Fonts) {
        if let Some((index, mut field)) = self.editing.take() {
            field.blur(fonts);
            self.bodies[index] = field.text();
            self.haystacks[index] = self.haystack(index);
            self.texts.remove(&index);
            self.cards_canvas
                .invalidate(card_bounds(card_rect(self.columns, index)));
        }
    }

    fn start_editing(&mut self, fonts: &mut Fonts, index: usize) {
        self.stop_editing(fonts);
        let mut field = TextField::new(&self.bodies[index], body_style(), true).with_padding(2.0);
        field.focus();
        self.texts.remove(&index);
        // The tiles show the card without its body while it is edited.
        self.cards_canvas
            .invalidate(card_bounds(card_rect(self.columns, index)));
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
    pub(crate) fn input(
        &mut self,
        fonts: &mut Fonts,
        clipboard: &mut Clipboard,
        input: &Input,
    ) -> bool {
        // The edited card's field sees input first, in world coordinates.
        if let Some((index, mut field)) = self.editing.take() {
            let bounds = self.body_rect(index);
            let response =
                field.input_with_clipboard(fonts, &self.world_input(input), bounds, clipboard);
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
        if self.mode == Mode::Shapes
            && let Some(changed) = self.handle_shapes(input)
        {
            return changed;
        }
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

    /// Selecting, dragging, deleting and undoing shapes, and hover; `None`
    /// for input the shared code handles (panning, zooming).
    fn handle_shapes(&mut self, input: &Input) -> Option<bool> {
        match input {
            Input::PointerMoved(position) => {
                let world = self.to_world(*position);
                if self.lifted.is_some() {
                    self.drag_to(world);
                    return Some(true);
                }
                if let Some((start, _)) = self.rubber {
                    self.rubber = Some((start, world));
                    return Some(true);
                }
                if self.dragging.is_some() {
                    return None;
                }
                let hovered = self
                    .viewport
                    .contains(*position)
                    .then(|| self.shapes_canvas.item_at(world))
                    .flatten()
                    .map(|id| id.0 as usize);
                let changed = hovered != self.hovered_shape;
                self.hovered_shape = hovered;
                Some(changed)
            }
            Input::PointerButton {
                primary: true,
                pressed: true,
                position,
                modifiers,
                ..
            } if self.viewport.contains(*position) => {
                let world = self.to_world(*position);
                match self.shapes_canvas.item_at(world) {
                    Some(id) if modifiers.shift => {
                        self.toggle_selected(id.0 as usize);
                        Some(true)
                    }
                    Some(id) => {
                        let index = id.0 as usize;
                        if !self.selection.contains(&index) {
                            self.select_only(index);
                        }
                        self.lift(world);
                        Some(true)
                    }
                    None if modifiers.shift => {
                        self.rubber = Some((world, world));
                        Some(true)
                    }
                    None => {
                        // A press on empty board clears the selection and
                        // pans.
                        self.selection.clear();
                        self.dragging = Some(*position);
                        Some(true)
                    }
                }
            }
            Input::PointerButton {
                primary: true,
                pressed: false,
                modifiers,
                ..
            } => {
                if self.lifted.is_some() {
                    self.drop_lift();
                    return Some(true);
                }
                if let Some((start, end)) = self.rubber.take() {
                    self.select_in(Rect::from_points(start, end), modifiers.shift);
                    return Some(true);
                }
                None
            }
            Input::Key {
                key,
                pressed: true,
                modifiers,
                ..
            } => {
                let step = if modifiers.shift { 10.0 } else { 1.0 };
                let command = |c: &str| {
                    modifiers.command()
                        && matches!(key, Key::Character(typed) if typed.eq_ignore_ascii_case(c))
                };
                match key {
                    Key::Delete | Key::Backspace => self.delete_selection(),
                    Key::ArrowLeft => self.nudge(Vec2::new(-step, 0.0)),
                    Key::ArrowRight => self.nudge(Vec2::new(step, 0.0)),
                    Key::ArrowUp => self.nudge(Vec2::new(0.0, -step)),
                    Key::ArrowDown => self.nudge(Vec2::new(0.0, step)),
                    Key::Escape => self.selection.clear(),
                    _ if command("z") && modifiers.shift => {
                        self.redo();
                    }
                    _ if command("z") => {
                        self.undo();
                    }
                    _ if command("y") => {
                        self.redo();
                    }
                    _ if command("a") => self.select_all(),
                    _ => return None,
                }
                Some(true)
            }
            _ => None,
        }
    }

    fn hover(&mut self, card: Option<usize>) -> bool {
        let changed = self.hovered != card;
        self.hovered = card;
        changed
    }

    /// The caret blink of the edited card, or a zoom pause after which
    /// the canvas switches levels.
    pub(crate) fn wake_at(&self) -> Option<Instant> {
        let caret = self.editing.as_ref().and_then(|(_, field)| field.wake_at());
        let canvas = self.last_viewport.and_then(|viewport| match self.mode {
            Mode::Cards => self.cards_canvas.wake_at(&viewport),
            Mode::Shapes => self.shapes_canvas.wake_at(&viewport),
        });
        match (caret, canvas) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
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

    /// Draws the board into `viewport` (window coordinates): the dot grid,
    /// the items through the tile cache, then live overlays (hover, the
    /// edited card).
    pub(crate) fn paint(&mut self, fonts: &mut Fonts, scene: &mut Scene, viewport: Rect) {
        self.viewport = viewport;
        scene.fill(&viewport, PAPER);
        let view = Rect::from_points(
            self.to_world(viewport.origin()),
            self.to_world(Point::new(viewport.x1, viewport.y1)),
        );
        scene.push_clip(&viewport);
        scene.set_transform(self.camera());
        self.paint_grid(scene, view);
        scene.set_transform(Affine::IDENTITY);
        let canvas_view = Viewport {
            origin: self.origin,
            zoom: self.zoom,
            screen: viewport,
            scale_factor: imaginarium_ui::scale_factor(),
        };
        self.last_viewport = Some(canvas_view);
        let now = Instant::now();
        // Rendering every visible tile would delay the first frame (each is
        // its own GPU submission): draw it directly, tiles from the next.
        let tiles = self.tiles && self.painted > 0;
        self.painted += 1;
        let stats = match self.mode {
            Mode::Cards => {
                let mut content = CardContent {
                    fonts,
                    columns: self.columns,
                    titles: &self.titles,
                    bodies: &self.bodies,
                    texts: &mut self.texts,
                    matches: self.matches.as_deref(),
                    editing: self.editing.as_ref().map(|(index, _)| *index),
                    scale_factor: canvas_view.scale_factor,
                    layouts: 0,
                };
                let stats = if tiles {
                    self.cards_canvas
                        .frame(scene, &canvas_view, now, &mut content)
                } else {
                    self.cards_canvas
                        .draw_direct(scene, &canvas_view, &mut content)
                };
                self.last_layouts = content.layouts;
                stats
            }
            Mode::Shapes => {
                let lifted = self.lifted.as_ref().map(|lift| lift.ids.as_slice());
                let mut content = ShapeContent {
                    shapes: &self.shapes,
                    deleted: &self.deleted,
                    lifted,
                };
                self.last_layouts = 0;
                if tiles {
                    self.shapes_canvas
                        .frame(scene, &canvas_view, now, &mut content)
                } else {
                    self.shapes_canvas
                        .draw_direct(scene, &canvas_view, &mut content)
                }
            }
        };
        self.owed = stats.owed > 0 || (self.tiles && !tiles);
        self.last_canvas = stats;
        // Live overlays, drawn every frame, never baked into tiles.
        scene.set_transform(self.camera());
        match self.mode {
            Mode::Cards => {
                if let Some(index) = self.hovered
                    && self
                        .editing
                        .as_ref()
                        .is_none_or(|(editing, _)| *editing != index)
                {
                    let card = RoundedRect::from_rect(self.card_rect(index), 10.0);
                    scene.stroke(&card, 2.0 / self.zoom, ACCENT.with_alpha(0.7));
                }
                let columns = self.columns;
                if let Some((index, field)) = &mut self.editing {
                    let rect = card_rect(columns, *index);
                    let body = body_rect(rect);
                    scene.fill(&body, Color::WHITE.with_alpha(0.55));
                    scene.stroke(&RoundedRect::from_rect(rect, 10.0), 2.5 / self.zoom, ACCENT);
                    field.paint(fonts, scene, body, ACCENT);
                }
            }
            Mode::Shapes => {
                let level_scale = canvas_view.device_scale();
                self.paint_shape_overlay(scene, view, level_scale);
                if let Some(index) = self.hovered_shape
                    && self.lifted.is_none()
                    && !self.selection.contains(&index)
                    && let Some(shape) = self.shapes.get(index)
                {
                    scene.stroke(
                        &shape.bounds().inflate(2.0, 2.0),
                        1.5 / self.zoom,
                        ACCENT.with_alpha(0.5),
                    );
                }
            }
        }
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

    /// Adds the visible cards (and the edited card's field) to `tree`, with
    /// ids from `base`; returns the cards' ids.
    pub(crate) fn access_children(
        &mut self,
        fonts: &mut Fonts,
        tree: &mut AccessTree,
        base: u64,
    ) -> Vec<NodeId> {
        if self.mode == Mode::Shapes {
            return Vec::new();
        }
        let view = Rect::from_points(
            self.to_world(self.viewport.origin()),
            self.to_world(Point::new(self.viewport.x1, self.viewport.y1)),
        );
        let camera = self.camera();
        let mut children = Vec::new();
        for index in self.visible_cards(view) {
            let card_id = NodeId(base + CARD_NODES + index as u64);
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
                let field_id = field.access(fonts, tree, base + EDIT_NODES, body, "Card text");
                card.set_children(vec![field_id]);
            }
            tree.push(card_id, card);
            children.push(card_id);
        }
        children
    }
}

/// Paints cards into tiles: text above a zoom of 0.3, plain tiles below.
struct CardContent<'a> {
    fonts: &'a mut Fonts,
    columns: usize,
    titles: &'a [String],
    bodies: &'a [String],
    texts: &'a mut HashMap<usize, CardText>,
    matches: Option<&'a [bool]>,
    editing: Option<usize>,
    scale_factor: f64,
    /// Text layouts built (cache misses).
    layouts: usize,
}

impl CanvasContent for CardContent<'_> {
    fn paint_item(&mut self, id: ItemId, scene: &mut Scene, level_scale: f64) {
        let index = id.0 as usize;
        let rect = card_rect(self.columns, index);
        let dimmed = self.matches.is_some_and(|matches| !matches[index]);
        let tint = TINTS[index % TINTS.len()];
        let tint = if dimmed { tint.with_alpha(0.3) } else { tint };
        // The level's scale in logical pixels decides the detail.
        if level_scale / self.scale_factor < TEXT_ZOOM {
            scene.fill(&rect, tint);
            return;
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
            self.layouts += 1;
            let width = (CARD.width - 2.0 * PADDING) as f32;
            let text = CardText {
                // One line: long titles are cut at the card's edge.
                title: TextBlock::new(self.fonts, &self.titles[index], &title_style(), None),
                body: TextBlock::new(self.fonts, &self.bodies[index], &body_style(), Some(width)),
            };
            self.texts.insert(index, text);
        }
        if let Some(text) = self.texts.get(&index) {
            text.title
                .paint(scene, Point::new(rect.x0 + PADDING, rect.y0 + 12.0), false);
            if self.editing != Some(index) {
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
    }
}

/// Paints shapes into tiles, except deleted ones and those being dragged
/// (drawn live on top).
struct ShapeContent<'a> {
    shapes: &'a [Shape],
    deleted: &'a [bool],
    /// Sorted.
    lifted: Option<&'a [usize]>,
}

impl CanvasContent for ShapeContent<'_> {
    fn paint_item(&mut self, id: ItemId, scene: &mut Scene, level_scale: f64) {
        let index = id.0 as usize;
        if self.deleted.get(index).copied().unwrap_or(true)
            || self
                .lifted
                .is_some_and(|lifted| lifted.binary_search(&index).is_ok())
        {
            return;
        }
        if let Some(shape) = self.shapes.get(index) {
            shape.paint(scene, level_scale);
        }
    }
}

/// What a card paints: the card and its shadow.
fn card_bounds(card: Rect) -> Rect {
    Rect::new(card.x0 - 1.0, card.y0 - 1.0, card.x1 + 1.0, card.y1 + 4.0)
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
        let mut board = Board::new(100, 100, true);
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
