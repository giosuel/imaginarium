// SPDX-License-Identifier: AGPL-3.0-or-later

//! Imaginarium desktop application entry point.
//!
//! # Responsibility
//! - Parse command-line arguments, initialize profiling and settings, and
//!   start the UI event loop.
//! - Wire the subsystems together: register built-in commands, settings,
//!   importers/exporters, AI providers and add-ons.
//!
//! # Non-goals
//! - No feature logic lives here. This crate only *composes* the others, so
//!   it should stay small and readable.
//!
//! Status: M3.4. It opens one board window on a new, untitled board
//! (`--demo` shows the mockups' "Launch plan" board instead). Files, the
//! start window and the startup work in parallel come later in M3.
//!
//! ```sh
//! cargo run -p imaginarium-app                          # a new board
//! cargo run -p imaginarium-app -- --demo                # the "Launch plan" board
//! cargo run -p imaginarium-app -- --light --frames 60   # light theme, close after 60 frames
//! cargo run -p imaginarium-app -- --version
//! ```

use std::cell::RefCell;
use std::rc::Rc;

use anyhow::Context as _;
use imaginarium_canvas::{BoardCanvas, demo};
use imaginarium_model::{Document, DocumentKind};
use imaginarium_render::{CpuMode, TileWorkers};
use imaginarium_settings::Registry;
use imaginarium_shell::BoardWindow;
use imaginarium_text::Fonts;
use imaginarium_ui::{DEFAULT_ACCENT, Theme, set_theme};

/// Application version, taken from the workspace `Cargo.toml`.
const VERSION: &str = env!("CARGO_PKG_VERSION");

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let flag = |name: &str| args.iter().any(|arg| arg == name);
    let value = |name: &str| {
        args.iter()
            .position(|arg| arg == name)
            .and_then(|at| args.get(at + 1))
            .cloned()
    };
    if flag("--version") {
        println!("Imaginarium {VERSION}");
        return Ok(());
    }
    if flag("--light") {
        set_theme(Theme::light(DEFAULT_ACCENT));
    }
    let mut settings = Registry::new();
    settings
        .register_all(imaginarium_shell::SETTINGS)
        .context("registering the shell's settings")?;
    let (document, name) = if flag("--demo") {
        (demo::launch_plan()?, "Launch plan")
    } else {
        (Document::new(DocumentKind::Board)?, "Untitled board")
    };
    let workers = TileWorkers::new(TileWorkers::default_threads(), CpuMode::Fast);
    let board = BoardCanvas::new(document).with_workers(workers);
    let mut window = BoardWindow::new(board, name, Fonts::new(), Rc::new(RefCell::new(settings)));
    if let Some(frames) = value("--frames").and_then(|frames| frames.parse().ok()) {
        window = window.close_after(frames);
    }
    let options = window.options();
    imaginarium_ui::run(options, window).context("running the board window")?;
    Ok(())
}
