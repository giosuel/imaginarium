---
title: The UI toolkit
description: Windows, widgets, signals, text, accessibility and the canvas, and how to build UI with them.
---

Imaginarium draws its own UI. The decision is
[ADR-0007](/developers/reference/adr/0007-own-ui-toolkit/): the toolkit
had to pass a prototype gate on an integrated GPU first, and it did in M2.
The design notes are in
[UI toolkit](/developers/reference/design/ui-toolkit/) and
[Rendering](/developers/reference/design/rendering/). This page explains
how to use the toolkit.

| Crate | What it does |
|---|---|
| `imaginarium-reactive` | Signals, memos, effects and scopes: state that knows who reads it |
| `imaginarium-text` | Fonts, text layout and editing, on Parley |
| `imaginarium-render` | Scenes, the GPU and CPU renderers (Vello), the tiled canvas |
| `imaginarium-ui` | The window, input, the widget tree on Taffy, widgets, text fields, accessibility (AccessKit), the clipboard |

The Rust API docs have the details: `cargo doc --workspace --no-deps --open`.

## A first window

[`crates/imaginarium-ui/examples/counter.rs`](https://github.com/giosuel/imaginarium/blob/main/crates/imaginarium-ui/examples/counter.rs)
is the smallest app on the toolkit. It has a text input, a button, and a
label that greets you and counts the clicks:

```sh
cargo run --release -p imaginarium-ui --example counter
```

State lives in **signals**, outside the widgets. The widgets change it,
and the label is computed from it:

```rust
let name = signal(String::new());
let clicks = signal(0u32);

let mut ui = Ui::new();
let input = ui.add(
    TextInput::new("Name", "Your name", style, InputColors::themed())
        .on_change(move |text| name.set(text.to_owned())),
);
let button = ui.add(Button::new("Click me", style, ButtonColors::primary(), move || {
    clicks.update(|clicks| *clicks += 1);
}));
// Lays out again when `name` or `clicks` change, and nothing else does.
let greeting = label(&mut ui, style, Token::Text, move || {
    let name = name.get();
    let who = if name.is_empty() { "there" } else { &name };
    format!("Hello, {who}! You clicked {} times.", clicks.get())
});
let column = ui.add_with_children(
    Flex::column().gap(12.0).padding(24.0, 24.0).fill().background(Token::PanelBg),
    &[input, button, greeting],
);
ui.set_root(column);
imaginarium_ui::run(options, view)?;
```

The window shows a **view** (`View`). Here the view is a thin layer that
passes the window's events to the widget tree (`Ui`): painting, input,
the caret's timer, the input method's position, and the screen reader's
tree and requests. Each of these is one line in the example.

## No frame without a reason

The event loop sleeps until something happens. A frame is drawn when:
- the window appears, changes size or moves to a monitor with another
  scale;
- the view handled an input (`View::input` returned `true`);
- the view's timer came (`View::wake_at`), such as a caret blink;
- something called `request_frame()`, which is what effects do when a
  signal changes.

`FrameInfo::causes` tells which. An idle window draws nothing and uses no
CPU (gate criterion 6). So don't return `true` from `View::presented`
unless something is animating, and let effects ask for frames.

A frame skipped because the window is hidden isn't retried in a loop:
the next one comes when the window shows again.

## Signals

`imaginarium-reactive` works like the signals in SolidJS and Leptos:
- `signal(value)` is state. Reading it in a memo or an effect subscribes
  them to it, and `set`/`update` updates them.
- `memo(f)` is a derived value. It is lazy, cached, and recomputed only
  when something it read changed. An equal result (`PartialEq`) stops the
  update there.
- `effect(f)` is a side effect. It runs again when something it read
  changed, at most once per write or `batch`.
- `Scope` owns what is created in it and disposes it, with its
  `on_cleanup` callbacks.

Updates are glitch-free: no computation sees a mix of old and new values.
They are also minimal: a computation runs only if a value it read really
changed. A computation depends on what it read in its latest run.

In the toolkit, effects are where signals meet widgets:
- `Ui::watch(widget, layout, f)` repaints (or re-lays out) `widget`
  whenever something `f` reads changes. The effect is disposed with the
  widget.
- `label(&mut ui, style, color, f)` is a label built this way.
- Widgets read signals untracked when they paint (`get_untracked`). What
  they show changes through `watch`, so a signal change touches exactly
  the widgets that show it.
- Put derived data in a memo. The gate's palette results are a memo over
  the query, computed once per keystroke when the list reads them.
- Per-frame numbers that must not cause frames (the gate's status line)
  live in plain cells and are refreshed with `Ui::refresh` during a frame.

The graph is per thread, and handles are `Copy` but not `Send`. Results
from workers come back through channels and are written to signals on
the UI thread. A write to one signal with an effect takes about 0.1 µs,
and one write read by 10,000 effects takes 0.9 ms
(`cargo bench -p imaginarium-reactive`).

## Widgets

A widget implements what it needs of the `Widget` trait:

| Method | For |
|---|---|
| `style` | Its layout: Taffy's flexbox style (size, flex, padding, gap, position) |
| `measure` | A leaf's size for the space offered (a label measures its text) |
| `paint` | Drawing itself into its scene; children draw after it, on top |
| `event` | Input aimed at it (`true`: handled; otherwise it bubbles to the parent) |
| `focusable`, `focus_changed` | Taking the keyboard focus |
| `hover_changed` | The pointer entering or leaving |
| `clips` | Clipping its children to its box |
| `accessibility`, `access_action` | Its node for screen readers, and their requests |
| `ime_area` | Where the input method's window goes while it has the focus |
| `wake_at`, `wake` | A timer (a caret blink) |

Each method gets a `Cx`: the widget's id and box, whether it has the focus
or the pointer, the fonts and the clipboard. Through it the widget asks
for things: `request_paint`, `request_layout`, `request_focus`,
`resign_focus`, `capture_pointer` and `release_pointer`.

The tree keeps each widget's drawing until it is invalidated. A frame
repaints only the widgets that changed, and composes the rest from their
cached scenes. In the gate, a palette keystroke repaints 5 of 20 widgets.

Changing a widget from outside: `ui.widget_mut::<TextInput>(id)` gives
the concrete widget, marks it for paint and layout, and reads its style
again at the next layout. Ids are generational: an id of a removed widget
never reaches the widget that takes its slot. `set_children` detaches the
children it leaves out, and `remove` removes a subtree.

Built in: `Flex` (rows and columns with background, border, radius and
shadow), `Label` and `label`, `Button` (styles `ButtonColors::primary`,
`secondary`, `quiet` and `danger`), `TextInput`, and `Icon` (a line
icon from SVG path data on Lucide's 24-unit grid).

Labels wrap like text in CSS: in a box that gets too narrow, a label
breaks lines down to its longest word. `Label::single_line()` keeps one on
a single line (a status line, which its parent clips).

Focus rings follow the keyboard, like CSS's `:focus-visible`: a click
focuses a button without a ring, and the next navigating key (not
Ctrl, Shift or Alt on its own, nor a Ctrl or Alt shortcut) shows it, as
does a screen reader moving the focus. Widgets ask
`cx.is_focus_visible()` when they paint; a view that handles a shortcut
itself calls `ui.note_keyboard()`.

## Layers: popovers, dialogs, tooltips

Anything that floats above the window is a layer. Build its content like
any widget, without a parent, and open it next to an anchor:

```rust
use imaginarium_ui::{Anchor, LayerOptions, Placement};

let menu = ui.add_with_children(Flex::column().background(Token::PanelBg), &items);
let layer = ui.open_layer(menu, LayerOptions::popover(Anchor::Widget(button)));
// A dialog in the window's center, or a tooltip that takes no input:
ui.open_layer(dialog, LayerOptions::modal());
ui.open_layer(tip, LayerOptions::tooltip(Anchor::Widget(tool)).placement(Placement::Right));
```

The tree does the rest:
- it places the layer and flips it to the other side when there's no room;
- it moves the focus into it, and back when it closes;
- `Esc` and a click outside close a popover, and that click doesn't also
  reach what's below;
- a dialog takes all input, and Tab stays inside it.

`ui.close_layer(fonts, layer)` closes a layer, or a widget in it calls
`cx.close_layer()`. `LayerOptions::on_close` tells the opener, and
`LayerOptions::focus(widget)` picks where the focus starts (a dialog's
safe button, not its destructive one).

## Component gallery

`cargo run --release -p imaginarium-ui --example gallery` shows every
component in its states, in the current theme, with a switch between dark
and light (`--light` starts light; `--open popover`, `dialog` or `tooltip`
opens a layer at once, for screenshots). Every new component gets a section
there before the shell uses it, and its screenshots go into the PR.

## Themes

The interface's colors are **tokens** of a `Theme` (`Token::PanelBg`,
`Token::Text`, `Token::AccentInk`, …), with the dark theme first and the
light one on the same tokens. Widgets take a `Paint`, which is a token or
a fixed color (`Color` and `Token` both convert into it):

```rust
use imaginarium_ui::{Button, ButtonColors, Flex, InputColors, Label, TextInput, Token};

let panel = Flex::column().background(Token::PanelBg).border(1.0, Token::PanelBorder);
let hint = Label::new("Saved", style).color(Token::TextMuted);
let save = Button::new("Save…", style, ButtonColors::primary(), save);
let name = TextInput::new("Name", "", style, InputColors::themed());
```

Labels draw in `Token::Text` unless `.color(…)` says otherwise (the text
style's own color isn't used). Use tokens for everything that is part of
the interface, and fixed colors only for content whose color is data (a
swatch, a shape's fill).
`set_theme(Theme::light(accent))` switches the theme: every widget
repaints before the next frame, and labels shape their text again in the
new color. Inside an effect or memo, `theme()` is a dependency, so derived
values follow the theme too. A unit test keeps every text token at 4.5:1
or more against its backgrounds, in both themes and for every accent the
settings offer, and the focus ring at 3:1; a new token pair belongs in
it. Don't call `set_theme` from an effect that reads `theme()`: the
switch runs that effect again.

## Input and focus

- **Pointer:** input goes to the deepest widget under the pointer, later
  siblings first (they are drawn on top). Children that stick out of a
  parent that doesn't clip still get clicks. A press captures the pointer
  until every button is up, so a drag goes on outside the widget.
  Losing the window's focus ends the capture.
- **Focus:** a press focuses the nearest focusable ancestor of the
  target, or nothing. Keys and input method events go to the focused
  widget and bubble up to its parents until one handles them. An
  unhandled Tab or Shift+Tab moves the focus in tree order. A widget that
  is hidden or detached loses the focus at the next layout, so keys never
  go to a field nobody sees.
- **Shortcuts** are matched by their Latin letters. With Ctrl or ⌘ held,
  a key on a Cyrillic or Greek layout stands for the Latin letter on the
  same key, as in other apps. App-wide shortcuts (the gate's Ctrl+K and
  Ctrl+F) are handled by the view before the tree for now. The command
  system's keymaps (`imaginarium-commands`) take them over in M3.

## Text

- `Fonts::new()` loads the bundled Inter and the system's fonts. A script
  that Inter lacks falls back to a system font. `Fonts::bundled_only()`
  is for tests: the same pixels on every machine.
- `TextStyle` is a size, weight, color and line height. `TextBlock` is
  laid-out text to measure and paint.
- `Editor` is editable plain text, on Parley's `PlainEditor`. It has a
  selection, editing operations (`Edit`, `Motion`) and the input method's
  composition. While a composition is under way, offsets are in the text
  as shown (`Editor::shown_text`), not in `Editor::text`.
- `TextField` adds focus, the platform's editing keys, the input method, a
  caret that blinks for 10 s after the last key and then rests (so an idle
  focused field draws nothing), the clipboard, and its accessible node with
  one text run per line. `TextInput` is the widget around it.

**Input method:** while a field has the focus, `View::ime_area` places
the candidate window next to the caret, and the composition shows
underlined in the field. When the focus moves to another field, the input
method starts over, so a composition can't end up in the wrong field.

**Clipboard:** `Clipboard` works on a thread of its own, because reading
it can wait for another app (on X11 the app that copied is asked, and may
take seconds to answer). `set_text` copies. `request_text` asks for the
text, and the text arrives later as `Input::Paste` for the focused widget.
Without a system clipboard, copy and paste still work inside the app.

## Accessibility

Screen readers (Orca, NVDA, VoiceOver) get an accessibility tree through
[AccessKit](https://github.com/AccessKit/accesskit):
- A widget describes itself in `Widget::accessibility` by returning a
  `Node` with a role, label, bounds and actions. A widget that returns
  `None` is transparent: its children belong to its parent's node.
- Node ids come from `WidgetId::access_base()`. A widget can add nodes of
  its own below that id (text runs, canvas items, list options) and list
  them with `Cx::add_access_child`.
- After every frame, while a screen reader listens, the window sends the
  whole tree with its focus. A focus on a node that isn't in the tree
  falls back to the window, as AccessKit requires.
- Requests from the screen reader (click, focus, set the selection,
  replace text) go to the widget that owns the node. Focus is handled by
  the tree.

Test new widgets the way the toolkit's own tests do (`src/tests.rs`):
build the update and feed it to `accesskit_consumer::Tree::new`, which
panics on an update the platform adapters would reject.

## The canvas

Boards are drawn through `imaginarium-render`'s `Canvas`, a tile cache:
- Items are an id, a stacking order and the bounds they paint in.
  `upsert`, `remove` and `invalidate` mark the tiles over their areas
  stale. A `CanvasContent` paints an item into a tile when asked, at the
  tile's scale, so it can pick a level of detail.
- `frame(scene, viewport, now, content)` draws the visible tiles. It renders
  the stale ones within a time budget, and shows a stand-in for the rest
  until a later frame: the tile's stale copy, or another zoom level's tiles
  that cover it. Panning renders only tiles coming into view.
- Tiles are 256 device pixels at levels half an octave apart. A zoom
  gesture keeps drawing the current level, scaled, and switches 150 ms
  after it stops.
- `with_workers(TileWorkers::new(threads, mode))` renders tiles on worker
  threads with `vello_cpu`. Frames upload what came back, 16 tiles at most
  per frame. Tell the canvas the renderer's time and the rest of the frame
  (`note_render_time`, `note_rest_of_frame`), and it fits its work into
  the frame.
- A dragged item is lifted out of the tiles and drawn live on top. On
  drop, `upsert_drawn_elsewhere` redraws only its new place.
- Before dropping a canvas whose renderer stays, call `release` to free
  its textures.

The [rendering design](/developers/reference/design/rendering/) has the
numbers behind each choice.

## Measuring

- **Budgets** are requirements
  ([PLAN §10](/developers/reference/plan/#10-performance-strategy)). Add or
  extend a benchmark when you touch a hot path: `cargo bench -p
  imaginarium-reactive` and `cargo bench -p imaginarium-render` (canvas
  edits).
- **The gate app** (`imaginarium-gate`) measures the toolkit on a real
  machine and writes `gate-report.json`. It covers frame times on a
  10,000-shape board, typing, the palette's search, cold start and idle.
  How to run it and read the report:
  [M2 gate](/developers/reference/design/m2-gate/).
- **Profiling:** `IMAGINARIUM_LOG=debug` for logs, Tracy with
  `--features imaginarium-profiling/tracy --profile profiling`.

## Rules of thumb

- **Never block the UI thread.** File IO, clipboard reads, network and
  layout of off-screen content belong on a worker. Their results come
  back as signals or inputs.
- **Repaint only what changed.** Use `watch`, `request_paint` and
  `invalidate` with the smallest area. Never invalidate everything for a
  change that one overlay can show (the gate's search dims cards over
  the tiles instead of in them).
- **State in signals or the document, widgets show it.** Edits to a
  document go through commands (`imaginarium-commands`), never straight to
  the model.
