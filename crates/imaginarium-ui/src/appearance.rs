// SPDX-License-Identifier: AGPL-3.0-or-later

//! The look the user chose, against what the system prefers (`shell.md`
//! §5, §7): the theme (follow the system, light or dark), the accent, and
//! reduced motion.
//!
//! The app sets the choice from its settings ([`set_appearance`]); where
//! it says to follow the system, the toolkit does, live:
//! - on Windows and macOS through the windows' theme (winit);
//! - on Linux through the desktop portal's `org.freedesktop.appearance`
//!   settings (`color-scheme`, and GNOME's `enable-animations` for motion),
//!   read over D-Bus on a thread of their own, which also hears when they
//!   change.
//!
//! What the system says is kept in atomics (any thread may learn it); the
//! theme is switched on the UI thread, before the next frame
//! (`refresh`).

use std::cell::{Cell, RefCell};
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};

use imaginarium_render::peniko::Color;

use crate::theme::{Theme, set_theme, theme};

/// Which theme the user picked.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ThemeChoice {
    /// Light or dark, as the system prefers (dark when it doesn't say).
    #[default]
    System,
    /// Always light.
    Light,
    /// Always dark.
    Dark,
}

/// Whether the user wants motion reduced.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MotionChoice {
    /// As the system says (full motion when it doesn't say).
    #[default]
    System,
    /// Always reduced.
    Reduce,
    /// Never reduced.
    Full,
}

/// The user's choice.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Appearance {
    /// The theme.
    pub theme: ThemeChoice,
    /// The accent color (its light version: the theme derives the rest).
    pub accent: Color,
    /// Reduced motion.
    pub motion: MotionChoice,
}

/// What the system said: 0 nothing (yet), 1 no, 2 yes.
static SYSTEM_DARK: AtomicU8 = AtomicU8::new(0);
static SYSTEM_REDUCE: AtomicU8 = AtomicU8::new(0);
/// Bumped whenever what the system says changes.
static SYSTEM_CHANGES: AtomicU64 = AtomicU64::new(0);

thread_local! {
    static CHOICE: RefCell<Option<Appearance>> = const { RefCell::new(None) };
    /// The system's change count the look was last worked out for.
    static SEEN: Cell<u64> = const { Cell::new(u64::MAX) };
}

fn load(flag: &AtomicU8) -> Option<bool> {
    match flag.load(Ordering::Relaxed) {
        1 => Some(false),
        2 => Some(true),
        _ => None,
    }
}

fn store(flag: &AtomicU8, value: Option<bool>) -> bool {
    let new = match value {
        None => 0,
        Some(false) => 1,
        Some(true) => 2,
    };
    flag.swap(new, Ordering::Relaxed) != new
}

/// Whether the system prefers dark, if it says.
pub fn system_prefers_dark() -> Option<bool> {
    load(&SYSTEM_DARK)
}

/// Whether the system asks for reduced motion, if it says.
pub fn system_reduces_motion() -> Option<bool> {
    load(&SYSTEM_REDUCE)
}

/// The system's preference changed (from a window's theme, or the
/// portal's thread): every window draws again with it. Any thread.
pub fn set_system_prefers_dark(dark: Option<bool>) {
    if store(&SYSTEM_DARK, dark) {
        system_changed();
    }
}

/// See [`set_system_prefers_dark`], for reduced motion.
pub fn set_system_reduces_motion(reduce: Option<bool>) {
    if store(&SYSTEM_REDUCE, reduce) {
        system_changed();
    }
}

fn system_changed() {
    // Released after the value, so a refresh that sees the new count sees
    // the new value too.
    SYSTEM_CHANGES.fetch_add(1, Ordering::Release);
    (crate::window::frame_waker())();
}

/// Sets the user's choice (from the app's settings) and applies it at
/// once; while it follows the system, so does the look.
pub fn set_appearance(appearance: Appearance) {
    CHOICE.with(|choice| *choice.borrow_mut() = Some(appearance));
    apply();
}

/// The user's choice, if the app set one.
pub fn appearance() -> Option<Appearance> {
    CHOICE.with(|choice| *choice.borrow())
}

