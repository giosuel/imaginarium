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
//! - The command palette (`Ctrl+K`): the board's and the shell's commands
//!   and the settings, found by name.
//! - Declares the shell's settings ([`SETTINGS`]).
//!
//! # Boundaries
//! - Does NOT change documents itself: every change is a board command
//!   (`imaginarium-canvas`), so the palette and the AI can do the same.
//! - Does NOT open, save or name files (M3.6), run the AI (M6) or share
//!   (M8). Their buttons are placeholders that say what is coming when
//!   clicked.
//! - The contextual toolbar above the selection, built from the commands
//!   available for it.
//! - The context menu (a right-click, Shift+F10, the Menu key), F6
//!   between the board and the panels, and the inspector (`Ctrl+I`) on
//!   the right.
//! - No start window yet (the last step of M3.4).
//!
//! # Key types
//! - [`BoardWindow`]: a board window's view.
//! - [`Dock`]: where the tool bar is.

mod app;
mod chrome;
mod commands;
mod contextbar;
mod inspector;
mod menu;
mod palette;
mod start;
mod toolbar;
mod window;

#[cfg(test)]
mod tests;

pub use app::{App, SHOW_START_WINDOW};
pub use start::StartWindow;
pub use toolbar::{Dock, TOOLBAR_POSITION};
pub use window::BoardWindow;

/// The shell's settings, for the app to register.
pub static SETTINGS: &[&dyn imaginarium_settings::AnySetting] =
    &[&TOOLBAR_POSITION, &SHOW_START_WINDOW];
