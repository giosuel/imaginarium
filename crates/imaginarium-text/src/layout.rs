// SPDX-License-Identifier: AGPL-3.0-or-later

//! Styles, laid-out text and painting it into a scene.

use std::borrow::Cow;

use imaginarium_render::kurbo::{Point, Rect, Size};
use imaginarium_render::peniko::Color;
use imaginarium_render::{Glyph, GlyphRun, Scene};
use parley::{
    Alignment, AlignmentOptions, FontFamily, FontFamilyName, FontWeight, GenericFamily, Layout,
    LineHeight, PositionedLayoutItem, StyleProperty,
};

use crate::fonts::{Brush, Fonts, UI_FAMILY};

/// How a piece of text looks. Sizes are in logical pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextStyle {
    /// The font size.
    pub size: f32,
    /// The weight, 100 (thin) to 900 (black); 400 is regular, 700 bold.
    pub weight: f32,
    /// The distance between baselines, as a multiple of the font size.
    pub line_height: f32,
    /// The text color.
    pub color: Color,
    /// How lines line up.
    pub align: Align,
}

/// How lines line up within the text's width.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Align {
    /// At the start of the line: left for left-to-right text, right for
    /// right-to-left text.
    #[default]
    Start,
    /// Centered.
    Center,
    /// At the end of the line.
    End,
}

impl TextStyle {
    /// Regular text of `size` in black.
    pub fn new(size: f32) -> Self {
        Self {
            size,
            weight: 400.0,
            line_height: 1.3,
            color: Color::BLACK,
            align: Align::Start,
        }
    }

    /// The same style with another weight.
    pub fn weight(self, weight: f32) -> Self {
        Self { weight, ..self }
    }

    /// The same style in another color.
    pub fn color(self, color: Color) -> Self {
        Self { color, ..self }
    }

    /// The same style with another line height (a multiple of the size).
    pub fn line_height(self, line_height: f32) -> Self {
        Self {
            line_height,
            ..self
        }
    }

    /// The same style with another alignment.
    pub fn align(self, align: Align) -> Self {
        Self { align, ..self }
    }

    /// The style as Parley properties.
    pub(crate) fn properties(&self) -> [StyleProperty<'static, Brush>; 5] {
        [
            StyleProperty::FontFamily(family()),
            StyleProperty::FontSize(self.size),
            StyleProperty::FontWeight(FontWeight::new(self.weight)),
            StyleProperty::LineHeight(LineHeight::FontSizeRelative(self.line_height)),
            StyleProperty::Brush(Brush(self.color)),
        ]
    }

    pub(crate) fn alignment(&self) -> Alignment {
        match self.align {
            Align::Start => Alignment::Start,
            Align::Center => Alignment::Center,
            Align::End => Alignment::End,
        }
    }
}

/// The bundled UI font, then the system's sans-serif. Scripts neither has
/// fall back per script (fontique's fallback lists).
fn family() -> FontFamily<'static> {
    const STACK: &[FontFamilyName<'static>] = &[
        FontFamilyName::named(UI_FAMILY),
        FontFamilyName::Generic(GenericFamily::SansSerif),
    ];
    FontFamily::List(Cow::Borrowed(STACK))
}

/// Text laid out once: shaped, broken into lines and aligned. Build a new
/// one when the text or its style changes; [`TextBlock::set_width`] only
/// re-breaks the lines.
pub struct TextBlock {
    layout: Layout<Brush>,
    align: Alignment,
}

impl TextBlock {
    /// Lays out `text` in `style`, wrapping at `max_width` logical pixels
    /// (`None`: only at line breaks in the text).
    pub fn new(fonts: &mut Fonts, text: &str, style: &TextStyle, max_width: Option<f32>) -> Self {
        let mut builder = fonts
            .layout_cx
            .ranged_builder(&mut fonts.font_cx, text, 1.0, true);
        for property in style.properties() {
            builder.push_default(property);
        }
        let layout = builder.build(text);
        let mut block = Self {
            layout,
            align: style.alignment(),
        };
        block.set_width(max_width);
        block
    }

    /// Breaks the lines again for another width.
    pub fn set_width(&mut self, max_width: Option<f32>) {
        self.layout.break_all_lines(max_width);
        self.layout.align(self.align, AlignmentOptions::default());
    }

    /// The size of the laid-out text.
    pub fn size(&self) -> Size {
        Size::new(
            f64::from(self.layout.width()),
            f64::from(self.layout.height()),
        )
    }

    /// The number of lines.
    pub fn line_count(&self) -> usize {
        self.layout.len()
    }

    /// Draws the text with its top left corner at `origin`. `hint` snaps
    /// glyph outlines to pixels (UI text; not for zoomed content).
    pub fn paint(&self, scene: &mut Scene, origin: Point, hint: bool) {
        paint_layout(&self.layout, scene, origin, hint);
    }
}

/// Draws a laid-out text: its glyph runs and their decorations.
pub(crate) fn paint_layout(layout: &Layout<Brush>, scene: &mut Scene, origin: Point, hint: bool) {
    let (dx, dy) = (origin.x as f32, origin.y as f32);
    let mut coords = Vec::new();
    for line in layout.lines() {
        for item in line.items() {
            let PositionedLayoutItem::GlyphRun(glyph_run) = item else {
                continue;
            };
            let run = glyph_run.run();
            let style = glyph_run.style();
            coords.clear();
            coords.extend(run.normalized_coords().iter().map(|coord| coord.to_bits()));
            scene.glyphs(
                GlyphRun {
                    font: run.font(),
                    size: run.font_size(),
                    coords: &coords,
                    hint,
                    color: style.brush.0,
                },
                glyph_run.positioned_glyphs().map(|glyph| Glyph {
                    id: glyph.id,
                    x: glyph.x + dx,
                    y: glyph.y + dy,
                }),
            );
            // Underline and strikethrough: offsets are above the baseline.
            let metrics = run.font_metrics();
            let x0 = f64::from(glyph_run.offset() + dx);
            let x1 = x0 + f64::from(glyph_run.advance());
            let baseline = f64::from(glyph_run.baseline() + dy);
            let decorations = [
                (
                    &style.underline,
                    metrics.underline_offset,
                    metrics.underline_size,
                ),
                (
                    &style.strikethrough,
                    metrics.strikethrough_offset,
                    metrics.strikethrough_size,
                ),
            ];
            for (decoration, default_offset, default_size) in decorations {
                let Some(decoration) = decoration else {
                    continue;
                };
                let offset = f64::from(decoration.offset.unwrap_or(default_offset));
                let size = f64::from(decoration.size.unwrap_or(default_size)).max(1.0);
                let top = baseline - offset;
                scene.fill(&Rect::new(x0, top, x1, top + size), decoration.brush.0);
            }
        }
    }
}
