// SPDX-License-Identifier: AGPL-3.0-or-later

//! The board window as a [`View`]: the board, the panels over it, and what
//! their buttons ask for.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Instant;

use imaginarium_canvas::BoardCanvas;
use imaginarium_commands::Args;
use imaginarium_render::Scene;
use imaginarium_render::kurbo::{Point, Rect, Size};
use imaginarium_settings::Registry;
use imaginarium_text::{Fonts, TextStyle};
use imaginarium_ui::{
    AccessTree, Anchor, Flex, FrameInfo, Input, Label, LayerId, LayerOptions, Placement, Token, Ui,
    View, WidgetId, WindowOptions, theme,
};
use taffy::prelude::length;

use crate::chrome::{self, Chrome, Placeholder};
use crate::toolbar::{self, Dock, DropZones, TOOLBAR_POSITION, ToolBar};

/// What a panel's button asked for. The window does it after the input
/// that asked, when it can reach the board and the tree.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Ask {
    /// Run a board command.
    Run(&'static str),
    /// Show what a placeholder will do.
    Explain(Placeholder),
    /// Open the zoom menu.
    ZoomMenu,
    /// Close the menu that is open.
    CloseMenu,
    /// The tool bar's grip was pressed here (window space).
    Grab(Point),
    /// …dragged here…
    Drag(Point),
    /// …and released here, with Shift held or not.
    Drop(Point, bool),
    /// Put the tool bar back on the left (a double-click on the grip).
    ResetBar,
}

/// The queue of what the panels asked for.
pub(crate) type Asks = Rc<RefCell<Vec<Ask>>>;

/// The tool bar being dragged: where the grip was pressed and where the
/// bar's corner was then.
struct Drag {
    grab: Point,
    origin: Point,
}

/// A board window: the board fills it, the panels float over it
/// (`shell.md` §3). Run it with [`imaginarium_ui::run`] and
/// [`BoardWindow::options`].
pub struct BoardWindow {
    fonts: Fonts,
    ui: Ui,
    board: WidgetId,
    name: String,
    asks: Asks,
    chrome: Chrome,
    tools: ToolBar,
    /// Holds the tool bar; laid out for where it docks.
    stage: WidgetId,
    zones: WidgetId,
    hint: (WidgetId, WidgetId),
    dock: Dock,
    vertical: bool,
    /// The floating tool bar's top left corner (window space).
    floating_at: Point,
    drag: Option<Drag>,
    /// The menu or note open, if any.
    popup: Option<LayerId>,
    settings: Rc<RefCell<Registry>>,
    size: Size,
    close_after: Option<u64>,
    frames: u64,
}

impl BoardWindow {
    /// A window showing `board`, named `name` ("Untitled board"). The
    /// tool bar starts where `settings` say ([`TOOLBAR_POSITION`], which
    /// must be registered) and moving it changes them.
    pub fn new(
        board: BoardCanvas,
        name: &str,
        fonts: Fonts,
        settings: Rc<RefCell<Registry>>,
    ) -> Self {
        let mut ui = Ui::new();
        let asks: Asks = Rc::default();
        let (tool, status) = (board.tool(), board.status());
        let keymap = board.keymap().clone();
        let board = ui.add(board);
        let chrome = chrome::chrome(&mut ui, name, status, &keymap, &asks);
        let tools = toolbar::tool_bar(&mut ui, tool, &keymap, &asks);
        let zones = ui.add(DropZones { target: None });
        let hint_text = ui.add(
            Label::new("", TextStyle::new(12.0).weight(500.0))
                .color(Token::OnAccent)
                .single_line(),
        );
        let hint = ui.add_with_children(
            Flex::row()
                .height(30.0)
                .padding(10.0, 0.0)
                .radius(8.0)
                .background(Token::TooltipBg)
                .style(|style| {
                    style.position = taffy::Position::Absolute;
                    style.display = taffy::Display::None;
                }),
            &[hint_text],
        );
        let stage = ui.add_with_children(
            Flex::row().pass_through().style(|style| {
                style.position = taffy::Position::Absolute;
                style.inset = full();
                style.padding = taffy::Rect {
                    left: length(16.0),
                    right: length(16.0),
                    top: length(16.0),
                    bottom: length(16.0),
                };
            }),
            &[tools.bar],
        );
        let root = ui.add_with_children(
            Flex::column().fill(),
            &[board, chrome.overlay, zones, stage, hint],
        );
        ui.set_root(root);
        let dock = settings
            .borrow()
            .get(&TOOLBAR_POSITION)
            .unwrap_or(Dock::Left);
        let mut window = Self {
            fonts,
            ui,
            board,
            name: name.to_owned(),
            asks,
            chrome,
            tools,
            stage,
            zones,
            hint: (hint, hint_text),
            dock: Dock::Left,
            vertical: true,
            floating_at: Point::new(96.0, 96.0),
            drag: None,
            popup: None,
            settings,
            size: Size::new(1440.0, 900.0),
            close_after: None,
            frames: 0,
        };
        // Without a remembered spot, a floating bar starts docked.
        window.dock_bar(if dock == Dock::Floating {
            Dock::Left
        } else {
            dock
        });
        window.ui.set_focus(&mut window.fonts, Some(board));
        window
    }

