// SPDX-License-Identifier: AGPL-3.0-or-later

//! The windows around the documents (`docs/design/shell.md`). For now, the
//! board window (§3): the board fills it, and panels float over it, 16 px
//! from its edges:
//!
//! - top left, the document pill: the app button, the board's name and its
//!   save state;
//! - top right, Search, the AI assistant and Share;
//! - on an edge of the user's choice, the tool bar ([`Dock`]);
//! - bottom left, zoom and undo; bottom right, help.
//!
//! # Responsibility
//! - Builds a board window's widgets and runs them as a
//!   [`View`](imaginarium_ui::View) ([`BoardWindow`]).
//! - Wires the panels to the board: buttons run the board's commands by
//!   id, and the tool bar, zoom and undo follow the board's signals.
//! - Moves the tool bar: dragged by its grip, it snaps to the nearest edge
//!   or floats ([`TOOLBAR_POSITION`]).
//! - Declares the shell's settings ([`SETTINGS`]).
//!
//! # Boundaries
//! - Does NOT change documents itself: every change is a board command
//!   (`imaginarium-canvas`), so the palette and the AI can do the same.
//! - Does NOT open, save or name files (M3.6), search (the palette, next),
//!   run the AI (M6) or share (M8). Their buttons are placeholders that
//!   say what is coming when clicked.
//! - No contextual toolbar, inspector, context menu or start window yet
//!   (later steps of M3.4).
//!
//! # Key types
//! - [`BoardWindow`]: a board window's view.
//! - [`Dock`]: where the tool bar is.

mod chrome;
mod commands;
mod toolbar;
mod window;

#[cfg(test)]
mod tests;

pub use toolbar::{Dock, TOOLBAR_POSITION};
pub use window::BoardWindow;

/// The shell's settings, for the app to register.
pub static SETTINGS: &[&dyn imaginarium_settings::AnySetting] = &[&TOOLBAR_POSITION];
