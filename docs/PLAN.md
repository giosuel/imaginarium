# Imaginarium: Project Plan

Status: **v2** (2026-10-12). This replaces v1. The reasoning behind each
decision is in [`docs/adr/`](adr/), and detailed designs are in
[`docs/design/`](design/).

---

## 1. Vision

**Imaginarium** is one fast, native application where documents,
presentations, page layouts, diagrams and drawings live on one
whiteboard-like surface. Generative AI is built in, using local or API
models, and teams collaborate in real time through an end-to-end encrypted,
self-hostable hub.

### Pillars

1. **One surface, one model.** Board, Document, Deck, Layout and Paint are
   views of a single CRDT document model. Anything can go anywhere.
2. **Performance as a feature.** We own the UI toolkit, renderer and text
   stack. Updates are fine-grained, and nothing redraws while idle.
3. **Calm UX.** The UI is empty by default. Everything is a searchable
   command, and controls appear in context. Every setting is in the UI.
4. **AI-native, provider-agnostic.** Text, images and an assistant that
   edits through the same commands as the user. Users choose local models
   (Qwen, Mistral, Flux 2, SD) or APIs (Claude, ChatGPT, …).
5. **Private collaboration.** Real-time editing, sharing, permissions and
   comments, while the server can read nothing.
6. **Open.** AGPL-3.0-or-later, an open and documented file format, an add-on
   system, and docs for users and developers.

### Business model

The whole platform is open source (AGPL). Revenue comes from **subscription
hosting of the hub**: storage, sync, uptime and backups. E2EE means the
hosted offering never sees customer data, which is a selling point. AGPL
means any modified hosted competitor must publish its changes.

### Non-goals (for now)

Spreadsheets (the model reserves room for them), a web editor (a WASM web
*viewer* is possible later), and mobile apps.

---

## 2. UX principles

1. **Empty by default:** the canvas and a small floating pill (mode, zoom,
   share, AI). There is no ribbon and no menu bar (macOS gets a native menu
   for platform conventions only).
2. **Contextual toolbars** appear next to the selection and are generated
   from the commands available for it.
3. **Command palette (`Ctrl/Cmd+K`)** is the entry point for everything:
   commands, settings, templates, AI actions. It uses fuzzy search, synonyms
   and recency, and shows shortcuts.
4. **Quick insert:** type `/` in text, or double-click the canvas.
5. **Inspector** for advanced properties. It is hidden until requested
   (`Ctrl+I`) and slides over the canvas.
6. **AI in context:**
   - an assistant panel (`Ctrl+J`),
   - inline "✦" actions on selections (rewrite, summarize, generate image,
     remove background, …),
   - AI suggestions shown as a reviewable diff (accept or reject).
7. **All configuration in the UI.** Settings are searchable, generated from a
   typed registry (ADR-0014), and include server administration.
8. **Nothing blocks.** IO, AI, export, sync and crypto all run in the
   background with inline progress.
9. **Accessible and keyboard-complete.** AccessKit from day one, plus
   high-contrast and reduced-motion themes.
10. **Library drawer** for template packages, shape libraries, themes, brushes
    and AI workflow templates.

---

## 3. Technology overview

| Concern | Choice | ADR |
|---|---|---|
| Language / build | Rust stable, Cargo workspace, pinned toolchain | 0001 |
| UI toolkit | **Own**: winit + wgpu + Vello + Parley + Taffy + AccessKit; fine-grained signals | 0007 |
| Rendering | Vello (GPU) + CPU fallback, tile cache, single scene for panels and canvas | 0003 |
| Document model | **Loro CRDT** + materialized `imbl` view model | 0009 |
| Native format | `.imag`: encrypted, append-only object pack | 0010, [design](design/file-format.md) |
| Interchange | DOCX/PPTX via `ooxmlsdk`, ODT/ODP via our own crate, PDF/SVG/PNG export | 0005 |
| Security | XChaCha20-Poly1305, BLAKE3, hybrid PQ (X-Wing, Ed25519 + ML-DSA), Argon2id, OPAQUE login | 0011, 0016 |
| Collaboration | `imaginarium-hub`: axum + SQLite/Postgres + FS/S3, zero-knowledge relay | 0012 |
| AI | Capability traits + provider adapters (API, Ollama/llama.cpp, ComfyUI, sd.cpp, ONNX), MCP | 0013 |
| Add-ons | WASM components (wasmtime, WIT) + declarative packages; AGPL-compatible only | 0006, 0008 |
| Settings | Typed registry → generated UI | 0014 |
| Distribution / docs | cargo-dist installers, Docker hub image, Astro Starlight website | 0015 |
| License | AGPL-3.0-or-later | 0008 |

