// SPDX-License-Identifier: AGPL-3.0-or-later

//! The gate's own widgets: the board, the logo, and the command palette's
//! popup and result list.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Instant;

use accesskit::{Action, Node, NodeId, Role};
use imaginarium_reactive::{Memo, Signal};
use imaginarium_render::Scene;
use imaginarium_render::kurbo::{Circle, Point, Rect, RoundedRect, Size};
use imaginarium_render::peniko::Color;
use imaginarium_text::{TextBlock, TextStyle};
use imaginarium_ui::{Cx, Input, Key, Widget, access_rect};
use taffy::prelude::{auto, length, percent};
use taffy::{AvailableSpace, Display, Position, Style};

use crate::board::{ACCENT, Board, INK};
use crate::commands::Commands;

/// The board as a widget: it fills the space left by the bars.
pub(crate) struct BoardWidget {
    pub(crate) board: Rc<RefCell<Board>>,
}

impl Widget for BoardWidget {
    fn style(&self) -> Style {
        Style {
            flex_grow: 1.0,
            flex_basis: length(0.0),
            min_size: taffy::Size {
                width: length(0.0),
                height: length(0.0),
            },
            ..Style::default()
        }
    }

    fn paint(&mut self, cx: &mut Cx<'_>, scene: &mut Scene) {
        let bounds = cx.bounds();
        let owed = {
            let mut board = self.board.borrow_mut();
            board.paint(cx.fonts(), scene, bounds);
            board.owed
        };
        // Tiles the budget left over: draw again right away.
        if owed {
            imaginarium_ui::invalidate(cx.id(), false);
        }
    }

    fn event(&mut self, cx: &mut Cx<'_>, input: &Input) -> bool {
        let (fonts, clipboard) = cx.fonts_and_clipboard();
        let changed = self.board.borrow_mut().input(fonts, clipboard, input);
        if changed {
            cx.request_paint();
        }
        changed
    }

    fn focusable(&self) -> bool {
        true
    }

    fn focus_changed(&mut self, cx: &mut Cx<'_>, focused: bool) {
        if !focused {
            self.board.borrow_mut().stop_editing(cx.fonts());
            cx.request_paint();
        }
    }

    fn clips(&self) -> bool {
        true
    }

    fn ime_area(&mut self, cx: &mut Cx<'_>) -> Option<Rect> {
        self.board.borrow_mut().ime_area(cx.fonts())
    }

    fn wake_at(&self) -> Option<Instant> {
        self.board.try_borrow().ok()?.wake_at()
    }

    fn wake(&mut self, cx: &mut Cx<'_>, _now: Instant) {
        cx.request_paint();
    }

    fn accessibility(&mut self, cx: &mut Cx<'_>) -> Option<Node> {
        let base = cx.id().access_base();
        let bounds = cx.bounds();
        let (fonts, tree) = cx.fonts_and_access();
        let children = match tree {
            Some(tree) => self.board.borrow_mut().access_children(fonts, tree, base),
            None => Vec::new(),
        };
        for child in children {
            cx.add_access_child(child);
        }
        let mut node = Node::new(Role::Canvas);
        let label = match self.board.borrow().mode() {
            crate::board::Mode::Cards => "Board".to_owned(),
            crate::board::Mode::Shapes => {
                format!("Shapes board, {} shapes", self.board.borrow().shape_count())
            }
        };
        node.set_label(label);
        node.set_bounds(access_rect(bounds));
        Some(node)
    }
}

/// The app's mark in the toolbar.
pub(crate) struct Logo;

impl Widget for Logo {
    fn style(&self) -> Style {
        Style {
            size: taffy::Size {
                width: length(24.0),
                height: length(24.0),
            },
            flex_shrink: 0.0,
            ..Style::default()
        }
    }

    fn paint(&mut self, cx: &mut Cx<'_>, scene: &mut Scene) {
        let bounds = cx.bounds();
        scene.fill(&RoundedRect::from_rect(bounds, 7.0), ACCENT);
        scene.fill(&Circle::new(bounds.center(), 5.0), Color::WHITE);
    }
}

/// What the palette shows: matching commands, the selected row, and how
/// long the search took.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Results {
    pub(crate) rows: Vec<usize>,
    pub(crate) search_ms: f64,
}

/// The palette's popup: centered near the top, hidden while closed. It
/// handles the keys its input leaves to it: ↑/↓, Enter, Escape.
pub(crate) struct Palette {
    pub(crate) open: Signal<bool>,
    pub(crate) selected: Signal<usize>,
    pub(crate) results: Memo<Results>,
    /// Runs the selected command.
    pub(crate) run: Box<dyn FnMut(usize)>,
}

