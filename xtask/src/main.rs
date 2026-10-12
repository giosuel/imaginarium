// SPDX-License-Identifier: AGPL-3.0-or-later

//! Project automation for Imaginarium, run as `cargo xtask <task>`.
//!
//! The [xtask pattern](https://github.com/matklad/cargo-xtask) keeps
//! development tooling in plain Rust, so it behaves the same on Linux, Windows
//! and macOS without shell scripts. The `cargo xtask` alias is defined in
//! `.cargo/config.toml`.
//!
//! # Adding a task
//! Write a `fn(&[String]) -> ExitCode` and add it to [`TASKS`]. Tasks print
//! human-readable output and signal failure through a non-zero exit code, so
//! CI can run them directly.
//!
//! Status: skeleton (M0.1). `check-layers` (crate layering and SPDX headers)
//! arrives in M0.2.

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
const TASKS: &[Task] = &[Task {
    name: "help",
    about: "List available tasks",
    run: help,
}];

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