---

## 4. Architecture

### 4.1 Crate map

Dependencies point **downwards only**. Crates below the line know nothing
about UI, networking or AI.

```
 apps        imaginarium-app (desktop)          imaginarium-hub (server)       imag (CLI)
 ─────────────────────────────────────────────────────────────────────────────────────────
 product     shell · canvas · ai-ui · admin-ui
 features    diagram · layout · paint · io-ooxml · io-odf · io-export · packages
 services    ai · ai-providers · ai-local · mcp · sync · addon-host · settings
 toolkit     ui · reactive · render · text
 core        model (Loro + view model) · commands · format · crypto · protocol
 support     profiling · test-utils · addon-api (public, semver-stable)
 tooling     xtask (repo automation, never a dependency of product crates)
```

| Crate | Responsibility |
|---|---|
| `imaginarium-format` | `.imag` container: records, index, append/compaction, recovery. Standalone, fuzzed. |
| `imaginarium-crypto` | Suites, key hierarchy, AEAD/sign/HPKE wrappers, keyring, ACL chain verification |
| `imaginarium-model` | Schema on Loro, view model, transactions, ChangeSets, undo, migrations |
| `imaginarium-commands` | Command registry, argument schemas (also used as AI tools), keymaps, search index |
| `imaginarium-settings` | Typed settings registry, scopes, persistence |
| `imaginarium-protocol` | Wire types shared by client and hub (versioned) |
| `imaginarium-sync` | Client sync engine: object upload/download, live frames, presence, offline queue |
| `imaginarium-reactive` / `-ui` | Signals; widget tree, input, IME, focus, layout, a11y |
| `imaginarium-render` / `-text` | Scene building, tiles, compositor; rich text layout and editing |
| `imaginarium-ai*` / `-mcp` | Capabilities, jobs, tool bridge; provider adapters; local ONNX models; MCP server and client |
| `imaginarium-hub` | Accounts (OPAQUE), workspaces, ACL enforcement, object store, WebSocket fan-out, admin API |

### 4.2 Data flow

```
user input ─┐
AI agent ───┼─► command ─► CRDT transaction ─► Loro doc ─► diff events ─► view model
remote peer ┘                    │                                         │ ChangeSet
                                 ▼                                         ▼
                     encrypt+sign UPDT record                 signals mark dirty widgets/nodes
                       ├─► append to local .imag              ▼
                       └─► hub (WebSocket) ─► peers    incremental layout ─► dirty tiles ─► one Vello scene ─► GPU
```

The user, the AI assistant, add-ons and remote collaborators all go through
the **same path**. There is exactly one way to change a document.

### 4.3 Threading and runtimes

| Thread / pool | Work |
|---|---|
| **UI thread** | Input, signals, widget layout, scene submission. Never blocks. |
| **Worker pool** (rayon) | Off-screen layout, parsing, export, crypto, compression, thumbnails, search indexing |
| **Async runtime** (tokio, dedicated threads) | Hub sync, AI provider calls, MCP, downloads. Kept out of the core crates. |
| **Add-on sandboxes** (wasmtime) | Add-on code with fuel and epoch limits |

Communication uses channels and immutable snapshots, with no locks on the
hot path.

### 4.4 Command system

