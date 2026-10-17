---
title: Settings
description: The theme, the accent color, the interface size and the other settings.
sidebar:
  order: 4
---

**Settings** (`Ctrl+,` / `⌘,`, or the menu behind the logo) has every
setting by section, with a search across them. Changes apply right away,
in every window; a changed setting says **Changed**, with a button that
puts it back.

## Appearance

- **Theme**: follow the system (light or dark, as your desktop says), or
  pick **Light** or **Dark**. High contrast is coming.
- **Accent color**: used for the selection, the active tool and buttons.
- **Board background**: dots, a grid or plain.
- **Reduce motion**: follow the system, or turn animations off or on.
- **Interface size**: 80 to 200 %. It scales panels, menus and dialogs,
  not the board: at 100 % zoom a shape keeps its size.
- **Tool bar position**: the edge it is docked to, or floating (dragging
  it by its grip changes this too).

## This board

Some settings, like the board's background, can differ per board: pick
**This board** at the top of the section to change them for the board
you worked in last. The others keep following your own settings. A
board's own values are saved in its file.

## The settings file

Your settings are saved as you change them, in `settings.toml` in your
configuration folder (`~/.config/imaginarium/` on Linux,
`~/Library/Application Support/Imaginarium/` on macOS,
`%APPDATA%\Imaginarium\` on Windows). You never need to edit it; if it
can't be read, it is set aside and Settings tells you where.
