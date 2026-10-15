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

Since M2.8, each canvas phase also reports:
- `gpu`: the GPU's own time, from timestamp queries. It has the frame's
  scene, its tile renders, and the span from the first render to the
  last. `--no-gpu-times` turns the queries off.
- `acquire`: the swapchain wait.
- `physical_size`: the window's size.
- `settled_before`: the wait for owed tiles before the phase. Each shapes
  phase starts once none are owed.
- `slowest_textures`: the parts of the phase's slowest tile frame.

`monitor` has the screen, and `window` is the last frame's size.

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
scrolling lists (the palette shows the best 10).

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

## M2.8 run (the canvas on the integrated GPU)

What changed since the fourth run (`rendering.md` §10, "Since M2.8"):
- Clips no longer make Vello layers. On the VM this halved the GPU's
  time per frame.
- Glyph atlas pages are a quarter the size, for the first tile frame
  with text.
- The window surface is opaque.
- Tile work gets what the rest of the frame leaves.
- The script settles before each shapes phase, and the report times the
  GPU.

Three runs, fullscreen, integrated GPU first:

```sh
./imaginarium-gate --measure --fullscreen --report gate-report-igpu.json
./imaginarium-gate --measure --fullscreen --gpu high --report gate-report-dgpu.json
./imaginarium-gate --measure --fullscreen --no-vsync --report gate-report-novsync.json
```

### Results (2026-10-14, "M2.8 test 1 (e77b5d1)")

All three ran fullscreen at 3456×2160 (7.5 megapixels, window scale
1.5). Times are in ms.

| | iGPU | dGPU | iGPU, no vsync |
|---|---|---|---|
| C1 shapes pan, p99 (< 16) | **12.7** | 11.2 | 11.5 |
| C1 shapes zoom, p99 (< 16) | **17.7** ✗ | 13.1 | 17.0 |
| C1 intervals over 20 ms: pan / zoom / drag, of 240 | **0 / 1 / 1** | 0 / 0 / 0 | 0 / 28 / 49 |
| C2 tiles per drag frame (≤ 4) | **3** | 3 | 3 |
| C3 palette search, p95 (< 1) | **0.34** | 0.36 | 0.29 |
| C6 cold start without the GPU's setup (< 400) | **231** | 172 | 243 |
| C6 idle frames / CPU | **0 / 0 %** | 0 / 0.3 % | 0 / 0 % |
| GPU time per shapes pan frame, median | 9.0 | 1.0 | 8.9 |
| Tiles filling the screen before the pan: frames / ms | 31 / 535 | 22 / 383 | 31 / 333 |
| Slowest tile work in a frame (of it, one submission) | 68.6 (66.6) | 13.9 (0.9) | 120 (117) |

- **Criterion 1 passes on the dGPU.** On the iGPU only the zoom misses:
  6 of its 240 frames took over 16 ms, one of them a 60 ms stall.
- **The iGPU keeps up at 7.5 megapixels:** 9 ms of GPU work per frame
  (the fourth run had one frame in four or five miss vertical sync).
- **Criterion 2 passes:** with the script settling first, a drag frame
  renders at most 3 tiles.
- **Slow fill (owner, watching):** the screen filled with tiles in
  rings from the middle, with blank areas meanwhile, over 22–31 frames.
  Tile work ran on the UI thread within its budget: 1.4–3 tiles per
  frame, of up to 228 owed.
- **Stalls on the iGPU only:** single submissions of 3–4 tile renders
  took 58–117 ms, while the GPU's own work on them took under 1 ms.
- **Scale 1.5 against the monitor's 2.0:** likely not a bug. On Wayland
  an output reports a whole-number scale (2), and the window gets the
  fractional one (1.5) on its own. Text crispness is for the manual
  check (criterion 5).
- Cold start in the iGPU run: loading the Vulkan driver took 1.5 s
  (89–106 ms in the other runs), likely the first start after boot. It
  doesn't count (owner, 2026-10-13).

## M2.8 run 2: tiles on worker threads

The owner chose both fixes for M2.8 (2026-10-14; ADR-0019):
- **Tiles render on worker threads** with `vello_cpu`, and frames upload
  them. The UI thread only paints the tiles' items.
- **Tiles share pages on the GPU**, made ahead of need on a thread of
  their own. GPU memory comes in smaller blocks.
- Tiles drawn one texel per pixel are sampled nearest (one load per
  pixel on the GPU instead of four).