```rust
/// A user-invokable action. The palette, toolbars, keymaps, add-ons and the
/// AI assistant all go through this single registry.
pub trait Command: Send + Sync {
    fn id(&self) -> CommandId;                    // "text.bold"
    fn meta(&self) -> &CommandMeta;               // title, description, synonyms, icon
    fn args_schema(&self) -> &ArgsSchema;         // typed args (→ JSON Schema for AI tools)
    fn is_available(&self, cx: &Context) -> Availability;
    fn run(&self, cx: &mut Context, args: Args) -> Result<Outcome>;
}
```

---

## 5. File format

This was designed first, because it is the hardest decision to change
later. Full design: **[docs/design/file-format.md](design/file-format.md)**.

In short:

- **Logical layer:** a Loro CRDT document with a documented schema (node
  tree, rich text, styles, assets, pages, AI provenance, namespaced
  extensions). Comments live in a separate CRDT stream.
- **Object layer:** immutable typed objects (`SNAP`, `UPDT`, `BLOB`, `KEYR`,
  `ACLC`, `VERS`, …). Blobs are content-addressed with keyed BLAKE3 and
  segmented into 256 KiB pieces.
- **Container layer:** append-only records (each with its own zstd
  compression, encryption and CRC), with an index and trailer at the end.
  Incremental saves, compaction and crash recovery.
- **Same bytes locally and on the hub.** Sync is object exchange, and the hub
  never holds a key.
- **Extensibility:** PNG-style CRITICAL/SAFE_TO_COPY flags, required-feature
  list, protobuf field evolution, crypto-suite ids, and a codec id for the
  CRDT.

---

## 6. Collaboration: Imaginarium Hub

- **Self-hostable** single binary or Docker image (SQLite + filesystem by
  default; Postgres + S3 for scale). The **hosted subscription** runs the
  same code.
- **Accounts:** email + OPAQUE password login (the server never sees the
  password), a recovery key, multiple devices, and optional TOTP.
- **Workspaces, folders and files:** names and content are encrypted. Quotas,
  trash and version history (named versions + time travel).
- **Sharing:** with users, teams, or links (the key lives in the URL
  fragment; optional password and expiry). Roles are
  owner/admin/editor/commenter/viewer.
- **Real-time:** WebSocket live update frames (~50 ms batches), presence
  (cursors, selections, viewport follow), and offline editing with merge on
  reconnect.
- **Comments:** threads anchored to nodes or text ranges, @mentions,
  resolve/reopen, and content-free notifications.
- **Admin:** done from the desktop client. The hub prints a one-time setup
  code on first start.
- The sync protocol spec (`docs/design/sync-protocol.md`) is written together
  with the format in M1.

## 7. Security model

The full decision is in ADR-0011, and the cryptographic details are in
§6 of the format design. In summary:

- **E2EE for everything:** content, names, comments, thumbnails, presence.
  TLS 1.3 on top.
- **Keys:**
  - per-document epoch keys, wrapped for members via HPKE
    (re-keyed when membership changes),
  - device keys signed by the user identity,
  - signed edits (the server checks writers without reading; clients
    re-verify).
- **What the hub can see** (accounts, sharing graph, sizes, timing) is
  documented openly in the security whitepaper.
- **Performance:** fast primitives (≥1 GB/s per core), lazy and parallel
  decryption, batching, never on the UI thread. Benchmarked in M1.
- An **independent audit** happens before the hosted service launches.

## 8. Generative AI

Full decision: ADR-0013.

- **Text:** generate, rewrite, tone, translate, summarize, and outline →
  document/slides.
- **Board/vector:** describe → diagram, sketch → clean shapes, auto-layout,
  explain or critique a board, AI-generated SVG and vector edits.
- **Image suite:** txt2img, img2img, multi-reference images, sketch → image,
  inpaint/outpaint, background removal, click-to-segment masks, style
  transfer, upscaling, object removal, variations, instruction-based edits.
  All non-destructive, with stored recipes.
- **Assistant:** chat with tool use over our command registry, a vision
  view of the canvas, and suggestions as a reviewable diff. MCP server and
  client.
- **Providers:**
  - API: Claude, OpenAI, Gemini, Mistral, BFL, OpenAI-compatible
  - local: Ollama, llama.cpp, LM Studio, vLLM, ComfyUI, stable-diffusion.cpp
  - built-in ONNX models