impl Widget for Palette {
    fn style(&self) -> Style {
        let open = self.open.get_untracked();
        Style {
            display: if open { Display::Flex } else { Display::None },
            position: Position::Absolute,
            flex_direction: taffy::FlexDirection::Column,
            size: taffy::Size {
                width: length(560.0),
                height: auto(),
            },
            inset: taffy::Rect {
                left: percent(0.5),
                top: length(72.0),
                right: auto(),
                bottom: auto(),
            },
            margin: taffy::Rect {
                left: length(-280.0),
                right: length(0.0),
                top: length(0.0),
                bottom: length(0.0),
            },
            padding: taffy::Rect {
                left: length(10.0),
                right: length(10.0),
                top: length(10.0),
                bottom: length(8.0),
            },
            gap: taffy::Size {
                width: length(6.0),
                height: length(6.0),
            },
            ..Style::default()
        }
    }

    fn paint(&mut self, cx: &mut Cx<'_>, scene: &mut Scene) {
        let bounds = cx.bounds();
        for (spread, alpha) in [(18.0, 0.015), (10.0, 0.025), (5.0, 0.035), (2.0, 0.05)] {
            let shadow = RoundedRect::from_rect(
                bounds.inflate(spread, spread)
                    + imaginarium_render::kurbo::Vec2::new(0.0, spread / 2.0),
                12.0 + spread,
            );
            scene.fill(&shadow, Color::BLACK.with_alpha(alpha));
        }
        let shape = RoundedRect::from_rect(bounds, 12.0);
        scene.fill(&shape, Color::WHITE);
        scene.stroke(&shape, 1.0, INK.with_alpha(0.12));
    }

    fn event(&mut self, cx: &mut Cx<'_>, input: &Input) -> bool {
        let Input::Key {
            key, pressed: true, ..
        } = input
        else {
            return false;
        };
        let count = self.results.with(|results| results.rows.len());
        match key {
            Key::ArrowDown if count > 0 => {
                self.selected
                    .update(|selected| *selected = (*selected + 1) % count);
            }
            Key::ArrowUp if count > 0 => {
                self.selected
                    .update(|selected| *selected = (*selected + count - 1) % count);
            }
            Key::Enter => {
                let selected = self.selected.get_untracked();
                if let Some(&command) = self
                    .results
                    .with(|results| results.rows.get(selected).copied())
                    .as_ref()
                {
                    (self.run)(command);
                }
                self.open.set(false);
                cx.resign_focus();
            }
            Key::Escape => {
                self.open.set(false);
                cx.resign_focus();
            }
            _ => return false,
        }
        true
    }

    fn accessibility(&mut self, cx: &mut Cx<'_>) -> Option<Node> {
        let mut node = Node::new(Role::Dialog);
        node.set_label("Command palette");
        node.set_bounds(access_rect(cx.bounds()));
        Some(node)
    }
}

const ROW: f64 = 34.0;
const ROWS: usize = 10;

/// The palette's result rows: only visible rows are laid out, so the cost
/// doesn't grow with the number of matches.
pub(crate) struct ResultList {
    pub(crate) commands: Rc<RefCell<Commands>>,
    pub(crate) results: Memo<Results>,
    pub(crate) selected: Signal<usize>,
    pub(crate) run: Box<dyn FnMut(usize)>,
    /// Row texts, laid out once per command.
    texts: Vec<Option<(TextBlock, Option<TextBlock>, TextBlock)>>,
    hovered: Option<usize>,
}

impl ResultList {
    pub(crate) fn new(
        commands: Rc<RefCell<Commands>>,
        results: Memo<Results>,
        selected: Signal<usize>,
        run: impl FnMut(usize) + 'static,
    ) -> Self {
        let len = commands.borrow().len();
        Self {
            commands,
            results,
            selected,
            run: Box::new(run),
            texts: (0..len).map(|_| None).collect(),
            hovered: None,
        }
    }

    fn row_at(&self, bounds: Rect, point: Point) -> Option<usize> {
        if !bounds.contains(point) {
            return None;
        }
        let row = ((point.y - bounds.y0) / ROW) as usize;
        (row < self.results.with_untracked_rows()).then_some(row)
    }
}

/// Reading the result count without subscribing (inside event handlers).
trait UntrackedRows {
    fn with_untracked_rows(&self) -> usize;
}

impl UntrackedRows for Memo<Results> {
    fn with_untracked_rows(&self) -> usize {
        imaginarium_reactive::untrack(|| self.with(|results| results.rows.len().min(ROWS)))
    }
}

impl Widget for ResultList {
    fn measure(
        &mut self,
        _cx: &mut Cx<'_>,
        _known: taffy::Size<Option<f32>>,
        _available: taffy::Size<AvailableSpace>,
    ) -> Size {
        let rows = self.results.with_untracked_rows().max(1);
        Size::new(540.0, rows as f64 * ROW)
    }

