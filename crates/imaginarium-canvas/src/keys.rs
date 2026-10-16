// SPDX-License-Identifier: AGPL-3.0-or-later

//! The board's shortcuts (M3.3): a keymap of `imaginarium-commands` from
//! key presses to the board's commands, as the context menu's mockup
//! shows them.

use imaginarium_commands::{CommandId, Key, KeyPress, Keymap, Platform};
use imaginarium_ui::{Key as UiKey, Modifiers as UiModifiers};

/// The default shortcuts: (shortcut, command id).
pub const DEFAULT_SHORTCUTS: &[(&str, &str)] = &[
    ("Mod+Z", "edit.undo"),
    ("Mod+Shift+Z", "edit.redo"),
    ("Mod+Y", "edit.redo"),
    ("Delete", "board.delete"),
    ("Backspace", "board.delete"),
    ("Mod+D", "board.duplicate"),
    ("Mod+A", "board.select_all"),
    ("Mod+]", "board.bring_to_front"),
    ("Mod+[", "board.send_to_back"),
    ("Mod+Alt+]", "board.bring_forward"),
    ("Mod+Alt+[", "board.send_backward"),
    // Brackets need AltGr on some layouts (German): the arrows work on all.
    ("Mod+Shift+Up", "board.bring_to_front"),
    ("Mod+Shift+Down", "board.send_to_back"),
    ("Mod+Up", "board.bring_forward"),
    ("Mod+Down", "board.send_backward"),
    ("Mod+G", "board.group"),
    ("Mod+Shift+G", "board.ungroup"),
    ("Mod+Shift+L", "board.lock"),
    ("Mod+=", "view.zoom_in"),
    ("Mod+-", "view.zoom_out"),
    ("Mod+0", "view.zoom_100"),
    ("Mod+1", "view.zoom_to_fit"),
    ("Mod+2", "view.zoom_to_selection"),
    ("Enter", "text.edit"),
    ("V", "tool.select"),
    ("H", "tool.hand"),
    ("R", "tool.rectangle"),
    ("O", "tool.ellipse"),
    ("D", "tool.diamond"),
    ("Shift+T", "tool.triangle"),
    ("L", "tool.line"),
    ("T", "tool.text"),
    ("F", "tool.frame"),
    ("Left", "board.nudge_left"),
    ("Right", "board.nudge_right"),
    ("Up", "board.nudge_up"),
    ("Down", "board.nudge_down"),
    ("Shift+Left", "board.nudge_left_far"),
    ("Shift+Right", "board.nudge_right_far"),
    ("Shift+Up", "board.nudge_up_far"),
    ("Shift+Down", "board.nudge_down_far"),
];

/// The board's default keymap for `platform`.
pub fn default_keymap(platform: Platform) -> Keymap {
    let mut keymap = Keymap::new(platform);
    for (shortcut, command) in DEFAULT_SHORTCUTS {
        // The table is checked by a test: every entry parses.
        let _ = keymap.bind(shortcut, CommandId::new(command));
    }
    keymap
}

/// The toolkit's key event as a key press for a keymap. `None` for keys a
/// keymap has no name for.
pub fn key_press(key: &UiKey, modifiers: UiModifiers) -> Option<KeyPress> {
    let key = match key {
        UiKey::Character(text) => {
            let mut chars = text.chars();
            let c = chars.next()?;
            if chars.next().is_some() {
                return None;
            }
            if c == ' ' { Key::Space } else { Key::Char(c) }
        }
        UiKey::Enter => Key::Enter,
        UiKey::Tab => Key::Tab,
        UiKey::Escape => Key::Escape,
        UiKey::Backspace => Key::Backspace,
        UiKey::Delete => Key::Delete,
        UiKey::ArrowLeft => Key::Left,
        UiKey::ArrowRight => Key::Right,
        UiKey::ArrowUp => Key::Up,
        UiKey::ArrowDown => Key::Down,
        UiKey::Home => Key::Home,
        UiKey::End => Key::End,
        UiKey::PageUp => Key::PageUp,
        UiKey::PageDown => Key::PageDown,
        UiKey::Other(name) => {
            let number = name.strip_prefix('F')?.parse::<u8>().ok()?;
            Key::F(number)
        }
    };
    // On Windows, AltGr arrives as Ctrl+Alt: a character typed with it is
    // just that character (as the toolkit's own key handling treats it).
    let altgr = cfg!(target_os = "windows")
        && modifiers.ctrl
        && modifiers.alt
        && matches!(key, Key::Char(_));
    Some(KeyPress {
        key,
        ctrl: modifiers.ctrl && !altgr,
        meta: modifiers.meta,
        shift: modifiers.shift,
        alt: modifiers.alt && !altgr,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_default_shortcut_parses_and_names_a_board_command() {
        let ids: Vec<&str> = crate::commands::board_commands()
            .iter()
            .map(|command| imaginarium_commands::Command::id(*command).as_str())
            .collect();
        for platform in [Platform::Linux, Platform::MacOs, Platform::Windows] {
            let mut keymap = Keymap::new(platform);
            for (shortcut, command) in DEFAULT_SHORTCUTS {
                keymap
                    .bind(shortcut, CommandId::new(command))
                    .unwrap_or_else(|err| panic!("{shortcut}: {err}"));
                assert!(ids.contains(command), "{command}");
            }
        }
    }

    #[test]
    fn key_events_become_key_presses() {
        let keymap = default_keymap(Platform::Linux);
        let ctrl = UiModifiers {
            ctrl: true,
            ..UiModifiers::default()
        };
        let press = key_press(&UiKey::Character("z".into()), ctrl).unwrap();
        assert_eq!(
            keymap.command_for_press(press).unwrap().as_str(),
            "edit.undo"
        );
        let shifted = UiModifiers {
            ctrl: true,
            shift: true,
            ..UiModifiers::default()
        };
        // Shift gives a capital: still the same key.
        let press = key_press(&UiKey::Character("G".into()), shifted).unwrap();
        assert_eq!(
            keymap.command_for_press(press).unwrap().as_str(),
            "board.ungroup"
        );
        let press = key_press(&UiKey::ArrowLeft, UiModifiers::default()).unwrap();
        assert_eq!(
            keymap.command_for_press(press).unwrap().as_str(),
            "board.nudge_left"
        );
    }
}