- **Privacy:** per-document AI policy (off / local only / any), a clear
  indicator when content goes to the cloud, keys in the OS keychain, and the
  hub never involved.

## 9. Add-ons and packages

- **Packages** (no code): shape libraries, templates, themes, styles,
  brushes, AI prompt and workflow templates. Content licenses are declared.
- **Add-ons** (WASM components, `imaginarium:addon@1` WIT):
  - commands, tools, importers/exporters, AI providers, settings, and
    declarative panels,
  - capability-based permissions,
  - AGPL-compatible licenses required (ADR-0008).
- First-party features use the same registries, so the API gets exercised
  constantly.

## 10. Performance strategy

### Budgets (enforced by CI benchmarks)

| Metric | Target |
|---|---|
| Input to pixels (typing, dragging) | < 1 frame (8 ms at 120 Hz) |
| Cold start to interactive empty board | < 400 ms |
| Open a 300-page document: first page visible | < 500 ms; the rest lays out in the background |
| 10,000-shape board pan/zoom | 60 fps or more on an integrated GPU |
| Applying a remote update (typical) | < 1 ms including decrypt + verify |
| Idle CPU | ~0 % |

### Practices we adopt from React and GUI research

| Practice | How we apply it |
|---|---|
| Fine-grained reactivity (SolidJS/Leptos) instead of re-rendering a virtual DOM | Signals update only the widgets that read changed data. ChangeSets from the model feed the signals. |
| Immutable data with structural sharing | `imbl` view-model versions; equality is a pointer comparison |
| Virtualization / windowing | Only visible pages, slides, tiles and list rows are laid out and rendered |
| Batched updates | One transaction and one redraw per event or frame. Pointer moves are coalesced. |
| Concurrent priorities (`startTransition`) | Urgent work (cursor, drag) runs this frame. Deferred work (reflow, thumbnails, spellcheck, AI previews) yields. |
| Keep the main thread free | Workers and an async runtime, fed with immutable snapshots |
| Lazy loading and code splitting | Formats, AI providers, add-ons, fonts and blobs load on first use |
| Retained display lists, GPU compositing, dirty rectangles | Per-node scene cache, 256² tile cache, a compositor that pans without re-rasterizing |
| Measure everything | `tracing` + tracy, frame-time HUD (`F12`), criterion benchmarks, CI regression gates |

**Profiling comes first:** `imaginarium-profiling` and the benchmark fixtures
are built in M0, before any feature work.

## 11. Code quality and maintenance

- Each crate has a crate-level doc describing its responsibility and
  boundaries. `#![warn(missing_docs)]` is on, and examples are written as
  doctests.
- `MAINTAINERS.md` and `CONTRIBUTING.md` describe conventions, the crate map, and
  "how to add a command, setting, format, AI provider or add-on".
- Lints and checks: clippy `-D warnings`, rustfmt, `cargo deny` (AGPL-compatible
  license allowlist and advisories), `cargo machete`, and a layering check.
- Tests:
  - unit tests,
  - proptests (CRDT convergence, undo, format round-trip),
  - **fuzzing** of the format parser and protocol,
  - golden-image render tests (CPU backend),
  - DOCX/ODT/PPTX/ODP corpus round-trips,
  - multi-client sync simulation tests.
- Error handling: `thiserror` in libraries, `anyhow` only in binaries, and no
  `unwrap()` outside tests.
- Git: Conventional Commits, one commit per module, refactor or decision.
  Author Giosuel, co-author Claude. DCO sign-off for external contributors.

## 12. Roadmap

Each milestone ends with a tagged, usable build **and its docs**.

### Phase A: Foundations

| # | Milestone | Content |
|---|---|---|
| M0 | **Foundations and profiling** | Workspace, CI on 3 OSes, `cargo deny`, `imaginarium-profiling` + benchmark fixtures, `settings` registry, `commands` registry, website skeleton, `MAINTAINERS.md`/`CONTRIBUTING.md` |
| M1 | **Format, crypto, model** | `format` (records, index, append, compaction, recovery) + spec v1 + test vectors + fuzzing; `crypto` (suite 1, keyring, ACL chain); `model` on Loro (schema, view model, undo, migrations) with a Loro/Yrs/Automerge benchmark; sync-protocol design doc; `imag` CLI |
| M2 | **UI prototype gate** | `reactive` + minimal `ui` + tile renderer, a stress-test app, and the 6 pass criteria of ADR-0007 (GPUI fallback if it fails) |

