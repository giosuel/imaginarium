// SPDX-License-Identifier: AGPL-3.0-or-later

//! What the gate window shows: a toolbar with a search field, the board,
//! and a status line with live frame timings.

use std::time::{Duration, Instant};

use accesskit::{Node, NodeId, Role};
use imaginarium_render::Scene;
use imaginarium_render::kurbo::{Circle, Point, Rect, RoundedRect, Size};
use imaginarium_render::peniko::Color;
use imaginarium_text::{Fonts, TextBlock, TextStyle};
use imaginarium_ui::{AccessTree, Input, Key, TextField, access_rect};

use crate::board::{ACCENT, Board, INK, PAPER};

const TOOLBAR: f64 = 48.0;
const STATUS: f64 = 26.0;
const CHROME: Color = Color::from_rgb8(0xff, 0xff, 0xff);
const CHROME_EDGE: Color = Color::from_rgb8(0xd9, 0xd6, 0xcc);

/// Accessibility node ids.
const TOOLBAR_NODE: NodeId = NodeId(1);
const SEARCH_NODE: u64 = 2; // and its lines after it
const BOARD_NODE: NodeId = NodeId(100);
const STATUS_NODE: NodeId = NodeId(101);

/// The window's content.
pub(crate) struct App {
    pub(crate) fonts: Fonts,
    pub(crate) board: Board,
    search: TextField,
    matches: Option<usize>,
    title: TextBlock,
    placeholder: TextBlock,
    status: String,
    gpu: String,
    /// The previous frame's paint and encode time, for the status line.
    frame: Option<(Duration, Duration)>,
    size: Size,
    /// Time spent in the last search, for the report.
    pub(crate) last_search: Duration,
}

impl App {
    /// The content, with a board of `cards`.
    pub(crate) fn new(mut fonts: Fonts, cards: usize) -> Self {
        let title = TextBlock::new(
            &mut fonts,
            "Imaginarium",
            &TextStyle::new(15.0).weight(700.0).color(INK),
            None,
        );
        let placeholder = TextBlock::new(
            &mut fonts,
            "Search cards…  (Ctrl+F)",
            &TextStyle::new(14.0).color(INK.with_alpha(0.45)),
            None,
        );
        Self {
            board: Board::new(cards),
            search: TextField::new("", TextStyle::new(14.0).color(INK), false).with_padding(7.0),
            matches: None,
            title,
            placeholder,
            status: String::new(),
            gpu: String::new(),
            frame: None,
            size: Size::ZERO,
            last_search: Duration::ZERO,
            fonts,
        }
    }

    /// The GPU's name, for the status line.
    pub(crate) fn set_gpu(&mut self, gpu: String) {
        self.gpu = gpu;
    }

    /// The last frame's timings, shown in the next frame's status line.
    pub(crate) fn set_frame(&mut self, paint: Duration, encode: Duration) {
        self.frame = Some((paint, encode));
    }

    fn search_bounds(&self) -> Rect {
        let x1 = self.size.width - 12.0;
        let x0 = (x1 - 320.0).max(220.0);
        Rect::new(x0, 9.0, x1.max(x0 + 100.0), TOOLBAR - 9.0)
    }

    fn board_viewport(&self) -> Rect {
        Rect::new(
            0.0,
            TOOLBAR,
            self.size.width,
            (self.size.height - STATUS).max(TOOLBAR),
        )
    }

    /// Types `text` into the search field (scripted measurement).
    pub(crate) fn type_text(&mut self, text: &str) {
        if !self.search.is_focused() {
            self.search.focus();
        }
        self.search.editor().insert(&mut self.fonts, text);
        self.run_search();
    }

    /// Empties and leaves the search field.
    pub(crate) fn clear_search(&mut self) {
        self.search.editor().set_text("");
        self.search.blur(&mut self.fonts);
        self.run_search();
    }

    fn run_search(&mut self) {
        let start = Instant::now();
        self.matches = self.board.search(&self.search.text());
        self.last_search = start.elapsed();
    }

    pub(crate) fn paint(&mut self, scene: &mut Scene, size: Size) {
        self.size = size;
        let viewport = self.board_viewport();
        self.board.paint(&mut self.fonts, scene, viewport);
        self.paint_toolbar(scene);
        self.paint_status(scene);
    }

