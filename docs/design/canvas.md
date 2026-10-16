# Canvas: the document on the board

Status: **Draft** (M3.0). Crate: `imaginarium-canvas` (new in M3.2), between
the model (`imaginarium-model`) and the tile canvas (`imaginarium-render`,
`rendering.md`). The window around it is `shell.md`.

The canvas turns the document's nodes into canvas items, keeps them up to
date from `ChangeSet`s, finds what's under the pointer, and turns gestures
into commands. It does not own the document (the window does, ADR-0020)
and changes it only through commands.

## 1. Threads

The open `Document` lives on the UI thread (ADR-0020). The canvas reads
its `DocumentView` there, builds display lists there, and the tile workers
rasterize (ADR-0019). Opening builds the document on a worker; big
pastes and imports are converted on a worker and applied in one
transaction.

## 2. Nodes to items

- **One item per visible node** that paints something: `ItemId` is the
  node id. Groups paint nothing (their children are items); hidden nodes
  and their subtrees have no items.
- **World transform.** A node's geometry is in its parent's space (schema
  §5.2: the frame, group or board). The canvas keeps each node's world
  transform (translate, then rotate around the box center, composed down
  the tree), so a moved frame or group re-places its descendants.
- **Z order** follows the tree: a depth-first walk in sibling order (back
  to front). Items get sparse `u64` keys from that walk (gaps of 2^20), so
  inserting a node between two others gets a key between theirs without
  touching the rest; when a gap runs out, the siblings are re-keyed.
- **Display lists** (`rendering.md` §3), per kind:
  - `shape`: the outline from `shape_path(node)` (ADR-0021) in box space,
    filled and stroked with the effective style, under the world transform;
    its text (`t`), if any, is its label: wrapped in the box less 8 pt on
    each side, centered (a triangle's at its centroid), 15 pt medium, as
    in the mockups;
  - `text`: the text laid out in its box (`TextBlock`, plain in M3, rich in
    M5), cached per node and laid out again only when the text, the box
    width or the style changes;
  - `frame`: its background and border, and its name above it; its
    descendants' display lists are wrapped in a clip to the frame;
  - unknown kinds: a dashed box from the geometry (schema rule 4);
  - `image`, `connector`, `raster`: later milestones (placeholder boxes
    until then).
- **Effective styles.** A property's value is the node's override, its
  style, the style's ancestors, then the built-in default
  (`DocumentView::style_prop`, schema §8). A palette color is then shown
  in the theme's version (`shell.md` §7): the theme is an input of
  painting, and switching it repaints every item. The canvas keeps an index from
  each style to the nodes that use it, for style changes.
- **Bounds:** the transformed path bounds, grown by half the stroke width
  and the miter allowance; text by its box. The canvas trusts them
  (`rendering.md` §3).

## 3. Updates from `ChangeSet`s

Every change, local, undo or remote, arrives as a `ChangeSet`; the canvas
updates only what it names:

| Change | Canvas |
|---|---|
| `created` | items for the new nodes and their subtrees |
| `deleted` | remove their items |
| `moved` | new z keys for the node's subtree; new world transforms |
| `changed` geometry | the node's item, and its descendants' (world transforms) |
| `changed` style, text, props (`hidden`, `shape`, `radii`, …) | the node's item; `hidden` adds or removes the subtree |
| `styles` | the items of every node using those styles |
| a rebuilt view | everything (rare: the model rebuilt instead of updating) |

`Canvas::upsert` and `remove` invalidate the old and new bounds, so a
remote change or an undo repaints only its tiles.

**As built (M3.2).** `BoardCanvas::transact`, `undo`, `redo` and
`apply_remote` queue the `ChangeSet`; the next paint or input applies it
(laying out text needs the fonts, which they bring), so several changes
between frames cost one pass. Every visible node keeps its place (its
world transform and the clips of its children), groups included, so a
subtree is built again on its own under its parent. Created and moved
subtrees get keys between their neighbours in the walk, and when a gap
is too small every item is re-keyed. Text layouts are cached by text,
look and width, so moving a shape doesn't lay its label out again.
Nodes using a changed style are found by scanning the visible nodes'
style references, inheriting styles included. A property test checks
that any sequence of edits, undos and redos leaves the items a fresh
build makes.

**Opening** reads the nodes in bulk and builds every item before the first
frame. The M1 note (339 ms for 10,000 nodes, mostly Loro import and
reading key by key) is addressed in M3.2: read with
`LoroTree::get_value_with_meta` on the worker that opens, so the UI thread
gets a ready view.

## 4. Hit testing

`hit(point)` returns the topmost node under a point:

1. candidates from the canvas's grid, front to back (today
   `Canvas::item_at` returns the topmost item by bounds; M3.2 adds a query
   for all items whose bounds hold the point);
2. an exact test per candidate in its box space: inside the filled path,
   or within the stroke's half width plus a tolerance of 4 screen pixels
   (so thin lines stay easy to hit at any zoom); unfilled shapes are hit
   on their outline only;
3. locked and hidden nodes are skipped; a click inside a frame's empty
   area hits nothing (it starts a marquee), its name label hits the frame;
4. a group is selected as a whole: the hit is mapped to the outermost
   unselected group, and a double-click enters the group (as in Figma).

`hit_rect(rect)` (the marquee) returns the nodes whose bounds lie fully
inside, at the same level as the first hit.