/// Applies what the system said since the last time (the window loop
/// calls this before frames; cheap when nothing changed).
pub(crate) fn refresh() {
    let changes = SYSTEM_CHANGES.load(Ordering::Acquire);
    if SEEN.with(|seen| seen.replace(changes)) != changes {
        apply();
    }
}

/// The theme `appearance` asks for when the system prefers `system_dark`.
pub fn resolve(appearance: &Appearance, system_dark: Option<bool>) -> Theme {
    let dark = match appearance.theme {
        ThemeChoice::Light => false,
        ThemeChoice::Dark => true,
        ThemeChoice::System => system_dark.unwrap_or(true),
    };
    if dark {
        Theme::dark(appearance.accent)
    } else {
        Theme::light(appearance.accent)
    }
}

/// Whether `appearance` reduces motion when the system says `system`.
pub fn resolve_motion(appearance: &Appearance, system: Option<bool>) -> bool {
    match appearance.motion {
        MotionChoice::Reduce => true,
        MotionChoice::Full => false,
        MotionChoice::System => system.unwrap_or(false),
    }
}

fn apply() {
    let Some(appearance) = appearance() else {
        return;
    };
    let wanted = resolve(&appearance, system_prefers_dark());
    if *theme() != wanted {
        set_theme(wanted);
    }
    let reduce = resolve_motion(&appearance, system_reduces_motion());
    if crate::animation::reduce_motion() != reduce {
        crate::animation::set_reduce_motion(reduce);
    }
}

/// Starts the thread that asks the desktop portal (Linux), once. The
/// window loop does; the app may sooner (its first frame then has the
/// system's theme). Tests don't, so they never talk to the session bus.
pub fn listen_to_the_system() {
    #[cfg(target_os = "linux")]
    {
        static STARTED: std::sync::Once = std::sync::Once::new();
        STARTED.call_once(|| {
            let _ = std::thread::Builder::new()
                .name("system-look".into())
                .spawn(portal::listen);
        });
    }
}

/// The desktop portal's settings (`org.freedesktop.portal.Settings`).
#[cfg(target_os = "linux")]
mod portal {
    use zbus::zvariant::OwnedValue;

    const DESTINATION: &str = "org.freedesktop.portal.Desktop";
    const PATH: &str = "/org/freedesktop/portal/desktop";
    const INTERFACE: &str = "org.freedesktop.portal.Settings";
    const APPEARANCE: &str = "org.freedesktop.appearance";
    const GNOME: &str = "org.gnome.desktop.interface";

    /// Reads the settings, then follows their changes until the bus goes
    /// away. Without a portal nothing is said (the defaults hold).
    pub(super) fn listen() {
        let Ok(connection) = zbus::blocking::Connection::session() else {
            return;
        };
        let Ok(proxy) = zbus::blocking::Proxy::new(&connection, DESTINATION, PATH, INTERFACE)
        else {
            return;
        };
        // Subscribed first, so a change between the read and the
        // subscription isn't missed.
        let changes = proxy.receive_signal("SettingChanged");
        if let Some(value) = read(&proxy, APPEARANCE, "color-scheme") {
            super::set_system_prefers_dark(color_scheme(&value));
        }
        if let Some(value) = read(&proxy, GNOME, "enable-animations") {
            super::set_system_reduces_motion(bool::try_from(value).ok().map(|on| !on));
        }
        let Ok(changes) = changes else {
            return;
        };
        for message in changes {
            let Ok((namespace, key, value)) =
                message.body().deserialize::<(String, String, OwnedValue)>()
            else {
                continue;
            };
            match (namespace.as_str(), key.as_str()) {
                (APPEARANCE, "color-scheme") => {
                    super::set_system_prefers_dark(color_scheme(&value));
                }
                (GNOME, "enable-animations") => {
                    super::set_system_reduces_motion(bool::try_from(value).ok().map(|on| !on));
                }
                _ => {}
            }
        }
    }

