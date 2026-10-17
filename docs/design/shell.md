# Shell: the window around the board

Status: **Being built** (M3.4: the board window, §3.1 to §3.4, is
built; the rest follows). Crates: `imaginarium-shell` (new in M3.4) and
`imaginarium-app`, on the toolkit (`imaginarium-ui`, `ui-toolkit.md`).
The board itself, its items and its editing, is in `canvas.md`.

Where this comes from: the owner's mockups (2026-10-16, HTML pages sent to
the owner and not committed) and the decisions in PLAN §14. The mockups
cover more than M3 builds (documents, sheets, presentations, the draw
tool); this document says where those go, so M3 leaves room for them, and
marks them with their milestone.

## 1. The look

Clean, modern and simple, in the style and feel of Excalidraw's interface,
not its layout (owner). Concretely:

- The board fills the window. Everything else floats over it in **panels**:
  rounded rectangles (radius 12 to 14 for panels, 8 to 10 for controls)
  with a 1 px border and a soft shadow, 16 px from the window's edges.
- Nothing is shown that the current task doesn't need. Panels open on
  request (inspector, palette) or for the selection (contextual toolbar).
- One accent color, from the logo (`#3A4BD6`). It marks the active tool,
  the selection, primary buttons and focus.
- Inter (bundled) for the whole interface: 13 px for controls and lists,
  12 px for hints and shortcuts, 14 to 15 px for titles.
- Line icons (Lucide, §8), 18 to 20 px, 1.75 px strokes.
- Shapes on the board are crisp, not hand-drawn.

**Dark first** (owner, 2026-10-16): M3 builds and tests the dark theme.
Light and high contrast follow on the same tokens (§7).

## 2. Windows

- **One window per document**, and a **start window** like Blender's
  splash (owner).
- **The title bar is the system's** (owner, 2026-10-16). The window
  manager draws it; where it draws none, the window has no bar.
  - Hyprland and other tiling compositors: no bar. The board's name and
    save state are in the document pill (§3.1), so nothing is lost.
  - KDE Plasma, Windows, macOS: their normal bar.
  - GNOME draws none for apps; winit then draws its plain fallback
    (`wayland-csd-adwaita-notitle`, already enabled).
- **Window title:** `<board name> — Imaginarium` (status bars and window
  rules show it). The start window is `Imaginarium`.
- **App ID** (Wayland) and window class (X11): `ch.giosuel.Imaginarium`,
  so window rules match. It is also the name of the `.desktop` file when
  M3.9 packages the app.
- A window remembers its size and position per document (in the user's
  local state, not in the file).

## 3. The board window

```
┌──────────────────────────────────────────────────────────────┐
│ [≡ Launch plan ▾ │ ✓ Saved]          [Search Ctrl K][✦ AI][Share] │
│ ┌──┐                                                         │
│ │⠿ │      ┌─────────────────────────────┐                    │
│ │▸ │      │ contextual toolbar          │                    │
│ │✋│      └─────────────────────────────┘   ┌──────────────┐ │
│ │──│        ┌───────────┐                   │ inspector    │ │
│ │□ │        │ selection │                   │ (Ctrl+I)     │ │
│ │○ │        └───────────┘                   │              │ │
│ │… │                                        │              │ │
│ └──┘                                        └──────────────┘ │
│ [− 100 % + │ ↶ ↷]                                        [?] │
└──────────────────────────────────────────────────────────────┘
```

### 3.1 The document pill (top left)

The app button (the logo: main menu), the board's name (click to rename or
move it), and the save state:

| State | Shown | When |
|---|---|---|
| Saved | ✓ Saved | Every change is in the file |
| Saving | Saving… | An update is being written (shown only after 500 ms, so quick saves don't flicker) |
| Not saved | `Not saved` badge and a **Save…** button | A new, untitled board: its changes are kept for crash recovery, but it has no place yet |
| Can't save | ⚠ Can't save · retrying | The write failed (disk full, the file went away). Keeps retrying; the menu offers Save a copy |
| Read-only | 🔒 Read-only · made with a newer version | `Mode::ReadOnly` (newer `required_features`) |

### 3.2 Top right

Search (opens the palette, `Ctrl+K`), AI (the assistant, a placeholder
until M6) and Share (a placeholder until M8). Placeholders are visible but
say what's coming when clicked; they don't pretend to work.

### 3.3 The tool bar

- **Its place is the user's** (owner, 2026-10-16): docked to the **left**
  (the default), top, right or bottom edge, or floating. It's dragged by
  its grip (⠿) and snaps to an edge when released near one; Shift while
  dropping keeps it floating; a double-click on the grip puts it back.
  Setting `ui.toolbar.position` (`left`, `top`, `right`, `bottom`,
  `floating`), also in Settings › Appearance.
- Docked left or right it's a column, top or bottom a row; it's centered
  along its edge.
- **Tools**, in groups:
  1. Select `V`, Hand `H`;
  2. Rectangle `R`, Ellipse `O`, Diamond `D`, Triangle `Shift+T`,
     Line `L`, Draw `P` (ink, M4), Text `T`, Frame `F`;
  3. Document `Shift+D` (M7), Sheet `Shift+S` (later), Insert `/`
     (image, sticky note, embed and more, as they come).

  A tool appears once its milestone builds it; M3 shows groups 1 and 2
  without Draw, and Insert with what exists.
- Hovering a tool shows its name and key after 500 ms. A tool stays
  active for one shape, then Select returns; double-clicking a tool keeps
  it (as in Excalidraw).
- Keyboard: `F6` moves the focus between the board and the panels; in the
  tool bar the arrow keys move between tools (`role="toolbar"`).

### 3.4 Bottom

Zoom (−, the percentage, which opens Fit / 100 % / zoom to selection, +),
undo and redo on the left; Help and shortcuts (`?`) on the right. If the
tool bar is docked at the bottom, they keep their corners and the bar sits
between them.

**As built (M3.4, the board window).** `imaginarium-shell`'s
`BoardWindow` is the window's view; `imaginarium-app` opens one on a new
board (`--demo` shows the mockups' board).
- **Layout.** The board fills the window. The panels sit in an overlay
  that fills it too, made of boxes that let the pointer through
  (`Flex::pass_through`), so the board gets every click between the
  panels. The tool bar has a stage of its own laid out for its dock.
- **Commands.** Every button runs a board command by id (`tool.*`,
  `view.zoom_*`, `edit.undo`, `edit.redo`) through a queue the window
  drains after each input, so the palette and the AI can do the same.
  Tooltips take their keys from the board's keymap (Ctrl or ⌘), so they
  always show the real shortcut. The selected tool, the zoom percentage
  and undo and redo follow the board's signals (`tool()`, `status()`).
- **Focus.** Tool bar and panel buttons take a click without taking the
  keyboard (`Widget::focus_on_click`): after a click on a tool, the
  board's keys still work. Tab reaches them; keys a focused button
  doesn't use go to the board (its shortcuts work), and Esc gives the
  keys back to it. A click on a panel's background does too. Esc closes
  a note even while the board has a selection.
- **Placeholders.** The app button, the name, Save…, Search, AI, Share
  and help open a note saying which milestone brings them (M3.6, the
  palette next, M6, M8, M3.9).
- **The tool bar.** Docked left by default (`ui.toolbar.position`). The
  grip drags it: the edges show their drop zones, the nearest stronger,
  with a hint ("Release to dock it on the left · Shift keeps it
  floating"). Released with the pointer in the outer quarter of the
  window toward an edge, it docks there; elsewhere, or with Shift, it
  floats, kept inside the window (also when the window shrinks). A
  press drags only after 4 px, so a click leaves the bar alone; a
  double-click on the grip puts it back on the left; a drag cut short
  (the window loses the focus) puts it back where it was. Commands do
  the same (`view.dock_toolbar` with an `edge`, `view.reset_toolbar`),
  and the window follows the setting when it changes elsewhere. Docked,
  the bar is centered on its edge when that keeps it clear of the
  corner panels, else centered between them, else placed past them (a
  narrow or short window), its grip in reach. The setting isn't saved
  to disk yet: settings files come with M3.5.
- **Zoom.** The percentage opens a menu: fit, 100 % and the selection,
  with their keys.
- **Not yet:** the arrow keys between tools and `F6` (the focus between
  the board and the panels), double-clicking a tool to keep it, and the
  title following a rename (M3.6).

### 3.5 The contextual toolbar

Floats above the selection (below it when there's no room), and moves
with it. It's **built from the commands available for the selection**
(PLAN §2), in three groups:

1. style: fill, stroke color, stroke width, corner radius (for shapes
   that have one), text style (for text);
2. arrange (front, back, align), duplicate, delete;
3. AI actions (M6) and More (every other command for the selection, as in
   the context menu).

Each control opens a popover: the fill popover has the board's colors (no
fill, white, pastels, strong colors), a hex field and opacity. During a
drag the toolbar hides and comes back when the gesture ends.

**As built (M3.4, the contextual toolbar).** Built per frame from the
selection's state (which commands apply, the first node's look); rebuilt
only when that changes.
- **Place.** Centered on the selection's box, 32 px above it (clear of
  the turn handle), or 16 px below when the top panels would cover it;
  kept inside the window. Hidden while the board has a gesture (moving,
  resizing, drawing, panning).
- **Controls**, each shown only when its command applies: fill, outline,
  outline width, corner radius (rectangles), text color (text boxes,
  instead of fill); arrange (a menu), duplicate, delete; AI (a note,
  M6) and More (the context menu's commands).
- **Commands** behind them, new in the canvas: `style.set_fill`,
  `set_stroke`, `set_stroke_width`, `set_radius`, `set_text_color`, one
  undo step over the selected nodes that take them, hidden when none
  does.
- **Popovers.** Colors: the board's palette (in the theme's version),
  no color, a hex field and the opacity (`style.set_fill_opacity`,
  `set_stroke_opacity`: each node keeps its own color). A pick closes
  the popover and keeps the node's opacity. Widths and radii: short
  menus with the current one checked.
- **Cost.** Its state is computed only when the document, the
  selection, the theme or a gesture changed (`BoardCanvas::revision`);
  each control shows the first node it applies to. A frame with 10,000
  nodes selected takes 2.8 ms (the board's part 1.4 ms, benchmarked).
- **Keyboard.** F6 and Shift+F6 move the keys between the board, the
  contextual toolbar, the tool bar and the top panels; Shift+F10 or the
  Menu key opens the context menu at the selection.

### 3.6 The inspector (`Ctrl+I`)

A panel on the right, 300 px wide, for everything the toolbar is too small
for, in sections: name and kind; the shape (type, corner radius with
per-corner values, **Convert to path** `Ctrl+Shift+C`); position, size,
rotation, keep proportions; fill; stroke; layer (opacity, lock, hide);
style (the named style, which values are changed here, Reset to style).
Every field is a command with arguments, so the palette and the AI can
set the same values.

**As built (M3.4, the inspector).** `Ctrl+I` (`view.inspector`) or its
close button; 300 px wide, 72 from the top, 16 from the right and
bottom, scrolling when the window is short.
- **Sections**, each when its commands apply: the kind and the name
  (`board.rename`); Shape: type (`shape.set_preset`) and corner radius
  (a slider and a field, `style.set_radius`); Position and size
  (`board.set_box` in the parent's space, Keep proportions on by
  default); Fill, Stroke, Text (text boxes): a swatch opening the
  board's colors, the hex, opacity, width or size; Layer: opacity
  (`style.set_opacity`), Lock or Unlock, Hide (`board.set_hidden`);
  Style: the named style (`style.apply`) and Reset to style
  (`style.reset`).
- **Updates.** Its snapshot is taken only when the document, the
  selection or the theme changed (not during a gesture), and the panel
  is rebuilt only when what it shows changed: typing in a text box
  doesn't rebuild it. Any control that had the keys keeps them.
- **Fields.** Each section reads the first node it applies to, as the
  board draws it (frames with their frame colors), and locks count
  when inherited. Enter on an unchanged field does nothing; text that
  isn't a value, or one a command refuses, shows the real value again;
  `%`, `pt` and `°` are taken; a typed hex keeps the color's opacity.
- **Layer.** Opacity for shapes and text; Lock or Unlock; Hide, undone
  with Show hidden (`board.show_hidden`, in the board's menu and the
  palette). The panel ends above the bottom row and a right-docked tool
  bar moves left of it.
- **Not yet:** per-corner radii and Convert to path (paths come with
  the vector work); a mixed multi-selection shows the first node's
  values.

### 3.7 The command palette (`Ctrl+K`)

Centered near the top, 640 px wide. One input; results from commands,
settings and (later) templates and AI actions, fuzzy-matched with
synonyms and recency (M0's search). A result shows its shortcut; a
command with arguments takes them inline (`Set corner radius` › `12 px`,
`Tab` to the next argument); a boolean setting is a toggle right in the
list. The footer lists the keys (↑↓ choose, ↵ run, Tab argument, Esc
close).

**As built (M3.4, the palette).** `Ctrl+K` (a shell keymap with the
command `palette.open`, so it works wherever the keys are) or the
Search button opens it as a lightly dimmed dialog.
- **What it finds.** The board's and the shell's commands that run
  without arguments, and the boolean and enum settings, in one index
  (`SearchIndex`); `SearchIndex::explain` gives the matched letters,
  or the synonym, for the rows shown ("G**rou**p", "· also **rou**nd
  corners"). Results sit under their categories; without a query, the
  recent commands come first, then everything.
- **Availability** is a snapshot taken when it opens. Hidden commands
  aren't listed; a disabled one is greyed and says why ("Nothing to
  redo"), and Enter does nothing.
- **Keys.** ↑↓ move (the row scrolls into view), Enter runs a command
  and closes the palette, or changes a setting in place; Tab changes a
  setting's value (an enum steps to its next value); Esc or a click
  beside it closes it, and the keys go back to the board.
- **Cost.** Rows are built once per open and reused: a keystroke
  reorders them and updates the changed highlights (2.2 ms with its
  frame in release, 44 entries). Their signals live in a scope disposed
  when the palette closes. It follows the window's size.
- **Not yet:** arguments typed inline (`Set corner radius` › `12 px`):
  commands with required arguments aren't listed. Templates, help and
  AI actions come with their milestones.

### 3.8 The context menu

Right-click on a selection or the board. The same commands as the
toolbar's More, in groups (clipboard; order and group; Convert to path,
copy style and paste style; lock and delete), with shortcuts.

**As built (M3.4, the context menu).** A right-click on a node selects
it (as a click would) and lists the selection's commands: clipboard;
order and grouping; edit text and lock; Delete in the danger color. On
the board: paste, select all, zoom, undo. Rows that don't apply are
left out, those that can't run are greyed and skipped; ↑↓ move, Enter
runs. A right-click inside a selection's box keeps the selection; the
board's menu offers Unlock (locked nodes don't take clicks). Only the
right button opens it. The same menu serves the toolbar's More and
Arrange.

### 3.9 Dialogs

Modal, centered, over a dimmed window. The one M3 needs: **closing a board
with unsaved changes** works as in any other program (owner, 2026-10-16):

> Save changes to "Untitled board" before closing?
> If you don't save, your changes are lost. Once it's saved, every
> change is saved on its own.
> [Don't save] … [Cancel] [Save…]

Since every change to a saved board is in its file, only untitled boards
(and boards whose save failed) ask.

## 4. The start window

960 × 600, opened at startup (setting `startup.show_start_window`) and
when the last board closes.

- **Left:** the logo and version; **New board** (`Ctrl+N`) and **Open…**
  (`Ctrl+O`); Show at startup; links to Settings, Shortcuts and What's new.
  The "Start from" templates come with M4's shape libraries.
- **Right:** a crash-recovery notice when the app closed unexpectedly
  ("*Roadmap 2027* was kept, with every change. [Open it]"), then
  **Recent** as a grid of thumbnails (name, folder, when) with a filter,
  "Open another board…", and "Drop an .imag file here to open it."

**As built (M3.4, the start window and windows).** `StartWindow` in
`imaginarium-shell`, opened by `App::first_window` when
`startup.show_start_window` is on (the default; `--new` skips it):
- **Left:** the logo (`imaginarium_ui::Logo`, in the gallery), the
  name and version, **New board** (`Ctrl+N`, `file.new_board`), and
  **Show at startup** as a switch rather than the mockup's checkbox (the
  toolkit's control for a setting). **Open…** is disabled, with a tooltip,
  until files come (M3.6). The links to Settings, Shortcuts and What's new
  come with the windows they open; the templates with M4.
- **Right:** **Recent** with an empty tile saying boards will show there.
  The recovery notice, the filter, the thumbnails and dropping files come
  with files (M3.6).
- **Fixed size** (960 × 600, `WindowOptions::resizable = false`), so
  Hyprland and Sway float it like a dialog. Nothing has the keys at
  first (no focus ring, as in the mockup); `Ctrl+N` works anywhere in it
  and Tab reaches New board first.
- **One window per document:** `imaginarium-ui` runs several windows in
  one event loop. Views ask for windows (`View::take_requests`) and hear
  the close button (`View::close_requested`); the run ends with the last
  window. The windows share one GPU device (`SharedGpu`), the app's
  fonts (`Fonts::share`, one scan of the system's fonts), settings and
  tile workers (`App`). Once a window that opens others
  (`WindowOptions::more_windows`: the start window and boards) has drawn,
  the next window's Vello renderer is built on a thread, so that window
  opens without compiling pipelines first. Programs of one window (the
  gate, the examples) don't build one.
- **Per window:** a frame asked for by a view (`request_frame`) draws
  only its window; a signal change draws the window whose widget tree
  watches it, wherever the change came from; the scale factor, input
  method resets and pasted text are the window's own (text asked for by
  a window that closed meanwhile is dropped). A theme switch, or a
  setting another window follows (the tool bar's position), draws every
  window (`request_frame_everywhere`). A window that
  fails (no surface, no GPU) closes alone, listed in
  `RunStats::window_errors` (the app prints them); the run fails only if
  it was the last.
- **Held keys:** `Ctrl+N` and `Ctrl+W` act once while held, and the
  start window opens one board however often New board is pressed.
- **Closing:** `Ctrl+W` (`window.close`) or the close button. A board
  stops counting as open as soon as it starts closing, so boards closed
  together still bring the start window back once. Closing the start
  window ends the app. Unsaved changes can't be asked about until M3.6.
- **Settings are saved** (M3.5): Show at startup, like every app
  setting, is kept in the settings file (§5 "As built").
- **Timings** (release, llvmpipe on the 8-vCPU VM, 2026-10-17): the first
  frame of the start window 240–255 ms after `main` starts (about 55 ms
  of setup, mostly the font scan, while the GPU opens on its thread; then
  about 150 ms waiting for the GPU), under PLAN §10's 400 ms. A new board
  window draws its first frame 75 ms after `Ctrl+N` in the start window
  and 137 ms after it in a board (its renderer is ready: 2 ms instead of
  the 130 ms its pipelines take to compile). Two idle board windows use
  no CPU.

## 5. Settings

A window of its own (`Ctrl+,`), generated from the settings registry
(ADR-0014): sections in a sidebar (General, Appearance, Board, Files and
saving, Keyboard shortcuts, Advanced) with a search across all of them;
scope tabs **For me** and **This board** where a setting has both; a
changed value shows "Changed" and a reset button. M3's new settings:

| Key | Values | Scope |
|---|---|---|
| `ui.theme` | `system`, `light`, `dark`, `high_contrast` | user |
| `ui.accent` | a color (the logo's indigo, teal, orange, violet, …) | user |
| `ui.scale` | interface size, 80 to 200 % | user |
| `ui.reduce_motion` | `system`, `on`, `off` | user |
| `ui.toolbar.position` | §3.3 | user |
| `board.background` | `dots`, `grid`, `plain` | user and board |
| `startup.show_start_window` | bool | user |

**As built (M3.5, the settings file).** The app reads `settings.toml`
from the platform's configuration folder at startup
(`$XDG_CONFIG_HOME/imaginarium/` or `~/.config/imaginarium/` on Linux,
`~/Library/Application Support/Imaginarium/` on macOS,
`%APPDATA%\Imaginarium\` on Windows; `--settings PATH` for another file).
After every change of an app setting the registry hands its TOML to a
writer thread (`Registry::persist_to`, `FileWriter`), which writes the
newest text atomically (a temporary file of its own per write, removed
if the write fails; a symbolic link stays a link) and finishes before
the app exits; the UI never waits for the disk. Entries this version
can't read are kept (`persist.rs`). A file that can't be read at all
(broken TOML, not UTF-8) is moved aside to
`settings.toml.unreadable-<seconds>` and the defaults are used; if it
can't be moved, nothing is saved that session, so the user's file is
never overwritten (`Registry::open_file`). For now these notices go to
standard error, and the Settings window shows them. Two instances of the
app write the same file: the last save wins.

**As built (M3.5, the Settings window).** `SettingsWindow` in
`imaginarium-shell`, 1120 × 720, opened with `Ctrl+,` (`settings.open`)
from a board or the start window (whose Settings and Shortcuts links
open it), one at a time:
- **Sidebar:** a search field, then the sections. A setting's section
  follows its group (`.group("appearance")`, else its key's first
  segment: `ui` → Appearance, `board`/`canvas` → Board, `files` → Files
  and saving, others → General); advanced settings go to Advanced.
- **Rows** (generated from the registry): title, a "Changed" badge and a
  reset button when the value isn't the default, the description, and an
  editor chosen by the value's kind: a switch (bool), a segmented control
  (an enum of up to three values), a dropdown (more values, or a numeric
  range of at most 16 steps), a text field (numbers, text, paths, lists
  as comma-separated items; it keeps its value on Enter or when the focus
  leaves, and says why a value was refused), a swatch and hex field
  (colors), and the keychain entry's name (secrets, edited with M6).
  Changes apply at once and are saved (above); a change from elsewhere
  (the palette, another window) shows at once.
- **Search:** every word in the title, description, synonyms or key;
  the results are grouped by section, shortcuts included.
- **Keyboard shortcuts** lists the board's and the windows' commands with
  their keys; changing them comes later.
- A notice above the settings when saving fails or the file was
  unreadable.
- **Not yet:** the scope tabs For me and This board (part 4), the
  mockup's theme cards and accent swatches (part 3, with the settings
  they edit), ↑↓ in a dropdown's list (Tab moves through it), and
  bringing an open Settings window to the front (`Ctrl+,` does nothing
  while it is open).

## 6. Layers, popovers and focus

The window paints, back to front: the **board**, the **chrome** (pill,
tool bar, zoom, inspector), **popovers** (toolbar popovers, menus,
tooltips, the palette) and **modals** (dialogs). M3.1 adds the layers to
the toolkit:

- a popover belongs to the widget that opened it; a click outside it or
  `Esc` closes the topmost one, and the focus returns to its opener;
- a modal takes all input until it closes;
- tooltips never take input;
- `F6` cycles board → tool bar → contextual toolbar → inspector, and
  `Esc` on a panel returns to the board.

## 7. Themes as tokens

Colors, radii, shadows and type sizes are **tokens**, and widgets read
tokens, never literal colors. Tokens are signals (M3.1), so switching the
theme repaints live. The dark values, from the mockups:

| Token | Dark | Light (later) |
|---|---|---|
| `board.bg` / `board.dot` | `#141416` / `#2A2A31` | `#FCFCFD` / `#DEDEE3` |
| `panel.bg` / `panel.border` | `#1E1E22` / `#2E2E35` | `#FFFFFF` / `#E7E7EA` |
| `control.border` / `divider` | `#34343B` / `#28282E` | `#E4E4E7` / `#EFEFF2` |
| `control.hover` / `control.active` | `#2A2A30` / accent at 28 % | `#F1F1F4` / accent at 14 % |
| `text` / `text.muted` / `text.subtle` | `#EDEDF0` / `#A1A1AA` / `#8E8E99` | `#18181B` / `#52525B` / `#6B6B76` |
| `accent` (fills: primary buttons, the selection) | the accent | the accent |
| `accent.ink` (accent text and icons on panels) | the accent, 45 % toward white | the accent, 15 % toward black |
| `on.accent` (text on an accent fill) | `#FFFFFF` | `#FFFFFF` |
| `danger` / `warning` | `#F87171` / `#F5A524` | `#B91C1C` / `#B45309` |
| `shadow.panel` | `0 1 2 / 40 %`, `0 8 28 / 45 %` black | `0 1 2 / 6 %`, `0 8 24 / 8 %` |
| `tooltip.bg` | `#3F3F46` | `#18181B` |

Shape colors on the board are document data (styles), not theme tokens.
The default palette has a light and a dark version of each color, and the
dark theme shows a shape's dark version when it uses a palette color
(owner, 2026-10-16). A color outside the palette shows as stored, and text
without a color of its own follows the theme's text color. The file
always stores the palette color itself.

Text meets 4.5:1 on its background (3:1 for 24 px and up) in every theme;
a test checks the token pairs.

## 8. Icons

**Lucide** (owner, 2026-10-16; ISC license, and MIT for the icons that
come from Feather, both allowed in `deny.toml`). We vendor the icons we
use, not the whole set:

- `cargo xtask icons` reads a pinned Lucide release (`lucide-static` from
  npm, 1.50.0; it refuses one younger than 14 days and a tarball that
  doesn't match npm's sha512) and writes the icons
  listed in `crates/imaginarium-ui/src/icons/icons.txt` as SVG path data
  into `lucide.rs` there (circles, rectangles, lines and polygons
  converted to paths), with Lucide's license next to them
  (`LICENSE-lucide`, both notices). The generated file's SPDX line is
  `ISC AND MIT`. 98 icons as of M3.1.
- At startup the path data is parsed once (`kurbo::BezPath::from_svg`) and
  stroked at 1.75 px in the token's color, so icons follow the theme and
  stay sharp at every scale. No new dependency.
- An icon is named by its Lucide name (`mouse-pointer-2`, `hand`, …), and
  the list is in one place, so unused icons are easy to drop.

## 9. Later views (placed now, built later)

The mockups place these; the shell reserves the pieces they need:

- **Documents** (M7): a document is an object on the board, edited in
  place; Enter opens the **document view** over the dimmed board (back
  with `Esc`): a formatting toolbar at the top, an outline on the left,
  suggestions and comments on the right.
- **Sheets** (later): the same pattern, with a formula bar, a grid and
  sheet tabs.
- **Presentations** (M9): a presentation panel on the right lists the
  steps and starts presenting; animation adds a timeline at the bottom;
  presenting is a full-screen window per display, with a presenter view.
- **Draw** (ink in M4, pixel brushes in M11): a draw panel next to the
  tool bar, and a small bar at the top with the brush, size, opacity,
  color and pressure.

So M3's panels are general: a side panel (left or right, the inspector is
the first), a top bar, a bottom bar (the timeline later), and overlays.

## 10. Dependencies to research (M3.6)

- **File dialogs:** native open and save dialogs, through the desktop
  portal on Linux (so they work on Hyprland, GNOME and KDE alike, and in a
  Flatpak). Candidate: `rfd` 0.17.2 (2026-01-12, MIT) with its portal
  backend, which uses `ashpd` 0.13 (MIT) over D-Bus. Both pass the
  two-week rule today; M3.6 checks the whole tree they pull in (an async
  runtime for `ashpd`, `zbus`) with `cargo deny` and `dep-age`, and runs
  the dialogs on a worker so the UI never waits.
- **The system theme:** whether the desktop prefers dark, for
  `ui.theme = system`. winit reports it on Windows and macOS; on Linux it
  comes from the portal's `color-scheme` setting, which `ashpd` also
  reads.

## 11. Open questions

None at the moment.
