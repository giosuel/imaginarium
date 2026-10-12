# 0002. iced as application shell

- Status: Proposed (needs owner confirmation before M1)
- Date: 2026-10-12

## Context
We need a cross-platform Rust GUI framework for the *chrome*: palette,
floating toolbars, inspector, dialogs and windows. The heavy content (the
canvas, text and pages) is rendered by our own stack (ADR-0003). The
framework must support Linux (Wayland and X11), Windows and macOS, IME,
accessibility, multiple windows (presenter view), and must let us embed a
custom wgpu surface.

## Decision
Use **iced** (0.14+) for the shell:
- Its retained, Elm-style architecture (state → view → message → update)
  matches our unidirectional data flow and React-like memoization (`lazy`).
- It is wgpu-based, so our canvas pipeline shares the device and queue, and
  the shell composites our canvas texture.
- It is proven in large production apps (System76 COSMIC desktop, Kraken
  desktop) and is actively maintained.
- It is MIT licensed.

iced types are **confined to `tafel-shell`**. Lower crates never import iced.

## Consequences
- We follow iced's release cadence and must upgrade iced, wgpu and Vello
  together.
- Some widgets (rich-text editing in panels, docking) we build ourselves.

## Alternatives considered
- **egui:** easiest and very popular, but immediate mode makes large,
  deeply styled UIs and fine-grained memoization harder. It looks "tool-like"
  unless heavily themed.
- **GPUI (Zed):** best-in-class performance and a good text story, but the
  API is unstable outside Zed and has less documentation and community
  support.
- **Slint:** polished declarative DSL, but its royalty-free license has
  conditions (or GPL/commercial) and it is less suited to a custom-canvas-heavy
  app.
- **Xilem/Masonry:** architecturally attractive (same people as Vello and
  Parley), but still too immature. Re-evaluate around M9.
- **Tauri / webview:** rejected, because webview rendering contradicts the
  performance focus and the native requirement.
