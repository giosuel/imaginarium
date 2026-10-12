# Tafel: Project Plan

Status: **Draft v1** (2026-10-12). Decisions marked *(ADR-xxxx)* are recorded
in [`docs/adr/`](adr/). Each one can be revisited, but changing one means
writing a superseding ADR.

---

## 1. Vision

One application with one surface, the **board**. Documents, slides, page
layouts, diagrams and raster drawings are all *objects on the board* or
*views of the same document model*:

- A **Board** is an infinite canvas (whiteboard, draw.io and Visio style).
- A **Document** is a flowing, paginated text stream (Word, Writer).
- A **Deck** is an ordered set of fixed-size frames (PowerPoint, Impress).
- A **Layout** is a set of fixed pages with linked frames (Publisher).
- A **Paint layer** is a tiled raster object (Paint). It can sit anywhere,
  including on a slide or a board.

Because these share one object model, a diagram drawn on a board can be
dropped into a document or a slide without conversion. Every mode uses the
same selection, styling, undo, search and add-on APIs.

### Non-goals (for now)

- Spreadsheets. The model reserves a `Sheet` object kind so it can be added
  later.
- Real-time collaboration. The operation log (§4.3) is designed so that a
  CRDT can be added later without rewriting the model.
- Pixel-perfect fidelity with every MS Office feature. The goal is
  *lossless round-trip* for anything we don't understand (see §6.3).

---

## 2. UX principles

The UI should look like a clean whiteboard, with features appearing only
when the user asks for them.

1. **Empty by default.** On startup the user sees the canvas, a small floating
   "pill" (mode, zoom, share/export) and nothing else. There is no ribbon
   and no menu bar. On macOS the native menu bar is filled for platform
   conventions only.
2. **Contextual over global.** Selecting an object shows a small floating
   toolbar next to it. The toolbar is generated from the commands that apply
   to the selection: text selected shows text styles, a connector shows
   arrowheads and routing.
3. **One entry point for everything: the command palette** (`Ctrl/Cmd+K`).
   Every feature is a *command* (§4.4), so every feature can be searched.
   Results are ranked fuzzily with recency/frequency boosts, include
   synonyms ("bold", "fett", "strong"), and show the keyboard shortcut so
   users learn it.
4. **Slash and quick-insert.** Typing `/` in text, or double-clicking empty
   canvas, opens an insert picker (shape, table, image, slide, template)
   that is filtered as you type.
5. **Progressive disclosure.** Advanced properties live in an *inspector*
   panel that is hidden until the user opens it (`Ctrl+I` or "More…" on the
   floating toolbar). It slides over the canvas and does not reflow it.
6. **Direct manipulation first.** Drag to connect, drag a handle to resize,
   alt-drag to duplicate, show snapping guides. Dialogs are a last resort.
7. **Nothing blocks.** Loading, saving, exporting and add-on work run in the
   background with inline progress. No modal spinners.
8. **Keyboard-complete and accessible.** Every action works by keyboard.
   Screen-reader support goes through AccessKit. High-contrast and
   reduced-motion themes are available.
9. **Template packages** (shape libraries, slide themes, document styles,
   brushes) are browsed in a single "Library" drawer that the palette can
   also open. Packages are add-ons (§5).

---

## 3. Technology choices (summary)

| Concern              | Choice                                              | ADR  |
|----------------------|-----------------------------------------------------|------|
| Language / build     | Rust (stable), Cargo workspace, MSRV pinned         | 0001 |
| App shell / widgets  | **iced** (retained, Elm-style, wgpu)                | 0002 |
| Canvas rendering     | Own render layer on **wgpu**; **Vello** for vector paths, tiled caches | 0003 |
| Text shaping/layout  | **Parley** (+ swash, fontique) behind our own trait | 0003 |
| Document model       | Arena + persistent (structurally shared) data       | 0004 |
| Undo / history       | Operation log of invertible ops                     | 0004 |
| OOXML (docx/pptx)    | **ooxmlsdk** for (de)serialization, own mapping layer | 0005 |
| ODF (odt/odp)        | Own crate on `quick-xml` + `zip`                    | 0005 |
| Native format        | Zip container: JSON manifest + binary chunks        | 0005 |
| Add-ons              | **WebAssembly components** (wasmtime + WIT); declarative packages for templates | 0006 |
| Accessibility        | AccessKit (used by iced and winit)                  | 0002 |
| Async / background   | Dedicated worker pool (rayon) + channels; no tokio in core | 0003 |
| CI                   | GitHub Actions matrix: ubuntu, windows, macos       | 0001 |