### Phase B: Core editor

| # | Milestone | Content |
|---|---|---|
| M3 | Shell and canvas | Palette, contextual toolbar, inspector, settings UI, basic shapes, selection/transform, themes, local `.imag` save/load |
| M4 | Diagramming | Connectors and routing, snapping, groups, alignment, package format, shape libraries, SVG/PNG/PDF export, draw.io import (stretch goal) |
| M5 | Text engine | Rich text, IME, bidi, text in shapes, find/replace, spellcheck hook |
| M6 | **AI foundation** | Capability layer, providers (API + Ollama/llama.cpp + ComfyUI), AI settings UI, assistant with command tools and diff review, text generation, txt2img, ONNX background removal, MCP server |

### Phase C: Office modes and collaboration

| # | Milestone | Content |
|---|---|---|
| M7 | Documents | Pagination, styles, tables, headers/footers, footnotes, TOC; DOCX + ODT import/export; PDF |
| M8 | **Collaboration (Hub)** | Hub server, OPAQUE accounts, devices, E2EE sharing, roles, real-time sync and presence, comments, version history, admin UI, Docker |
| M9 | Presentations | Masters/layouts, themes, presenter view, transitions; PPTX + ODP |
| M10 | Layout (Publisher) | Fixed pages, linked text frames, master pages, bleed and crop marks |
| M11 | **Paint + AI image suite** | Raster layers and masks, pressure brushes; img2img, references, sketch-to-image, inpaint/outpaint, segmentation, style, upscaling, variations |

### Phase D: Ecosystem and 1.0

| # | Milestone | Content |
|---|---|---|
| M12 | Add-on SDK | wasmtime host, WIT v1, Rust SDK, templates, package library |
| M13 | 1.0 | Accessibility audit, i18n (Fluent), installers and auto-update, complete website, **external security audit** |
| — | Hosted service launch | Billing, multi-tenant operations, status page, backups |
| — | Later | Spreadsheets, WASM web viewer, mobile viewer, PDF import, VSDX import, post-quantum suite by default |

## 13. Risks

| Risk | Mitigation |
|---|---|
| The scope is very large | Strict milestones, each shippable. Shared model and commands avoid building features twice. |
| Building our own UI toolkit costs too much | M2 gate with measurable criteria, and GPUI as the fallback |
| Loro dependency or encoding changes | CRDT codec id in the format, benchmark in M1, upstream is stable since 1.0 |
| E2EE + CRDT + permissions complexity | Design docs first, proptests and simulation tests, external audit, and reliance on proven designs (HPKE, signed ACL chain) |
| Users lose keys | Mandatory recovery key, multi-device, clear UX, plain local files by default |
| AI providers change quickly | Capability traits + adapters, ComfyUI workflows as data, providers as add-ons |
| DOCX/PPTX fidelity | Preserve unknown XML, corpus testing, prioritize features by how often they appear |
| Vello is pre-1.0 | CPU backend always working, pinned versions, upstream contact |

## 14. Decisions log and open questions

Decided (2026-10-12):
- Local files are plain by default, with an optional password or account
  keys. History keeps 90 days plus named versions. Thumbnails are on.
  Hybrid post-quantum crypto from the first release (ADR-0016).
- Phase C order is confirmed: Documents → Collaboration → Presentations.
- Contributions use the **DCO** (`Signed-off-by`), not a CLA (ADR-0008). If
  dual licensing or App Store distribution is wanted later, a CLA can be
  introduced before external contributions grow. Earlier contributions
  would then need their authors' consent.

Open:
1. **Hub name.** "Imaginarium Hub" is the working name.

The current progress and the next concrete steps are tracked in
[`docs/STATUS.md`](STATUS.md).
