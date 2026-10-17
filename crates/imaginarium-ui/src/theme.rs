// SPDX-License-Identifier: AGPL-3.0-or-later

//! Themes: the interface's colors as named tokens (`docs/design/shell.md`
//! §7). Widgets take a [`Paint`], a token or a fixed color, and resolve it
//! when they paint, so switching the theme ([`set_theme`]) repaints the
//! whole interface without rebuilding it.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use imaginarium_reactive::Signal;
use imaginarium_render::peniko::Color;

use crate::window::request_frame;

/// A named color of the interface.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Token {
    /// The board behind everything.
    BoardBg,
    /// The board's dot grid.
    BoardDot,
    /// Floating panels: the pill, tool bar, inspector, popovers.
    PanelBg,
    /// The 1 px edge of a panel.
    PanelBorder,
    /// The edge of inputs and outlined buttons.
    ControlBorder,
    /// Lines between groups inside a panel.
    Divider,
    /// A control under the pointer.
    ControlHover,
    /// The active tool or chosen option (a tint of the accent).
    ControlActive,
    /// The selected tool in the tool bar: a stronger tint than
    /// [`Token::ControlActive`], as the tool bar mockup draws it. Only
    /// [`Token::AccentInk`] icons go on it.
    AccentSoft,
    /// Body text and icons.
    Text,
    /// Body text that steps back a little: a dialog's explanation.
    TextSecondary,
    /// Secondary text: labels, hints.
    TextMuted,
    /// The quietest text: shortcuts, placeholders.
    TextSubtle,
    /// Icons and text of a control that can't be used now (redo with
    /// nothing to redo). Exempt from the contrast rule, as WCAG allows.
    Disabled,
    /// Accent fills: primary buttons, the selection, toggles.
    Accent,
    /// An accent fill under the pointer.
    AccentHover,
    /// An accent fill while pressed.
    AccentPressed,
    /// Accent text and icons on panels (the active tool, links).
    AccentInk,
    /// An accent outline on a panel (the pill's Save… button).
    AccentLine,
    /// Text and icons on an [`Token::Accent`] fill.
    OnAccent,
    /// Destructive actions and errors.
    Danger,
    /// Warnings ("Can't save").
    Warning,
    /// A warning's soft tile (the unsaved-changes dialog's icon).
    WarningSoft,
    /// Icons and text on [`Token::WarningSoft`].
    OnWarningSoft,
    /// Tooltips' background (their text is [`Token::OnAccent`]).
    TooltipBg,
    /// Dims the window behind a dialog.
    Scrim,
}

impl Token {
    /// Every token, in declaration order.
    pub const ALL: [Token; 26] = [
        Token::BoardBg,
        Token::BoardDot,
        Token::PanelBg,
        Token::PanelBorder,
        Token::ControlBorder,
        Token::Divider,
        Token::ControlHover,
        Token::ControlActive,
        Token::AccentSoft,
        Token::Text,
        Token::TextSecondary,
        Token::TextMuted,
        Token::TextSubtle,
        Token::Disabled,
        Token::Accent,
        Token::AccentHover,
        Token::AccentPressed,
        Token::AccentInk,
        Token::AccentLine,
        Token::OnAccent,
        Token::Danger,
        Token::Warning,
        Token::WarningSoft,
        Token::OnWarningSoft,
        Token::TooltipBg,
        Token::Scrim,
    ];
}

/// Which family of colors a theme is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThemeKind {
    /// Dark panels on a dark board, the theme M3 builds first.
    Dark,
    /// Light panels on a light board.
    Light,
}

/// The logo's indigo, the default accent.
pub const DEFAULT_ACCENT: Color = Color::from_rgb8(0x3a, 0x4b, 0xd6);

/// The colors of one theme, and the shape of panels.
#[derive(Clone, Debug, PartialEq)]
pub struct Theme {
    kind: ThemeKind,
    colors: [Color; Token::ALL.len()],
    /// Corner radius of panels.
    pub panel_radius: f64,
    /// Corner radius of buttons and inputs.
    pub control_radius: f64,
    /// The panels' shadow, drawn as layers of black from the widest to the
    /// tightest: how far each spreads past the panel, and its alpha.
    pub shadow: [ShadowLayer; 3],
}

/// One layer of a panel's shadow.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShadowLayer {
    /// How far it spreads past the panel's edge, in logical pixels.
    pub spread: f64,
    /// Black at this alpha.
    pub alpha: f32,
}

const fn layer(spread: f64, alpha: f32) -> ShadowLayer {
    ShadowLayer { spread, alpha }
}