- Items paint straight into their tile's scene.
- The glyph atlas takes up to 32 of its pages again.

On the VM (llvmpipe, 1280×800, 4 tile threads against tiles in the
frame), the shapes pan's tiles filled in 47 ms instead of 196 ms, and
the UI thread's tile work per frame dropped from about 1 ms (at most
11–23 ms) to 0.25 ms (at most 1–3.6 ms).

The same three runs, plus one with tiles in the frame to compare:

```sh
./imaginarium-gate --measure --fullscreen --report gate-report-igpu.json
./imaginarium-gate --measure --fullscreen --gpu high --report gate-report-dgpu.json
./imaginarium-gate --measure --fullscreen --no-vsync --report gate-report-novsync.json
./imaginarium-gate --measure --fullscreen --tile-threads 0 --report gate-report-nothreads.json
```

The report adds per canvas phase: `tiles_arrived`, `tiles_in_flight`,
`worker_ms_per_tile`, `tile_build` and `build_ms_per_tile` (the UI
thread's painting), `upload`, `tile_pages` and `tile_pages_made` (pages
made in a frame because none was ready), and `content.tile_threads`.

### Results (2026-10-14, "M2.8 test 2 (8cb93a5)")

All four ran fullscreen at 3456×2160, with 8 tile threads except the
last. Times are in ms.

| | iGPU | dGPU | iGPU, no vsync | iGPU, tiles in the frame |
|---|---|---|---|---|
| C1 shapes pan, p99 (< 16) | **7.7** | 5.5 | 6.1 | 16.6 ✗ |
| C1 shapes zoom, p99 (< 16) | **11.0** | 10.9 | 10.8 | 13.3 |
| C1 intervals over 20 ms: pan / zoom / drag, of 240 | **0 / 2 / 0** | 0 / 0 / 0 | 0 / 0 / 0 | 3 / 2 / 1 |
| C2 tiles per drag frame (≤ 4) | **3** | 3 | 3 | 3 |
| C3 palette search, p95 (< 1) | **0.46** | 0.35 | 0.32 | 0.30 |
| C6 cold start without the GPU's setup (< 400) | **197** | 199 | 290 | 209 |
| C6 idle frames / CPU | **0 / 0 %** | 0 / 0.2 % | 0 / 0 % | 0 / 0 % |
| GPU time per shapes pan frame, median | 8.5 | 1.0 | 8.7 | 8.8 |
| Tiles filling the screen before the pan: frames / ms | **9 / 166** | 8 / 149 | 9 / 103 | 37 / 636 |
| The same before the drag | **5 / 101** | 4 / 83 | 7 / 74 | 28 / 485 |
| Zoom: tiles owed per frame, mean | **15** | 16 | 15 | 62 |
| Slowest tile work in a frame: cards / zoom | **32 / 20** | 5 / 5 | 52 / 5 | 32 / 6 |
| Peak memory (RSS), MB | 287 | 492 | 301 | 298 |

Per tile on the iGPU run: the workers took 1.2 ms (cards), 0.4 ms
(shapes pan) and 1.0 ms (zoomed out); the UI thread's painting 0.30,
0.08 and 0.15 ms.

- **Criterion 1 passes on the iGPU**: zoom p99 11.0 ms (17.7 in the
  first M2.8 run). It passes in every run with tile threads. With tiles
  in the frame, the pan misses (16.6 ms).
- **The fill is 3–4 times faster**: 9 frames before the shapes pan,
  against 31 in the first M2.8 run and 37 with tiles in the frame.
- **The big stalls are gone**, also with tiles in the frame: the first
  run's single submissions of 58–117 ms are down to 15 ms at most. The
  smaller GPU memory blocks are the likely reason.
- **Two hitches are left on the iGPU:**
  - The cards board's first frame: 32 ms (52 ms without vsync), making
    the first tile page in that frame. Fixed since (run 3).
  - One zoom frame's upload of 17 tiles took 20 ms (two intervals over
    20 ms, 24 ms at most). Whether it waited behind a page being made
    on its thread, the next report says (`pages_coming`).
- **Owner, watching:** smoother than before; the run with tiles in the
  frame was visibly laggier. Zooming in was smooth. Zooming out on the
  shapes had some minor glitching. The board still filled from the
  middle out, in rings, but faster.
  - The glitching is likely the blank tiles: with workers, a tile newly
    in view with nothing to stand in for it (zooming out reveals them at
    the edges) stayed blank for at least a frame. The report didn't count
    them yet. Fixed since (run 3).
  - The rings at opening: a fullscreen view needs up to 228 tiles. At
    0.3 ms of painting per cards tile on the UI thread, that's at least
    70 ms, so the fill takes several frames whatever the threads do.
    It can be covered (owner: maybe an animation, eventually).
