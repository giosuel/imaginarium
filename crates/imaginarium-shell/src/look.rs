// SPDX-License-Identifier: AGPL-3.0-or-later

//! The app's look (`shell.md` §5, §7, the Settings mockup's Appearance):
//! the theme, the accent and reduced motion, as settings, applied through
//! [`imaginarium_ui::appearance`] (which follows the system where they
//! say so).

use imaginarium_render::peniko::Color;
use imaginarium_settings::{Registry, Scope, Setting, setting_enum};
use imaginarium_ui::appearance::{Appearance, MotionChoice, ThemeChoice};

/// Which theme (`ui.theme`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ThemeSetting {
    /// As the system prefers.
    #[default]
    System,
    /// Light.
    Light,
    /// Dark.
    Dark,
}

setting_enum!(ThemeSetting {
    System => "system", "System",
    Light => "light", "Light",
    Dark => "dark", "Dark",
});

/// The accent (`ui.accent`): the Settings mockup's swatches.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Accent {
    /// The logo's indigo.
    #[default]
    Indigo,
    /// Teal.
    Teal,
    /// Orange.
    Orange,
    /// Violet.
    Violet,
    /// Pink.
    Pink,
    /// Green.
    Green,
    /// Graphite.
    Graphite,
}

setting_enum!(Accent {
    Indigo => "indigo", "Indigo",
    Teal => "teal", "Teal",
    Orange => "orange", "Orange",
    Violet => "violet", "Violet",
    Pink => "pink", "Pink",
    Green => "green", "Green",
    Graphite => "graphite", "Graphite",
});

impl Accent {
    /// The color the themes are built from (the light mockup's swatch).
    pub fn color(self) -> Color {
        let hex: u32 = match self {
            Accent::Indigo => 0x3a4bd6,
            Accent::Teal => 0x0f766e,
            Accent::Orange => 0xc2410c,
            Accent::Violet => 0x7c3aed,
            Accent::Pink => 0xbe185d,
            Accent::Green => 0x15803d,
            Accent::Graphite => 0x52525b,
        };
        rgb(hex)
    }

    /// How its swatch looks in a dark theme (the dark mockup lightens the
    /// darkest three, so they stand out on its panels).
    pub fn dark_swatch(self) -> Color {
        match self {
            Accent::Orange => rgb(0xfb923c),
            Accent::Green => rgb(0x4ade80),
            Accent::Graphite => rgb(0xa1a1aa),
            other => other.color(),
        }
    }
}

fn rgb(hex: u32) -> Color {
    let [_, r, g, b] = hex.to_be_bytes();
    Color::from_rgb8(r, g, b)
}

/// Reduced motion (`ui.reduce_motion`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Motion {
    /// As the system says.
    #[default]
    System,
    /// Reduced.
    On,
    /// Full motion.
    Off,
}

setting_enum!(Motion {
    System => "system", "Follow the system",
    On => "on", "On",
    Off => "off", "Off",
});

/// `ui.theme`.
pub static THEME: Setting<ThemeSetting> = Setting::new(
    "ui.theme",
    || ThemeSetting::System,
    Scope::App,
    "Theme",
    "Follow the system, or pick one.",
)
.synonyms(&["dark mode", "light mode", "colors", "appearance", "night"]);

/// `ui.accent`.
pub static ACCENT: Setting<Accent> = Setting::new(
    "ui.accent",
    || Accent::Indigo,
    Scope::App,
    "Accent color",
    "Used for the selection, the active tool and buttons.",
)
.synonyms(&["highlight color", "color", "tint"]);

/// `ui.reduce_motion`.
pub static REDUCE_MOTION: Setting<Motion> = Setting::new(
    "ui.reduce_motion",
    || Motion::System,
    Scope::App,
    "Reduce motion",
    "Turns off animations, such as panels sliding in.",
)
.synonyms(&["animations", "motion", "accessibility"]);

/// The look `settings` ask for.
pub fn appearance(settings: &Registry) -> Appearance {
    let theme = match settings.get(&THEME).unwrap_or_default() {
        ThemeSetting::System => ThemeChoice::System,
        ThemeSetting::Light => ThemeChoice::Light,
        ThemeSetting::Dark => ThemeChoice::Dark,
    };
    let motion = match settings.get(&REDUCE_MOTION).unwrap_or_default() {
        Motion::System => MotionChoice::System,
        Motion::On => MotionChoice::Reduce,
        Motion::Off => MotionChoice::Full,
    };
    Appearance {
        theme,
        accent: settings.get(&ACCENT).unwrap_or_default().color(),
        motion,
    }
}

/// Applies the look `settings` ask for now, and again whenever one of its
/// settings changes (from Settings, the palette, the settings file).
pub fn follow(settings: &mut Registry) {
    imaginarium_ui::appearance::set_appearance(appearance(settings));
    settings.subscribe(|change| {
        use imaginarium_settings::SettingType;
        let Some(mut look) = imaginarium_ui::appearance::appearance() else {
            return;
        };
        if change.key == THEME.key() {
            look.theme = match ThemeSetting::from_value(&change.new).unwrap_or_default() {
                ThemeSetting::System => ThemeChoice::System,
                ThemeSetting::Light => ThemeChoice::Light,
                ThemeSetting::Dark => ThemeChoice::Dark,
            };
        } else if change.key == ACCENT.key() {
            look.accent = Accent::from_value(&change.new).unwrap_or_default().color();
        } else if change.key == REDUCE_MOTION.key() {
            look.motion = match Motion::from_value(&change.new).unwrap_or_default() {
                Motion::System => MotionChoice::System,
                Motion::On => MotionChoice::Reduce,
                Motion::Off => MotionChoice::Full,
            };
        } else {
            return;
        }
        imaginarium_ui::appearance::set_appearance(look);
    });
}
