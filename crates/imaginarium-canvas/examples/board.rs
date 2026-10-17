// SPDX-License-Identifier: AGPL-3.0-or-later

//! A board on the canvas: the mockups' "Launch plan" board, or a grid of
//! shapes. Scroll to pan, Ctrl + scroll to zoom.
//!
//! ```sh
//! cargo run --release -p imaginarium-canvas --example board
//! cargo run --release -p imaginarium-canvas --example board -- --light
//! cargo run --release -p imaginarium-canvas --example board -- --grid 10000
//! cargo run --release -p imaginarium-canvas --example board -- --zoom 1.4 --frames 30
//! ```

use std::time::Instant;

use imaginarium_canvas::{BoardCanvas, demo};
use imaginarium_render::kurbo::{Point, Rect, Size};
use imaginarium_render::{CpuMode, Scene, TileWorkers};
use imaginarium_text::Fonts;
use imaginarium_ui::{
    AccessTree, DEFAULT_ACCENT, Flex, FrameInfo, Input, Theme, Ui, UiError, View, WindowOptions,
    set_theme,
};

struct Board {
    fonts: Fonts,
    ui: Ui,
    close_after: Option<u64>,
    frames: u64,
}

impl View for Board {
    fn paint(&mut self, scene: &mut Scene, size: Size) {
        self.ui
            .layout(&mut self.fonts, Rect::from_origin_size((0.0, 0.0), size));
        self.ui.paint(&mut self.fonts, scene);
    }

    fn input(&mut self, input: &Input) -> bool {
        self.ui.input(&mut self.fonts, input)
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
        self.ui.access_action(&mut self.fonts, request)
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let flag = |name: &str| args.iter().position(|arg| arg == name);
    let value = |name: &str| flag(name).and_then(|at| args.get(at + 1)).cloned();
    if flag("--light").is_some() {
        set_theme(Theme::light(DEFAULT_ACCENT));
    }
    let document = match value("--grid").and_then(|count| count.parse().ok()) {
        Some(count) => demo::grid(count)?,
        None => demo::launch_plan()?,
    };
    let workers = TileWorkers::new(TileWorkers::default_threads(), CpuMode::Fast);
    let mut board = BoardCanvas::new(document).with_workers(workers);
    if let Some(zoom) = value("--zoom").and_then(|zoom| zoom.parse().ok()) {
        board.set_view(Point::ZERO, zoom);
    }
    let mut ui = Ui::new();
    let board = ui.add(board);
    let root = ui.add_with_children(Flex::column().fill(), &[board]);
    ui.set_root(root);
    let options = WindowOptions {
        title: "Board".into(),
        size: Size::new(1440.0, 866.0),
        ..WindowOptions::default()
    };
    let view = Board {
        fonts: Fonts::new(),
        ui,
        close_after: value("--frames").and_then(|frames| frames.parse().ok()),
        frames: 0,
    };
    imaginarium_ui::run(options, view)
        .map_err(|err: UiError| Box::new(err) as Box<dyn std::error::Error>)?;
    Ok(())
}
