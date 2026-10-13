# M2 gate: running it, the checklist, results

Status: **Draft**. The procedure and checklist are in use; the results
come with M2.7.

The gate (ADR-0007) decides whether our UI toolkit stays or GPUI replaces
it. It is measured by `imaginarium-gate`, a prebuilt binary the owner runs
on the gate machine, plus a manual checklist for what a program can't
judge (IME, screen reader, how it feels). This page is the checklist and,
after M2.7, the results.

**Gate machine:** the owner's laptop. EndeavourOS (Arch), kernel 7.2,
Hyprland (Wayland), Intel Core i9-11900H with Intel UHD (integrated) and a
GeForce RTX 3050 Laptop GPU, 32 GB RAM.

## What the binary covers so far

| Criterion (ADR-0007) | How | Since |
|---|---|---|
| 1. 10,000 shapes pan and zoom at ≥ 60 fps, p99 < 16 ms | Board of 2,000 text cards now; the 10k-shape canvas with tiles is M2.5 | partly, M2.2 |
| 2. Dragging re-renders only dirty tiles | M2.5 | – |
| 3. Palette: 2,000 commands, < 1 ms per keystroke | Search over 2,000 cards now; the command palette is M2.4 | partly, M2.3 |
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
```

For fcitx5 on Hyprland, add `exec-once = fcitx5 -d` to the Hyprland config
and set `XMODIFIERS=@im=fcitx` (for example with `env = XMODIFIERS,@im=fcitx`).
The gate uses Wayland's text-input protocol when it runs as a Wayland
client, which fcitx5 supports on Hyprland. Add Mozc in
`fcitx5-configtool`. Ctrl+Space switches the input method.

## Measurements

Run each and keep the reports. Don't use the window while `--measure`
runs (about 30 seconds). Scripted phases ignore input, but the window
should stay visible and unobstructed.

```sh
./imaginarium-gate --measure --report gate-report-igpu.json                 # integrated GPU (default)
./imaginarium-gate --measure --gpu high --report gate-report-dgpu.json      # the RTX 3050
./imaginarium-gate --measure --no-glyph-atlas --report gate-report-noatlas.json
./imaginarium-gate --measure --no-vsync --report gate-report-novsync.json   # how fast frames could be
```

The report has, per phase, the toolkit's share of each frame (`paint`)
apart from the renderer's (`encode`, `submit`), the wait for the
swapchain (`acquire`, which is vertical sync when everything keeps up)
and frame intervals. It also has typing (`typing.handle`: one keystroke
including searching all cards), idle CPU and frames (`idle.pass`), cold
start (`cold_start.pass`), the GPU, the machine and peak memory.

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

**Idle and start (criterion 6)**
17. Leave the window alone for a minute: `top -p "$(pidof imaginarium-gate)"`
    shows about 0 % CPU.
18. Does it open quickly? (The report has the number.)

**Known gaps (not bugs yet):** no clipboard (copy/paste comes with the
widgets in M2.4), no Tab focus traversal, no pinch zoom, no context menus,
and the board is one big scene (tiles come in M2.5).

## What to send back

The four `gate-report-*.json` files, the checklist notes, and a screenshot
of anything that looks wrong.

## Results

To be filled in at M2.7.