**As built (M3.2).** Candidates are the items whose bounds overlap a
square of the tolerance around the pointer (`Canvas::items_overlapping`;
a horizontal line's bounds are thinner than the tolerance), front to
back. Points a frame clips away don't hit its children. Labels and text
boxes are hit on their text too. The marquee takes each item's
selectable unit (the outermost group not entered), and the outermost
frame wholly inside it instead of the frame's children; a unit counts
when its turned box lies wholly inside.

## 5. Selection and handles

- **Selection** is a signal in the window's state: an ordered set of node
  ids, kept valid as nodes go (`deleted` removes them). Remote changes
  keep it; a remote delete drops the node from it.
- The selection, its handles and the hover outline are drawn **on top of
  the tiles** each frame (`Canvas::draw_direct`), not baked into them, so
  selecting never invalidates tiles.
- **Handles:** 8 resize handles, a rotation handle above the top edge, a
  corner-radius handle inside a rounded rectangle's corner, and a size
  badge ("220 × 88") under the selection during a resize. Multi-selections
  resize and rotate as one box.
- **Keyboard:** arrows nudge by 1 pt (Shift: 10), Esc clears, Tab moves to
  the next node in z order, Enter edits text or enters a group.
- **As built (M3.2, selection):** `BoardCanvas::selection()` is the
  signal; a click selects, Shift+click adds or removes, a press on empty
  board starts a marquee (Shift keeps the selection), a double-click
  enters a group and Esc leaves it with the group selected, then clears.
  The hover outline and the selection's outlines (the accent, 35 %
  toward white in the dark theme) and the marquee are drawn on top of
  the tiles. Screen readers get the visible items (2,000 at most) below
  the board's `Canvas` node, the last selected one focused; focusing one
  selects it.

## 6. The editing path

**Every edit is a command** (invariant 2). Gestures call the same
commands as the palette, the menus and the AI, with arguments:

| Gesture | Command | Undo |
|---|---|---|
| drag the selection | `board.move` (`dx`, `dy`) per pointer move | one step for the drag |
| resize, rotate | `board.transform` (box per node) per pointer move | one step |
| draw a shape with a tool | `board.insert_shape` (`preset`, box), then the size per move | one step |
| type in a text box | `text.insert`, `text.delete` | merged per burst (500 ms, `UndoConfig`) |
| corner-radius handle | `shape.set_radius` | one step |

- A **gesture** opens an undo group at the press (`begin_undo_group`) and
  closes it at the release, so a drag of 200 pointer moves is one undo
  step. Each move transacts at once: the model and the tiles stay current,
  and collaborators (M8) see the drag live.
- Pointer moves are coalesced per frame (PLAN §10): at most one
  transaction per frame per gesture.
- If a drag repaints too many tiles, the later optimization in
  `rendering.md` §5 applies (draw the dragged items live on top and bake
  them in at the release); M3.2's benchmarks decide.
- **The command context** (`BoardCx`): the document, the selection, the
  viewport and the actor. Availability checks read the view, so the
  contextual toolbar and the palette can ask every frame cheaply.
- **As built (M3.2, gestures):** the commands' context is the board
  itself (`FnCommand<BoardCanvas>`); `commands::board_commands()` lists
  `board.move` (dx, dy for the selection), `board.set_box` (one node's
  x, y, w, h and rotation in its parent's space) and `shape.set_radius`,
  and gestures and keys run them through a `Registry`. A press on a
  selected node drags it once the pointer travels 3 px; the 8 handles
  resize (Shift keeps the shape at the corners), the handle 20 px above
  rotates around the box's center (Shift snaps to 15°), the one inside a
  rectangle's top left corner sets its radius, and a badge shows the
  size while resizing. Several nodes share one straight box: they move,
  scale and turn with it. Arrows nudge by 1 pt, Shift by 10. A gesture
  is one undo group from press to release, and its pointer moves are
  applied once a frame, before painting, from the latest position (move
  by the delta since the last frame; resize and rotate from where the
  gesture started). Known gap: the model merges undo steps within
  500 ms, so a gesture that starts right after another edit undoes
  together with it.

## 7. Text on the board

M3 edits plain text in place (rich text is M5): a double-click or Enter
puts a `TextField` over the text box at the board's zoom; each edit is a
transaction (`insert_text`, `delete_text`), so the model is always the
truth and undo merges typing. The field is the toolkit's (IME, clipboard,
accessibility), positioned in screen space and scaled with the zoom.

## 8. Clipboard

Copying nodes puts two forms on the clipboard:

- **HTML** with the nodes as JSON in a marked comment
  (`<!--imaginarium:nodes:…-->`, base64), plus a readable rendering of
  their text. It survives every platform's clipboard (custom formats
  don't on all of them).
- **Plain text**: the nodes' text, for pasting elsewhere.

The JSON holds the subtrees (kinds, geometry, local styles, text, props)
and the named styles they use (by value, so a paste into another board
keeps its look). A paste creates new ids, offsets by 16 pt when pasting
into the same place, and is one transaction. Pasted plain text becomes a
text box; images come with M4.

## 9. Accessibility

Each item is an accessibility node below the canvas widget (as in the
gate): role by kind (shape, text, frame as a group), the name from `name`,
else the text, else the preset ("Rectangle"); bounds in window
coordinates. The selection is the focused node, so a screen reader follows
it, and the keyboard commands (§5) work without a pointer.

## 10. Benchmarks (M3.2)

Added to `imaginarium-canvas`'s benches (criterion, the conventions in
`imaginarium-profiling`):

- open a 10,000-node board: view, items, first frame;
- a drag step on it: the command, the `ChangeSet` and the dirty tiles
  (budget: under 1 ms for the model and canvas, before rendering);
- a style change used by 1,000 nodes;
- hit testing at 10,000 nodes;
- a 1,000-node paste.

## 11. Decided questions

1. **Frame names are drawn on top of the tiles** (owner, 2026-10-16), at
   the interface's size at every zoom, like the selection.
