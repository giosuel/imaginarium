# M2 gate: running it, the checklist, results

Status: **Decided** (2026-10-13): we keep our toolkit (see "Decision"
at the end). The procedure and the binary stay in use as a benchmark.

The gate (ADR-0007) decides whether our UI toolkit stays or GPUI replaces
it. It is measured by `imaginarium-gate`, a prebuilt binary the owner runs
on the gate machine, plus a manual checklist for what a program can't
judge (IME, screen reader, how it feels). This page is the checklist and,
after M2.7, the results.

**Gate machine:** the owner's laptop. EndeavourOS (Arch), kernel 7.2,
Hyprland (Wayland), Intel Core i9-11900H with Intel UHD (integrated) and a
GeForce RTX 3050 Ti Laptop GPU (NVIDIA 615.71), 32 GB RAM.

## What the binary covers so far

| Criterion (ADR-0007) | How | Since |
|---|---|---|
| 1. 10,000 shapes pan and zoom at ≥ 60 fps, p99 < 16 ms | Shapes board (Ctrl+2) through the tile cache: `shapes.criterion_1_pass`, intervals | M2.5 |
| 2. Dragging re-renders only dirty tiles | Scripted drag: `shapes.criterion_2_pass` (≤ 4 tiles per frame); Tile updates (Ctrl+T) shows them | M2.5 |
| 3. Palette: 2,000 commands, < 1 ms per keystroke | Command palette (Ctrl+K) over 2,000 commands: `palette.pass` | M2.4 |
| 4. Text field with IME (CJK) and a screen reader | Search field and card editing; checklist below | M2.3 |
| 5. HiDPI and mixed-DPI monitors | Checklist below | M2.2 |
| 6. Idle CPU ≈ 0 %, cold start < 400 ms | `--measure`: `idle.pass`, `cold_start.pass` | M2.2 |

## Setup (once)

```sh
chmod +x imaginarium-gate
# Fonts for all the sample scripts on the cards (CJK, Hindi, emoji, …):
sudo pacman -S --needed noto-fonts noto-fonts-cjk noto-fonts-emoji
# A Japanese input method (or fcitx5-chinese-addons, fcitx5-hangul):
sudo pacman -S --needed fcitx5 fcitx5-mozc fcitx5-configtool
# The screen reader:
sudo pacman -S --needed orca
# The integrated GPU's Vulkan driver (without it, --gpu low gets the NVIDIA):
sudo pacman -S --needed vulkan-intel
vulkaninfo --summary   # should list both GPUs (package vulkan-tools)
```

For fcitx5 on Hyprland, add `exec-once = fcitx5 -d` to the Hyprland config
and set `XMODIFIERS=@im=fcitx` (for example with `env = XMODIFIERS,@im=fcitx`).
The gate uses Wayland's text-input protocol when it runs as a Wayland
client, which fcitx5 supports on Hyprland. Add Mozc in
`fcitx5-configtool`. Ctrl+Space switches the input method.

## Measurements

Run each and keep the reports. Don't use the window while `--measure`
runs (about a minute). Scripted phases ignore input, but the window
should stay visible and unobstructed. `--fullscreen` makes every run
measure the same window size.

```sh
./imaginarium-gate --measure --fullscreen --report gate-report-igpu.json                 # integrated GPU (default)
./imaginarium-gate --measure --fullscreen --gpu high --report gate-report-dgpu.json      # the RTX 3050 Ti
./imaginarium-gate --measure --fullscreen --no-glyph-atlas --report gate-report-noatlas.json
./imaginarium-gate --measure --fullscreen --no-vsync --report gate-report-novsync.json   # how fast frames could be
./imaginarium-gate --measure --fullscreen --no-tiles --report gate-report-notiles.json   # without the tile cache, to compare
```

The binary prints its build first (e.g. `imaginarium-gate M2.7 fix 3`);
the report has it as `build`.