Why these were chosen, and the risks, are in the ADRs. The most important
architectural point: **the GUI framework is only the shell.** The canvas,
text engine and document model are our own crates and do not depend on
iced. If iced stops being a good fit, only `tafel-shell` has to be replaced.

---

## 4. Architecture

### 4.1 Crate layout (Cargo workspace)

Dependencies only point *downwards*. No crate below `shell` depends on a GUI
framework.

```
crates/
  tafel-app          # binary: wires everything together, CLI args, startup
  tafel-shell        # iced UI: windows, palette, floating toolbars, inspector
  tafel-canvas       # viewport, hit-testing, tools (select, pen, connector…), gestures
  tafel-render       # scene → GPU (wgpu + vello), tile cache, raster compositing
  tafel-text         # rich text model, shaping, line breaking, pagination
  tafel-model        # document model: nodes, styles, ops, history, selection
  tafel-commands     # command registry, keymaps, palette search index
  tafel-layout       # page/flow layout for Document & Layout modes
  tafel-diagram      # connectors, routing, snapping, shape geometry
  tafel-paint        # raster layers, brushes, tiled pixel storage
  tafel-io           # format traits + registry
  tafel-io-native    # .tafel container
  tafel-io-ooxml     # docx, pptx
  tafel-io-odf       # odt, odp
  tafel-io-export    # pdf, svg, png
  tafel-addon-api    # stable public API types + WIT definitions
  tafel-addon-host   # wasmtime host, sandboxing, permissions
  tafel-packages     # template/shape-library packages (declarative)
  tafel-test-utils   # fixtures, golden-image & round-trip harness
addons/              # first-party add-ons built on the public API
  shapes-basic, shapes-uml, shapes-network, themes-default, …
```

Rules (enforced in CI with `cargo deny` and a small layering check):

- `tafel-model` has **no** dependencies on rendering, IO or UI.
- Each `tafel-io-*` crate implements the `Importer`/`Exporter` traits from
  `tafel-io` and is registered at startup, the same way add-ons are.
- First-party features use the same command and extension points as
  third-party add-ons ("dogfood the API").

### 4.2 Data flow (unidirectional, React/Elm-style)

```
 input event ──► tool / command ──► Op(s) ──► Model (new version)
                                               │
                         ┌─────────────────────┤ change set (dirty node ids)
                         ▼                     ▼
               layout (incremental)     shell view state
                         ▼
               render scene diff ──► tile invalidation ──► GPU
```

- **Single source of truth:** the model. UI state that should persist
  (e.g. inspector open) lives in a small `ViewState`. Ephemeral state lives
  in widgets.
- **Ops in, change sets out.** Every mutation is an `Op`. Applying ops yields
  a `ChangeSet` listing the touched node ids. Layout, render and UI re-run
  *only* for those ids. This is the equivalent of React's reconciliation,
  but explicit and O(changes) instead of O(tree).

### 4.3 Document model

- **Arena of nodes** keyed by stable `NodeId` (generational slotmap). Node
  kinds: `Frame` (board/page/slide), `Shape`, `Connector`, `TextFlow`,
  `Image`, `RasterLayer`, `Group`, `Table`, plus `Unknown` for preserved
  foreign XML.
- **Persistent data structures** (`imbl` / `rpds`) for node maps and child
  lists. Cloning a document version is O(1), and old versions share
  structure. This gives cheap snapshots for undo, autosave in the background,
  and readers on other threads without locks.
- **Styles are shared and referenced** (`StyleId`), not copied per node.
  This mirrors DOCX/ODF style inheritance and keeps memory low.
- **Operation log**: `Op` is small, serializable and invertible. Undo means
  applying inverse ops. Grouping ("typing a word" counts as one undo step)
  is done with transactions. The op log is also the basis for autosave,
  crash recovery and future collaboration.

### 4.4 Command system (the backbone of the UX)

```rust
/// A user-invokable action. Everything the user can do is a Command.
pub trait Command: Send + Sync {
    fn id(&self) -> CommandId;                 // "text.bold"
    fn meta(&self) -> &CommandMeta;            // title, synonyms, icon, category
    fn is_available(&self, cx: &Context) -> Availability; // hidden/disabled/enabled
    fn run(&self, cx: &mut Context, args: Args) -> Result<Outcome>;
}
```