- **GPU memory:** 21 tile pages at the end, 336 MB of the iGPU's shared
  memory (not in RSS). The tile caches keep up to 1,000 tiles per board.
  A window-wide memory budget for tiles is for M3.
- Cold start in the iGPU run: loading the Vulkan driver took 793 ms
  (95–144 ms in the others), likely the first start after boot again.
  It doesn't count (owner, 2026-10-13).

## M2.8 run 3: the stalls and blanks left

What changed since run 2:
- **Tile pages are made as tiles go to the workers**
  (`Scene::expect_uploads`), a frame before they come back. An upload
  that finds no free slot waits for the page on its way.
- **A frame waits for the tiles nothing stands in for**, up to 4 ms and
  within what the rest of the frame leaves of 12 ms. They show in that
  frame instead of blank until the next.
- **Threads take up to 8 tiles at once** (4 before).
- **Tile work no longer counts the canvas's own painting twice**: a frame
  that painted for its whole budget left the next one only 2 ms.

On the VM (llvmpipe, 1280×800, 4 tile threads):
- the cards board's first frames: slowest tile work 3.7 ms instead of
  19.5 ms with run 2's build, no page made in a frame;
- with the wait, 8 tiles per thread and the budget fix, against the
  page fix alone: blank tile-frames 58 → 0 in the shapes pan and 24 → 0
  in the zoom; the cards board covered after 2 frames (46 ms) instead
  of 3 (64 ms);
- the wait took 0.05 ms per frame on average, 3.7 ms at most.

The report adds per canvas phase:
- `tiles_blank` and `frames_with_blanks`: owed tiles nothing stood in for;
- `covered_after` and `complete_after`: frames and time to the first frame
  with no blank tile, and with none owed. The cards' pan starts as the
  board opens, so these measure its fill;
- `blank_wait`: the frames' wait for tiles;
- `slowest_textures.pages_coming`.

```sh
./imaginarium-gate --measure --fullscreen --report gate-report-igpu.json
./imaginarium-gate --measure --fullscreen --gpu high --report gate-report-dgpu.json
./imaginarium-gate --measure --fullscreen --no-vsync --report gate-report-novsync.json
```

Then, in the same session, the manual checklist's items for criteria 4
and 5 (HiDPI, text, input method, screen reader), and watching 5a: is
zooming out on the shapes still glitchy, and how does the board fill as
it opens?

### Results (2026-10-14, "M2.8 test 3 (c23c2d4)")

All three ran fullscreen at 3456×2160, with 8 tile threads. Times are in
ms.

| | iGPU | dGPU | iGPU, no vsync |
|---|---|---|---|
| C1 shapes pan, p99 (< 16) | **10.0** | 7.4 | 6.7 |
| C1 shapes zoom, p99 (< 16) | **19.3 ✗** | 13.5 | 13.6 |
| C1 zoom frames over 16 ms, of 240 | **4** | 0 | 1 |
| C1 intervals over 20 ms: pan / zoom / drag, of 240 | **2 / 8 / 2** | 0 / 2 / 0 | 1 / 1 / 0 |
| C2 tiles per drag frame (≤ 4) | **3** | 3 | 3 |
| C3 palette search, p95 (< 1) | **0.32** | 0.33 | 0.41 |
| C6 cold start without the GPU's setup (< 400) | **308** | 278 | 267 |
| C6 idle frames / CPU | **0 / 0 %** | 0 / 0.3 % | 0 / 0.1 % |
| GPU time per shapes pan frame, median | 8.5 | 1.0 | 8.6 |
| Cards board opening: covered / complete, frames | **16 / 18** | 14 / 15 | 17 / 19 |
| The same in ms | **318 / 351** | 243 / 260 | 315 / 336 |
| Tiles filling the screen before the shapes pan: frames / ms | **7 / 133** | 6 / 116 | 6 / 74 |
| Zoom: tiles owed per frame, mean | **15** | 12 | 11 |
| Zoom: blank tiles / frames with any | **39 / 6** | 3 / 1 | 0 / 0 |
| Slowest tile work in a frame: cards / zoom | **5 / 13** | 5 / 6 | 3 / 6 |
| Peak memory (RSS), MB | 220 | 437 | 230 |

