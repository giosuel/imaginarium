# UI toolkit: windows, widgets, signals, text, accessibility

Status: **Accepted** (M2: the gate passed and the owner kept the toolkit,
2026-10-13, ADR-0007; reviewed in M2.9). Crates: `imaginarium-ui`,
`imaginarium-text`, `imaginarium-reactive`, `imaginarium-render`. How to
use it: the developer guide's "UI toolkit" page
(`website/src/content/docs/developers/ui-toolkit.md`) and the example
`crates/imaginarium-ui/examples/counter.rs`.

## Layers

```
 app / gate view ──► Ui (widget tree) ──► Scene ──► WindowRenderer (vello_gpu) ─► GPU
        ▲                 ▲  │ Taffy layout          CpuRenderer (vello_cpu) ─► pixmap
        │ Input           │  └ imaginarium-text (Parley): shaping, lines, editing
 winit event loop ── Shell (frame scheduler, IME, AccessKit adapter)
        signals (imaginarium-reactive) ── effects ── invalidate(widget)
```

- **Shell** (`window.rs`): one winit window. `ControlFlow::Wait`; a frame
  only for a resize (to another size), a scale change, handled input, a
  view timer, or `request_frame()`; `FrameInfo::causes` says which. The
  view paints one scene in logical pixels, drawn at the scale factor. It
  forwards IME events, places the candidate window (`View::ime_area`), and
  sends the accessibility tree after frames while a screen reader listens.
  A frame the renderer skips says why (`Skip`): the surface was set up
  again (draw again now), was lost (made again), the window is hidden
  (wait until it shows: no redraw loop while minimized), or no texture
  came in time (try again in 100 ms).
