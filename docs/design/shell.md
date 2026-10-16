# Shell: the window around the board

Status: **Draft** (M3.0). Crates: `imaginarium-shell` (new in M3.4) and
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

### 3.6 The inspector (`Ctrl+I`)

A panel on the right, 300 px wide, for everything the toolbar is too small
for, in sections: name and kind; the shape (type, corner radius with
per-corner values, **Convert to path** `Ctrl+Shift+C`); position, size,
rotation, keep proportions; fill; stroke; layer (opacity, lock, hide);
style (the named style, which values are changed here, Reset to style).
Every field is a command with arguments, so the palette and the AI can
set the same values.

### 3.7 The command palette (`Ctrl+K`)

Centered near the top, 640 px wide. One input; results from commands,
settings and (later) templates and AI actions, fuzzy-matched with
synonyms and recency (M0's search). A result shows its shortcut; a
command with arguments takes them inline (`Set corner radius` › `12 px`,
`Tab` to the next argument); a boolean setting is a toggle right in the
list. The footer lists the keys (↑↓ choose, ↵ run, Tab argument, Esc
close).

### 3.8 The context menu

Right-click on a selection or the board. The same commands as the
toolbar's More, in groups (clipboard; order and group; Convert to path,
copy style and paste style; lock and delete), with shortcuts.

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
| `accent` | the accent, 20 % lighter | the accent |
| `accent.text` (on accent) | `#FFFFFF` | `#FFFFFF` |
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

**Lucide** (owner, 2026-10-16; ISC license, allowed in `deny.toml`). We
vendor the icons we use, not the whole set:

- `cargo xtask icons` reads a pinned Lucide release (two-week rule) and
  writes the chosen icons as SVG path data into
  `crates/imaginarium-ui/src/icons/` (circles and rectangles converted to
  paths), with Lucide's license next to them.
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