- **Criterion 1 misses on the iGPU in the zoom**: p99 19.3 ms (11.0 in
  run 2), and zoom p99 is up about 2.6 ms in the other two runs too. The
  owner felt it: zooming in on the shapes was a bit laggy.
  - **Uploads came in bursts.** With 8 tiles per thread, up to 57 tiles
    came back in one frame (32 in run 2), and the frame uploaded them
    all: 13 ms. Uploads weren't in any frame's budget: they counted only
    in the time per tile, as tile work the frame before.
  - **A burst costs more per tile**: 0.23 ms against 0.08 ms on average
    (and 0.08 ms in the no-vsync run's 64-tile frame). Likely the
    driver backing fresh staging memory for the burst; not confirmed.
  - **Building overshot its budget**: up to 9.2 ms against 6, as the
    estimate of a tile's cost was low (57 tiles built in one frame).
    Run 2's double-counting bug had hidden this by halving the budget.
  - **The UI thread was about 20 % slower than in run 2** on identical
    work (the shapes pan: painting 1.21 against 1.00 ms per frame,
    encoding 2.12 against 1.83), maybe a power setting. The other two
    runs weren't. That alone doesn't explain the zoom.
- **Zooming out is fine** (owner). In the zoom, 39 blank tiles in 6
  frames on the iGPU, none without vsync.
- **No tile page was made in a frame** in any phase, and none was being
  made when the slowest uploads ran (`pages_coming` 0).
- **The cards board's first frame is still slow**: 52 ms, now in encode
  (39 ms; 7.5 ms on the dGPU), not in making a page. The board covers
  the screen after 16 frames (318 ms). Owner: similar to before, not the
  biggest deal.
- No word yet on the manual checklist (criteria 4 and 5) with this run.
- GPU memory: 22 tile pages at the end, 352 MB.

## M2.8 run 4: uploads within the frame's budget

What changed since run 3:
- **Uploads take part of a frame's tile time**: half of it at most, by
  their measured cost per tile, and 16 tiles at most. Tiles that come
  back beyond that wait (`parked`) for the next frame, the current
  level's first. Building gets what the uploads leave.
- **The clock stops building** when the estimate of a tile's cost is
  wrong: a frame builds no further tile once its time is spent.
- **Waiting for tiles nothing stands in for** counts their uploads in
  the wait's time.
- Parked tiles count against the 8 per thread, so the workers don't get
  far ahead of the uploads.

On the VM (llvmpipe, a 1920×1200 window at scale 1.5, 4 tile threads;
its frames take 30–40 ms in llvmpipe's submit, so only the tile work
compares):
- zoom: at most 16 uploads in a frame (32 before), the slowest 4.2 ms
  (6.7 before); up to 28 tiles parked;
- the cards board's opening: the slowest upload 6.0 ms (9.5 before).

The report adds per canvas phase:
- `slowest_frames`: the five slowest frames on the UI thread, with their
  parts (painting, building, waiting, uploads, encode, submit) and tile
  counts;
- `tiles_parked`, and `upload_ms_per_tile`.

```sh
./imaginarium-gate --measure --fullscreen --report gate-report-igpu.json
./imaginarium-gate --measure --fullscreen --gpu high --report gate-report-dgpu.json
./imaginarium-gate --measure --fullscreen --no-vsync --report gate-report-novsync.json
```

If they hold, M2.8 is done. Then, in the same session if possible, the
manual checklist's items for criteria 4 and 5 (HiDPI, text, input
method, screen reader), and watching: is zooming in on the shapes still
laggy?

### Results (2026-10-14, "M2.8 test 4 (5336189)")

All three ran fullscreen at 3456×2160, with 8 tile threads. Times are in
ms.