impl Theme {
    /// The dark theme with `accent` (the user's accent color).
    ///
    /// ```
    /// use imaginarium_ui::{DEFAULT_ACCENT, Theme, Token};
    ///
    /// let theme = Theme::dark(DEFAULT_ACCENT);
    /// assert_ne!(theme.color(Token::Accent), theme.color(Token::AccentInk));
    /// ```
    pub fn dark(accent: Color) -> Self {
        let panel = rgb(0x1e1e22);
        Self::build(
            ThemeKind::Dark,
            [
                (Token::BoardBg, rgb(0x141416)),
                (Token::BoardDot, rgb(0x2a2a31)),
                (Token::PanelBg, panel),
                (Token::PanelBorder, rgb(0x2e2e35)),
                (Token::ControlBorder, rgb(0x34343b)),
                (Token::Divider, rgb(0x28282e)),
                (Token::ControlHover, rgb(0x2a2a30)),
                (Token::ControlActive, mix(accent, panel, 0.72)),
                (Token::AccentSoft, mix(accent, panel, 0.62)),
                (Token::Text, rgb(0xededf0)),
                (Token::TextSecondary, rgb(0xd4d4d8)),
                (Token::TextMuted, rgb(0xa1a1aa)),
                // Light enough for 4.5:1 on a hovered row too.
                (Token::TextSubtle, rgb(0x9494a0)),
                (Token::Disabled, rgb(0x5b5b66)),
                // Fills keep the accent itself (white text on it passes for
                // every offered accent); text and icons on dark panels use
                // a lighter version.
                (Token::Accent, accent),
                (Token::AccentHover, mix(accent, Color::BLACK, 0.14)),
                (Token::AccentPressed, mix(accent, Color::BLACK, 0.26)),
                (Token::AccentInk, mix(accent, Color::WHITE, 0.45)),
                (Token::AccentLine, mix(accent, panel, 0.35)),
                (Token::OnAccent, Color::WHITE),
                (Token::Danger, rgb(0xf87171)),
                (Token::Warning, rgb(0xf5a524)),
                (Token::WarningSoft, rgb(0x46380f)),
                (Token::OnWarningSoft, rgb(0xfcd34d)),
                (Token::TooltipBg, rgb(0x3f3f46)),
                (Token::Scrim, Color::BLACK.with_alpha(0.5)),
            ],
            // Black on black needs more weight than the light theme's.
            [layer(14.0, 0.10), layer(6.0, 0.14), layer(2.0, 0.24)],
        )
    }

    /// The light theme with `accent`. Built after the dark one (owner,
    /// 2026-10-16); the values are the light mockups'.
    pub fn light(accent: Color) -> Self {
        let panel = Color::WHITE;
        Self::build(
            ThemeKind::Light,
            [
                (Token::BoardBg, rgb(0xfcfcfd)),
                (Token::BoardDot, rgb(0xdedee3)),
                (Token::PanelBg, panel),
                (Token::PanelBorder, rgb(0xe7e7ea)),
                (Token::ControlBorder, rgb(0xe4e4e7)),
                (Token::Divider, rgb(0xefeff2)),
                (Token::ControlHover, rgb(0xf1f1f4)),
                (Token::ControlActive, mix(accent, panel, 0.88)),
                (Token::AccentSoft, mix(accent, panel, 0.86)),
                (Token::Text, rgb(0x18181b)),
                (Token::TextSecondary, rgb(0x3f3f46)),
                (Token::TextMuted, rgb(0x52525b)),
                (Token::TextSubtle, rgb(0x6b6b76)),
                (Token::Disabled, rgb(0xb4b4bc)),
                (Token::Accent, accent),
                (Token::AccentHover, mix(accent, Color::BLACK, 0.14)),
                (Token::AccentPressed, mix(accent, Color::BLACK, 0.26)),
                // A little darker, so accent text keeps 4.5:1 on the
                // active tool's tint too (orange missed it by a hair).
                (Token::AccentInk, mix(accent, Color::BLACK, 0.15)),
                (Token::AccentLine, mix(accent, panel, 0.6)),
                (Token::OnAccent, Color::WHITE),
                (Token::Danger, rgb(0xb91c1c)),
                (Token::Warning, rgb(0xb45309)),
                (Token::WarningSoft, rgb(0xfef3c7)),
                (Token::OnWarningSoft, rgb(0x92400e)),
                (Token::TooltipBg, rgb(0x18181b)),
                (Token::Scrim, rgb(0x18181b).with_alpha(0.22)),
            ],
            [layer(12.0, 0.03), layer(6.0, 0.05), layer(2.0, 0.08)],
        )
    }