    fn paint(&mut self, cx: &mut Cx<'_>, scene: &mut Scene) {
        let bounds = cx.bounds();
        let (rows, selected) = imaginarium_reactive::untrack(|| {
            (
                self.results.with(|results| results.rows.clone()),
                self.selected.get(),
            )
        });
        if rows.is_empty() {
            let text = TextBlock::new(
                cx.fonts(),
                "No matching commands",
                &TextStyle::new(14.0).color(INK.with_alpha(0.5)),
                None,
            );
            text.paint(scene, Point::new(bounds.x0 + 12.0, bounds.y0 + 8.0), true);
            return;
        }
        let commands = self.commands.borrow();
        for (row, &command) in rows.iter().take(ROWS).enumerate() {
            let top = bounds.y0 + row as f64 * ROW;
            let rect = Rect::new(bounds.x0, top, bounds.x1, top + ROW);
            if row == selected {
                scene.fill(&RoundedRect::from_rect(rect, 7.0), ACCENT.with_alpha(0.14));
            } else if self.hovered == Some(row) {
                scene.fill(&RoundedRect::from_rect(rect, 7.0), INK.with_alpha(0.05));
            }
            let entry = &commands.commands[command];
            if self.texts[command].is_none() {
                let fonts = cx.fonts();
                let title =
                    TextBlock::new(fonts, &entry.title, &TextStyle::new(14.0).color(INK), None);
                let shortcut = entry.shortcut.as_ref().map(|shortcut| {
                    TextBlock::new(
                        fonts,
                        shortcut,
                        &TextStyle::new(12.0).color(INK.with_alpha(0.55)),
                        None,
                    )
                });
                let category = TextBlock::new(
                    fonts,
                    entry.category,
                    &TextStyle::new(12.0).color(INK.with_alpha(0.45)),
                    None,
                );
                self.texts[command] = Some((title, shortcut, category));
            }
            if let Some((title, shortcut, category)) = &self.texts[command] {
                let middle = top + ROW / 2.0;
                title.paint(
                    scene,
                    Point::new(rect.x0 + 12.0, middle - title.size().height / 2.0),
                    true,
                );
                let mut right = rect.x1 - 12.0;
                if let Some(shortcut) = shortcut {
                    let size = shortcut.size();
                    let chip = Rect::new(
                        right - size.width - 12.0,
                        middle - 10.0,
                        right,
                        middle + 10.0,
                    );
                    scene.fill(&RoundedRect::from_rect(chip, 5.0), INK.with_alpha(0.06));
                    shortcut.paint(
                        scene,
                        Point::new(chip.x0 + 6.0, middle - size.height / 2.0),
                        true,
                    );
                    right = chip.x0 - 10.0;
                }
                let size = category.size();
                category.paint(
                    scene,
                    Point::new(right - size.width, middle - size.height / 2.0),
                    true,
                );
            }
        }
    }

    fn event(&mut self, cx: &mut Cx<'_>, input: &Input) -> bool {
        let bounds = cx.bounds();
        match input {
            Input::PointerMoved(position) => {
                let row = self.row_at(bounds, *position);
                if row != self.hovered {
                    self.hovered = row;
                    cx.request_paint();
                }
                true
            }
            Input::PointerButton {
                primary: true,
                pressed: false,
                position,
                ..
            } => {
                if let Some(row) = self.row_at(bounds, *position) {
                    let command = imaginarium_reactive::untrack(|| {
                        self.results.with(|results| results.rows.get(row).copied())
                    });
                    if let Some(command) = command {
                        (self.run)(command);
                    }
                }
                true
            }
            Input::PointerButton { .. } => true,
            _ => false,
        }
    }

    fn hover_changed(&mut self, cx: &mut Cx<'_>, hovered: bool) {
        if !hovered && self.hovered.take().is_some() {
            cx.request_paint();
        }
    }

    fn accessibility(&mut self, cx: &mut Cx<'_>) -> Option<Node> {
        let bounds = cx.bounds();
        let base = cx.id().access_base();
        let (rows, selected) = imaginarium_reactive::untrack(|| {
            (
                self.results.with(|results| results.rows.clone()),
                self.selected.get(),
            )
        });
        let commands = self.commands.borrow();
        let mut ids = Vec::new();
        if let Some(tree) = cx.access_tree() {
            for (row, &command) in rows.iter().take(ROWS).enumerate() {
                let id = NodeId(base + 1 + row as u64);
                let mut item = Node::new(Role::ListBoxOption);
                item.set_label(commands.commands[command].title.clone());
                let top = bounds.y0 + row as f64 * ROW;
                item.set_bounds(access_rect(Rect::new(bounds.x0, top, bounds.x1, top + ROW)));
                if row == selected {
                    item.set_selected(true);
                }
                item.add_action(Action::Click);
                tree.push(id, item);
                ids.push(id);
            }
        }
        drop(commands);
        for id in ids {
            cx.add_access_child(id);
        }
        let mut node = Node::new(Role::ListBox);
        node.set_label("Commands");
        node.set_bounds(access_rect(bounds));
        Some(node)
    }
}