    fn paint_toolbar(&mut self, scene: &mut Scene) {
        let width = self.size.width;
        scene.fill(&Rect::new(0.0, 0.0, width, TOOLBAR), CHROME);
        scene.fill(&Rect::new(0.0, TOOLBAR - 1.0, width, TOOLBAR), CHROME_EDGE);
        scene.fill(&RoundedRect::new(12.0, 12.0, 36.0, 36.0, 7.0), ACCENT);
        scene.fill(&Circle::new((24.0, 24.0), 5.0), CHROME);
        let title_height = self.title.size().height;
        self.title.paint(
            scene,
            Point::new(46.0, (TOOLBAR - title_height) / 2.0),
            true,
        );
        let bounds = self.search_bounds();
        let field = RoundedRect::from_rect(bounds, 8.0);
        scene.fill(&field, PAPER);
        if self.search.is_focused() {
            scene.stroke(&field, 2.0, ACCENT);
        } else {
            scene.stroke(&field, 1.0, CHROME_EDGE);
        }
        if self.search.text().is_empty() && !self.search.is_focused() {
            self.placeholder
                .paint(scene, Point::new(bounds.x0 + 7.0, bounds.y0 + 7.0), true);
        }
        self.search.paint(&mut self.fonts, scene, bounds, ACCENT);
        if let Some(count) = self.matches {
            let label = TextBlock::new(
                &mut self.fonts,
                &format!("{count} matching"),
                &TextStyle::new(13.0).color(INK.with_alpha(0.6)),
                None,
            );
            let at = Point::new(
                bounds.x0 - label.size().width - 12.0,
                (TOOLBAR - label.size().height) / 2.0,
            );
            label.paint(scene, at, true);
        }
    }

    fn paint_status(&mut self, scene: &mut Scene) {
        let top = self.size.height - STATUS;
        scene.fill(
            &Rect::new(0.0, top, self.size.width, self.size.height),
            CHROME,
        );
        scene.fill(
            &Rect::new(0.0, top, self.size.width, top + 1.0),
            CHROME_EDGE,
        );
        let mut status = format!(
            "{} cards · zoom {:.0} %",
            self.board.cards(),
            self.board.zoom() * 100.0
        );
        if let Some((paint, encode)) = self.frame {
            status.push_str(&format!(
                " · last frame: paint {:.2} ms, encode {:.2} ms",
                paint.as_secs_f64() * 1000.0,
                encode.as_secs_f64() * 1000.0
            ));
        }
        if !self.gpu.is_empty() {
            status.push_str(" · ");
            status.push_str(&self.gpu);
        }
        status.push_str(
            "  —  wheel: pan · Ctrl+wheel: zoom · drag: pan · double-click a card: edit · Esc: done",
        );
        let text = TextBlock::new(
            &mut self.fonts,
            &status,
            &TextStyle::new(12.0).color(INK.with_alpha(0.65)),
            None,
        );
        text.paint(scene, Point::new(12.0, top + 6.0), true);
        self.status = status;
    }

    pub(crate) fn input(&mut self, input: &Input) -> bool {
        if let Input::Key {
            key: Key::Character(c),
            pressed: true,
            modifiers,
            ..
        } = input
            && modifiers.command()
            && c.eq_ignore_ascii_case("f")
        {
            self.board.stop_editing(&mut self.fonts);
            self.search.focus();
            self.search
                .editor()
                .edit(&mut self.fonts, imaginarium_text::Edit::SelectAll);
            return true;
        }
        let bounds = self.search_bounds();
        let response = self.search.input(&mut self.fonts, input, bounds);
        if response.changed {
            self.run_search();
        }
        if response.handled {
            if self.search.is_focused() {
                self.board.stop_editing(&mut self.fonts);
            }
            return true;
        }
        let board = self.board.input(&mut self.fonts, input);
        board || response.repaint
    }

    pub(crate) fn ime_area(&mut self) -> Option<Rect> {
        let bounds = self.search_bounds();
        self.search
            .ime_area(&mut self.fonts, bounds)
            .or_else(|| self.board.ime_area(&mut self.fonts))
    }

    pub(crate) fn wake_at(&self) -> Option<Instant> {
        match (self.search.wake_at(), self.board.wake_at()) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    pub(crate) fn access(&mut self, tree: &mut AccessTree) {
        let mut toolbar = Node::new(Role::Toolbar);
        toolbar.set_bounds(access_rect(Rect::new(0.0, 0.0, self.size.width, TOOLBAR)));
        let bounds = self.search_bounds();
        let search = self
            .search
            .access(&mut self.fonts, tree, SEARCH_NODE, bounds, "Search cards");
        toolbar.set_children(vec![search]);
        tree.push(TOOLBAR_NODE, toolbar);
        tree.add_top_level(TOOLBAR_NODE);
        self.board.access(&mut self.fonts, tree, BOARD_NODE);
        tree.add_top_level(BOARD_NODE);
        let mut status = Node::new(Role::Status);
        status.set_value(self.status.clone());
        status.set_bounds(access_rect(Rect::new(
            0.0,
            self.size.height - STATUS,
            self.size.width,
            self.size.height,
        )));
        tree.push(STATUS_NODE, status);
        tree.add_top_level(STATUS_NODE);
    }

    pub(crate) fn access_action(&mut self, request: &accesskit::ActionRequest) -> bool {
        let id = request.target_node.0;
        if (SEARCH_NODE..SEARCH_NODE + 50).contains(&id) {
            let changed = self
                .search
                .access_action(&mut self.fonts, request, SEARCH_NODE);
            if changed {
                self.run_search();
            }
            return changed;
        }
        false
    }
}
