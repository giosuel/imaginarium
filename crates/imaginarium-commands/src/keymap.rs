// SPDX-License-Identifier: AGPL-3.0-or-later

//! Keyboard shortcuts and keymaps.
//!
//! Shortcuts are written once, platform-neutrally: `Mod+Shift+K`, where
//! `Mod` is Ctrl on Linux/Windows and ⌘ on macOS. A [`Keymap`] is built for
//! one [`Platform`]. It matches physical key presses and renders shortcuts
//! the way that platform expects (`Ctrl+Shift+K` or `⇧⌘K`).

use std::fmt::Write as _;
use std::str::FromStr;

use crate::error::CommandError;
use crate::id::CommandId;

/// Desktop platform, for modifier semantics and shortcut display.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    /// macOS: `Mod` is ⌘ (Command), and Ctrl is a separate modifier.
    MacOs,
    /// Windows: `Mod` is Ctrl.
    Windows,
    /// Linux and other Unix desktops: `Mod` is Ctrl.
    Linux,
}

impl Platform {
    /// The platform this binary was compiled for.
    pub const fn current() -> Self {
        if cfg!(target_os = "macos") {
            Platform::MacOs
        } else if cfg!(target_os = "windows") {
            Platform::Windows
        } else {
            Platform::Linux
        }
    }
}

/// A non-modifier key.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Key {
    /// A printable character, stored lowercase (`K` and `k` are the same key).
    Char(char),
    /// Return / Enter.
    Enter,
    /// Escape.
    Escape,
    /// Tab.
    Tab,
    /// Backspace.
    Backspace,
    /// Forward delete.
    Delete,
    /// Space bar.
    Space,
    /// Arrow up.
    Up,
    /// Arrow down.
    Down,
    /// Arrow left.
    Left,
    /// Arrow right.
    Right,
    /// Home.
    Home,
    /// End.
    End,
    /// Page up.
    PageUp,
    /// Page down.
    PageDown,
    /// Function key F1–F24.
    F(u8),
}

/// Modifier state, in platform-neutral terms.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Modifiers {
    /// `Mod`: Ctrl on Linux/Windows, ⌘ on macOS.
    pub primary: bool,
    /// Shift.
    pub shift: bool,
    /// Alt / ⌥ Option.
    pub alt: bool,
    /// The *physical* Ctrl key as a separate modifier. Only meaningful on
    /// macOS. On other platforms Ctrl is `primary`.
    pub ctrl: bool,
    /// The Super/Windows key as a separate modifier. Only meaningful on
    /// Linux and Windows. On macOS that key is ⌘, which is `primary`.
    pub meta: bool,
}

/// A keyboard shortcut: modifiers plus one key.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Shortcut {
    /// Modifiers.
    pub mods: Modifiers,
    /// The key.
    pub key: Key,
}

/// Parses `"Mod+Shift+K"`, `"Alt+Enter"`, `"F2"`, `"Ctrl+Tab"`, `"Mod++"`, …
/// Names are case-insensitive. `Cmd`/`Command` are aliases for `Mod`,
/// `Win`/`Windows`/`Meta` for `Super`, and `Option` for `Alt`. A trailing
/// `++` means the `+` key.
impl FromStr for Shortcut {
    type Err = CommandError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let invalid = |reason: &str| CommandError::InvalidShortcut {
            shortcut: text.to_owned(),
            reason: reason.to_owned(),
        };
        let trimmed = text.trim();
        // `+` separates modifiers, so the `+` key needs special handling:
        // "+" alone, or "…++" at the end.
        let (mod_text, key_part) = if trimmed == "+" {
            ("", "+")
        } else if let Some(prefix) = trimmed.strip_suffix("++") {
            (prefix, "+")
        } else {
            trimmed.rsplit_once('+').unwrap_or(("", trimmed))
        };
        let mod_parts: Vec<&str> = if mod_text.is_empty() {
            Vec::new()
        } else {
            mod_text.split('+').map(str::trim).collect()
        };
        let mut mods = Modifiers::default();
        for part in mod_parts {
            let flag = match part.to_ascii_lowercase().as_str() {
                "mod" | "cmd" | "command" => &mut mods.primary,
                "super" | "win" | "windows" | "meta" => &mut mods.meta,
                "shift" => &mut mods.shift,
                "alt" | "option" => &mut mods.alt,
                "ctrl" | "control" => &mut mods.ctrl,
                _ => return Err(invalid("unknown modifier")),
            };
            if *flag {
                return Err(invalid("modifier repeated"));
            }
            *flag = true;
        }
        let key = parse_key(key_part.trim()).ok_or_else(|| invalid("unknown key"))?;
        Ok(Shortcut { mods, key })
    }
}