| | iGPU | dGPU | iGPU, no vsync |
|---|---|---|---|
| C1 shapes pan, p99 (< 16) | **8.8** | 9.1 | 8.7 |
| C1 shapes zoom, p99 (< 16) | **13.5** | 11.4 | 11.6 |
| C1 zoom frames over 16 ms, of 240 | **1** | 0 | 1 |
| C1 intervals over 20 ms: pan / zoom / drag, of 240 | **1 / 2 / 0** | 0 / 1 / 0 | 0 / 0 / 0 |
| C2 tiles per drag frame (≤ 4) | **3** | 3 | 3 |
| C3 palette search, p95 (< 1) | **0.30** | 0.46 | 0.33 |
| C6 cold start without the GPU's setup (< 400) | **387** | 205 | 269 |
| C6 idle frames / CPU | **0 / 0 %** | 0 / 0.3 % | 0 / 0 % |
| GPU time per shapes pan frame, median | 8.4 | 1.0 | 8.5 |
| Zoom: most uploads in a frame / slowest frame's uploads | **30 / 1.7** | 30 / 2.6 | 30 / 1.8 |
| Zoom: upload per tile | **0.065** | 0.055 | 0.059 |
| Zoom: tiles owed per frame, mean / parked at most | **18 / 48** | 17 / 48 | 17 / 48 |
| Zoom: blank tiles / frames with any | **16 / 5** | 0 / 0 | 0 / 0 |
| Cards board opening: covered / complete, frames | **26 / 29** | 25 / 27 | 20 / 22 |
| The same in ms | **568 / 619** | 485 / 518 | 330 / 351 |
| Peak memory (RSS), MB | 206 | 413 | 205 |

- **Criterion 1 passes in all three runs.** On the iGPU the zoom's p99 is
  13.5 ms (19.3 in run 3, 11.0 in run 2), with 2 intervals over 20 ms
  (8 in run 3, 2 in run 2).
  - The uploads stay small: 1.7 ms at most in a frame. The frames over
    16 uploads (up to 30) are tiles nothing stood in for, which the
    frame waited for (zooming out).
  - Building stays within its budget: 6.1 ms at most (9.2 in run 3).
  - The slowest zoom frame (19.2 ms on the iGPU, 18.9 without vsync,
    both the 27th frame, zooming out) spent 10.5 ms in encode, with 102
    tiles drawn by stand-ins. The next slowest are 12–14 ms.
  - Tiles sharpen a little more slowly: 18 owed per frame on average
    (15 in runs 2 and 3), and up to 48 back from the threads waiting to
    be uploaded.
- **Owner, watching:** zooming out is fine. Zooming in, "some tiles or
  shapes glitch in and out of existence" (it isn't slow: the frames are
  on time). Earlier runs' "a bit laggy" was the same thing.
  - The cause: levels are half octaves apart (√2), so a tile straddles
    up to four tiles of the next coarser level, but a tile waiting to be
    rendered drew only the one at its top-left corner as its stand-in.
    The rest of it stayed empty until it came back: strips of the screen
    at each level switch zooming in, for a few frames. Zooming out, the
    finer stand-ins already took every tile they touch. The report
    didn't count these as blank. Run 4's upload cap kept tiles owed
    longer, so the gaps lasted longer. Fixed since (run 5).
- **The cards board now takes longer to cover the screen:** 26 frames
  (568 ms) on the iGPU, against 16 (318 ms) in run 3. Its frames now
  keep to their budget: run 3 built many tiles at once, in longer frames.
  Some cards tiles cost a lot to paint the first time (text layout on
  the UI thread): one frame built 12 tiles in 22.7 ms, as the clock
  stops only between tiles. The board's first two frames take 89 and
  29 ms (58 and 26 ms of it encode).
- **One slow upload in the cards' pan**, in both iGPU runs at the same
  frame (198): 20–28 tiles in 15 ms, against about 1 ms on the dGPU. No
  page was made or coming then. Not yet explained.
- **Cold start 387 ms on the iGPU**, close to the limit: likely the first
  start after boot again (the Vulkan driver took 857 ms, the font list
  201 ms against 115–176 in the others).
- No word yet on the manual checklist (criteria 4 and 5).

## M2.8 run 5: stand-ins that cover the whole tile

What changed since run 4:
- **A stand-in covers the whole tile**: the nearest level whose cached
  tiles cover all of it, coarser first, then finer; all of them are
  drawn. Failing that, the nearest level with any covers part of it,
  and the tile counts as blank: it renders first and the frame waits
  for it.
