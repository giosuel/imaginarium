# UI toolkit: windows, widgets, signals, text, accessibility

Status: **Draft** (implemented as the M2 prototype; accepted or replaced
with the M2 gate, ADR-0007). Crates: `imaginarium-ui`, `imaginarium-text`,
`imaginarium-reactive`, `imaginarium-render`.

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
  the accessibility node; `TextInput` is the widget around it.

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
  siblings first: they are drawn on top). A press captures the pointer
  until release, so drags keep going outside the widget. Hover changes are
  reported to both widgets.
- A press focuses the nearest focusable ancestor of the target (or
  nothing). Keys and IME go to the focused widget. Unhandled input bubbles
  to the parents; unhandled Tab / Shift+Tab moves the focus in tree order.
- App-wide shortcuts (Ctrl+K, Ctrl+F) are handled by the view before the
  tree, for now; the command system's keymaps take over in M3.

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

## Accessibility

AccessKit through `accesskit_winit`. The tree builds one node per widget
that returns one (`Widget::accessibility`); widgets without a node are
transparent. Node ids are the widget's `access_base()` (index + 1 in the
upper 32 bits), and a widget may add nodes of its own below it (text runs,
canvas items, list options). Bounds are logical pixels; the window node
carries the scale factor. Requests from the screen reader go back to the
widget owning the node; Focus is handled by the tree. A text field exposes
its value, one text run per line, and the selection.

## Not yet

Scroll containers, menus and popovers with their own layers, drag and
drop, animation, incremental accessibility updates (the whole tree is sent
after each frame), and moving text layout of off-screen content to the
worker pool (PLAN §4.3).