    fn build(
        kind: ThemeKind,
        colors: [(Token, Color); Token::ALL.len()],
        shadow: [ShadowLayer; 3],
    ) -> Self {
        let mut table = [Color::TRANSPARENT; Token::ALL.len()];
        for (token, color) in colors {
            table[token as usize] = color;
        }
        Self {
            kind,
            colors: table,
            panel_radius: 12.0,
            control_radius: 8.0,
            shadow,
        }
    }

    /// Dark or light.
    pub fn kind(&self) -> ThemeKind {
        self.kind
    }

    /// The color of `token`.
    pub fn color(&self, token: Token) -> Color {
        self.colors[token as usize]
    }
}

impl Default for Theme {
    fn default() -> Self {
        Self::dark(DEFAULT_ACCENT)
    }
}

/// A color as a widget is given it: a theme token, or a fixed color (for
/// content whose color is data, like a swatch).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Paint {
    /// Follows the theme.
    Token(Token),
    /// Always this color.
    Fixed(Color),
}

impl Paint {
    /// The color in `theme`.
    pub fn resolve(self, theme: &Theme) -> Color {
        match self {
            Paint::Token(token) => theme.color(token),
            Paint::Fixed(color) => color,
        }
    }

    /// The color in the current theme ([`theme`]).
    pub fn current(self) -> Color {
        match self {
            Paint::Fixed(color) => color,
            Paint::Token(token) => THEME.with(|theme| theme.borrow().color(token)),
        }
    }
}

impl From<Token> for Paint {
    fn from(token: Token) -> Self {
        Paint::Token(token)
    }
}

impl From<Color> for Paint {
    fn from(color: Color) -> Self {
        Paint::Fixed(color)
    }
}

thread_local! {
    static THEME: RefCell<Rc<Theme>> = RefCell::new(Rc::new(Theme::default()));
    /// Bumped by every [`set_theme`]; the widget tree compares it before
    /// each layout.
    static GENERATION: Cell<u64> = const { Cell::new(0) };
    /// Lets effects and memos depend on the theme (created on first use).
    static CHANGED: RefCell<Option<Signal<u64>>> = const { RefCell::new(None) };
}

/// The current theme of this (the UI) thread. Inside an effect or memo it
/// is a dependency: the effect runs again after [`set_theme`].
pub fn theme() -> Rc<Theme> {
    // The handle is copied out before reading: reading inside an effect
    // must not hold the cell, since `set_theme` reruns effects.
    let changed = CHANGED.with(|changed| {
        *changed.borrow_mut().get_or_insert_with(|| {
            imaginarium_reactive::detached(|| imaginarium_reactive::signal(0))
        })
    });
    changed.get();
    THEME.with(|theme| theme.borrow().clone())
}

/// Switches the theme. Every widget repaints (and labels re-shape) before
/// the next frame, which this asks for.
///
/// Don't call it from an effect that reads [`theme`]: the switch runs that
/// effect again. Settings changes call it from their handler instead.
pub fn set_theme(new: Theme) {
    let unchanged = THEME.with(|theme| **theme.borrow() == new);
    if unchanged {
        return;
    }
    THEME.with(|theme| *theme.borrow_mut() = Rc::new(new));
    let generation = GENERATION.with(|count| {
        count.set(count.get() + 1);
        count.get()
    });
    // Setting runs the effects that read `theme`, so no cell is borrowed
    // while it happens.
    if let Some(signal) = CHANGED.with(|changed| *changed.borrow()) {
        signal.set(generation);
    }
    request_frame();
}

/// How many times the theme changed on this thread.
pub(crate) fn generation() -> u64 {
    GENERATION.with(Cell::get)
}