- **Startup:** the GPU opens on a thread of its own (`GpuStart`: drivers,
  device, Vello's pipelines), started first thing (`run_with_gpu`), while
  the program loads fonts, builds its view and lays out the first frame;
  the first frame then waits for it and creates the window's surface.
  `View::opened` comes after that first layout.
- **Widget tree** (`tree.rs`): the retained UI. Widgets live in an arena
  with generational ids. The tree owns layout, input routing, focus,
  painting and the accessibility tree; widgets implement only what they
  need of the `Widget` trait.
- **Text** (`imaginarium-text`): fonts (bundled Inter, then system fonts
  with per-script fallback), `TextBlock` and `Editor` on Parley 0.12. The
  font stack names no generic family: with fontconfig, `sans-serif` is
  every installed family, and a character Inter lacks was looked up in
  each of them (a one-second freeze with 2,000 families; M2.7).
  `TextField` (ui) adds focus, keys, IME, clipboard, caret blinking and
  the accessibility node; `TextInput` is the widget around it. While a
  composition is under way, the editor's offsets (selection, lines) are
  in the text as shown, composition included (`Editor::shown_text`), and
  so is the accessible value.
- **Clipboard** (`clipboard.rs`): on a thread of its own, since a read
  can wait for another app (X11 asks the app that copied; arboard waits
  up to 4 s). A paste is a request, and the text comes back as
  `Input::Paste` for the focused widget, through the event loop. Without
  a system clipboard, the app's own copy pastes.

## Layout

Taffy 0.15 (flexbox, absolute positioning). Each widget has a Taffy node;
`Widget::style` gives the style, leaves measure themselves
(`Widget::measure`, e.g. a label's text for the offered width). After
layout the tree computes window coordinates; a widget whose box moved is
repainted. Labels shape their text once and only re-break lines when Taffy
offers other widths.

## Painting

Every widget keeps its own drawing in a `Scene` until it is invalidated
(`Cx::request_paint`, `request_layout`, a moved box, or `invalidate` from
an effect). A frame repaints the dirty widgets and composes all scenes in
tree order (`Scene::append`), clipping where a widget asks for it.
Measured in the gate app: a palette keystroke repaints 5 of 20 widgets.
The canvas is one widget that repaints itself; its tiles (M2.5) cache
within it.

## Input and focus

- Pointer input goes to the deepest widget under the pointer (later
  siblings first: they are drawn on top). Children outside a parent that
  doesn't clip still get it, as they are drawn. A press captures the
  pointer until every button is up (and the window keeps its focus), so
  drags keep going outside the widget. Hover changes are reported to both
  widgets.
- A press focuses the nearest focusable ancestor of the target (or
  nothing). Keys and IME go to the focused widget. Unhandled input bubbles
  to the parents; unhandled Tab / Shift+Tab moves the focus in tree order.
  A hidden or detached widget loses the focus (and the capture) at the
  next layout. When the focus moves, the window turns the input method
  off and on, so a composition starts over.
- **Focus rings follow the keyboard** (CSS's `:focus-visible`): the tree
  remembers whether the last input was a press or a key, and
  `Cx::is_focus_visible` is true only for the focused widget after a key.
  The focused widget repaints when that changes.
- App-wide shortcuts (Ctrl+K, Ctrl+F) are handled by the view before the
  tree, for now; the command system's keymaps take over in M3. With Ctrl
  or ⌘ held, a non-Latin letter stands for the Latin letter on the same
  key (shortcuts are matched by Latin names).

## Signals

Widgets read signals when they lay out or paint (untracked); `Ui::watch`
ties a widget to what a closure reads, and the effect calls
`invalidate(widget)` when it changes. So a signal change touches exactly
the widgets that show it, the effect is disposed with the widget, and the
invalidation is applied before the next layout. A memo is the natural home
for derived data: the palette's results are a memo over the query signal,
computed once per keystroke when the list reads it. Per-frame data that
must not cause frames (the status line's timings) is read from plain
cells and refreshed with `Ui::refresh` during a frame.

## Layers

Popovers and menus, modal dialogs and tooltips are **layers**: extra roots
above the window's root, in a stack (`layers.rs`, `Ui::open_layer`;
`shell.md` §6). A layer's content is laid out at its own size (Taffy with
max-content space), then placed next to its anchor: a widget (followed as
it moves; the layer closes when it goes), a rectangle, a point or the
window's center. It opens on the asked side, flips when only the other
side fits, and stays 8 px inside the window; a layer wider than the
window is laid out again at the window's width, so its text wraps (a
taller one needs a scroll container).

- **Input:** hit testing goes from the top layer down. Tooltips are
  passed through; nothing below a modal is reached, and a click beside a
  dialog does nothing, not even unfocus. A press outside the top popovers
  closes them and is used up by that, so it doesn't also click what lies
  below. `Esc`, when the focused widget doesn't use it, closes the top
  dismissable layer. While a dialog is open, only it and the layers above
  it take input: keys, the input method and pastes go to it even before
  the focus has moved in (or when it has nothing to focus), a drag and
  the hover below it end when it opens, and a screen reader's requests
  for widgets behind it are refused.
- **Focus:** a popover or dialog takes the focus (its first focusable
  widget) after its first layout, and gives it back to whatever had it
  when it opened. Tab cycles inside the top dialog, or inside the layer
  the focus is in.
- **Closing:** `Ui::close_layer`, or `Cx::close_layer` from a widget in
  it (a menu item that ran), closes it and the layers opened after it
  (submenus), removes their widgets and runs their `on_close`.
- **Painting and accessibility:** layers paint after the root, a dialog
  over a scrim (`Token::Scrim`); their nodes are top-level nodes of the
  window, after the root's, and what a dialog covers is left out.
- **Re-entrancy:** a widget that closes its layer from `focus_changed`
  (losing the focus) closes it after the focus change is done, so the
  focus never detours.

## Themes

The interface's colors are tokens of a `Theme` (`theme.rs`; values in
`shell.md` §7), dark first. Widgets hold a `Paint` (a token or a fixed
color) and resolve it when they paint. `set_theme` bumps a generation that
the tree compares before each layout: when it changed, every widget is
laid out and repainted again (labels keep the color they shaped with and
shape again). A theme switch is rare, so it doesn't track which widgets
use which token. `theme()` reads a detached signal, so effects and memos
that call it run again after a switch. A unit test checks that every text
token reaches 4.5:1 on its backgrounds in both themes and for every
offered accent.

## Accessibility

AccessKit through `accesskit_winit`. The tree builds one node per widget
that returns one (`Widget::accessibility`); widgets without a node are
transparent. Node ids are the widget's `access_base()`: its slot + 1 in
the top 24 bits, the low 16 bits of its generation below that, so a late
request about a removed widget doesn't reach the widget that took its
slot. A widget may add `ACCESS_LOCAL_IDS` (2²⁴ − 1) nodes of its own below
it (text runs,
canvas items, list options). Bounds are logical pixels; the window node
carries the scale factor. Requests from the screen reader go back to the
widget owning the node; Focus is handled by the tree (only for a live,
focusable widget). A text field exposes its value, one text run per line
(characters are grapheme clusters), and the selection; moving the caret
by screen reader asks the tree for the focus.

Every update is a whole tree with its `TreeInfo`: AccessKit builds its
tree from the first update after a screen reader starts, which needn't
be the one it asked for. A focus on a node missing from the update falls
back to the window. Both panicked in AccessKit before the M2.9 review;
the tests now feed the updates to `accesskit_consumer`.

## Not yet

Scroll containers, drag and
drop, animation, incremental accessibility updates (the whole tree is sent
after each frame), and moving text layout of off-screen content to the
worker pool (PLAN §4.3).
