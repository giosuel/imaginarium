// SPDX-License-Identifier: AGPL-3.0-or-later

//! The document on the board (`docs/design/canvas.md`).
//!
//! The canvas turns a document's nodes into items of the tile canvas
//! (`imaginarium-render`): one item per visible node that paints
//! something, with its world transform, z order, look in the current
//! theme and bounds. [`BoardCanvas`] is the widget that shows a board: the
//! background, the tiles, and what is drawn on top of them (frame names).
//!
//! # Boundaries
//! - It does **not** own document state beyond the open document it shows
//!   (ADR-0020), and changes it only through commands (invariant 2).
//! - It does **not** rasterize: tiles are the render crate's, on its
//!   workers (ADR-0019).
//! - It does **not** draw the window around the board (panels, the tool
//!   bar): that is the shell's.
//!
//! # Key types
//! - [`BoardCanvas`]: the widget.
//! - [`shape_path`]: a shape's outline from its preset and box (ADR-0021).
//! - [`style`]: effective styles and the palette's theme versions.

#![warn(missing_docs)]

mod board;
pub mod clipboard;
pub mod commands;
pub mod commands_style;
mod copy;
pub mod demo;
mod gesture;
mod hit;
mod items;
pub mod keys;
mod paint;
mod shapes;
pub mod style;

pub use board::{BoardCanvas, BoardStatus, MAX_ZOOM, MIN_ZOOM, Preset, Tool};
pub use copy::{NodeCopy, ParagraphCopy, RunCopy};
pub use items::{BoardColors, node_look};
pub use shapes::{Outline, shape_path};

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_clipboard;
#[cfg(test)]
mod tests_commands;
#[cfg(test)]
mod tests_gesture;
#[cfg(test)]
mod tests_select;
#[cfg(test)]
mod tests_style;
#[cfg(test)]
mod tests_text;
#[cfg(test)]
mod tests_tools;
#[cfg(test)]
mod tests_updates;
