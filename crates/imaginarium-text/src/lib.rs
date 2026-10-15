// SPDX-License-Identifier: AGPL-3.0-or-later

//! Text for the toolkit, on Parley (ADR-0003): fonts, shaping, line
//! breaking, bidirectional text, editing and painting into a
//! [`Scene`](imaginarium_render::Scene).
//!
//! # Pieces
//! - [`Fonts`]: the font database (the bundled Inter, then the system's
//!   fonts with per-script fallback) and layout scratch space. One per UI
//!   thread.
//! - [`TextStyle`] and [`TextBlock`]: text laid out once (labels, card
//!   text) and drawn as glyph runs.
//! - [`Editor`]: editable plain text with a selection, word and line
//!   selection by clicks, an input method's composition (CJK), and the
//!   per-line data screen readers need ([`AccessLine`]). Operations are
//!   [`Edit`]s, not keys: mapping keys is the toolkit's job.
//!
//! # Not here
//! - Rich text (spans of different styles) is the document model's
//!   (M3/M4). This crate lays out what it is given.
//! - Focus, caret blinking, clipboard and the accessibility tree belong to
//!   `imaginarium-ui`.
//!
//! ```
//! use imaginarium_text::{Fonts, TextBlock, TextStyle};
//!
//! let mut fonts = Fonts::bundled_only();
//! let text = TextBlock::new(&mut fonts, "Hello, board", &TextStyle::new(14.0), None);
//! assert!(text.size().width > 50.0);
//! assert_eq!(text.line_count(), 1);
//! ```

mod editor;
mod fonts;
mod layout;

pub use editor::{AccessLine, Edit, Editor, Motion};
pub use fonts::{Brush, Fonts, UI_FAMILY};
pub use layout::{Align, TextBlock, TextStyle};
