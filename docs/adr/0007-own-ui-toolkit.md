# 0007. Own UI toolkit on the Linebender stack

- Status: Accepted (subject to the M2 prototype gate)
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