    /// Closes the window after `frames` frames (for scripted runs).
    pub fn close_after(mut self, frames: u64) -> Self {
        self.close_after = Some(frames);
        self
    }

    /// The window's options: its title (`<name> — Imaginarium`), app ID,
    /// size and background.
    pub fn options(&self) -> WindowOptions {
        WindowOptions {
            title: format!("{} — Imaginarium", self.name),
            size: Size::new(1440.0, 900.0),
            background: theme().color(Token::BoardBg),
            app_id: Some("ch.giosuel.Imaginarium".into()),
            ..WindowOptions::default()
        }
    }

    /// Where the tool bar is.
    pub fn dock(&self) -> Dock {
        self.dock
    }

    /// The tree, for tests and scripted runs.
    pub fn ui(&mut self) -> (&mut Ui, &mut Fonts) {
        (&mut self.ui, &mut self.fonts)
    }

    /// The board.
    pub fn board(&self) -> Option<&BoardCanvas> {
        self.ui.widget::<BoardCanvas>(self.board)
    }

    /// Lays the window out and draws it into `scene` at `size`.
    pub fn draw(&mut self, scene: &mut Scene, size: Size) {
        self.size = size;
        if self.dock == Dock::Floating && self.drag.is_none() {
            // A smaller window must not lose the bar.
            let clamped = self.clamp(self.floating_at);
            if clamped != self.floating_at {
                self.place_floating(clamped);
            }
        }
        self.ui
            .layout(&mut self.fonts, Rect::from_origin_size((0.0, 0.0), size));
        self.ui.paint(&mut self.fonts, scene);
    }

    /// Does what the panels asked for, until they ask for nothing more.
    fn handle_asks(&mut self) {
        loop {
            let asks = std::mem::take(&mut *self.asks.borrow_mut());
            if asks.is_empty() {
                break;
            }
            for ask in asks {
                self.handle(ask);
            }
        }
        // Clicks on a panel's background, or a closed popup, leave no
        // focus: the keyboard goes back to the board.
        if self.ui.focused().is_none() {
            self.ui.set_focus(&mut self.fonts, Some(self.board));
        }
    }

