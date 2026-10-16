# 0007. Own UI toolkit on the Linebender stack

- Status: Accepted (M2 gate decided 2026-10-13: we keep our toolkit)
- Date: 2026-10-12 (gate procedure amended the same day, owner decision)
- Supersedes: 0002

## Context
The product is canvas-heavy: boards, pages, slides and raster layers, with
selection handles, guides and floating toolbars drawn on top. The owner
wants a heavy UI with top performance and accepts added complexity. Using
a third-party framework for the panels means either handing textures across
renderers (GPUI) or rebuilding the whole view tree after every change (iced).

## Decision
- Build `imaginarium-ui`, a retained widget toolkit, on **winit** (windows
  and input/IME), **wgpu**, **Vello** (rendering), **Parley** (text),
  **Taffy** (flex/grid layout) and **AccessKit** (accessibility).
  Linebender's Masonry is used as a reference, or as a dependency if it is
  stable enough.
- `imaginarium-reactive` provides **fine-grained signals**, memos and effects
  in the style of SolidJS or Leptos. A changed signal updates only the
  widgets that read it. There is no virtual DOM and no diffing.
- Panels and the canvas build **one Vello scene per frame**, with one frame
  scheduler and one text stack.

## Gate
A prototype in M2 with these pass criteria (integrated GPU):
1. 10,000 shapes pan and zoom at 60 fps or more, with p99 frame time under 16 ms.
2. Dragging re-renders only dirty tiles. Verified with tracy.
3. The palette searches 2,000 commands in under 1 ms per keystroke.
4. A text field handles IME (CJK) and works with a screen reader
   (Orca, NVDA, VoiceOver).
5. HiDPI and mixed-DPI multi-monitor setups are correct.
6. Idle CPU is about 0 %, and cold start is under 400 ms.

**Procedure** (owner decision, 2026-10-12):
- **No calendar time box, one decision point.** When the gate app runs
  (end of M2.5), the owner runs a prebuilt binary that measures the
  criteria and writes a report, plus a short manual checklist (IME, screen
  reader). If a criterion fails, there is **one focused fix round** for it.
  Then we decide: keep our toolkit, or fall back. There is no open-ended
  "one more fix" iteration, because everything built on the toolkit
  meanwhile makes the fallback costlier.
- **Linux only for now**, because that's the hardware available. Windows
  and macOS stay open and are checked when hardware is available, before
  those platforms ship. Known risks there (from the M2 research, see
  `docs/design/m2-ui-stack.md`): winit's open mixed-DPI bugs on Windows 11,
  IMM32-only IME on Windows, and macOS Zhuyin composition.

If the prototype fails criteria we cannot fix, we write a new ADR and fall
back to **GPUI** for the shell.

## Consequences
- We own the widget code. This is the main added cost.
- Model, render, text and format crates stay independent of the toolkit.

## Amendment: criterion 6's cold start (owner decision, 2026-10-13)
Cold start is measured **without the GPU's setup**: loading the graphics
drivers, opening the device, creating the window's surface and compiling
the renderer's pipelines. That costs any GPU toolkit the same, GPUI
included: 296–449 ms on the gate laptop, 727 ms on the first start after a
boot. The program opens the GPU on a thread of its own while it starts,
and the gate report has both the total time to the first frame and
`without_gpu_setup_ms` (the total minus what the window waited for the GPU
and spent on it); the criterion uses the latter.

## Amendment: criterion 1's frame rate (owner decision, 2026-10-16)
"60 fps or more" is measured as at most 1 % of the frame intervals over
20 ms (a missed refresh at 60 Hz), with vertical sync (since the M2.9
review). It is judged over all the runs of a session on one GPU
together. A run has 240 intervals per phase, so on its own two slow
frames from a driver hiccup would decide it. The p99 frame time must
pass in each run. In the M2.9 runs one integrated-GPU run of five
failed alone (4 of 240), and all five together had 0.67 % (pan) and
0.42 % (zoom).

## Gate result (owner decision, 2026-10-13)
**We keep our toolkit.** The results are in `docs/design/m2-gate.md`. The
decision rests on the fourth run, with the fix-3 binary on the gate
laptop and a fullscreen window.
- Criteria 3 and 6 pass in every run.
- Criterion 2 wasn't measured cleanly: the scripted drag started while
  the zoom's tiles were still owed. It is measured again after the gate.
- Criterion 1 fails on the integrated GPU. The GPU can't keep up at this
  size, and the p99 frame time is 17.8 ms. The RTX 3050 Ti holds 60 fps.
  The cause is in the canvas renderer: rendering the tiles and
  compositing them through Vello (ADR-0003). A GPUI shell would keep that
  code, so it is no reason to fall back. It is fixed in the next step
  (M2.8) and judged again on the integrated GPU.
- The manual checklist for criteria 4 and 5 (IME, screen reader, HiDPI)
  hadn't been reported back when the decision was made. Its notes go in
  the results when they come.