- `tiles_blank` and `frames_with_blanks` now count those tiles too (the
  gaps run 4 didn't count).

On the VM (1920×1200, 1 tile thread), the zoom's encode is unchanged
(1.28 ms on average); the VM keeps few tiles owed, so the next report's
`slowest_frames` says what the extra stand-in draws cost on the laptop.

```sh
./imaginarium-gate --measure --fullscreen --report gate-report-igpu.json
./imaginarium-gate --measure --fullscreen --gpu high --report gate-report-dgpu.json
./imaginarium-gate --measure --fullscreen --no-vsync --report gate-report-novsync.json
```

Watching: do tiles or shapes still disappear while zooming in? If not,
M2.8 is done (the owner, 2026-10-14: rendering the next level ahead only
if still needed). And, in the same session if possible, the manual
checklist's items for criteria 4 and 5.

### Results (2026-10-14, "M2.8 test 5 (ae7cd45)")

All three ran fullscreen at 3456×2160, with 8 tile threads. Times are in
ms.

| | iGPU | dGPU | iGPU, no vsync |
|---|---|---|---|
| C1 shapes pan, p99 (< 16) | **7.4** | 9.0 | 7.1 |
| C1 shapes zoom, p99 (< 16) | **12.8** | 11.7 | 12.4 |
| C1 zoom frames over 16 ms, of 240 | **0** | 0 | 1 |
| C1 intervals over 20 ms: pan / zoom / drag, of 240 | **0 / 1 / 0** | 1 / 0 / 0 | 1 / 1 / 0 |
| C2 tiles per drag frame (≤ 4) | **3** | 3 | 3 |
| C3 palette search, p95 (< 1) | **0.32** | 0.37 | 0.39 |
| C6 cold start without the GPU's setup (< 400) | **337** | 208 | 227 |
| C6 idle frames / CPU | **0 / 0 %** | 0 / 0.4 % | 0 / 0.1 % |
| Zoom: blank tiles / frames with any | **1 / 1** | 0 / 0 | 0 / 0 |
| Cards board opening: covered / complete, frames | **25 / 26** | 21 / 23 | 21 / 23 |
| The same in ms | **688 / 704** | 342 / 375 | 338 / 353 |
| Cards: slowest frame | **244** | 22 | 74 |
| Peak memory (RSS), MB | 196 | 416 | 208 |

- **The zoom bug is fixed in all three runs** (owner). The stand-ins'
  extra draws cost nothing visible: the zoom's encode p95 is 3.3 ms on
  the iGPU (4.6 in run 4).
- **Criterion 1 passes in all three runs**, as in run 4.
- **Owner:** the no-vsync run showed fewer frames and more motion blur.
  That is the mode, not a regression: without vsync the gate draws as
  fast as it can (the zoom about 120 frames a second) and takes the
  integrated GPU from the compositor, so frames reach the screen
  unevenly. The app always waits for vsync; the mode measures headroom.
- **Owner:** in all runs, something froze shortly before the command
  palette opened. The report didn't show it: the typing phase recorded
  only painting and encoding. It was the search: the script types one
  character per frame, and each keystroke made every visible card tile
  render again (cards the search misses were dimmed in the tiles), more
  than a frame can render. The board showed stale tiles until the
  typing stopped. Fixed since: the dimming is a veil over the tiles,
  and the report has the run's longest frame intervals in any
  phase (`longest_intervals`) and the typing's tile renders.
- **The cards board on the iGPU with vsync** (the first start after a
  boot again: the Vulkan driver took 937 ms): 244 ms as it opened (215 of
  it encode) and 136 ms at the pan's frame 198 (120 in encode, 12 in
  building). Since run 4, every iGPU run has a hitch at that frame, of
  12–15 ms in uploads otherwise; on the dGPU none. On the VM the same
  frame spends about 15 ms replaying a few runs of text: likely glyphs
  drawn again after glifo's cache evicted them (it evicts glyphs unused
  for 64 frames, every 64 frames; the status line's numbers move the
  text after them). The 120 ms isn't explained. For M3, with the cards'
  opening.
- No word yet on the manual checklist (criteria 4 and 5).

## M2.8: done (2026-10-14)

Criterion 1 passes on the integrated GPU, fullscreen at 3456×2160, in
runs 4 and 5 (shapes pan p99 7.4–8.8 ms, zoom 12.8–13.5 ms), and on the
dGPU. Criteria 2, 3 and 6 pass in every run. Zooming in no longer makes
shapes disappear (owner, run 5).

Left for M3 (the canvas):
- covering the fill as a board opens (the owner: maybe an animation),
  and the cards' painting on the UI thread (text layout), which limits
  it;
- the cards board's slow frames on the iGPU (its first frame, and frame
  198 of the pan);
- rendering the next level ahead while zooming in, if needed;
- a window-wide memory budget for tile pages (22 pages, 352 MB, at the
  end of a run).

The search fix and the new report fields come with the next build; the
gate's last run is part of M2.9.

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