    fn handle(&mut self, ask: Ask) {
        match ask {
            Ask::Run(command) => {
                if let Some(board) = self.ui.widget_mut::<BoardCanvas>(self.board) {
                    // A command that isn't available now (redo with nothing
                    // to redo) does nothing, as its key would.
                    let _ = board.execute(command, &Args::new());
                }
            }
            Ask::Explain(placeholder) => {
                let Some(&anchor) = self.chrome.anchors.get(&placeholder) else {
                    return;
                };
                let content = chrome::note(&mut self.ui, placeholder);
                let placement = match placeholder {
                    Placeholder::Help => Placement::Above,
                    _ => Placement::Below,
                };
                self.open_popup(content, anchor, placement);
            }
            Ask::ZoomMenu => {
                let keymap = match self.board() {
                    Some(board) => board.keymap().clone(),
                    None => return,
                };
                let content = chrome::zoom_menu(&mut self.ui, &keymap, &self.asks);
                self.open_popup(content, self.chrome.zoom, Placement::Above);
            }
            Ask::CloseMenu => {
                if let Some(popup) = self.popup.take() {
                    self.ui.close_layer(&mut self.fonts, popup);
                }
            }
            Ask::Grab(at) => {
                let origin = self
                    .ui
                    .bounds(self.tools.bar)
                    .map_or(Point::ZERO, |bounds| bounds.origin());
                self.drag = Some(Drag { grab: at, origin });
            }
            Ask::Drag(at) => {
                let Some(drag) = &self.drag else {
                    return;
                };
                let corner = self.clamp(drag.origin + (at - drag.grab));
                self.place_floating(corner);
                let target = toolbar::snap(at, self.size);
                self.show_drop(Some(target), corner);
            }
            Ask::Drop(at, shift) => {
                let Some(drag) = self.drag.take() else {
                    return;
                };
                self.show_drop(None, Point::ZERO);
                let target = if shift {
                    None
                } else {
                    toolbar::snap(at, self.size)
                };
                match target {
                    Some(edge) => self.dock_bar(edge),
                    None if at == drag.grab => {
                        // A click on the grip without a drag: it stays.
                        let dock = self.dock;
                        self.dock_bar(dock);
                    }
                    None => {
                        let corner = self.clamp(drag.origin + (at - drag.grab));
                        self.place_floating(corner);
                        self.remember(Dock::Floating);
                    }
                }
            }
            Ask::ResetBar => {
                self.drag = None;
                self.show_drop(None, Point::ZERO);
                self.dock_bar(Dock::Left);
            }
        }
    }

    fn open_popup(&mut self, content: WidgetId, anchor: WidgetId, placement: Placement) {
        if let Some(popup) = self.popup.take() {
            self.ui.close_layer(&mut self.fonts, popup);
        }
        let options = LayerOptions::popover(Anchor::Widget(anchor)).placement(placement);
        self.popup = Some(self.ui.open_layer(content, options));
    }

    /// Docks the tool bar to `edge`, centered along it.
    fn dock_bar(&mut self, edge: Dock) {
        let vertical = edge.is_vertical().unwrap_or(self.vertical);
        self.vertical = vertical;
        self.dock = edge;
        self.tools
            .orient(&mut self.ui, vertical, edge.tooltip_side());
        if let Some(stage) = self.ui.widget_mut::<Flex>(self.stage) {
            let style = stage.style_mut();
            let (direction, justify) = match edge {
                Dock::Left => (taffy::FlexDirection::Row, taffy::JustifyContent::FLEX_START),
                Dock::Right => (taffy::FlexDirection::Row, taffy::JustifyContent::FLEX_END),
                Dock::Top => (
                    taffy::FlexDirection::Column,
                    taffy::JustifyContent::FLEX_START,
                ),
                _ => (
                    taffy::FlexDirection::Column,
                    taffy::JustifyContent::FLEX_END,
                ),
            };
            style.flex_direction = direction;
            style.justify_content = justify;
            style.align_items = taffy::AlignItems::CENTER;
        }
        if let Some(bar) = self.ui.widget_mut::<Flex>(self.tools.bar) {
            let style = bar.style_mut();
            style.position = taffy::Position::Relative;
            style.inset = auto();
        }
        self.remember(edge);
    }

    /// Lets the tool bar float with its top left corner at `corner`.
    fn place_floating(&mut self, corner: Point) {
        self.floating_at = corner;
        self.dock = Dock::Floating;
        let side = Dock::Floating.tooltip_side();
        self.tools.orient(&mut self.ui, self.vertical, side);
        if let Some(bar) = self.ui.widget_mut::<Flex>(self.tools.bar) {
            let style = bar.style_mut();
            style.position = taffy::Position::Absolute;
            style.inset = taffy::Rect {
                left: length(corner.x as f32),
                top: length(corner.y as f32),
                right: taffy::LengthPercentageAuto::auto(),
                bottom: taffy::LengthPercentageAuto::auto(),
            };
        }
    }