fn rgb(hex: u32) -> Color {
    Color::from_rgb8((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

/// `a` moved toward `b` by `t` (0: `a`, 1: `b`), per sRGB channel, as the
/// mockups mix their tints.
pub fn mix(a: Color, b: Color, t: f32) -> Color {
    let [ar, ag, ab, aa] = a.components;
    let [br, bg, bb, ba] = b.components;
    let lerp = |x: f32, y: f32| x + (y - x) * t;
    Color::new([lerp(ar, br), lerp(ag, bg), lerp(ab, bb), lerp(aa, ba)])
}

/// The WCAG contrast ratio of two opaque colors (1 to 21).
pub fn contrast(a: Color, b: Color) -> f64 {
    let luminance = |color: Color| {
        let [r, g, b, _] = color.components;
        let linear = |c: f32| {
            let c = f64::from(c);
            if c <= 0.04045 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * linear(r) + 0.7152 * linear(g) + 0.0722 * linear(b)
    };
    let (la, lb) = (luminance(a), luminance(b));
    let (light, dark) = if la > lb { (la, lb) } else { (lb, la) };
    (light + 0.05) / (dark + 0.05)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Text and the backgrounds it is drawn on, which must reach 4.5:1
    /// (WCAG AA for body text) in every theme and for every accent the
    /// settings offer.
    const PAIRS: [(Token, Token); 21] = [
        (Token::Danger, Token::ControlHover),
        (Token::Danger, Token::ControlActive),
        (Token::Text, Token::PanelBg),
        (Token::TextSecondary, Token::PanelBg),
        (Token::OnWarningSoft, Token::WarningSoft),
        (Token::Text, Token::BoardBg),
        (Token::Text, Token::ControlHover),
        (Token::Text, Token::ControlActive),
        (Token::TextMuted, Token::PanelBg),
        (Token::TextMuted, Token::ControlHover),
        (Token::TextSubtle, Token::PanelBg),
        (Token::TextSubtle, Token::ControlHover),
        (Token::AccentInk, Token::PanelBg),
        (Token::AccentInk, Token::ControlActive),
        (Token::AccentInk, Token::AccentSoft),
        (Token::OnAccent, Token::Accent),
        (Token::OnAccent, Token::AccentHover),
        (Token::OnAccent, Token::AccentPressed),
        (Token::OnAccent, Token::TooltipBg),
        (Token::Danger, Token::PanelBg),
        (Token::Warning, Token::PanelBg),
    ];

    /// Non-text parts that must be seen (WCAG 1.4.11, 3:1): the focus ring
    /// on panels and on the board. Left out on purpose: control borders,
    /// which are quiet in the design (`shell.md` §7; a field is told by its
    /// placeholder and focus ring), and accent fills, which are told by the
    /// text on them.
    const PARTS: [(Token, Token); 2] = [
        (Token::AccentInk, Token::PanelBg),
        (Token::AccentInk, Token::BoardBg),
    ];

    /// The accents in Settings › Appearance (indigo, teal, orange, violet).
    const ACCENTS: [u32; 4] = [0x3a4bd6, 0x0f766e, 0xc2410c, 0x7c3aed];

    #[test]
    fn text_has_enough_contrast_in_every_theme() {
        for accent in ACCENTS {
            for theme in [Theme::dark(rgb(accent)), Theme::light(rgb(accent))] {
                for (text, background) in PAIRS {
                    let ratio = contrast(theme.color(text), theme.color(background));
                    assert!(
                        ratio >= 4.5,
                        "{:?} accent #{accent:06x}: {text:?} on {background:?} is {ratio:.2}:1",
                        theme.kind()
                    );
                }
            }
        }
    }

    #[test]
    fn focus_rings_and_accent_fills_stand_out() {
        for accent in ACCENTS {
            for theme in [Theme::dark(rgb(accent)), Theme::light(rgb(accent))] {
                for (part, background) in PARTS {
                    let ratio = contrast(theme.color(part), theme.color(background));
                    assert!(
                        ratio >= 3.0,
                        "{:?} accent #{accent:06x}: {part:?} on {background:?} is {ratio:.2}:1",
                        theme.kind()
                    );
                }
            }
        }
    }

    #[test]
    fn every_token_has_a_color() {
        for theme in [Theme::dark(DEFAULT_ACCENT), Theme::light(DEFAULT_ACCENT)] {
            for token in Token::ALL {
                assert_ne!(theme.color(token), Color::TRANSPARENT, "{token:?}");
            }
        }
    }

    #[test]
    fn set_theme_bumps_the_generation_once_per_change() {
        let before = generation();
        set_theme(Theme::light(DEFAULT_ACCENT));
        set_theme(Theme::light(DEFAULT_ACCENT));
        assert_eq!(generation(), before + 1);
        assert_eq!(theme().kind(), ThemeKind::Light);
        set_theme(Theme::default());
        assert_eq!(theme().kind(), ThemeKind::Dark);
    }

    #[test]
    fn effects_follow_the_theme() {
        let seen = Rc::new(Cell::new(0));
        let effect = imaginarium_reactive::effect({
            let seen = seen.clone();
            move || {
                theme();
                seen.set(seen.get() + 1);
            }
        });
        set_theme(Theme::light(DEFAULT_ACCENT));
        set_theme(Theme::default());
        assert_eq!(seen.get(), 3);
        effect.dispose();
    }

    #[test]
    fn mixing_and_contrast_match_known_values() {
        assert_eq!(mix(Color::BLACK, Color::WHITE, 0.5).components[0], 0.5);
        assert!((contrast(Color::BLACK, Color::WHITE) - 21.0).abs() < 1e-9);
    }
}