fn parse_key(name: &str) -> Option<Key> {
    let lower = name.to_ascii_lowercase();
    Some(match lower.as_str() {
        "enter" | "return" => Key::Enter,
        "escape" | "esc" => Key::Escape,
        "tab" => Key::Tab,
        "backspace" => Key::Backspace,
        "delete" | "del" => Key::Delete,
        "space" => Key::Space,
        "up" => Key::Up,
        "down" => Key::Down,
        "left" => Key::Left,
        "right" => Key::Right,
        "home" => Key::Home,
        "end" => Key::End,
        "pageup" => Key::PageUp,
        "pagedown" => Key::PageDown,
        _ => {
            if let Some(n) = lower.strip_prefix('f').and_then(|n| n.parse::<u8>().ok()) {
                return (1..=24).contains(&n).then_some(Key::F(n));
            }
            let mut chars = name.chars();
            let c = chars.next()?;
            if chars.next().is_some() || c.is_whitespace() || c.is_control() {
                return None;
            }
            Key::Char(c.to_lowercase().next().unwrap_or(c))
        }
    })
}

impl Shortcut {
    /// Normalizes for a platform. Outside macOS, Ctrl *is* `Mod`, so
    /// `Ctrl+K` and `Mod+K` are the same shortcut there. On macOS, Super is ⌘,
    /// so `Super+K` and `Mod+K` are the same.
    pub fn normalized(mut self, platform: Platform) -> Self {
        if platform == Platform::MacOs {
            if self.mods.meta {
                self.mods.meta = false;
                self.mods.primary = true;
            }
        } else if self.mods.ctrl {
            self.mods.ctrl = false;
            self.mods.primary = true;
        }
        self
    }

    /// Renders the shortcut the way the platform's menus do: `⌃⌥⇧⌘K` on
    /// macOS (Apple's modifier order), `Ctrl+Alt+Shift+K` elsewhere.
    pub fn display(&self, platform: Platform) -> String {
        let s = self.normalized(platform);
        let key = key_label(s.key, platform);
        let mut out = String::new();
        if platform == Platform::MacOs {
            for (on, symbol) in [
                (s.mods.ctrl, "⌃"),
                (s.mods.alt, "⌥"),
                (s.mods.shift, "⇧"),
                (s.mods.primary, "⌘"),
            ] {
                if on {
                    out.push_str(symbol);
                }
            }
            out.push_str(&key);
        } else {
            let super_name = if platform == Platform::Windows {
                "Win"
            } else {
                "Super"
            };
            for (on, name) in [
                (s.mods.primary, "Ctrl"),
                (s.mods.meta, super_name),
                (s.mods.alt, "Alt"),
                (s.mods.shift, "Shift"),
            ] {
                if on {
                    let _ = write!(out, "{name}+");
                }
            }
            out.push_str(&key);
        }
        out
    }
}

