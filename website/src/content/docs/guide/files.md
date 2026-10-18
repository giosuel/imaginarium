---
title: Files
description: Saving, opening, recent boards, renaming and what happens after a crash.
sidebar:
  order: 3
---

## Every change is saved on its own

A board that has a file **saves every change into it as you work**: there
is no Save button, and the top left says **Saved**. If the file can't be
written for a moment (the disk is full, the file went away), it says
**Can't save** and keeps trying.

A file opens **read-only**, and says why at the top left, when:

- it is already open in another window or in another Imaginarium;
- you may not change it;
- it was made with a newer version of Imaginarium;
- it couldn't be brought up to date for this version.

## New boards

A **new board** says **Not saved** until it has a place of its own, but
nothing is lost: it is kept for you as you work. If Imaginarium closes
unexpectedly, the start window offers it the next time ("Untitled board
was kept, with every change"), and **Open it** brings it back.

- **Save…** (`Ctrl+S` / `⌘S`, or the Save… button next to Not saved)
  gives it a name and a folder. From then on it saves itself there.
- Closing a new board you changed asks first: **Save…**, **Don't save**
  or **Cancel**. So does closing a board whose last changes couldn't be
  saved, so you can save it somewhere else.

## Opening, renaming, copies

- **Open…** (`Ctrl+O` / `⌘O`, in a board or the start window) opens a
  board file (`.imag`). You can also drop a board file on a window (not
  yet on Wayland desktops, such as GNOME, Hyprland or Sway on Wayland).
- **Click the board's name** at the top left to **rename or move** it:
  the file moves, and the board goes on saving there.
- **Save a copy…** (`Ctrl+Shift+S`, or the menu behind the logo) saves the
  board as it is into a new file; your window stays with the original.

## Recent boards

The start window's **Recent** lists the boards you opened or saved, newest
first, with a picture of each, their folder and when. The **filter**
finds one by name or folder, and **Open another board…** opens one that
isn't listed. A board whose file is gone leaves the list when you click
it.

## Where things are kept

- New boards wait, until they are saved or closed, in
  `~/.local/share/imaginarium/recovery/` on Linux,
  `~/Library/Application Support/Imaginarium/Recovery/` on macOS and
  `%LOCALAPPDATA%\Imaginarium\Recovery\` on Windows.
- The list of recent boards is `recent.txt` next to that folder.