The scripted run pans the cards, types into the search field, types into
the command palette, pans, zooms and drags on the 10,000-shape board, and
idles (about a minute). The report has, per phase, the toolkit's
share of each frame (`paint`, and the widget tree's `layout`)
apart from the renderer's (`encode`, `submit`), the wait for the
swapchain (`acquire`, which is vertical sync when everything keeps up)
and frame intervals. It also has typing (`typing.handle`: one keystroke
including searching all cards), idle CPU and frames (`idle.pass`), cold
start (`cold_start.pass`), the GPU, the machine and peak memory.

How criterion 6 is judged:
- **Cold start** without the GPU's setup (owner decision, ADR-0007
  amendment): `cold_start.without_gpu_setup_ms` is the time to the first
  frame minus what the window waited for the GPU and spent on it. The GPU
  opens on a thread of its own from the start of `main` (`cold_start.gpu`
  has its steps); the total is `main_to_first_frame_ms`.
- **Idle** counts the frames the program draws on its own (a timer, a
  request: `idle.frames_by_itself`), not the ones the window system or
  the user ask for (`idle.frame_causes` has them all), and the CPU must
  stay under 1 %. The scripted run waits until no tiles are owed before
  it starts idling.

## Manual checklist

Start it with `./imaginarium-gate` (no arguments). Note for each item:
works, odd (what), or broken.

**Board and rendering**
1. Wheel, touchpad scrolling and dragging with the left button pan the
   board. Does it follow the fingers without lag or stutter?
2. Ctrl + wheel zooms about the pointer, down to all 2,000 cards and up
   to 800 %. Is text sharp at every zoom? Any flicker or jumping glyphs?
3. Hovering outlines the card under the pointer.
4. The status line shows the last frame's paint and encode time and the
   GPU's name. Note the typical numbers while panning.

**Shapes and tiles (criteria 1 and 2)**
5a. Ctrl+2 (or "Shapes" in the toolbar) shows 10,000 shapes. Pan and
    zoom from 800 % down to all of them: smooth? Any tiles that show up
    late or blurry, or seams between tiles?
5b. Drag shapes around. Turn on "Tile updates" (Ctrl+T): tiles flash red
    when they re-render. A drag flashes the tiles under the shapes when it
    starts and ends, none while they move (they are drawn live on top);
    panning flashes only the tiles coming into view.
5b2. Editing: click selects, Shift+click adds, Shift+drag on empty board
    draws a selection rectangle, Ctrl+A selects all; drag moves the
    selection, arrow keys nudge (Shift: 10), Delete removes, Ctrl+Z /
    Ctrl+Shift+Z undo and redo.
5c. Ctrl+1 goes back to the cards (their tiles are kept).

**HiDPI (criterion 5)**
5. With the laptop panel at a fractional scale (Hyprland `monitor=…,1.25`
   or `1.5`): are text and lines crisp, not blurry, and the window the
   expected size?
6. With an external monitor at another scale, move the window across:
   it redraws at the new scale without a size jump or blur.

**Text (criterion 4)**
7. Ctrl+F (or a click) focuses the search field. Typing filters the cards
   ("530 matching" for "review"). Backspace, Ctrl+Backspace, ←/→,
   Ctrl+←/→, Shift+arrows, Home/End, Ctrl+A behave as in other apps.
8. Double-click a word in the field selects it; triple-click selects all;
   dragging selects. Esc leaves the field.
9. Double-click a card: its text becomes editable in place (blue outline).
   Click to place the caret, type, Enter for a new line, ↑/↓ between
   lines. Zoom while editing. Esc or a click elsewhere ends editing.