- The palette, floating toolbars, keymaps, menus and add-ons all read from
  the same registry. The contextual toolbar is
  `registry.available(&selection).top_n(by_relevance)`.
- Commands are data-described (metadata + localized strings), so a new
  feature shows up in search with no UI code.

### 4.5 Rendering pipeline

1. **Scene graph per frame of content** is built from the model and cached
   per node (a retained display list).
2. **Tiled rendering**: the canvas is split into 256×256 px tiles per zoom
   level bucket. Only tiles intersecting dirty bounds are re-rendered.
   Panning just composites cached tiles; new tiles render progressively,
   with a low-res placeholder taken from the previous zoom level.
3. **Vello** renders vector content into tiles on the GPU. There is a CPU
   fallback (`vello_cpu` / tiny-skia) for software-only systems and for
   headless tests.
4. **Text** glyphs go through a glyph atlas. Layout results are cached per
   paragraph and keyed by (content hash, width, style version).
5. **Raster layers** (paint) are stored as sparse 256² tiles, so only painted
   tiles use memory. Brush strokes are rendered incrementally.
6. The **frame budget** is measured every frame (see §7). Work that would
   miss the frame is deferred to the next frame or moved to a worker.

### 4.6 Threading model

- **UI thread:** input, iced view, submitting GPU work. It must never block.
- **Worker pool (rayon):** layout of off-screen pages, file parsing,
  export, thumbnail generation, search indexing, tile rasterization when
  using the CPU backend.
- **Add-on threads:** each WASM instance runs off the UI thread with fuel and
  epoch limits.
- Communication uses channels plus immutable model snapshots (§4.3). This
  avoids locks on the hot path.

---

## 5. Add-on system *(ADR-0006)*

There are two tiers, so that simple things stay simple:

1. **Packages (declarative, no code):** shape libraries (SVG + JSON
   metadata, connection points, resize rules), slide themes and layouts,
   document style sets, brush presets, and templates. They are distributed as
   a zip with a `package.toml` manifest. These are the "template packages"
   from the brief.
2. **Add-ons (code, WASM components):** written in any language that targets
   the WASM component model (Rust first). They use a versioned WIT world
   (`tafel:addon@1.x`) and can:
   - register commands (and therefore appear in the palette and toolbars),
   - register tools (canvas interactions),
   - register importers and exporters,
   - read the model and submit ops (through the same op API as the core),
   - contribute inspector panels using a small declarative UI schema (not
     raw iced, so the API stays stable even if the shell changes).
   - Capability-based permissions (filesystem, network) are declared in the
     manifest and granted by the user.

Native (dylib) plugins are deliberately **not** supported. Rust has no stable
ABI, and they would make the program crash-prone and insecure.

---

## 6. File formats *(ADR-0005)*

### 6.1 Native `.tafel`

A zip container with `manifest.json`, `model.bin` (postcard-serialized model
with a version header), `ops.log` (recent ops for recovery), `media/`,
`thumbnails/`, and `fonts/` (optional embedding). The format is versioned and
has migrations; it is documented in `docs/format/`.

### 6.2 Interchange

| Format | Read | Write | Crate                         | Milestone |
|--------|------|-------|-------------------------------|-----------|
| SVG/PNG/JPEG/WebP | ✓ | ✓ | `resvg`/`usvg`, `image`    | M2        |
| PDF    | (later) | ✓  | `krilla` (or ooxmlsdk-pdf)    | M4        |
| DOCX   | ✓    | ✓     | `ooxmlsdk` + mapping          | M4        |
| ODT    | ✓    | ✓     | `tafel-io-odf`                | M4        |
| PPTX   | ✓    | ✓     | `ooxmlsdk` + mapping          | M5        |
| ODP    | ✓    | ✓     | `tafel-io-odf`                | M5        |
| draw.io (`.drawio`) | ✓ | (later) | own, XML          | M2 stretch |
| VSDX   | (later) | –  | `ooxmlsdk` (OPC)              | post-1.0  |

### 6.3 Fidelity strategy

- **Preserve what we don't understand.** Unknown elements and attributes are
  kept as opaque XML on the nearest node and written back unchanged
  (`Unknown` node / `extras` bag).
- **Corpus-driven testing.** A test corpus (LibreOffice and Apache POI test
  files plus our own) runs on every PR: parse → write → re-parse must be
  stable, and rendering is compared against golden images.

