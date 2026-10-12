# M2 UI stack: versions, choices and how the gate is measured

- Status: **Accepted** (2026-10-12). It informs M2.1–M2.6 (STATUS, M2
  checklist).
- Scope: the third-party crates under our toolkit (ADR-0007) and renderer
  (ADR-0003), the two points ADR-0007 left open, and how each gate
  criterion is measured. It doesn't revisit the stack itself: winit, wgpu,
  Vello, Parley, Taffy, AccessKit and our own signals are decided.
- Sources: two research passes on 2026-10-12. Versions and dates come from
  the crates.io sparse index, the dependency graph and licenses from a
  scratch build (`cargo tree -d`, `cargo metadata`, `cargo deny`), and the
  issue states from GitHub. Claims that are inferences say so.

## 1. Policy

1. **Two-week rule** (MAINTAINERS.md): a version is used only once it has
   been public for 14 days, transitive dependencies included.
2. **Pin when a crate is added**, at the newest version that is old enough
   at that time, not at the version in this document. Several of the
   crates below get new minor versions every month, and three become
   eligible within two weeks (section 2). Then run `cargo xtask dev-profile
   --write` and `cargo xtask dep-age`.
3. **One major of wgpu and winit** in the lockfile. Smaller duplicates
   (hashbrown, syn 2/3, windows-sys, objc2 0.5/0.6) are tolerated.

## 2. Versions

