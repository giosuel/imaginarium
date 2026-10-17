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
//! cargo run -p imaginarium-app -- --settings my.toml    # another settings file
//! ```
//!
//! Settings are kept in `settings.toml` in the platform's configuration
//! folder ([`settings_path`]), read at startup and written on a thread of
//! their own after every change.
//!
//! With `--frames`, the time to the first frame is printed when the
//! window closes.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use anyhow::Context as _;
use imaginarium_canvas::demo;
use imaginarium_render::{CpuMode, GpuOptions, GpuStart, TileWorkers};
use imaginarium_settings::{FileWriter, Registry, Scope};
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
    // A settings file is never required (ADR-0014): one that can't be read
    // is moved aside and the defaults are used.
    let path = if args.iter().any(|arg| arg == "--settings") {
        Some(
            value("--settings")
                .map(PathBuf::from)
                .context("--settings needs a file")?,
        )
    } else {
        settings_path(|name| std::env::var_os(name))
    };
    let mut notice = None;
    let writer = match path {
        Some(path) => {
            let opened = settings.open_file(Scope::App, &path);
            for (key, reason) in &opened.report.invalid {
                eprintln!(
                    "imaginarium: {}: `{key}` kept as it was: {reason}",
                    path.display()
                );
            }
            if let Some(reason) = &opened.unreadable {
                eprintln!("imaginarium: {} can't be read: {reason}", path.display());
                let what = match &opened.set_aside {
                    Some(aside) => format!("It was moved to {}.", aside.display()),
                    None => "Changes won't be saved this time, so it stays as it is.".to_owned(),
                };
                eprintln!("imaginarium: {what}");
                notice = Some(format!(
                    "Your settings file couldn't be read, so the defaults are used. {what}"
                ));
            }
            opened.saving.then(|| {
                let writer = FileWriter::new(path);
                settings.persist_to(Scope::App, writer.sink());
                writer
            })
        }
        None => None,
    };
    let workers = TileWorkers::new(TileWorkers::default_threads(), CpuMode::Fast);
    let app = App::new(
        Fonts::new(),
        Rc::new(RefCell::new(settings)),
        workers,
        VERSION,
    );
    app.report_saving(writer.as_ref().map(FileWriter::status), notice);
    let frames: Option<u64> = value("--frames").and_then(|frames| frames.parse().ok());
    let (options, view): (WindowOptions, Box<dyn View>) = if flag("--demo") {
        let mut window = app.board_window(demo::launch_plan()?, "Launch plan");
        if let Some(frames) = frames {
            window = window.close_after(frames);
        }
        (window.options(), Box::new(window))
    } else {
        app.first_window(flag("--new"), frames)?
    };
    let setup = started.elapsed();
    let stats = imaginarium_ui::run_with_gpu(gpu, options, view).context("running the app")?;
    // Writes what is pending, then says if saving failed.
    if let Some(err) = writer.and_then(FileWriter::finish) {
        eprintln!("imaginarium: settings not saved: {err}");
    }
    // Windows that failed while others stayed open.
    for err in &stats.window_errors {
        eprintln!("imaginarium: a window closed: {err}");
    }
    if frames.is_some() {
        report(&stats, setup);
    }
    Ok(())
}

/// Where the settings file is, from the environment (`env`):
/// `$XDG_CONFIG_HOME/imaginarium` (or `~/.config/imaginarium`) on Linux,
/// `~/Library/Application Support/Imaginarium` on macOS,
/// `%APPDATA%\Imaginarium` on Windows. A relative `XDG_CONFIG_HOME` is
/// ignored, as the XDG spec says.
fn settings_path(env: impl Fn(&str) -> Option<std::ffi::OsString>) -> Option<PathBuf> {
    let var = |name: &str| {
        env(name)
            .map(PathBuf::from)
            .filter(|value| value.is_absolute())
    };
    let folder = if cfg!(target_os = "windows") {
        var("APPDATA")?.join("Imaginarium")
    } else if cfg!(target_os = "macos") {
        var("HOME")?.join("Library/Application Support/Imaginarium")
    } else {
        var("XDG_CONFIG_HOME")
            .or_else(|| var("HOME").map(|home| home.join(".config")))?
            .join("imaginarium")
    };
    Some(folder.join("settings.toml"))
}

#[cfg(all(test, unix, not(target_os = "macos")))]
mod tests {
    use super::*;

    fn env<'a>(vars: &'a [(&str, &str)]) -> impl Fn(&str) -> Option<std::ffi::OsString> + 'a {
        move |name| {
            vars.iter()
                .find(|(var, _)| *var == name)
                .map(|(_, value)| value.into())
        }
    }

    #[test]
    fn the_settings_file_follows_xdg() {
        assert_eq!(
            settings_path(env(&[("HOME", "/home/me"), ("XDG_CONFIG_HOME", "/cfg")])),
            Some(PathBuf::from("/cfg/imaginarium/settings.toml"))
        );
        assert_eq!(
            settings_path(env(&[("HOME", "/home/me")])),
            Some(PathBuf::from("/home/me/.config/imaginarium/settings.toml"))
        );
        // Relative or empty: ignored.
        assert_eq!(
            settings_path(env(&[("HOME", "/home/me"), ("XDG_CONFIG_HOME", "cfg")])),
            Some(PathBuf::from("/home/me/.config/imaginarium/settings.toml"))
        );
        assert_eq!(
            settings_path(env(&[("HOME", "/home/me"), ("XDG_CONFIG_HOME", "")])),
            Some(PathBuf::from("/home/me/.config/imaginarium/settings.toml"))
        );
        assert_eq!(settings_path(env(&[])), None);
    }
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