---

## 7. Performance strategy

### 7.1 Budgets (enforced by benchmarks in CI)

| Metric                                     | Target           |
|--------------------------------------------|------------------|
| Input → pixels (typing, dragging)          | < 1 frame (8 ms @120 Hz, 16 ms @60 Hz) |
| Cold start to interactive empty board      | < 400 ms         |
| Open 300-page DOCX → first page visible    | < 500 ms; rest lays out in background |
| Pan/zoom board with 10 000 shapes          | steady 60 fps on integrated GPU |
| Idle CPU                                   | ~0 % (no redraw when nothing changes) |
| Memory, empty doc                          | < 150 MB RSS     |

### 7.2 Best practices adopted (React / GUI research → our design)

| Practice (where it comes from)                         | How we apply it |
|--------------------------------------------------------|-----------------|
| Avoid unnecessary re-renders (React.memo, `shouldComponentUpdate`) | Change sets with node ids. Views memoize on `(node_id, version)`. iced `lazy` widgets for panels. |
| Immutable data and structural sharing (Redux, Immer)   | Persistent model versions. Equality checks are pointer comparisons. |
| Virtualized lists and windowing (react-window)         | Only visible pages/slides/tiles are laid out and rendered. Thumbnails and long lists are virtualized. |
| Batching state updates (React 18 automatic batching)   | All ops from one event are coalesced into one transaction and one redraw. |
| Concurrent rendering and transitions (React `startTransition`) | Two priorities: *urgent* (cursor, drag) runs this frame; *deferred* (reflow of later pages, thumbnails, spell-check) yields to input. |
| Keep the main thread free (web workers)                | Parse, layout, export and search run in the rayon pool on immutable snapshots. |
| Debounce and throttle expensive reactions              | Autosave, search indexing and thumbnailing are debounced. Pointer-move events are coalesced to one per frame. |
| Code splitting and lazy loading                        | Format crates, add-ons and fonts load on first use. Startup only initializes the shell and the board. |
| Retained display lists and GPU compositing (browsers, Figma) | Per-node cached scenes, tile cache, and a compositor that pans without re-rasterizing. |
| Dirty-rect redraw (classic GUI toolkits)                | Tile invalidation from the bounds of changed nodes. No redraw at all when idle. |
| Measure, don't guess                                   | `tracing` spans everywhere, `tracy` integration behind a feature flag, `criterion` benches for layout, render and IO, and a frame-time HUD (`F12`). |

### 7.3 Profiling infrastructure (built in M0, not bolted on later)

- The `tracing` + `tracing-tracy` feature, a frame-time overlay, and
  benchmark fixtures (big DOCX, 10k-shape board, 100-slide deck).
- CI benchmark job that comments regressions > 10 % on PRs.

---

## 8. Code quality and maintainability

- **Documentation:** every crate has a crate-level doc (`//!`) that explains
  its responsibility and its boundaries. `#![warn(missing_docs)]` is set on
  public crates. Architecture docs live in `docs/`, and decisions are
  recorded as ADRs.
- **`CLAUDE.md` / `CONTRIBUTING.md`:** conventions, how to run, where things
  live, and how to add a command, format or add-on. This lets humans and
  Claude pick up work quickly.
- **Lints:** `clippy -D warnings` (pedantic selectively), `rustfmt`,
  `cargo deny` (licenses/advisories), `cargo machete` (unused deps).
- **Tests:** unit tests per crate, property tests (`proptest`) for ops and
  undo (applying an op and then its inverse must leave the model unchanged),
  round-trip corpus tests, golden-image render tests (CPU renderer, so they
  are deterministic), and `insta` snapshots for layout.
- **Errors:** `thiserror` in libraries, `anyhow` only in the binary. No
  `unwrap()` outside tests (clippy-enforced).
- **Git:** Conventional Commits (`feat(model): …`), one commit per
  module, refactor or decision. Commits are authored as Giosuel with Claude
  as co-author.
- **Licensing:** dependencies are MIT/Apache-compatible. The project license
  is still to be decided (see open questions).

---

## 9. Roadmap

Each milestone ends with a usable build and a tagged release. The estimates
are relative, not calendar promises.