fn key_label(key: Key, platform: Platform) -> String {
    let mac = platform == Platform::MacOs;
    match key {
        Key::Char(c) => c.to_uppercase().collect(),
        Key::Enter => if mac { "↩" } else { "Enter" }.to_owned(),
        Key::Escape => if mac { "⎋" } else { "Esc" }.to_owned(),
        Key::Tab => if mac { "⇥" } else { "Tab" }.to_owned(),
        Key::Backspace => if mac { "⌫" } else { "Backspace" }.to_owned(),
        Key::Delete => if mac { "⌦" } else { "Delete" }.to_owned(),
        Key::Space => "Space".to_owned(),
        Key::Up => "↑".to_owned(),
        Key::Down => "↓".to_owned(),
        Key::Left => "←".to_owned(),
        Key::Right => "→".to_owned(),
        Key::Home => "Home".to_owned(),
        Key::End => "End".to_owned(),
        Key::PageUp => "PgUp".to_owned(),
        Key::PageDown => "PgDn".to_owned(),
        Key::F(n) => format!("F{n}"),
    }
}

/// A physical key press, as reported by the windowing layer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyPress {
    /// The key.
    pub key: Key,
    /// Ctrl held.
    pub ctrl: bool,
    /// ⌘ / Windows / Super key held.
    pub meta: bool,
    /// Shift held.
    pub shift: bool,
    /// Alt / Option held.
    pub alt: bool,
}

impl KeyPress {
    /// Converts physical modifiers to a platform-neutral shortcut.
    pub fn to_shortcut(self, platform: Platform) -> Shortcut {
        // Every physical modifier is kept, so e.g. Super+R never matches a
        // plain `R` binding.
        let (primary, ctrl, meta) = match platform {
            Platform::MacOs => (self.meta, self.ctrl, false),
            Platform::Windows | Platform::Linux => (self.ctrl, false, self.meta),
        };
        Shortcut {
            mods: Modifiers {
                primary,
                shift: self.shift,
                alt: self.alt,
                ctrl,
                meta,
            },
            key: match self.key {
                Key::Char(c) => Key::Char(c.to_lowercase().next().unwrap_or(c)),
                other => other,
            },
        }
    }
}

/// Shortcut → command bindings for one platform. Single key presses only.
/// Chords (`Mod+K Mod+S`) may come later.
#[derive(Clone, Debug)]
pub struct Keymap {
    platform: Platform,
    bindings: Vec<(Shortcut, CommandId)>,
}

impl Keymap {
    /// An empty keymap for `platform`.
    pub fn new(platform: Platform) -> Self {
        Self {
            platform,
            bindings: Vec::new(),
        }
    }

    /// The platform the keymap was built for.
    pub fn platform(&self) -> Platform {
        self.platform
    }

    /// Binds a shortcut (e.g. `"Mod+B"`). Fails if it is already bound to
    /// another command. Use [`Keymap::rebind`] to replace a binding on
    /// purpose. Binding the same pair twice is a no-op.
    pub fn bind(&mut self, shortcut: &str, command: CommandId) -> Result<(), CommandError> {
        let shortcut = shortcut.parse::<Shortcut>()?.normalized(self.platform);
        match self.command_for(shortcut) {
            Some(existing) if existing == command => Ok(()),
            Some(existing) => Err(CommandError::InvalidShortcut {
                shortcut: shortcut.display(self.platform),
                reason: format!("already bound to `{existing}`"),
            }),
            None => {
                self.bindings.push((shortcut, command));
                Ok(())
            }
        }
    }

    /// Binds a shortcut, replacing any existing binding. Returns the
    /// command it was bound to before.
    pub fn rebind(
        &mut self,
        shortcut: &str,
        command: CommandId,
    ) -> Result<Option<CommandId>, CommandError> {
        let shortcut = shortcut.parse::<Shortcut>()?.normalized(self.platform);
        let previous = self.unbind_shortcut(shortcut);
        self.bindings.push((shortcut, command));
        Ok(previous)
    }

    /// Removes a shortcut's binding, returning the command it was bound to.
    pub fn unbind_shortcut(&mut self, shortcut: Shortcut) -> Option<CommandId> {
        let shortcut = shortcut.normalized(self.platform);
        let index = self.bindings.iter().position(|(s, _)| *s == shortcut)?;
        Some(self.bindings.remove(index).1)
    }

