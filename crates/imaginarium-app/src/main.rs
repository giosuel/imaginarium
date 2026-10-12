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
//! Status: stub (M0.1). It only prints the version. The real startup
//! sequence arrives with the UI shell (M3).

/// Application version, taken from the workspace `Cargo.toml`.
const VERSION: &str = env!("CARGO_PKG_VERSION");

fn main() {
    println!("Imaginarium {VERSION}");
}
