---
title: The board
description: How a document is shown and edited on the board — the BoardCanvas widget, its commands, the selection, and editing text in place.
---

The board is `imaginarium-canvas`. It turns a document's nodes into
items of the toolkit's tile canvas, keeps them up to date as the document
changes, and turns clicks, drags and keys into commands. The design is
[Canvas](/developers/reference/design/canvas/); this page shows how to
use it.

## Showing a document

`BoardCanvas` is a widget. It owns the open document (ADR-0020: the
document lives on the UI thread):

```rust
use imaginarium_canvas::BoardCanvas;
use imaginarium_render::{CpuMode, TileWorkers};

let workers = TileWorkers::new(TileWorkers::default_threads(), CpuMode::Fast);
let board = ui.add(BoardCanvas::new(document).with_workers(workers));
```

It draws the board's background and dots, the items through the tiles,
and on top of them the frame names, the hover and selection outlines and
the handles. Scroll pans, Ctrl + scroll zooms around the pointer. A theme
switch draws every item again in the theme's colors (palette colors in
their dark versions in the dark theme).

Try it: `cargo run --release -p imaginarium-canvas --example board`
shows the mockups' "Launch plan" board (`--light`, `--grid 10000`,
`--zoom 1.4`).

## Changing the document

Every change goes through the board, so it can redraw only what changed:

```rust
let board = ui.widget_mut::<BoardCanvas>(board_id).unwrap();
board.transact(&Actor::User, |txn| txn.set_geometry(node, geometry))?;
board.undo(UndoScope::User, &Actor::User)?;
board.apply_remote(&[&update])?; // from a collaborator
```

The change set waits until the next paint or input, which applies all
waiting ones at once: the tiles under the changed items' old and new
bounds are drawn again, nothing else.

## Commands

Gestures and keys run commands, the same ones the palette and the AI
will: `commands::board_commands()` lists them for a registry.

| Command | Does | Shortcut |
|---|---|---|
| `board.move` | moves the selection by `dx`, `dy` points | |
| `board.nudge_left` … `_down`, `…_far` | moves it 1 pt (far: 10) | arrows, Shift+arrows |
| `board.set_box` | places one node: `x`, `y`, `w`, `h`, `rotation`, a line's `flip` | |
| `shape.set_radius` | rounds a rectangle's corners | |
| `text.replace` | replaces `delete` characters at `at` with `insert` | |
| `text.edit` | edits the selected node's text | Enter |
| `edit.undo`, `edit.redo` | | Ctrl+Z, Ctrl+Shift+Z or Ctrl+Y |
| `board.delete`, `board.duplicate` | | Delete or Backspace, Ctrl+D |
| `board.select_all` | | Ctrl+A |
| `board.bring_to_front`, `board.send_to_back` | | Ctrl+], Ctrl+[ |
| `board.bring_forward`, `board.send_backward` | | Ctrl+Alt+], Ctrl+Alt+[ |
| `board.group`, `board.ungroup` | | Ctrl+G, Ctrl+Shift+G |
| `board.lock`, `board.unlock` | | Ctrl+Shift+L |
| `view.zoom_in`, `zoom_out`, `zoom_100` | | Ctrl+=, Ctrl+-, Ctrl+0 |
| `view.zoom_to_fit`, `zoom_to_selection` | | Ctrl+1, Ctrl+2 |

(Ctrl is ⌘ on macOS.) `board.execute(id, &args)` runs one on the board
with the registry's checks; locked nodes are refused. The shortcuts are
a `Keymap` of `imaginarium-commands` (`keys::DEFAULT_SHORTCUTS`,
`board.keymap_mut()`).

## The selection

`board.selection()` is a signal: the selected nodes in the order they
were selected, kept valid as nodes go. A click selects, Shift adds,
dragging from empty board draws a marquee, a double-click enters a group,
Esc leaves it, Tab steps through the nodes. Handles resize (Shift keeps
the shape), rotate (Shift snaps to 15°) and round a rectangle's corners;
arrows nudge by 1 pt, Shift by 10. A gesture is one undo step.
`board.node_at(point)` hit-tests like a click.

## Editing text

A double-click on a text box or shape, or Enter with one selected, puts
the toolkit's text field over its text: a shape's text is its label. The
field zooms and turns with the node, and every change is one
`text.replace`, so the document is always up to date and undo merges
typing. Esc, a click elsewhere or losing the focus ends editing.
`board.edit_text(fonts, node)` starts it from code.

## Measuring

`cargo bench -p imaginarium-canvas --bench board` measures opening a
10,000-node board, a drag step, a style change, hit testing and a paste.
On the development VM a drag step costs about 0.03 ms of the 1 ms budget.
