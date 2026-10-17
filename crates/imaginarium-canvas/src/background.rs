// SPDX-License-Identifier: AGPL-3.0-or-later

//! The pattern behind a board's shapes (`shell.md` §5): dots, a grid or
//! nothing. It is only drawn on screen, never printed or exported.

use imaginarium_settings::{Scope, Setting, setting_enum};

/// The pattern behind the shapes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Background {
    /// Dots where grid lines would cross (the default).
    #[default]
    Dots,
    /// Thin lines.
    Grid,
    /// Nothing.
    Plain,
}

setting_enum!(Background {
    Dots => "dots", "Dots",
    Grid => "grid", "Grid",
    Plain => "plain", "Plain",
});

/// `board.background`.
pub static BACKGROUND: Setting<Background> = Setting::new(
    "board.background",
    || Background::Dots,
    Scope::App,
    "Board background",
    "The pattern behind your shapes. Not printed or exported.",
)
.group("appearance")
.per_document()
.synonyms(&["dots", "grid", "pattern", "canvas", "raster"]);