    /// Keeps a floating bar's corner inside the window.
    fn clamp(&self, corner: Point) -> Point {
        let bar = self
            .ui
            .bounds(self.tools.bar)
            .map_or(Size::ZERO, |bounds| bounds.size());
        Point::new(
            corner.x.clamp(0.0, (self.size.width - bar.width).max(0.0)),
            corner
                .y
                .clamp(0.0, (self.size.height - bar.height).max(0.0)),
        )
    }

    fn remember(&mut self, dock: Dock) {
        // The setting is registered by whoever made the registry; if it
        // isn't, the bar still moves, it just isn't remembered.
        let _ = self.settings.borrow_mut().set(&TOOLBAR_POSITION, dock);
    }

    /// Shows the drop zones and the hint while dragging (`target`: where
    /// the bar would go), or hides them (`None`).
    fn show_drop(&mut self, target: Option<Option<Dock>>, corner: Point) {
        if let Some(zones) = self.ui.widget_mut::<DropZones>(self.zones) {
            zones.target = target;
        }
        let (hint, text) = self.hint;
        let words = match target {
            Some(Some(edge)) => {
                format!(
                    "Release to dock it {} · Shift keeps it floating",
                    edge.words()
                )
            }
            Some(None) => "Release to leave it here · near an edge it docks".into(),
            None => String::new(),
        };
        if let Some(label) = self.ui.widget_mut::<Label>(text) {
            label.set_text(words);
        }
        let bar = self
            .ui
            .bounds(self.tools.bar)
            .map_or(0.0, |bounds| bounds.height());
        // Below the bar, or above it when the window ends first; never
        // past the right edge (the hint is about 360 px wide).
        const HINT_HEIGHT: f64 = 30.0;
        let y = if corner.y + bar + 8.0 + HINT_HEIGHT <= self.size.height {
            corner.y + bar + 8.0
        } else {
            (corner.y - 8.0 - HINT_HEIGHT).max(0.0)
        };
        let at = Point::new(corner.x.min((self.size.width - 360.0).max(0.0)), y);
        if let Some(hint) = self.ui.widget_mut::<Flex>(hint) {
            let style = hint.style_mut();
            style.display = if target.is_some() {
                taffy::Display::Flex
            } else {
                taffy::Display::None
            };
            style.inset = taffy::Rect {
                left: length(at.x as f32),
                top: length(at.y as f32),
                right: taffy::LengthPercentageAuto::auto(),
                bottom: taffy::LengthPercentageAuto::auto(),
            };
        }
    }
}

fn full() -> taffy::Rect<taffy::LengthPercentageAuto> {
    taffy::Rect {
        left: length(0.0),
        right: length(0.0),
        top: length(0.0),
        bottom: length(0.0),
    }
}

fn auto() -> taffy::Rect<taffy::LengthPercentageAuto> {
    taffy::Rect {
        left: taffy::LengthPercentageAuto::auto(),
        right: taffy::LengthPercentageAuto::auto(),
        top: taffy::LengthPercentageAuto::auto(),
        bottom: taffy::LengthPercentageAuto::auto(),
    }
}

impl View for BoardWindow {
    fn paint(&mut self, scene: &mut Scene, size: Size) {
        self.draw(scene, size);
    }

    fn input(&mut self, input: &Input) -> bool {
        let handled = self.ui.input(&mut self.fonts, input);
        self.handle_asks();
        handled
    }

    fn presented(&mut self, frame: &FrameInfo) -> bool {
        self.frames = frame.number;
        false
    }

    fn should_close(&mut self) -> bool {
        self.close_after.is_some_and(|frames| self.frames >= frames)
    }

    fn wake_at(&mut self) -> Option<Instant> {
        self.ui.wake_at()
    }

    fn wake(&mut self, now: Instant) -> bool {
        self.ui.wake(&mut self.fonts, now);
        false
    }

    fn ime_area(&mut self) -> Option<Rect> {
        self.ui.ime_area(&mut self.fonts)
    }

    fn accessibility(&mut self, tree: &mut AccessTree) {
        self.ui.access(&mut self.fonts, tree);
    }

    fn accessibility_action(&mut self, request: &accesskit::ActionRequest) -> bool {
        let handled = self.ui.access_action(&mut self.fonts, request);
        self.handle_asks();
        handled
    }
}