    /// One setting: `ReadOne` (version 2), else the older `Read`, whose
    /// answer is wrapped in one more variant.
    fn read(proxy: &zbus::blocking::Proxy<'_>, namespace: &str, key: &str) -> Option<OwnedValue> {
        if let Ok(value) = proxy.call::<_, _, OwnedValue>("ReadOne", &(namespace, key)) {
            return Some(value);
        }
        let wrapped: OwnedValue = proxy.call("Read", &(namespace, key)).ok()?;
        match &*wrapped {
            zbus::zvariant::Value::Value(inner) => inner.try_to_owned().ok(),
            _ => Some(wrapped),
        }
    }

    /// `color-scheme`: 1 prefers dark, 2 prefers light, 0 no preference.
    pub(super) fn color_scheme(value: &OwnedValue) -> Option<bool> {
        match u32::try_from(value.try_clone().ok()?).ok()? {
            1 => Some(true),
            2 => Some(false),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{DEFAULT_ACCENT, ThemeKind};

    #[test]
    fn the_choice_and_the_system_pick_the_theme() {
        let system = Appearance {
            theme: ThemeChoice::System,
            accent: DEFAULT_ACCENT,
            motion: MotionChoice::System,
        };
        assert_eq!(resolve(&system, Some(false)).kind(), ThemeKind::Light);
        assert_eq!(resolve(&system, Some(true)).kind(), ThemeKind::Dark);
        // The system says nothing: dark, the theme built first.
        assert_eq!(resolve(&system, None).kind(), ThemeKind::Dark);
        let light = Appearance {
            theme: ThemeChoice::Light,
            ..system
        };
        assert_eq!(resolve(&light, Some(true)).kind(), ThemeKind::Light);
        assert!(!resolve_motion(&system, None));
        assert!(resolve_motion(&system, Some(true)));
        let full = Appearance {
            motion: MotionChoice::Full,
            ..system
        };
        assert!(!resolve_motion(&full, Some(true)));
    }

    #[test]
    fn a_change_of_the_system_applies_at_the_next_refresh() {
        set_appearance(Appearance {
            theme: ThemeChoice::System,
            accent: DEFAULT_ACCENT,
            motion: MotionChoice::System,
        });
        set_system_prefers_dark(Some(false));
        refresh();
        assert_eq!(theme().kind(), ThemeKind::Light);
        set_system_prefers_dark(Some(true));
        refresh();
        assert_eq!(theme().kind(), ThemeKind::Dark);
        // A choice made after (a command-line flag) holds at the next
        // refresh.
        set_appearance(Appearance {
            theme: ThemeChoice::Light,
            ..appearance().unwrap_or(Appearance {
                theme: ThemeChoice::System,
                accent: DEFAULT_ACCENT,
                motion: MotionChoice::System,
            })
        });
        refresh();
        assert_eq!(theme().kind(), ThemeKind::Light);
        set_system_prefers_dark(None);
        set_appearance(Appearance {
            theme: ThemeChoice::Dark,
            accent: DEFAULT_ACCENT,
            motion: MotionChoice::Full,
        });
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn the_portal_says_dark_light_or_nothing() {
        use zbus::zvariant::OwnedValue;
        assert_eq!(portal::color_scheme(&OwnedValue::from(1u32)), Some(true));
        assert_eq!(portal::color_scheme(&OwnedValue::from(2u32)), Some(false));
        assert_eq!(portal::color_scheme(&OwnedValue::from(0u32)), None);
        assert_eq!(portal::color_scheme(&OwnedValue::from(true)), None);
    }

    #[test]
    fn setting_the_choice_switches_the_theme_now() {
        set_appearance(Appearance {
            theme: ThemeChoice::Light,
            accent: Color::from_rgb8(0x0f, 0x76, 0x6e),
            motion: MotionChoice::Reduce,
        });
        assert_eq!(theme().kind(), ThemeKind::Light);
        assert!(crate::animation::reduce_motion());
        set_appearance(Appearance {
            theme: ThemeChoice::Dark,
            accent: DEFAULT_ACCENT,
            motion: MotionChoice::Full,
        });
        assert_eq!(theme().kind(), ThemeKind::Dark);
        assert!(!crate::animation::reduce_motion());
    }
}
