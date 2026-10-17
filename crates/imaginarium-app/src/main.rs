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
//! Status: M3.4. It opens the start window (or, when the setting
//! `startup.show_start_window` is off, a new board), one window per
//! board. `--demo` opens the mockups' "Launch plan" board instead. Files
//! come with M3.6.
//!
//! ```sh
//! cargo run -p imaginarium-app                          # the start window
//! cargo run -p imaginarium-app -- --new                 # a new board, no start window
//! cargo run -p imaginarium-app -- --demo                # the "Launch plan" board
//! cargo run -p imaginarium-app -- --light --frames 60   # light theme, close after 60 frames
//! cargo run -p imaginarium-app -- --version
//! ```
//!
//! With `--frames`, the time to the first frame is printed when the
//! window closes.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use anyhow::Context as _;
use imaginarium_canvas::demo;
use imaginarium_render::{CpuMode, GpuOptions, GpuStart, TileWorkers};
use imaginarium_settings::Registry;
use imaginarium_shell::App;
use imaginarium_text::Fonts;
use imaginarium_ui::{DEFAULT_ACCENT, RunStats, Theme, View, WindowOptions, set_theme};

/// Application version, taken from the workspace `Cargo.toml`.
const VERSION: &str = env!("CARGO_PKG_VERSION");

fn main() -> anyhow::Result<()> {
    let started = Instant::now();
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
    // The GPU's drivers load on a thread of their own while the fonts are
    // scanned and the first window is built (PLAN §10: 400 ms).
    let gpu = GpuStart::spawn(&GpuOptions::default());
    if flag("--light") {
        set_theme(Theme::light(DEFAULT_ACCENT));
    }
    let mut settings = Registry::new();
    settings
        .register_all(imaginarium_shell::SETTINGS)
        .context("registering the shell's settings")?;
    let workers = TileWorkers::new(TileWorkers::default_threads(), CpuMode::Fast);
    let app = App::new(
        Fonts::new(),
        Rc::new(RefCell::new(settings)),
        workers,
        VERSION,
    );
    let frames: Option<u64> = value("--frames").and_then(|frames| frames.parse().ok());
    let (options, view): (WindowOptions, Box<dyn View>) = if flag("--demo") {
        let mut window = app.board_window(demo::launch_plan()?, "Launch plan");
        if let Some(frames) = frames {
            window = window.close_after(frames);
        }
        (window.options(), Box::new(window))
    } else if flag("--new") || !app.shows_start_window() {
        let mut window = app.new_board()?;
        if let Some(frames) = frames {
            window = window.close_after(frames);
        }
        (window.options(), Box::new(window))
    } else {
        let mut window = app.start_window();
        if let Some(frames) = frames {
            window = window.close_after(frames);
        }
        (window.options(), Box::new(window))
    };
    let setup = started.elapsed();
    let stats = imaginarium_ui::run_with_gpu(gpu, options, view).context("running the app")?;
    if frames.is_some() {
        report(&stats, setup);
    }
    Ok(())
}

/// The startup's timings, for scripted runs: `setup` is the time from
/// the start of `main` to the event loop's.
fn report(stats: &RunStats, setup: Duration) {
    let ms = |duration: Duration| format!("{:.0} ms", duration.as_secs_f64() * 1000.0);
    let opt = |duration: Option<Duration>| duration.map_or("–".to_owned(), ms);
    eprintln!(
        "first frame {} after start (setup {}, then window {}, GPU wait {}); {} frames on {}",
        opt(stats.first_frame.map(|first| setup + first)),
        ms(setup),
        opt(stats.window_created),
        opt(stats.gpu_opened),
        stats.frames,
        stats.gpu.as_ref().map_or("no GPU", |gpu| gpu.name.as_str()),
    );
}