    /// The command bound to a (platform-neutral) shortcut.
    pub fn command_for(&self, shortcut: Shortcut) -> Option<CommandId> {
        let shortcut = shortcut.normalized(self.platform);
        self.bindings
            .iter()
            .find(|(s, _)| *s == shortcut)
            .map(|(_, command)| *command)
    }

    /// The command bound to a physical key press.
    pub fn command_for_press(&self, press: KeyPress) -> Option<CommandId> {
        self.command_for(press.to_shortcut(self.platform))
    }

    /// All shortcuts of a command, in binding order (the palette shows the first).
    pub fn shortcuts_for(&self, command: CommandId) -> impl Iterator<Item = Shortcut> + '_ {
        self.bindings
            .iter()
            .filter(move |(_, c)| *c == command)
            .map(|(s, _)| *s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOLD: CommandId = CommandId::new("text.bold");
    const ITALIC: CommandId = CommandId::new("text.italic");

    fn press(key: Key, ctrl: bool, meta: bool, shift: bool) -> KeyPress {
        KeyPress {
            key,
            ctrl,
            meta,
            shift,
            alt: false,
        }
    }

    #[test]
    fn parses_shortcuts() {
        let s: Shortcut = "Mod+Shift+K".parse().unwrap();
        assert!(s.mods.primary && s.mods.shift && !s.mods.alt);
        assert_eq!(s.key, Key::Char('k'));
        assert_eq!(
            "cmd+option+enter".parse::<Shortcut>().unwrap().key,
            Key::Enter
        );
        assert_eq!("F12".parse::<Shortcut>().unwrap().key, Key::F(12));
        assert_eq!("Mod+/".parse::<Shortcut>().unwrap().key, Key::Char('/'));
        for bad in [
            "",
            "Mod+",
            "Hyper+K",
            "Mod+Mod+K",
            "Mod+Ctrl+Shift+Shift+K",
            "F25",
            "Mod+KK",
        ] {
            assert!(bad.parse::<Shortcut>().is_err(), "{bad:?} should fail");
        }
    }

    #[test]
    fn displays_per_platform() {
        let s: Shortcut = "Mod+Shift+K".parse().unwrap();
        assert_eq!(s.display(Platform::Linux), "Ctrl+Shift+K");
        assert_eq!(s.display(Platform::MacOs), "⇧⌘K");
        let all: Shortcut = "Ctrl+Alt+Shift+Mod+Enter".parse().unwrap();
        assert_eq!(all.display(Platform::MacOs), "⌃⌥⇧⌘↩");
        assert_eq!(
            "Alt+F4"
                .parse::<Shortcut>()
                .unwrap()
                .display(Platform::Windows),
            "Alt+F4"
        );
    }

    #[test]
    fn mod_means_ctrl_off_mac_and_cmd_on_mac() {
        let mut linux = Keymap::new(Platform::Linux);
        linux.bind("Mod+B", BOLD).unwrap();
        assert_eq!(
            linux.command_for_press(press(Key::Char('b'), true, false, false)),
            Some(BOLD)
        );
        assert_eq!(
            linux.command_for_press(press(Key::Char('b'), false, true, false)),
            None
        );

        let mut mac = Keymap::new(Platform::MacOs);
        mac.bind("Mod+B", BOLD).unwrap();
        assert_eq!(
            mac.command_for_press(press(Key::Char('b'), false, true, false)),
            Some(BOLD)
        );
        assert_eq!(
            mac.command_for_press(press(Key::Char('b'), true, false, false)),
            None
        );
    }

    #[test]
    fn ctrl_and_mod_are_the_same_shortcut_off_mac() {
        let mut keymap = Keymap::new(Platform::Windows);
        keymap.bind("Ctrl+I", ITALIC).unwrap();
        assert!(keymap.bind("Mod+I", BOLD).is_err(), "conflict detected");
        let mut mac = Keymap::new(Platform::MacOs);
        mac.bind("Ctrl+I", ITALIC).unwrap();
        assert_eq!(mac.bind("Mod+I", BOLD), Ok(()), "distinct on macOS");
    }

    #[test]
    fn uppercase_presses_match_lowercase_bindings() {
        let mut keymap = Keymap::new(Platform::Linux);
        keymap.bind("Mod+Shift+K", BOLD).unwrap();
        assert_eq!(
            keymap.command_for_press(press(Key::Char('K'), true, false, true)),
            Some(BOLD)
        );
    }

    #[test]
    fn super_key_is_a_separate_modifier_off_mac() {
        const RECTANGLE: CommandId = CommandId::new("board.rectangle_tool");
        const LOCK: CommandId = CommandId::new("app.lock");
        let super_press = |c| KeyPress {
            key: Key::Char(c),
            ctrl: false,
            meta: true,
            shift: false,
            alt: false,
        };
        let mut linux = Keymap::new(Platform::Linux);
        linux.bind("R", RECTANGLE).unwrap();
        linux.bind("Mod+B", BOLD).unwrap();
        assert_eq!(
            linux.command_for_press(super_press('r')),
            None,
            "Super+R is not R"
        );
        let ctrl_super_b = KeyPress {
            ctrl: true,
            ..super_press('b')
        };
        assert_eq!(
            linux.command_for_press(ctrl_super_b),
            None,
            "Ctrl+Super+B is not Mod+B"
        );
        linux.bind("Super+L", LOCK).unwrap();
        assert_eq!(linux.command_for_press(super_press('l')), Some(LOCK));
        let s: Shortcut = "Super+L".parse().unwrap();
        assert_eq!(s.display(Platform::Linux), "Super+L");
        assert_eq!(s.display(Platform::Windows), "Win+L");

        // On macOS, Super/Meta is ⌘, which is `Mod`.
        let mut mac = Keymap::new(Platform::MacOs);
        mac.bind("Super+K", BOLD).unwrap();
        assert_eq!(mac.command_for_press(super_press('k')), Some(BOLD));
        assert!(mac.bind("Mod+K", ITALIC).is_err(), "same shortcut on macOS");
    }

    #[test]
    fn the_plus_key_can_be_bound() {
        let plus: Shortcut = "Mod++".parse().unwrap();
        assert!(plus.mods.primary);
        assert_eq!(plus.key, Key::Char('+'));
        let shifted: Shortcut = "Mod+Shift++".parse().unwrap();
        assert!(shifted.mods.primary && shifted.mods.shift);
        assert_eq!("+".parse::<Shortcut>().unwrap().key, Key::Char('+'));
        assert_eq!(plus.display(Platform::Linux), "Ctrl++");
        assert_eq!(plus.display(Platform::MacOs), "⌘+");
        assert!("Mod+".parse::<Shortcut>().is_err());
    }

    #[test]
    fn conflicts_rebind_and_reverse_lookup() {
        let mut keymap = Keymap::new(Platform::Linux);
        keymap.bind("Mod+B", BOLD).unwrap();
        keymap.bind("Mod+B", BOLD).unwrap(); // same pair: no-op
        assert!(keymap.bind("Mod+B", ITALIC).is_err());
        assert_eq!(keymap.rebind("Mod+B", ITALIC).unwrap(), Some(BOLD));
        keymap.bind("Mod+Shift+B", BOLD).unwrap();
        keymap.bind("F9", BOLD).unwrap();
        let bold: Vec<String> = keymap
            .shortcuts_for(BOLD)
            .map(|s| s.display(Platform::Linux))
            .collect();
        assert_eq!(bold, ["Ctrl+Shift+B", "F9"]);
        let b: Shortcut = "Mod+B".parse().unwrap();
        assert_eq!(keymap.unbind_shortcut(b), Some(ITALIC));
        assert_eq!(keymap.command_for(b), None);
    }
}
