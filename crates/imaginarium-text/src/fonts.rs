// SPDX-License-Identifier: AGPL-3.0-or-later

//! The font database and the scratch space layouts are built in.

use std::sync::Arc;

use fontique::{Blob, Collection, CollectionOptions, SourceCache};
use imaginarium_render::peniko::Color;
use parley::{FontContext, LayoutContext};

/// Inter 4.001, variable (weight 100–900 and optical size). See
/// `fonts/README.md` for its license.
static INTER: &[u8] = include_bytes!("../fonts/InterVariable.ttf");

/// The family name of the bundled UI font.
pub const UI_FAMILY: &str = "Inter";

/// The paint of a span of text. Parley carries it through layout; a newtype
/// because `Color` has no `Default`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Brush(pub Color);

impl Default for Brush {
    fn default() -> Self {
        Self(Color::BLACK)
    }
}

/// The fonts text is laid out with, and reusable scratch space. One per UI
/// thread: building one scans the system's fonts.
pub struct Fonts {
    pub(crate) font_cx: FontContext,
    pub(crate) layout_cx: LayoutContext<Brush>,
}

impl Fonts {
    /// The bundled UI font, then the system's fonts for every script and
    /// emoji it lacks.
    pub fn new() -> Self {
        Self::build(true)
    }

    /// Only the bundled fonts, so layouts are the same on every machine
    /// (tests, golden images). Scripts Inter lacks show as missing glyphs.
    pub fn bundled_only() -> Self {
        Self::build(false)
    }

    fn build(system_fonts: bool) -> Self {
        let mut collection = Collection::new(CollectionOptions {
            system_fonts,
            ..CollectionOptions::default()
        });
        collection.register_fonts(Blob::new(Arc::new(INTER)), None);
        Self {
            font_cx: FontContext {
                collection,
                source_cache: SourceCache::default(),
            },
            layout_cx: LayoutContext::new(),
        }
    }

    /// Fonts for another window on this thread: the same database (no
    /// second scan of the system's fonts), scratch space of its own.
    pub fn share(&self) -> Self {
        Self {
            font_cx: self.font_cx.clone(),
            layout_cx: LayoutContext::new(),
        }
    }

    /// The number of font families known (bundled and system).
    pub fn family_count(&mut self) -> usize {
        self.font_cx.collection.family_names().count()
    }
}

impl Default for Fonts {
    fn default() -> Self {
        Self::new()
    }
}