| #  | Milestone                | Deliverable |
|----|--------------------------|-------------|
| M0 | **Foundations**          | Workspace, CI matrix (3 OS), lints, `tracing`, `tafel-model` (nodes, ops, undo, persistent versions), `tafel-commands` registry. No UI yet; fully unit-tested. |
| M1 | **Shell + canvas**       | iced window, infinite board, pan/zoom, tile renderer with Vello, rectangles/ellipses/lines, selection and transform handles, command palette, floating toolbar, hidden inspector, light/dark theme. Frame-time HUD. |
| M2 | **Diagramming**          | Connectors with orthogonal/curved routing, connection points, snapping and guides, groups, z-order, alignment and distribution, the declarative **package format** with basic, flowchart, UML and network shape libraries, `.tafel` save/load, autosave and recovery, SVG/PNG export. Stretch goal: `.drawio` import. |
| M3 | **Text engine**          | Rich text (runs, paragraphs, lists), Parley shaping, IME, bidi, text inside shapes, find/replace, spell-check hook. |
| M4 | **Document mode**        | Paginated flow layout (incremental and background), styles, headers/footers, tables, images, footnotes (basic), TOC. DOCX and ODT import/export, PDF export. Round-trip corpus in CI. |
| M5 | **Presentation mode**    | Slides as frames, masters and layouts, themes, presenter view (2nd window), transitions (basic), PPTX and ODP import/export. |
| M6 | **Layout mode (Publisher)** | Fixed pages, linked text frames (text flows across frames), master pages, bleed and crop marks in PDF export. |
| M7 | **Paint**                | Raster layers, brushes with pressure (tablet input), eraser, fill, selection, layer blending, PNG/JPEG/WebP. |
| M8 | **Add-on SDK**           | wasmtime host, WIT world v1, permissions, Rust SDK crate + template, docs, a sample add-on. (The internal extension points have existed since M0; this milestone opens them to third parties.) |
| M9 | **1.0 polish**           | Accessibility audit, i18n (Fluent), installers (MSI, DMG with notarization, Flatpak/AppImage), auto-update, crash reporting (opt-in), performance pass against §7.1. |
| –  | *Later*                  | Spreadsheets, collaboration (CRDT on the op log), VSDX import, PDF import. |

### First concrete steps (M0 breakdown)

1. Workspace skeleton with empty crates, crate docs and layering rules. *(commit)*
2. CI: fmt, clippy, test, deny on 3 OSes. *(commit)*
3. `tafel-model`: ids, node arena, persistent maps. *(commit)*
4. `tafel-model`: ops and transactions, undo/redo, plus proptests. *(commit)*
5. `tafel-commands`: registry, availability, fuzzy search index. *(commit)*
6. `tracing` setup and bench harness. *(commit)*
7. `CLAUDE.md` + `CONTRIBUTING.md`. *(commit)*

---

## 10. Risks and mitigations

| Risk | Mitigation |
|------|------------|
| Vello is still pre-1.0. GPU features (some blend modes, masks) are incomplete. | Hide it behind the `tafel-render` trait. Keep a CPU backend (vello_cpu/tiny-skia) that is always working. Pin versions. |
| iced's API changes between releases, and its wgpu version may not match Vello's. | Upgrade both in lock-step. The canvas is rendered by our own pipeline into a texture that iced only composites. Shell code is kept thin. |
| Rich-text editing (IME, bidi, complex scripts) is hard. | Use Parley (made for editors) and cosmic-text as a fallback, and write extensive tests with CJK, Arabic and emoji. |
| DOCX/PPTX fidelity is a bottomless pit. | Preserve unknown XML, test against a corpus, and prioritize features by how often they appear in real files. |
| ODF has no mature Rust crate. | Write our own crate early and keep it small: map only what our model supports, and preserve the rest. |
| Feature creep makes the UI cluttered. | Commands-first design. Any new UI surface needs a UX justification in its PR. |

---

## 11. Open questions for you

1. **Name.** Keep "Tafel" or choose another? (Crates are prefixed, so it's
   best to decide before M0 step 1.)
2. **License.** For example MIT/Apache-2.0 (permissive), GPL-3.0, or MPL-2.0.
   This also affects which add-on ecosystem we encourage.
3. **GUI framework.** I recommend iced (ADR-0002). The main alternatives are
   GPUI (Zed's framework: extremely fast, but less documented and less
   stable outside Zed) and Slint (polished, but its royalty-free license has
   conditions and it suits rich canvases less well). Do you want to confirm
   iced before M1?
4. **Priority order.** I put Board/Diagramming before Documents because it
   proves the renderer and UX early. Would you rather have documents first?
