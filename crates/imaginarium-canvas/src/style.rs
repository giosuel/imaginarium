// SPDX-License-Identifier: AGPL-3.0-or-later

//! Effective styles (schema-v1.md §8) and the palette's theme versions
//! (`shell.md` §7): what a node looks like in the current theme.

use imaginarium_model::{DocumentView, NodeId, Value};
use imaginarium_render::peniko::Color;
use imaginarium_ui::ThemeKind;

/// A color of the default palette: the light version is what the file
/// stores, the dark one what the dark theme shows (owner, 2026-10-16).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PaletteColor {
    /// Its name, as the inspector lists it.
    pub name: &'static str,
    /// The stored color, shown in the light theme.
    pub light: Color,
    /// What the dark theme shows instead.
    pub dark: Color,
}

const fn rgb(hex: u32) -> Color {
    Color::from_rgb8((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

/// The default palette, from the mockups (ColorCompare): fills, then
/// outline and text inks.
pub const PALETTE: &[PaletteColor] = &[
    PaletteColor {
        name: "Indigo",
        light: rgb(0xE0E7FF),
        dark: rgb(0x2B3163),
    },
    PaletteColor {
        name: "Amber",
        light: rgb(0xFEF3C7),
        dark: rgb(0x46380F),
    },
    PaletteColor {
        name: "Green",
        light: rgb(0xDCFCE7),
        dark: rgb(0x163A26),
    },
    PaletteColor {
        name: "Sky",
        light: rgb(0xE0F2FE),
        dark: rgb(0x13384F),
    },
    PaletteColor {
        name: "Pink",
        light: rgb(0xFCE7F3),
        dark: rgb(0x47203A),
    },
    PaletteColor {
        name: "Rose",
        light: rgb(0xFFE4E6),
        dark: rgb(0x4A2128),
    },
    PaletteColor {
        name: "Sticky",
        light: rgb(0xFEF08A),
        dark: rgb(0x5C4A0A),
    },
    PaletteColor {
        name: "White",
        light: rgb(0xFFFFFF),
        dark: rgb(0x1A1A1E),
    },
    PaletteColor {
        name: "Ink",
        light: rgb(0x1E293B),
        dark: rgb(0xC4C4CC),
    },
    PaletteColor {
        name: "Slate",
        light: rgb(0x475569),
        dark: rgb(0x8E8E99),
    },
    PaletteColor {
        name: "Text",
        light: rgb(0x334155),
        dark: rgb(0xC9C9D1),
    },
    PaletteColor {
        name: "Muted",
        light: rgb(0x6B6B76),
        dark: rgb(0xA1A1AA),
    },
];

/// The palette's ink: the default outline of shapes.
pub const INK: Color = rgb(0x1E293B);

/// Default outline width of shapes, in points.
pub const STROKE_WIDTH: f64 = 1.5;

/// Default size of a text box's text, in points.
pub const TEXT_SIZE: f32 = 16.0;

/// Default size of a shape's label, in points.
pub const LABEL_SIZE: f32 = 15.0;

/// Parses a schema color: `#RRGGBB` or `#RRGGBBAA` (schema-v1.md §2).
pub fn parse_color(text: &str) -> Option<Color> {
    let hex = text.strip_prefix('#')?;
    if !(hex.len() == 6 || hex.len() == 8) || !hex.is_ascii() {
        return None;
    }
    let byte = |at: usize| u8::from_str_radix(hex.get(at..at + 2)?, 16).ok();
    let (r, g, b) = (byte(0)?, byte(2)?, byte(4)?);
    let a = if hex.len() == 8 { byte(6)? } else { 255 };
    Some(Color::from_rgba8(r, g, b, a))
}

/// How a stored color shows in a theme: a palette color in its dark
/// version in the dark theme; any other color as stored.
pub fn shown(stored: Color, theme: ThemeKind) -> Color {
    if theme == ThemeKind::Light {
        return stored;
    }
    let rgba = stored.to_rgba8();
    PALETTE
        .iter()
        .find(|color| {
            let light = color.light.to_rgba8();
            (light.r, light.g, light.b) == (rgba.r, rgba.g, rgba.b)
        })
        .map_or(stored, |color| color.dark.with_alpha(stored.components[3]))
}

/// What a shape or frame looks like: its effective style in a theme.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShapeStyle {
    /// The fill (`None`: not filled).
    pub fill: Option<Color>,
    /// The outline (`None`: no outline).
    pub stroke: Option<Color>,
    /// The outline's width in points.
    pub stroke_width: f64,
    /// Opacity, 0 to 1.
    pub opacity: f32,
    /// The corner radius of rectangles, in points.
    pub radius: f64,
}

/// What a node's text looks like (plain text in M3; rich text is M5).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextLook {
    /// The size in points.
    pub size: f32,
    /// The weight, 400 regular, 700 bold.
    pub weight: f32,
    /// The color, already in the theme's version.
    pub color: Color,
}

/// A color property: `None` when absent or unparsable, transparent colors
/// as `None` too (nothing to paint).
fn color_prop(view: &DocumentView, node: NodeId, prop: &str) -> Option<Option<Color>> {
    let value = view.style_prop(node, prop)?;
    let color = value.as_str().and_then(parse_color)?;
    Some((color.components[3] > 0.0).then_some(color))
}

fn number_prop(view: &DocumentView, node: NodeId, prop: &str) -> Option<f64> {
    view.style_prop(node, prop)
        .and_then(Value::as_number)
        .filter(|value| value.is_finite())
}

/// The effective style of a shape or frame, with the built-in defaults
/// (no fill, the palette's ink outline 1.5 pt wide), in `theme`.
pub fn shape_style(view: &DocumentView, node: NodeId, theme: ThemeKind) -> ShapeStyle {
    let fill = color_prop(view, node, "fill").flatten();
    let stroke = color_prop(view, node, "stroke").unwrap_or(Some(INK));
    ShapeStyle {
        fill: fill.map(|color| shown(color, theme)),
        stroke: stroke.map(|color| shown(color, theme)),
        stroke_width: number_prop(view, node, "stroke_w")
            .unwrap_or(STROKE_WIDTH)
            .max(0.0),
        opacity: number_prop(view, node, "opacity").map_or(1.0, |value| value.clamp(0.0, 1.0))
            as f32,
        radius: number_prop(view, node, "radius").unwrap_or(0.0).max(0.0),
    }
}

/// The effective look of a node's text: the style's `size` and `color`,
/// else `size` points in the theme's text color (`theme_text`). Bold when
/// all of the text is marked bold (per-run formatting comes with rich text
/// in M5).
pub fn text_look(
    view: &DocumentView,
    node: NodeId,
    theme: ThemeKind,
    theme_text: Color,
    size: f32,
    weight: f32,
) -> TextLook {
    let color = color_prop(view, node, "color")
        .flatten()
        .map_or(theme_text, |color| shown(color, theme));
    let size = number_prop(view, node, "size")
        .filter(|size| *size > 0.0)
        .map_or(size, |size| size.min(4096.0) as f32);
    let bold = view
        .node(node)
        .and_then(|node| node.text())
        .is_some_and(|text| {
            let mut runs = text
                .paragraphs()
                .flat_map(|paragraph| paragraph.runs())
                .filter(|run| !run.text().trim().is_empty())
                .peekable();
            runs.peek().is_some()
                && runs.all(|run| run.marks().get("b").and_then(Value::as_bool) == Some(true))
        });
    TextLook {
        size,
        weight: if bold { 700.0 } else { weight },
        color,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colors_parse_as_the_schema_writes_them() {
        assert_eq!(parse_color("#E0E7FF"), Some(rgb(0xE0E7FF)));
        assert_eq!(parse_color("#e0e7ff"), Some(rgb(0xE0E7FF)));
        assert_eq!(
            parse_color("#11223380"),
            Some(Color::from_rgba8(0x11, 0x22, 0x33, 0x80))
        );
        for bad in ["E0E7FF", "#E0E7F", "#GGGGGG", "#ÄÄÄ", ""] {
            assert_eq!(parse_color(bad), None, "{bad}");
        }
    }

    #[test]
    fn the_dark_theme_shows_the_palettes_dark_versions() {
        let indigo = rgb(0xE0E7FF);
        assert_eq!(shown(indigo, ThemeKind::Light), indigo);
        assert_eq!(shown(indigo, ThemeKind::Dark), rgb(0x2B3163));
        // Outside the palette: as stored.
        let own = rgb(0x6366F1);
        assert_eq!(shown(own, ThemeKind::Dark), own);
        // A palette color keeps its own alpha.
        let faded = indigo.with_alpha(0.5);
        assert_eq!(shown(faded, ThemeKind::Dark).components[3], 0.5);
    }
}
