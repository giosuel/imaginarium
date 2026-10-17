---
title: Introduction
description: What Imaginarium is and how it works, for users.
sidebar:
  order: 0
---

Imaginarium puts every kind of office work on **one surface**:

- **Board**: an infinite canvas for diagrams, whiteboarding and mind maps
  (like draw.io, Visio or a whiteboard), with shape libraries from
  template packages.
- **Document**: flowing, paginated text (like Word or Writer), with DOCX
  and ODT in and out.
- **Presentation**: slides (like PowerPoint or Impress), with PPTX and ODP
  in and out.
- **Layout**: fixed pages with linked text frames (like Publisher).
- **Paint**: raster layers with brushes and AI image tools.

Because they share one document model, anything can go anywhere: a diagram
from a board can be dropped into a slide or a document as it is.

## Starting

Imaginarium opens with its **start window**: **New board** (`Ctrl+N` /
`⌘N`) opens an empty board, and later your recent boards are listed here
too. Turn off **Show at startup** to go straight to a new board.

**Settings** (`Ctrl+,` / `⌘,`) has every setting by section, with a
search across them; changes apply right away. Some settings, like the
board's background, can also differ per board: pick **This board** to
change them for the board you're working on only. Under **Appearance** you pick the theme (follow the system, light or dark)
and the accent color, and the **interface size** (panels and menus,
not the board); they apply to every window at once. Your settings
are saved as you change them, in `settings.toml` in your
configuration folder (`~/.config/imaginarium/` on Linux,
`~/Library/Application Support/Imaginarium/` on macOS,
`%APPDATA%\Imaginarium\` on Windows). You never need to edit it.

**Every change is saved on its own**, as you work: there is no Save
button for a board that has a file, and the top left says **Saved**. If
the file can't be written (it is open in another window, or comes from a
newer version), the board opens read-only and says why.

A **new board** says **Not saved** until it has a place of its own, but it
is kept anyway: if Imaginarium closes unexpectedly, the start window
offers it the next time, with every change. Closing it asks whether to
save it first. **Save…** (`Ctrl+S` / `⌘S`) gives it a name and a folder; from then
on it saves itself there. Click the board's name to **rename or move**
it, and use **Save a copy…** (`Ctrl+Shift+S`) from the menu behind the
logo to keep a copy elsewhere. **Open…** (`Ctrl+O` / `⌘O`) opens a board
file; the boards you opened lately are in the start window's **Recent**,
with a picture of each and a filter. You can also drop a board file on a
window (not yet on Wayland desktops).

Every board has a **window of its own**. `Ctrl+N` in a board opens
another one, and `Ctrl+W` / `⌘W` closes it; when the last board closes,
the start window comes back.

## Finding features

The interface stays out of your way. Instead of menus:

- **Command palette** (`Ctrl+K` / `⌘K`): type what you want ("align
  center", "insert table", "dark mode"). It also finds settings, and it
  learns what you use often.
- **Contextual toolbar**: appears next to your selection, with only what
  applies to it.
- **Quick insert**: type `/` in text, or double-click the canvas.
- **Inspector** (`Ctrl+I` / `⌘I`): every property of the selection, when you
  need the details.

:::note
These guides are being written alongside the features. Pages appear here as
each part ships.
:::
