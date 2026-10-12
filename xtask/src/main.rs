// SPDX-License-Identifier: AGPL-3.0-or-later

//! Project automation for Imaginarium, run as `cargo xtask <task>`.
//!
//! The [xtask pattern](https://github.com/matklad/cargo-xtask) keeps
//! development tooling in plain Rust, so it behaves the same on Linux, Windows
//! and macOS without shell scripts. The `cargo xtask` alias is defined in
//! `.cargo/config.toml`.
//!
//! # Tasks
//! - `check`: runs all repository checks below. CI runs this.
//! - `check-layers`: crate layering, dependency rules and license metadata
//!   (see [`layers`]).
//! - `check-headers`: SPDX license header on every `.rs` file (see [`headers`]).
//! - `check-tracked`: no agent/tool files or generated artifacts tracked by
//!   git (see [`tracked`]).
//!
//! # Adding a task
//! Write a `fn(&[String]) -> ExitCode` and add it to [`TASKS`]. Tasks print
//! human-readable output and signal failure through a non-zero exit code, so
//! CI can run them directly.

mod headers;
mod layers;
mod metadata;
mod tracked;

use std::process::ExitCode;

/// A task runnable via `cargo xtask <name>`.
struct Task {
    /// Name typed on the command line.
    name: &'static str,
    /// One-line description shown by `cargo xtask help`.
    about: &'static str,
    /// Entry point. Receives the arguments after the task name.
    run: fn(&[String]) -> ExitCode,
}

/// All available tasks, in the order `help` lists them.
const TASKS: &[Task] = &[
    Task {
        name: "check",
        about: "Run all repository checks (layers, headers, tracked files)",
        run: check_all,
    },
    Task {
        name: "check-layers",
        about: "Check crate layering, dependency rules and license metadata",
        run: check_layers,
    },
    Task {
        name: "check-headers",
        about: "Check the SPDX license header of every .rs file",
        run: check_headers,
    },
    Task {
        name: "check-tracked",
        about: "Check that no agent files or generated artifacts are tracked",
        run: check_tracked,
    },
    Task {
        name: "help",
        about: "List available tasks",
        run: help,
    },
];

/// Exit code for "the check found problems" (as opposed to 2 = usage error).
const FAILED: u8 = 1;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some((name, rest)) = args.split_first() else {
        return help(&[]);
    };
    match TASKS.iter().find(|task| task.name == name) {
        Some(task) => (task.run)(rest),
        None => {
            eprintln!("error: unknown task `{name}`. Run `cargo xtask help` for the list.");
            ExitCode::from(2)
        }
    }
}

/// Prints usage and the task list.
fn help(_args: &[String]) -> ExitCode {
    println!("Usage: cargo xtask <task> [args...]\n\nTasks:");
    for task in TASKS {
        println!("  {:<16} {}", task.name, task.about);
    }
    ExitCode::SUCCESS
}

/// Runs every check and fails if any of them fails. Each check still runs and
/// reports, even after an earlier one failed.
fn check_all(args: &[String]) -> ExitCode {
    let results = [check_layers(args), check_headers(args), check_tracked(args)];
    if results.iter().all(|code| *code == ExitCode::SUCCESS) {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(FAILED)
    }
}

/// `cargo xtask check-layers`
fn check_layers(_args: &[String]) -> ExitCode {
    let metadata = match metadata::load() {
        Ok(metadata) => metadata,
        Err(err) => {
            eprintln!("error: {err}");
            return ExitCode::from(FAILED);
        }
    };
    let violations = layers::check(&metadata, layers::LAYERS);
    if violations.is_empty() {
        println!(
            "check-layers: ok ({} crates, all dependencies follow the layer rules)",
            metadata.packages.len()
        );
        return ExitCode::SUCCESS;
    }
    eprintln!("check-layers: {} violation(s):", violations.len());
    for violation in &violations {
        eprintln!("  - {violation}");
    }
    eprintln!("See docs/PLAN.md §4.1 for the layer map.");
    ExitCode::from(FAILED)
}

/// `cargo xtask check-headers`
fn check_headers(_args: &[String]) -> ExitCode {
    let root = match metadata::load() {
        Ok(metadata) => metadata.workspace_root,
        Err(err) => {
            eprintln!("error: {err}");
            return ExitCode::from(FAILED);
        }
    };
    match headers::check(&root) {
        Ok(missing) if missing.is_empty() => {
            println!("check-headers: ok (every .rs file has the SPDX header)");
            ExitCode::SUCCESS
        }
        Ok(missing) => {
            eprintln!(
                "check-headers: {} file(s) don't start with `{}`:",
                missing.len(),
                headers::HEADER
            );
            for file in &missing {
                let shown = file.strip_prefix(&root).unwrap_or(file);
                eprintln!("  - {}", shown.display());
            }
            ExitCode::from(FAILED)
        }
        Err(err) => {
            eprintln!("error: {err}");
            ExitCode::from(FAILED)
        }
    }
}

/// `cargo xtask check-tracked`
fn check_tracked(_args: &[String]) -> ExitCode {
    let root = match metadata::load() {
        Ok(metadata) => metadata.workspace_root,
        Err(err) => {
            eprintln!("error: {err}");
            return ExitCode::from(FAILED);
        }
    };
    match tracked::check(&root) {
        Ok(found) if found.is_empty() => {
            println!("check-tracked: ok (no agent files or generated artifacts tracked)");
            ExitCode::SUCCESS
        }
        Ok(found) => {
            eprintln!(
                "check-tracked: {} file(s) must not be committed:",
                found.len()
            );
            for (path, reason) in &found {
                eprintln!("  - {path} ({reason})");
            }
            eprintln!("Remove them with `git rm --cached <path>`. They stay on disk.");
            ExitCode::from(FAILED)
        }
        Err(err) => {
            eprintln!("error: {err}");
            ExitCode::from(FAILED)
        }
    }
}