| Crate | Role | Eligible today | Next version (eligible from) | MSRV | License |
|---|---|---|---|---|---|
| `winit` | windows, input, IME | 0.30.13 | 0.31 is a beta and an API rewrite. Not before `accesskit_winit` supports it (AccessKit#656, open) | 1.70 | Apache-2.0 |
| `wgpu` | GPU device | 29.0.4 (with `vello_hybrid` 0.2) | 30.x with `vello_gpu` 0.3 (2026-10-16) | 1.87 | MIT OR Apache-2.0 |
| `vello_gpu` | **GPU renderer** (sparse strips) | `vello_hybrid` 0.2.0, its predecessor (0.1.0 of `vello_gpu` is an empty name reservation) | 0.3.0 (2026-10-16) | 1.89 | Apache-2.0 OR MIT |
| `vello_cpu` | **CPU renderer**: fallback, headless export, golden images | 0.2.0 | 0.3.0 (2026-10-16) | 1.89 | Apache-2.0 OR MIT |
| `parley` | text layout and the plain-text editor | 0.11.1 (with `fontique` 0.11.1, `skrifa` 0.44, `harfrust` 0.12) | 0.12.0 (2026-10-23), which splits out `parley_engine` | 1.88 | Apache-2.0 OR MIT |
| `taffy` | flex and grid layout | 0.14.0 | 0.15.0 (2026-10-26) | 1.71 | MIT |
| `accesskit` | accessibility tree | 0.25.1 | — | 1.87 | MIT OR Apache-2.0 |
| `accesskit_winit` | platform adapters (AT-SPI, UIA, NSAccessibility) | 0.34.1 (needs winit ^0.30.5) | — | 1.87 | Apache-2.0 |

All MSRVs are below our pinned Rust 1.99. A scratch build of the eligible
set resolved to 386 packages, all under licenses `deny.toml` allows
(`cargo deny check licenses bans`: ok). Pinning it to the two-week rule
took 20 `cargo update --precise` calls, `cc` 1.8.0 → 1.5.1 among them:
`cargo xtask dep-age` lists them when the crates are added.

**Not used:**
- **`vello`** (the compute-centric renderer). Since Vello's restructure in
  September 2026, its README calls it "the original compute-centric
  experimental implementation", kept under `research/`, while `vello_gpu`
  is "intended to become the primary renderer for production GPU use
  cases". The compute renderer also has a history of trouble on integrated
  GPUs (vello#813, #658: slow on Intel; #548: system hang on an 8 GB
  Apple M1). This is still "Vello" in the sense of ADR-0003.
- **Masonry** (see section 3.2).
- **tiny-skia**: `vello_cpu` covers the CPU path.
- **Parley's `accesskit` feature**: it needs accesskit ^0.24 and would
  duplicate it. We map text to AccessKit ourselves (inferred from the
  dependency entry, not built).

## 3. Choices

### 3.1 Renderer: Vello's sparse strips, on the CPU and the GPU

`vello_cpu` and `vello_gpu` share the sparse-strips architecture and the
scene types of `vello_common`. For us that means:

- **One scene-building path for both backends.** The `CpuBackend` and the
  `GpuBackend` of ADR-0003 take the same input. Golden images made on the
  CPU test the same scene code the GPU runs.
- **Our tile cache fits.** `vello_gpu` preprocesses paths on the CPU and
  rasterizes in an ordinary render pipeline without compute shaders.
  Rendering into our own 256² tile textures is plain render targets
  (inferred from the architecture; to be confirmed in M2.2). Integrated
  GPUs and lavapipe handle that kind of pipeline better than compute-heavy
  ones.
- **Deterministic golden images:** `vello_cpu` with its `f32` pipeline
  (the setting its README recommends for snapshots). Masonry relies on it
  for exact screenshot tests. Open question for M2.2: whether x86 and
  aarch64 SIMD paths give identical bytes (vello_cpu#1999, some grays blend
  to black, is a known bug).
- Known `vello_gpu` costs: non-default blend modes cost about 50 µs of GPU
  time per draw (vello#1990). The gate's 10k shapes use normal blending.

Order: M2.2 starts with the CPU backend, the display list and the tile
cache, which need no GPU. By the time the GPU backend follows, `vello_gpu`
0.3 and wgpu 30 are eligible, so it starts there directly.

### 3.2 Masonry: a reference, not a dependency

ADR-0007 allows "a reference, or a dependency if it is stable enough". It
isn't:

- The last release, 0.4.0 (2025-10-29), is a year old. It pins vello 0.6
  (wgpu 26), parley 0.6, accesskit 0.21 and accesskit_winit 0.29: next to
  our set, that duplicates wgpu, Vello, Parley, AccessKit and skrifa. Even
  its `main` branch lags by three to six minor versions. Its README calls
  it experimental.
- Its update model is Xilem's tree mutation, not fine-grained signals, and
  its paint pipeline owns the scene, while ours shares one scene per frame
  with the canvas tile cache. It has its own flex and grid layout instead
  of Taffy.

Worth studying, and copying where it fits (Apache-2.0, which is compatible
with AGPL; keep the attribution in the file):
- `masonry_testing`: a headless harness that simulates input, text,
  accessibility events and time, with `vello_cpu` snapshots compressed by
  oxipng and blessed by an environment variable;
- the `text_area` widget: IME preedit and its AccessKit mapping;
- the winit event translation in `masonry_winit`.

### 3.3 The text field: Parley's `PlainEditor`

Parley 0.11.1 has `PlainEditor` and `PlainEditorDriver`, with IME
composition (`set_compose`, `finish_compose`, cursor hiding while
composing). Parley's `examples/editor` combines it with winit IME and
AccessKit. Our M2 text field builds on it rather than on our own editing
code. Rich text editing is M5.

### 3.4 winit 0.30

0.31 has a better IME API (surrounding text, native popups), but it's a
beta, and AccessKit's winit adapter doesn't support it yet. 0.30.13
includes the IME fixes we'd otherwise miss: the macOS Pinyin
`set_marked_text` crash, Windows `WM_IME_SETCONTEXT`, the Windows preedit
cursor offset. Moving to 0.31 is a later, separate migration.

### 3.5 The gate app is its own binary

`imaginarium-gate` (apps layer, never shipped). It needs the test
fixtures (`imaginarium-test-utils`), which the product app must not
depend on, and it may later become the harness for UI budget benchmarks.
It's added to the layer table and the PLAN crate map in M2.5.

## 4. Platform risks for the gate

The gate machine is Linux with Hyprland (Wayland), an Intel UHD integrated
GPU and an RTX 3050 (STATUS). Windows and macOS are checked later
(ADR-0007, "Procedure").

| Area | Linux (gate now) | Windows, macOS (later) |
|---|---|---|
| IME | Wayland: text-input-v3 implemented. X11: XIM only, Fcitx5 key-release leak (winit#4525) | Windows: IMM32 only, no TSF; candidate window placement (#2780). macOS: selected and replacement ranges ignored, which breaks Zhuyin (#3617) |
| Mixed DPI | Wayland fractional scaling implemented | Windows 11 24H2: windows grow without bound when dragged between monitors (#4600, fix PR #4694 not merged), `ScaleFactorChanged` size ignored (#4721). We may need a patched winit |
| Idle CPU | Wayland: event loop spinning at 100 % with a zero timeout (#4668, open) | macOS: `ControlFlow::Wait` misbehaves when the pointer leaves (#3934) |
| Screen reader | AT-SPI adapter; `EditableText` only implements `set_text_contents` | UIA and NSAccessibility adapters exist. AccessKit has no concept of IME composition anywhere |

All issues above were open on 2026-10-12.

## 5. Measuring the six criteria

The gate binary runs scripted scenarios and writes `gate-report.json`. The
report holds the OS, compositor, GPU adapter (`wgpu::AdapterInfo`, run once
on the integrated GPU and once on the discrete one), scale factors,
monitors and the build hash, and per criterion the raw numbers and pass
or fail. The owner sends the report and a manual checklist.

| # | Criterion | How | Here (no GPU) | Gate machine |
|---|---|---|---|---|
| 1 | 10k shapes, ≥ 60 fps, p99 < 16 ms | A scripted 10 s pan, zoom and fling over `BoardFixture::LARGE` with vsync. Frame times (present to present) and CPU frame spans via `imaginarium-profiling`. Report p50, p95, p99 and missed vsyncs | functional only (lavapipe) | ✓ |
| 2 | Only dirty tiles re-render | Counters per frame (`tiles_rendered`, `tiles_composited`, the expected dirty set from the shape's old and new bounds) asserted during a scripted drag and pan. A Tracy capture as a manual cross-check | ✓ (in CI too) | ✓ |
| 3 | Palette < 1 ms per keystroke | Replay recorded keystroke sequences over 2,000 commands, timing the search *and* the list update through layout. p99 | ✓ | ✓ |
| 4 | IME and screen reader | Manual checklist: Fcitx5 (Pinyin, Mozc) on Wayland; Orca reads the field, the typed text, the selection and caret moves. Automated part: an AT-SPI tree test under Xvfb and D-Bus (roles, value, selection) | AT-SPI test | manual |
| 5 | HiDPI and mixed DPI | Automated: golden images at forced scale factors (1.0, 1.25, 1.5, 2.0). Manual: drag between monitors with different scales, `ScaleFactorChanged` and surface sizes logged to the report | forced scales | needs a second monitor |
| 6 | Idle ≈ 0 % CPU, cold start < 400 ms | Idle: after 5 s, process CPU time over 30 s (`getrusage`), and zero frames presented. Cold start: process creation (`/proc/self/stat`) to the first presented frame, median of 10 runs | idle (X11 only) | ✓ |

Thresholds not stated by ADR-0007, chosen here: missed vsyncs ≤ 1 %, idle
below 0.5 % of one core.

## 6. Build cost (measured)

Clean builds of the eligible set (winit, wgpu, vello, vello_cpu, parley,
taffy, accesskit, accesskit_winit) with our profiles, on the 8-vCPU VM
(load 5–18 from other users):

| Build | Wall | CPU-s | CPU | Peak RSS |
|---|---|---|---|---|
| debug | 5 min 30 s | 1,477 | 447 % | 873 MiB |
| release (thin LTO, 1 codegen unit) | 3 min 29 s | 821 | 392 % | 928 MiB |

- The most expensive crates: naga (260 s), x11rb-protocol, zbus,
  wgpu-core, zvariant_utils, read-fonts, zbus_macros (74–105 s each). The
  wgpu/naga stack takes about 430 s, AccessKit's Linux stack (zbus, atspi)
  about 320 s.
- Debug costs more CPU than release because the new proc-macro crates
  (zbus_macros, zerofrom-derive, yoke-derive, synstructure, …) aren't in
  the ADR-0017 opt-level-0 list yet. `cargo xtask dev-profile --write`
  adds them when the crates come in (inferred, measured again then).
- The machine is **not** the bottleneck: the cores were shared with other
  users rather than saturated, and memory stays under 1 GiB. Compare
  CPU-seconds, not wall time.