10. The caret blinks for about 10 seconds after the last key, then stays
    on (so an idle focused field doesn't keep redrawing).

**Input method (criterion 4)**
11. In the search field, switch to Mozc and type `nihongo`: the
    composition shows underlined in the field, the candidate window
    appears next to the caret, Space cycles candidates, Enter commits
    日本語.
12. The same while editing a card, also when zoomed in.
13. Escape or switching the input method off during a composition leaves
    no stray text.

**Screen reader (criterion 4)**
14. Start Orca (`orca`), then the gate. Orca announces the window
    ("Imaginarium gate").
15. Click into the search field: Orca says something like "Search cards,
    entry". Typing echoes characters; ←/→ read the character at the
    caret; Shift+arrows announce the selection.
16. Orca's flat review (Insert/KP keys) or the object navigation reads the
    visible cards' titles and text.

**Widgets and the command palette (criterion 3)**
17. Ctrl+K (or the Commands button) opens the palette. Type `ins tab`:
    "Insert table" is first; the footer shows the search time. ↑/↓ choose,
    Enter runs (the status line says "ran: …"), Esc closes, a click on a
    row runs it.
18. Tab and Shift+Tab move the focus between the Commands button, the
    search field and the board (focus rings). Enter or Space on the button
    opens the palette.
19. Copy and paste (Ctrl+C, Ctrl+X, Ctrl+V) in the search field and while
    editing a card, also to and from other apps.
20. With Orca: the palette is announced as a dialog, the results as a list
    with options.

**Idle and start (criterion 6)**
21. Leave the window alone for a minute: `top -p "$(pidof imaginarium-gate)"`
    shows about 0 % CPU.
22. Does it open quickly? (The report has the number.)

**Known gaps (not bugs yet):** no pinch zoom, no context menus, no
scrolling lists (the palette shows the best 10), and tiles render on the
UI thread (workers come later).

## What to send back

The five `gate-report-*.json` files, the checklist notes, and a screenshot
of anything that looks wrong. A manual session writes `gate-report.json`
too (its frames are in `manual`), worth sending after anything odd.

## Results

The decision is at the end. Criteria are judged on the integrated GPU
(ADR-0007).

### First run (2026-10-13, manual session, M2.5 binary)

- **Crash:** "rendering failed: Missing texture binding for
  TextureId(…)". Causes and fixes in `rendering.md` §10 (texture rules):
  both boards' canvases used the same texture keys, a stand-in could be
  evicted in the frame it was drawn, and a skipped frame dropped tile
  renders. The same key clash made the boards show each other's tiles
  after switching (reproduced: the cards board showing shapes), a likely
  cause of the "glitches while dragging". Also fixed: frames in which the
  board wasn't repainted rendered its last tiles again.
- **Not always 60 fps with 10,000 shapes** (RTX 3050 Ti): no frame data in
  a manual report of the M2.5 binary; the fixed binary records manual
  sessions too. The `--measure` runs decide criterion 1.
- **Cold start 746 ms** (fails < 400 ms): fonts 114 ms (2,139 families),
  first paint 218 ms; the remaining ~400 ms is window and GPU setup, now
  reported apart (`window_ms`, `gpu_ms`).
- `--gpu low` got the NVIDIA GPU: the Intel Vulkan driver is probably
  missing (setup above).

### Second run (2026-10-13, `--measure`, fix-1 binary)

- **All five runs panicked** in vello_gpu: "pixmap image sources are not
  supported by Vello GPU". Color emoji are bitmap glyphs, which vello_gpu
  draws only from its glyph atlas; the first frame is drawn without it,
  and a maximized window shows the emoji card in that frame. Fixed (fix 2,
  `rendering.md` §10, "Color emoji on the GPU"); reproduced and checked on
  the VM with Noto Color Emoji installed.

### Third run (2026-10-13, `--measure`, fix-2 binary): the five reports

Window 566×674 for the integrated GPU, RTX 3050 Ti and no-tiles runs,
1132×674 for the no-atlas and no-vsync ones (Hyprland tiled them); scale
1.0; 2,139 font families. Times in ms.

| | iGPU | dGPU | no atlas | no vsync | no tiles |
|---|---|---|---|---|---|
| C1 shapes pan, p99 (< 16) | **10.4** | 9.0 | 22.2 | 12.4 | 8.8 |
| C1 shapes zoom, p99 (< 16) | **26.4** ✗ | 23.9 | 35.3 | 35.0 | 25.6 |
| C2 tiles per drag frame (≤ 4) | **4** | 4 | 2 | 2 | – |
| C3 palette search, p95 (< 1) | **0.35** | 0.43 | 0.37 | 0.34 | 0.36 |
| C6 cold start (< 400) | **1,162** ✗ | 824 | 735 | 809 | 848 |
| C6 idle frames / CPU | **1 / 0.1 %** ✗ | 1 / 0.4 % | 2 / 0.4 % | 2 / 0.4 % | 4 / 0.5 %¹ |

¹ With 54 inputs: something moved over the window.

- **Criterion 1:** median frames take 3.5–4 ms with tiles (6.7 ms
  without). The zoom phase's 4–6 slow frames out of 240 are level
  switches rendering up to 24 tiles at once (up to 28 ms of tile work),
  also the cascade the owner saw. Without vertical sync the shapes pan at
  about 240 frames per second.
- **Criterion 2 and 3** pass in every run. The glyph atlas halves the
  encoding of the cards (1.7 vs 3.4 ms per pan frame): it stays on.
- **Criterion 6, cold start:** the GPU 296–449 ms (727 ms on the first run,
  the driver's shader cache cold), the first paint 232–302 ms, fonts
  116–130 ms, one after the other.
- **Criterion 6, idle:** 1–2 frames in 10 s without any input.
- **Also found:** in every run one frame of the cards pan painted for
  0.8–1.1 s (the UI thread frozen), and the first paint took ten times as
  long as on the VM. Both were font fallback (fix round, item 1).
- iGPU and dGPU are alike because what the report times is CPU work
  (layout, Vello's encoding, submitting); both GPUs keep up with 60 Hz.

### Fix round (M2.7, `fix/m2.7-fix-round`, fix-3 binary)

1. **Font fallback** (the freeze and the slow first paint): the font stack
   was "Inter, then sans-serif", and with fontconfig the generic family is
   every installed family. A character Inter lacks made the query load
   family after family, about 1,000 font files for the first CJK
   character. The stack is Inter alone now; other scripts fall back per
   script. On the VM with 1,537 families: first CJK layout 278 → 6 ms,
   first paint 385 → 24 ms, worst pan paint 104 → 17 ms.
2. **Tile time budget** (criterion 1): 6 ms of tile work per frame instead
   of 24 tiles, estimated per item and tile from the renderer's measured
   times; finer cached levels stand in when zooming out; blank areas
   render first; empty tiles need no texture. Shapes under a few pixels
   are drawn as boxes.
3. **GPU in parallel** (criterion 6): the GPU, including Vello's
   pipelines, opens on a thread from the start of `main` while fonts load
   and the first frame is laid out; Vulkan only (no OpenGL drivers loaded
   at startup). On the VM the first frame comes at 315–353 ms instead of
   407, 254–277 without the GPU's share (of which the VM's software GPU
   takes about 80 ms to draw the first frame).
4. **Idle** (criterion 6): resizes to the same size draw nothing; the
   report says why each idle frame was drawn; the scripted run waits for
   owed tiles before idling. On the VM: 0 frames, 0 % CPU.
5. `--fullscreen`, the build in the report and at start.

### Fourth run (2026-10-13, `--measure --fullscreen`, fix-3 binary)

The window was fullscreen in all five runs (scale 1.0). The reports say
1132×1348, but that is the first frame's size: Hyprland draws a new
window tiled before it makes it fullscreen. The phases ran at the
screen's full size, which this report doesn't have. From M2.8 on, the
report has the last frame's size, each phase's size, and the monitor.
Times are in ms.

| | iGPU | dGPU | no atlas | no vsync | no tiles |
|---|---|---|---|---|---|
| C1 shapes pan, p99 (< 16) | **17.8** ✗ | 13.9 | 18.0 | 16.8 | 41.7 |
| C1 shapes zoom, p99 (< 16) | **17.8** ✗ | 17.5 | 22.0 | 19.4 | 129 |
| C1 intervals over 20 ms: pan / zoom / drag, of 240 | **51 / 22 / 64** ✗ | 0 / 2 / 3 | 57 / 28 / 51 | 107 / 20 / 75 | 240 each |
| C2 tiles per drag frame (≤ 4) | **7** ✗¹ | 11 | 10 | 10 | – |
| C3 palette search, p95 (< 1) | **0.34** | 0.35 | 0.32 | 0.33 | 0.28 |
| C6 cold start without the GPU's setup (< 400) | **314** | 275 | 347 | 286 | 282 |
| C6 cold start, total | 835 | 438 | 358 | 298 | 291 |
| C6 idle frames / CPU | **0 / 0.1 %** | 0 / 0.2 % | 0 / 0.1 % | 0 / 0.1 % | 0 / 0.1 % |
| Cards pan, worst frame | 157 | 34 | 155 | 119 | 142 |

¹ Not a measurement of the drag. See below.

- **What the fix round fixed.**
  - The cards pan doesn't freeze any more: its worst frame dropped from
    about 1 s to 34 ms on the dGPU and 157 ms on the iGPU.
  - Cold start passes in all five runs.
  - Idle draws no frames at all, not even ones the window system asks
    for.
  - No crash, and no missing textures.
- **Criterion 2:** the script starts the drag right after the zoom. At
  that point about 190 tiles are still owed: `tiles_owed` peaks at
  182–196 during the drag, because the time budget spreads them over
  later frames. The drag's frames render those tiles as well. In the
  third run, which had no budget, the same drag rendered at most 4 tiles
  per frame. The script should wait until no tiles are owed before each
  phase, as it already does before idling.
- **Criterion 1:** two separate problems.
  - **CPU time per frame.** It fits at the median (6–14 ms) on both GPUs.
    The p99 is 17.5–17.8 ms, because at this window size encoding takes
    4–5 ms and the tile work's 6 ms budget comes on top.
  - **The iGPU can't keep up at this size.** One frame in four or five
    misses vertical sync while panning and dragging. Without vertical
    sync it reaches 50–60 frames per second, against about 190 at
    1132×674 in the third run. The waits for the swapchain show that the
    GPU is the limit: `acquire` p95 is 26 ms on the iGPU and 11.5 ms on
    the dGPU. The report doesn't time the GPU itself.
  - The dGPU holds 60 fps: at most 3 of 240 intervals are over 20 ms.
- **One slow frame per board on the iGPU:** when a board first gets
  tiles, one frame spends 100–144 ms on tile work (12–22 ms on the dGPU).
  The cause isn't known yet.
- The glyph atlas still pays for itself: encoding a shapes pan frame
  takes 3.7 ms with it and 5.4 ms without it.

## Decision (owner, 2026-10-13)

**We keep our toolkit** (ADR-0007, "Gate result").
- What the toolkit itself does passes with room to spare:
  - palette search 0.34 ms against the 1 ms bar;
  - 0.3 ms per keystroke;
  - 1–2 ms of painting per frame;
  - no idle frames;
  - cold start 275–347 ms without the GPU's setup.
- What fails, criterion 1 on the integrated GPU, is in the canvas
  renderer. A GPUI shell would keep that code.

Next is a focused step on the canvas (M2.8):
- The script waits until no tiles are owed before each shapes phase, so
  criterion 2 is measured on its own.
- The report times the GPU and records the monitor.
- The tiles go to the screen in fewer, cheaper steps.

Criteria 1 and 2 are then measured again on the integrated GPU. The
notes from the manual checklist (criteria 4 and 5) go here when they
come.
