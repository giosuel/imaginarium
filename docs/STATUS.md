# Status: hand-off log

> **Fresh session?** Read [`MAINTAINERS.md`](../MAINTAINERS.md) first, then this file.
> Continue at **"Next step"**. Update this file at the end of every work
> step and commit it.

Last updated: 2026-10-17

## Current position

- **Phase:** **Phase A, M3 (shell and canvas)**. M0, M1 and M2 are done
  (tags `v0.0.1-m0`, `v0.0.2-m1`, `v0.0.3-m2`). M2 closed on 2026-10-16
  without its manual checklist, which the owner runs later (Known issues).
- **Next step:** **M3.5, the Settings UI** (below): the window generated
  from the registry, and saving the settings to a file (Show at startup
  lasts only for the session until then). M3.4 is done (2026-10-17), in
  six parts, each reviewed by a fresh agent and stacked: the board window
  (https://github.com/giosuel/imaginarium/pull/56), the command palette
  (https://github.com/giosuel/imaginarium/pull/57), the contextual
  toolbar and the context menu
  (https://github.com/giosuel/imaginarium/pull/58), the inspector
  (https://github.com/giosuel/imaginarium/pull/59), and the start window
  with one window per document (part 6).
  M3.3 is done (2026-10-17): the board's commands, tools and
  clipboard, with shortcuts from a keymap. M3.2 is done (2026-10-16/17):
  `imaginarium-canvas` shows and edits a board. Each part was reviewed by
  a fresh agent; the PRs are stacked on M3.1's
  (https://github.com/giosuel/imaginarium/pull/44), M3.2's on
  https://github.com/giosuel/imaginarium/pull/45 to /52, M3.3's after.
  - **The component gallery** (owner, 2026-10-16): every component in its
    states, dark and light, before the shell uses it
    (`crates/imaginarium-ui/examples/gallery.rs`). New components get a
    section there, and their screenshots go to the owner.
  - M3.0 is done: the mockups (owner's design canvas, not committed),
    `shell.md`, `canvas.md`, ADR-0020 and ADR-0021 (accepted), schema
    §5.6 (#37).
- **Code status:** Cargo workspace with 18 crates. Working: `xtask` (repo
  checks and dev-profile generator), `imaginarium-profiling` (tracing,
  Tracy, frame stats), `imaginarium-test-utils` (fixtures),
  `imaginarium-settings` (typed registry, TOML), `imaginarium-commands`
  (registry, args/JSON Schema, keymaps, palette search),
  `imaginarium-format` (the `.imag` container), `imaginarium-crypto`
  (encryption, keyrings, signatures, ACL chain) and `imaginarium-model`
  (the document on Loro, view model, undo), `imaginarium-io-native`
  (`.imag` files as documents), `imaginarium-cli` (`imag inspect`,
  `verify`, `export-json`), `imaginarium-reactive` (signals, memos,
  effects, scopes), `imaginarium-render` (scenes on `vello_gpu` and
  `vello_cpu`, glyphs), `imaginarium-text` (Parley: fonts, layout,
  editing), `imaginarium-ui` (window, input, IME, text field, AccessKit,
  frame pipeline, the widget tree on Taffy with signals, widgets,
  clipboard, layers, scrolling, icons, animation), `imaginarium-canvas`
  (the board: a document's nodes on the tile canvas, selection, handles,
  text in place, the board's commands), `imaginarium-shell` (the board
  window with its panels, palette, contextual toolbar, context menu and
  inspector; the start window), `imaginarium-app` (the start window,
  one window per board) and `imaginarium-gate` (the M2 gate app: a board with text,
  search, in-place editing, a command palette, measurements,
  `gate-report.json`). The app only prints its version.
- **Remote:** <https://github.com/giosuel/imaginarium> (public). All work
  goes through PRs, which the owner merges (see MAINTAINERS.md, "Pull
  request workflow").
- **Environment seen in the planning session:** Linux, Rust 1.99.0 stable,
  Node 22 / npm 10 (for the website). `cargo-deny` 0.20.2 and
  `cargo-machete` 0.9.2 are installed in `~/.cargo/bin` (official release
  binaries, SHA-256 verified). CI pins the same versions.
  The pinned toolchain `1.99.0` (with rustfmt and clippy) was auto-installed
  by rustup next to `stable`.

## Development machine (measured 2026-10-12)

The owner wants to know whether this machine is a bottleneck (see
MAINTAINERS.md, "Reporting to the owner"). Known facts:

- **KVM VM**: 8 vCPUs (Haswell, AVX2, no AVX-512), 22 GiB RAM, **no swap**,
  emulated disk with about 34 GB free. `/tmp` is a **12 GB RAM-backed tmpfs
  shared with other sessions**. **Never put build `target` dirs in `/tmp`**:
  it filled up once and killed a build. Use subdirectories of the repo's
  `target/` for scratch builds.
- **Shared and busy**: other users' Rust builds, a game server and other
  Claude sessions often push the load to 9–16 on 8 cores. Wall-clock
  numbers vary a lot, so compare CPU-seconds.
- **No GPU** (emulated Cirrus VGA). Screenshots work with Xvfb plus Mesa
  lavapipe (software Vulkan). GPU performance gates (M2) **can't** be
  judged here; they need real hardware.
- **Screenshots** (owner rule): Playwright 1.63.0 with headless Chromium
  is installed in `target/screenshot-tool/` (git-ignored, outside the
  product).
  - If `target/` was cleaned, recreate it with `npm install --before=<date>
    --save-exact playwright@1.63.0` and
    `PLAYWRIGHT_BROWSERS_PATH=$PWD/browsers npx playwright install
    chromium --only-shell`.
  - For the website: `npx astro preview --port 4329` in `website/`, then
    drive Playwright against `http://localhost:4329/imaginarium/`.
  - For the native app (later): run it under `xvfb-run` with Mesa
    lavapipe.
- **The root disk can fill up without warning**: on 2026-10-12 at ~16:35
  other users' activity left 250 MB free (the disk is 193 GB, ~13 GB of it
  ours: `target/` ~8 GB, `~/.rustup` 3.4 GB, `~/.cargo` 1.7 GB). Writes
  failed with `ENOSPC`. It recovered within minutes (91 GB free). If it
  happens again, `target/debug/incremental` (~2 GB) is the safe thing of
  ours to delete. Check `df -h /` before big builds.
  **If it happens again, diagnose it in the background (owner request)**,
  read-only: sample `df` every 10 s for ~10 min, list recently started
  processes of all users (`ps -eo user,pid,etimes,args --sort=etimes`),
  find large recent files in readable places (`find / -xdev -size +200M
  -mmin -120`), and check our own deleted-but-open files (`lsof +L1`).
  Then report when, how much, and the likely cause. Never touch other
  users' files.
- **`/tmp` filled up too** (2026-10-12, ~20:20): 12 GB used, 76 MB free.
  The read-only check found 9.3 GB in another Claude session's scratchpad
  (project `somedesk`, same Unix user): two Cargo `target/` dirs, a
  Playwright Chromium and the Android command-line tools. It didn't grow
  over the minute watched. Ours was 25 MB. Our tests must not use
  `std::env::temp_dir()` (one format test did; it now uses
  `target/test-scratch`).
- The linker is the default `rust-lld`. `clang` is present, `mold`/`sccache`
  are not.
- **Baseline**: clean build of the planned heavy stack (Vello/wgpu, winit,
  Parley, Taffy, AccessKit, Loro, crypto) with our profiles:
  - debug: 8 min 43 s wall, 2,355 CPU-s, CPU-bound, peak 2.1 GB per process;
  - release: 5 min 7 s wall, 985 CPU-s.

  The most expensive crates are loro-internal, naga, wgpu-core,
  x11rb-protocol, syn, accesskit_unix and the zstd-sys C build.
  - With line-tables-only deps (PR #4): 2,099 CPU-s, peak 1.39 GB,
    `target/` 2.3 GB.
  - With compile-time-only crates at opt 0 (ADR-0017): **1,415 CPU-s**,
    3 min 55 s wall at load ~9.
  - The scratch project is not in the repo. To reproduce: create a crate
    depending on those crates, copy our profiles, and run
    `cargo build --timings` under `/usr/bin/time -v`.

## Completed

- [x] Plan v2 (`docs/PLAN.md`), ADRs 0001–0016, file-format design (accepted)
- [x] Project name **Imaginarium**, license **AGPL-3.0-or-later**, **DCO**
- [x] `MAINTAINERS.md`, `CONTRIBUTING.md`, this status file (agent files are git-ignored)

## M0: Foundations and profiling ✅ (tag `v0.0.1-m0`)

Commit after each item. Each item lists its acceptance criteria (AC).

- [x] **M0.1 Workspace skeleton** (done 2026-10-12, commits `597f749`, `e314a74`, `e992637`)
  - Root `Cargo.toml` with `[workspace]`, `resolver = "3"`, edition 2024,
    shared `[workspace.package]` (license `AGPL-3.0-or-later`, repository
    placeholder), `[workspace.dependencies]` and `[workspace.lints]`
    (clippy: `unwrap_used`, `expect_used` = deny in non-test code;
    `missing_docs` = warn; `unsafe_code` = forbid).
  - `rust-toolchain.toml` pinned to `1.99.0` with the rustfmt and clippy
    components. Add `rustfmt.toml` and `.editorconfig`.
  - Initial crates (only those needed for M0):
    - `imaginarium-profiling`
    - `imaginarium-settings`
    - `imaginarium-commands`
    - `imaginarium-test-utils`
    - `imaginarium-app` (stub binary that prints its version)
    - `xtask`
  - Every crate has a crate-level `//!` doc (responsibility, non-goals) and
    SPDX headers.
  - AC: `cargo build/test/clippy -D warnings/fmt --check` pass. ✅ Docs
    (`-D warnings`) pass too.
  - Notes for later steps:
    - `unsafe_code` is `deny`, not `forbid`, so a crate can opt out locally
      with an ADR. Core crates add `#![forbid(unsafe_code)]` themselves.
    - Dev profile builds dependencies at `opt-level = 2`. There is a
      `profiling` profile (release + debug info).
    - The workspace version is `0.0.1`. `repository` is set. `homepage`
      follows once the website is deployed (M0.7).
- [x] **M0.2 xtask checks** (done 2026-10-12, PR #2)
  - `cargo xtask check` runs `check-layers` and `check-headers`. CI will run
    this.
  - `check-layers` reads `cargo metadata --no-deps` (serde_json, an
    xtask-only dependency). It enforces:
    - the layer order (PLAN §4.1; dev-deps exempt),
    - `test-utils` dev-only, and nothing depending on `xtask`,
    - no UI/GPU/network/async crates in support/core,
    - AGPL-3.0-or-later on every crate.
  - The layer table `LAYERS` in `xtask/src/layers.rs` already lists every
    planned crate. The CLI crate is named `imaginarium-cli` (binary `imag`).
  - AC: ✅ 16 unit tests, including deliberately wrong dependencies. An
    end-to-end negative test (bad deps plus a file without a header) exits
    with code 1, with fix hints.
- [x] **M0.3 CI** (done 2026-10-12, PR #3, stacked on #2)
  - `.github/workflows/ci.yml` runs on PRs into `main` and pushes to `main`:
    - `lint`: rustfmt, `cargo xtask check`, rustdoc `-D warnings`
    - `deps`: cargo-deny 0.20.2, cargo-machete 0.9.2
    - `test`: clippy and tests on linux, windows and macos
    - `dco`: Signed-off-by
    - **`CI result`**: the aggregate, and the only required check in the
      ruleset
  - `deny.toml`: AGPL-compatible license allowlist, RustSec advisories, no
    wildcard versions or openssl, crates.io only, 6 targets.
  - New `cargo xtask check-tracked` blocks agent files and artifacts in git.
  - Ruleset JSON now also requires "CI result". **The owner must re-import
    `.github/rulesets/main.json`.**
  - AC: ✅ the first CI run was green on all 3 OSes (lint 28 s, deps 13 s,
    test linux 29 s, macos 48 s, windows 59 s).
  - Actions are pinned to commit SHAs. To update one, look up the new
    release's commit SHA with `gh api repos/<owner>/<repo>/commits/<tag>
    --jq .sha`.
  - The owner's token has the "Workflows" permission (the push succeeded).
    It **cannot** read repo administration settings (403 on branch
    protection).
- [x] **M0.4 `imaginarium-profiling`** (done 2026-10-12, PR #5)
  - `subscriber::init(&TracingConfig)`: compact, pretty or JSON logs to
    stderr. The filter comes from config, then `IMAGINARIUM_LOG`, then
    `RUST_LOG`, then `info`.
  - `tracy` feature: the client starts only when requested
    (`manual-lifetime`), with `timer-fallback` (works on this VM without an
    invariant TSC), `ondemand` and `only-localhost`. The default features
    would abort at program load on this VM.
  - `frame::FrameStats`: O(1) ring buffer, nearest-rank p50/p95/p99,
    `count_over(budget)`. Bench: record ~11 ns, summary over 600 samples
    ~12 µs.
  - Conventions for span names and levels, frame-phase span names and
    benchmarks are in the `imaginarium-profiling` crate docs.
  - `imaginarium-test-utils`: SplitMix64 `Rng` (reference-vector pinned),
    `BoardFixture::LARGE` (10k shapes), `DocumentFixture::LARGE` (300 pages),
    `DeckFixture::LARGE` (100 slides).
  - **Dev profile decided (ADR-0017)**: compile-time-only crates are built
    with `opt-level = 0` through a generated block in `Cargo.toml`
    (`cargo xtask dev-profile --write`, checked by `cargo xtask check`).
    Clean builds: this workspace −47 % CPU, heavy stack −33 %.
  - AC: ✅ percentile unit tests, and the criterion bench runs.
- [x] **M0.5 `imaginarium-settings`** (done 2026-10-12, PR #6, stacked on #5)
  - `Setting<T>` declarations are `static`s whose constructor requires a
    title and description (metadata is guaranteed at compile time). They
    have constraints (`range`, `max_len`), custom validators, group,
    synonyms and an advanced flag.
  - Types: bool, i64, f64, String, enums (`setting_enum!`), `Rgba`, paths,
    `SecretRef` (keychain id only), lists. Scopes: app/user/workspace/
    document/hub.
  - `Registry`:
    - key rules (dotted snake_case, no duplicates or prefix clashes);
    - **plain-text settings with secret-looking keys are rejected**;
    - defaults must validate;
    - typed `get`/`set`, dynamic `set_value` for the UI, observers;
    - **reading an unregistered setting returns `NotRegistered`**.
  - TOML for app scope: only changed values, nested tables, unknown entries
    preserved, invalid entries reported and skipped, atomic save.
  - Not yet: settings of add-ons (dynamic declarations, `&'static` today),
    user-scope sync, document-scope CRDT storage, keychain resolution.
    These come with their milestones.
  - AC: ✅ 25 unit tests and 3 doctests (registration, validation,
    round-trip of all kinds, unregistered read).
- [x] **M0.6 `imaginarium-commands`** (done 2026-10-12, PR #7, stacked on #6)
  - `Command<Cx>` is generic over the context, so the crate has no model
    dependency. `static FnCommand`s for built-ins (use
    `FnCommand::<Cx>::new` when builder calls follow). `CommandMeta`
    requires a title and description.
  - `Registry::execute` is **the single execution path**: it checks
    availability and validates arguments. `tool_definitions` and
    `execute_tool_call` make every enabled command an AI tool (JSON Schema;
    tool name `text.bold` → `text__bold`).
  - `Keymap`: `Mod+Shift+K` is platform-neutral. It displays as
    `Ctrl+Shift+K` or `⇧⌘K`, matches physical key presses, and detects
    conflicts. Single strokes only (chords later).
  - `SearchIndex`: fuzzy scoring with synonyms, field weights and a
    bounded frecency boost (`UsageStats`, time passed in explicitly).
    **Bench over 2,000 commands: worst 0.48 ms ("a"), typical
    0.07–0.14 ms.** Budget 1 ms ✅.
  - AC: ✅ 29 unit tests and 1 doctest, including ranking tests and the
    benchmark.
- [x] **M0.7 Website skeleton** (done 2026-10-12, PR #8, stacked on #7)
  - `website/`: Astro 7.3.5 + Starlight 0.42.4. Pages: landing, download,
    guide intro, self-hosting overview, developer overview, architecture
    (layer diagram component), contributing.
  - The developer reference (PLAN, file-format design, all ADRs) is
    **synced from `docs/` at build time** (`scripts/sync-docs.mjs`). The
    output is git-ignored, so edit `docs/`, not the copies.
  - Dependency policy: exact versions, `npm install --before=<two weeks
    ago>`, lockfile committed, `ignore-scripts=true`.
  - The CI job `website` builds it. **Not deployed yet**: GitHub Pages needs
    the owner to enable Pages and allow a deploy workflow. `site`/`base` in
    `astro.config.mjs` assume `giosuel.github.io/imaginarium`.
  - AC: ✅ `npm run build` (27 pages, ~5 s). Screenshots were sent to the
    owner.
- [x] **M0.8 Wrap-up** (done 2026-10-12)
  - [x] Build commands in `MAINTAINERS.md`, kept current with each step.
  - [x] **Code review of M0.4–M0.7** (`/code-review high`). 10 findings,
    all confirmed with a failing test first and fixed in their own PR
    branches, then merged upwards (#5 → #6 → #7 → #8):
    - settings: unreadable values were erased on save (data loss); a stray
      entry could replace a table; dynamic values weren't normalized;
      `#+f+f+f` parsed as a color;
    - keymap: Super/Win was ignored on Linux/Windows; the `+` key couldn't
      be bound;
    - profiling: an empty `IMAGINARIUM_LOG` didn't fall through;
    - xtask dev-profile: optional features were ignored; CRLF manifests
      looked stale. Added `.gitattributes` (LF everywhere);
    - website: the base path was hardcoded. Now `site.config.mjs` plus a
      remark plugin, plus a link checker in CI, which immediately found a
      missing favicon.
  - [x] #5–#8 merged. `main` CI green at `171c097`. Annotated tag
    **`v0.0.1-m0`** pushed.

## M1: Format, crypto, model ✅ (tag `v0.0.2-m1`)

Use **xhigh/max effort** throughout. These crates are the hardest to change
later. Commit after each item, and use one PR per item (or smaller). The
design is in `docs/design/file-format.md` and ADRs 0009–0011 and 0016.

- [x] **M1.0 Dependency research** (done 2026-10-12, PR #10). See
  [`docs/design/m1-dependencies.md`](design/m1-dependencies.md).
  - RustCrypto for everything:
    - `chacha20poly1305` (XChaCha), `blake3`, `ed25519-dalek` 3,
      `argon2`, `crc32c`, `zstd`;
    - **`hpke` 0.14 implements X-Wing as KEM 0x647A**, so key wrapping
      is standard HPKE `(0x647A, HKDF-SHA256, ChaCha20-Poly1305)`;
    - `ml-dsa` (ML-DSA-65).
  - PQ crates are pre-1.0 and unaudited. ML-DSA had 2026 CVEs, fixed
    before 0.1.0. Hybrid design: neither can make us weaker than
    X25519/Ed25519.
  - Protobuf: `prost` + `prost-build` + `protox` 0.9.1 (pure Rust, no
    `protoc`). The `.proto` files are normative.
  - CRDT: **only Loro has a movable tree and history trimming.** Yrs is
    the fallback. The M1.5 benchmark decides.
  - New rule in MAINTAINERS: **the two-week rule also applies to crates**.
    Pin `x-wing` 0.1.0 until 0.1.1 is 14 days old (2026-10-15).
- [x] **M1.1 Normative spec v1, Draft 1** (done 2026-10-12, PR #11).
  [`docs/format/spec-v1.md`](format/spec-v1.md) and
  [`proto/imag_v1.proto`](format/proto/imag_v1.proto) (validated with
  protox + prost-build: it compiles and round-trips). Also on the website.
  - It becomes normative at tag `v0.0.2-m1`. Until then, implementation
    findings (M1.3/M1.4) may change it, recorded in spec §14.
  - Refinements to the design (spec §14):
    - `doc_id` (not `file_id`) in the AAD;
    - `DocumentMeta.blob_id_key` independent of epochs;
    - `SIGNED` framing section;
    - `plain_len`;
    - a per-record `codec`;
    - ~100 B overhead per encrypted record.
  - Still open for later minors: the logical schema (`schema-v1.md`,
    M1.6), and the Hub-side ACL rules (sync protocol, M1.2).
- [x] **M1.2 Sync protocol design** (done 2026-10-12, PR #12, Draft).
  [`docs/design/sync-protocol.md`](design/sync-protocol.md). It becomes
  normative with the Hub in M8.
  - The Hub moves **the same spec-v1 record bytes**. It validates framing,
    encryption rules, signatures and roles from plaintext framing plus
    `ACLC`/`KEYR`, and never decrypts.
  - Per-document server `seq` for catch-up. Pushes are idempotent by
    `object_id` and acknowledged only after durable storage.
  - Clients create snapshots (`covers_through_seq`). GC follows only an
    admin-signed retention policy plus a 30-day grace period. Clients
    cross-check snapshots against covered updates.
  - Presence: encrypted with `K_presence`, padded, relayed and never stored.
  - The local cache *is* an `.imag` file (`org.imaginarium.sync-state`
    extension record).
  - To define in M8: `.proto` messages, the signed retention policy
    format, key transparency.
- [x] **M1.3 `imaginarium-format`** (done 2026-10-12, PR #13, stacked on
  #12). The unencrypted path is complete; encryption plugs in through
  `RecordSealer`/`RecordOpener`/`KeyProvider`, implemented by M1.4.
  - Writer (append, commit with fsync and truncation, resume), Reader
    (trailer → index, `key_records` for encrypted documents, blobs), and
    recovery (previous commit **plus adoption of complete records after
    it**, or a forward scan).
  - Compaction: default policy, `SAFE_TO_COPY` rules, atomic
    `compact_file`.
  - Header and signature bytes are kept **verbatim** (spec §6.3.1).
    Allocations never follow claimed lengths (DoS hardening).
  - Spec refinements found while implementing (spec §14, items 8–10):
    verbatim header bytes, `key_records` in the `INDX` header, recovery
    adopting records.
  - Tests:
    - 26 unit tests, 16 file-level tests, 5 vector tests, plus proptests;
    - **test vectors 1–4** in `docs/format/test-vectors/`;
    - fuzz targets: decode_record 19.1M, open_content 22.8M and open_file
      1.8M executions, no crashes.
  - Bench (10k updates): open 5.6 ms, autosave 3.3 ms, recovery scan 18 ms.
  - Tooling installed on the dev VM: `nightly-2026-09-25` (minimal, fuzz
    only) and `cargo-fuzz` 0.13.2.
- [x] **M1.4 `imaginarium-crypto`** (done 2026-10-12, branch
  `feat/m1.4-crypto`):
  - Suite 1 record encryption (XChaCha20-Poly1305 with the format's AAD,
    STREAM segments), epoch keys and subkeys, content-addressed blob ids.
  - Keyrings: Argon2id passwords (NFC), X-Wing HPKE recipients, folder and
    link keys. Key providers: `PasswordKeys`, `RecipientKeys`, `SharedKeys`,
    `KnownKeys` (cached DEKs, so Argon2 isn't paid on every open).
  - Signatures: Ed25519 devices, hybrid Ed25519 + hedged ML-DSA-65
    identities (both halves must verify).
  - `AclState`: replays the `ACLC` chain (genesis, linearity, forks,
    permission table, devices, rotation, single owner, ownership transfer)
    and authorizes content and keyrings **per epoch**.
  - Format fix: a CRC-valid index that fails to decrypt is reported as a
    key problem and **not "recovered"** (it used to open a file with a
    wrong cached key as "damaged").
  - Spec refinements (§8.4, §14 items 11–15): per-epoch authorization,
    keyrings checked against the ACL chain (otherwise a Hub could plant a
    key), no recovery on key errors, rotation after every `RemoveDevice`,
    hedged ML-DSA, vector 6 has five segments.
  - Tests: 34 unit tests, **test vectors 5–10** (11 files, written byte
    for byte from seeded randomness; README lists the keys). The crate
    dev-depends on itself with the `test-vectors` feature.
  - Benchmarks (`cargo bench -p imaginarium-crypto`), this VM vs. the
    design's table:

    | Operation | Measured | Design |
    |---|---|---|
    | Seal / open 1 MiB | 2.47 ms (405 MiB/s) / 1.28 ms (781 MiB/s) | ~1–2 GB/s |
    | BLAKE3 | 2.5 GiB/s | several GB/s |
    | Ed25519 sign / verify | 34 / 69 µs | ~15 / 40 µs |
    | Hybrid sign / verify | 1.0 ms / 224 µs | (rare records) |
    | HPKE wrap / unwrap | 221 / 303 µs | ~100 µs unwrap |
    | Argon2id (64 MiB, t=3) | 259 ms | ~250 ms by intent |
    | Edit batch (200 B): seal+sign / verify+open | 39 / 74 µs | |
    | ACL replay, 100 changes | 36 ms | |
    | Open encrypted doc, 10k updates | 4.6 ms | |

    Single-thread speed is about half of a current laptop's (Ed25519 and
    the AEAD both land ~2× below the table; BLAKE3 is fine). Nothing is
    over budget: a document open pays one HPKE unwrap or one Argon2 run.
    Optimizations found by the benchmarks are in: in-place encryption
    (−29 % seal), cached decoded identity keys (−42 % hybrid verify), no
    state clone per ACL change (replay −26 % overall).
  - Profile: bench build (release, thin LTO) 57 s wall; build plus run
    331 CPU-s, peak 449 MB.
- [x] **M1.5 CRDT benchmark** (done 2026-10-12, branch
  `feat/m1.5-crdt-benchmark`, stacked on #14). **ADR-0009 is confirmed:
  Loro.** Results and analysis are in
  [`docs/design/crdt-benchmark.md`](design/crdt-benchmark.md), and the code
  is in `benchmarks/crdt/` (a separate workspace, not in CI).
  - Only Loro has the hard requirements. With a parent-pointer layer on
    Yrs or Automerge, concurrent moves (A into B, B into A) leave a
    cycle. History trimming: Loro only. Collaboration-aware undo: Loro and
    Yrs (verified functionally); Automerge has none.
  - Budgets: Loro applies remote updates in 18–24 µs (p99 ~55 µs), well
    under 1 ms with crypto. It opens the 300-page document in 2.7 ms and
    the 10k-shape board in 106 ms.
  - Automerge is ruled out: 377 ms per remote keystroke at 911k
    characters (the cost grows with document size), and 665 ms to open
    the document.
  - Loro's weak spots, with follow-ups for M1.6:
    - snapshots ~3× larger than Yrs after zstd (1.5 MiB for the board),
      because Loro already LZ4-compresses;
    - 57 ms to merge a large offline session;
    - 36 MiB right after building 10k shapes (14 MiB after reopening).
  - Pitfall recorded: Yrs's catch-up encoding repeats the whole delete
    set (~2 KB per "update"). Live edits must use the transaction's
    update.
  - Profile:
    - benchmark build (3 CRDT libraries, release, thin LTO): 3 min 26 s
      wall, 496 CPU-s, 1.35 GB peak;
    - full run: 14 min 12 s, single-threaded, 849 CPU-s, 371 MB peak.
      13 of the 14 minutes are Automerge's typing phase.
- [x] **M1.6 `imaginarium-model`** (done 2026-10-12, branch
  `feat/m1.6-model`):
  - **Schema v1, Draft 1** in [`docs/format/schema-v1.md`](format/schema-v1.md).
    It covers root containers, node keys and kinds, geometry in points,
    character marks and paragraph marks (on the terminating `\n`), styles,
    assets, evolution and actors. It's on the website too.
  - `Document`:
    - `transact` runs typed, pre-validated `Transaction` edits and records
      the actor as the commit message;
    - a failing command is reverted, with the undo stacks paused;
    - `apply_remote`, export of snapshot, shallow snapshot and updates;
    - two undo stacks, user and assistant, separated by commit origins.
  - `DocumentView`:
    - persistent `imbl` maps of `Arc`'d nodes; text as chunked
      paragraphs of marked runs;
    - updated incrementally from Loro events, reporting a `ChangeSet`;
    - anything unexpected rebuilds and diffs.
  - Migrations and `required_features` (read-only mode).
  - **Findings built in** (also in ADR-0009 "Refinements"):
    - **Loro's undo re-creates deleted nodes under new tree ids.** Nodes
      therefore have a stable stored `id` (53-bit integer, exact in JS),
      and references survive undo and redo.
    - Concurrent creations at one position arrive as delete + create of
      the same node. The model reports that as a move.
    - Lazily created child containers are Loro *mergeable* containers
      (Loro ≥ 1.13.1), so concurrent first edits share one container.
    - Loro has no public rollback: the model validates before mutating,
      and on failure uses `revert_to` with the undo stacks paused.
  - Tests: 7 unit, 17 integration, 2 property tests and 1 doctest.
    - Property 1: every peer's incremental view equals a rebuild after
      every step, and peers converge.
    - Property 2: undo is an inverse.
    - 2,000 cases each pass. Mutation-checked: breaking the updater in two
      places fails both.
  - Benchmarks (`cargo bench -p imaginarium-model`, dev VM):

    | Operation | Measured | Budget |
    |---|---|---|
    | Open the 300-page document (import + view) | 47 ms | first page < 500 ms |
    | Open the 10k-shape board | 339 ms | (none yet) |
    | Keystroke in 300 pages: local / remote | 59 / 55 µs | remote < 1 ms incl. ~74 µs crypto |
    | Drag step on 10k board: local / remote | 29 / 38 µs | same |
    | Memory, opened board / document | 30.6 / 4.2 MiB | |
    | Snapshot, board / shallow | 2.8 / 1.6 MiB | |

    The benchmarks found and fixed:
    - keystrokes 504 → 59 µs (chunked text, run-level fast path);
    - opened board 40 → 31 MiB (exact-size property maps);
    - 37 → 9 bytes per node id (integer instead of string).
  - Profile:
    - bench build + run: 2 min 17 s wall, 182 CPU-s, 804 MB peak;
    - property soak (2,000 cases): 14 s.
- [x] **M1.7 `imaginarium-cli` (`imag`)** (done 2026-10-12, branch
  `feat/m1.7-cli`, stacked on #17):
  - New crate **`imaginarium-io-native`** (features layer).
    - Opens `.imag` files as model `Document`s: password or X-Wing key;
      the main stream from snapshot plus updates, in index order; `META`;
      recovery; the ACL chain.
    - Saves new files: unencrypted, or password-encrypted with a keyring.
      Saving to a path is atomic (temporary file, sync, rename).
    - It's the app's load/save path too. Incremental saves come in M2.
  - **`imag`**:
    - `inspect` reads the plaintext framing, so it works without keys;
      `--json` for scripts.
    - `verify` checks record CRCs, trailer and index (or recovery),
      decryption of every record, blobs, keyrings, signatures and the ACL
      chain, and the document. Exit code 1 on problems, 2 if it couldn't
      run.
    - `export-json` prints the decrypted document.
    - Secrets come only from stdin, an environment variable or a key
      file, never from arguments.
  - Website page `developers/imag`, linked in the sidebar.
  - Tests:
    - 4 io-native round trips (plain, password, atomic path, documentless
      vectors);
    - 6 CLI tests running the binary: corrupt bytes, an interrupted save,
      a wrong password, and vectors 10a, 10b and 10c with the owner key.
- [x] **M1.8 Wrap-up** (done 2026-10-12, PRs #19 and #20):
  - [x] **two-week rule automated**: `cargo xtask dep-age` checks every
    entry of every committed `Cargo.lock` against the crates.io sparse
    index `pubtime` (no API rate limit), and CI runs it in the deps job.
    Its first run found six too-new versions in
    `crates/imaginarium-format/fuzz/Cargo.lock`, now pinned;
  - [x] **weekly CI fuzz job** (`.github/workflows/fuzz.yml`): every
    format target for 10 minutes on Mondays, seeded with the test vectors,
    crashing inputs uploaded as artifacts;
  - [x] **code review of M1**: four reviewer agents at medium effort, one
    each for crypto, format, model, io-native + CLI, read-only. They
    reported 1 critical, 7 high, 15 medium and about 25 low findings. Every
    one was checked against the code. None was a false alarm, though three
    describe intended or inherent behaviour (below). Fixed test-first on
    branch `fix/m1.8-review`, one commit per finding or group; each new
    test was seen failing first. The most important fixes:
    - **crypto (critical):** removing or replacing the ACL chain made a
      forged keyring acceptable, so recipient-key documents now need an
      ACL chain started by an owner the reader trusts (**ADR-0018**, spec
      §6.4.2). `RecipientKeys::new(keypair, owner)`, `imag --owner`.
    - **format:** plaintext records were accepted in encrypted files;
      recovery read around an index it couldn't decrypt (also when only
      the newest trailer was damaged); `read_blob` could abort or exhaust
      memory on hostile segment headers; the trailer could reach the disk
      before its records; atomic replace followed planted symlinks and
      lost permissions (`AtomicFile`).
    - **model:** a rolled-back delete wiped undo history (destructive
      edits are now deferred until the closure succeeds); a skipped stale
      undo step lost its actor and landed on the other stack; undoing a
      move of a node another actor's undo removed revived an empty "ghost"
      node (found while testing); read-only mode wasn't re-checked after
      remote changes.
    - **io-native/CLI:** records weren't checked against the ACL chain on
      open; `imag verify` without a key said "ok" (now exit 3,
      "incomplete", and the chain is checked without a key).
    - Not changed, with reasons in the PR: a bad KEYR in an *authenticated*
      index fails the open (fail-closed is intended: only key holders can
      write that list); fake trailers inside unencrypted blobs (inherent:
      unencrypted files have no authenticity); key copies on the stack
      (Rust moves can't be zeroized reliably). Deferred: see Known issues.
  - [x] spec, schema and protobuf v1 **normative** (spec §14 records the
    review's changes); website `imag` page updated;
  - [x] tag `v0.0.2-m1` (annotated, on the merge commit `f95151c`).

## M2: UI prototype gate ✅ (tag `v0.0.3-m2`)

ADR-0007 makes our own toolkit conditional on a **time-boxed prototype**
that meets six pass criteria on Linux, Windows and macOS with integrated
GPUs. If a criterion fails in a way we can't fix, a new ADR switches the
shell to GPUI. The prototype's crates are real crates, not throwaway code:
they become the toolkit if the gate passes.

Effort: **xhigh** for `reactive`, the `ui` core and `render`, **high** for
`text` and the gate app. One branch and PR per step (or smaller).
Screenshots go to the owner from M2.4 on (Xvfb + Mesa lavapipe).

**Gate machine** (owner's laptop, 2026-10-12): EndeavourOS (Arch, rolling,
kernel 7.2), **Hyprland (Wayland)**, Intel Core i9-11900H with its **Intel
UHD (Xe) integrated GPU** plus a GeForce RTX 3050 Laptop (hybrid graphics),
32 GB RAM. The gate runs on the integrated GPU (ADR-0007) and reports the
RTX 3050 as well. A workstation with the same setup (Ryzen 9 9950X3D, RTX
5090, 96 GB) exists but isn't used for now. Binaries built on this VM
(glibc 2.41) run on Arch's newer glibc. Wayland-specific risks for the
gate: winit's idle-loop spin (#4668), text-input-v3 IME, fractional
scaling.

**This VM has no GPU** (and neither do the CI runners). Functional checks
run here with lavapipe and the CPU backend. The real numbers come from the
owner's Linux machine: a prebuilt gate binary writes a report (owner
decision 2026-10-12, ADR-0007 "Procedure"). Windows and macOS are checked
later, when hardware is available. There is no time box, but one decision
point: after M2.5, one focused fix round for failed criteria, then decide.

- [x] **M2.0 Stack research** (done 2026-10-12). See
  [`docs/design/m2-ui-stack.md`](design/m2-ui-stack.md).
  - The stack stays as decided (ADR-0007). Versions for the two-week
    rule: winit 0.30.13, accesskit 0.25.1 / accesskit_winit 0.34.1 now;
    **`vello_gpu` and `vello_cpu` 0.3 with wgpu 30 from 2026-10-16**,
    parley 0.12 from 10-23, taffy 0.15 from 10-26. Pin each crate when it's
    added, at the newest eligible version.
  - Vello's sparse-strips renderers (`vello_gpu`, `vello_cpu`) for both
    backends, not the compute renderer Vello now calls experimental
    (noted in ADR-0003).
  - Masonry only as a reference (a year-old release that would duplicate
    wgpu, Vello, Parley and AccessKit). The text field builds on Parley's
    `PlainEditor`. The gate app is its own binary, `imaginarium-gate`.
  - Gate procedure (owner, ADR-0007): one decision point after M2.5,
    Linux first, prebuilt binary with a report.
  - Clean build of the stack: 1,477 CPU-s debug, 821 release. The machine
    isn't the bottleneck.
  - **Order:** M2.1 needs none of these crates. M2.2 starts with the CPU
    backend and the tile cache, so the GPU backend starts on `vello_gpu`
    0.3. M2.3 uses parley 0.12 (owner exception, see below).
- [x] **M2.1 `imaginarium-reactive`** (done 2026-10-12): `Signal`,
  `Trigger`, `Memo` (lazy, cut off by `PartialEq`), `Effect`, `Scope`,
  `batch`, `untrack`, `on_cleanup`. No dependencies outside `std`.
  - Push-pull coloring (as in Reactively and Solid 2): glitch-free, at
    most once per batch, only when an input really changed; dependencies
    follow the latest run. One graph per thread, `Copy` and `!Send`
    handles.
  - Unchanged re-runs keep their edges, and disposal unsubscribes lazily,
    so 10k readers of one signal cost linear time.
  - **Module review** (separate agent, medium effort) found 2 critical,
    2 high and 3 medium bugs. The worst: a write during a computation's
    run was lost and could leave an effect stuck forever, and a child
    effect could run before its parent disposed it. All fixed, each with
    a regression test that failed first (`6c1d13b`). Writes are now
    allowed anywhere, and panics leave nodes usable.
  - Tests: 24 behaviour tests, and property tests on random graphs
    against a direct evaluation, including effects that write signals
    (also their own source). Planted versions of the bugs make them fail.
  - Benchmarks: write to effect 110 ns; 10k effects on one signal
    0.88 ms; a batch of 10k signal → memo → effect chains 2.8 ms; a
    1,000-memo chain 132 µs; a 1,000-row scope created and disposed
    0.53 ms. Later optimization if needed: fewer `RefCell` round trips per
    node, and an iterative `update` for memo chains deeper than a few
    thousand.
**Reordered 2026-10-12 (owner):** the UI risks first (they decide own
toolkit vs GPUI), then the canvas; the decision point stays after the gate
app runs. Two owner exceptions for M2:
- **Two-week rule:** Vello 0.3 (`vello_gpu`, `vello_cpu`, `vello_common`),
  Parley 0.12 and Taffy 0.15 may be used although younger than 14 days,
  plus the transitive versions they require (pinned older wherever one
  still fits). Listed in `xtask/dep-age-exceptions.txt`.
- **Module reviews deferred to M2.9:** no per-module review agent during
  M2; one review of all M2 code before the tag. Tests first, property
  tests and all checks before every PR still apply.

- [x] **M2.2 Window and frame pipeline** (done 2026-10-13):
  - `imaginarium-render`: a `Scene` (fills, strokes, clips, opacity
    layers, transforms, `append`) that both backends replay.
    `WindowRenderer` (`vello_gpu` 0.3 on wgpu 30, low/high-power GPU or
    an adapter by name, vsync on/off) reports per frame the swapchain
    wait, the encode (Vello builds its strips on the CPU) and the submit.
    `CpuRenderer` (`vello_cpu`, a deterministic mode for golden images)
    and `OffscreenRenderer` (GPU into a pixmap). Tests: a CPU golden
    image, repeatability, GPU output = CPU output within antialiasing
    (passes on lavapipe; skipped without a GPU, e.g. in CI).
  - `imaginarium-ui`: winit 0.30 with `ControlFlow::Wait`. A `View`
    paints one scene per frame in logical pixels, drawn at the scale
    factor. Frames only on resize, scale change, handled input,
    `request_frame()` (for effects) or the view's timer
    (`View::wake_at`, also the caret blink in M2.3). Input: pointer,
    wheel (Ctrl/⌘ = zoom), keys.
  - `imaginarium-gate` (apps, temporary): a board of 2,000 cards with a
    toolbar; wheel/drag pans, Ctrl + wheel zooms about the pointer, hover
    outlines. `--measure` times a scripted pan, then idles 10 s and
    checks criterion 6. Writes `gate-report.json` (cold start, paint vs
    encode vs submit vs swapchain wait, frame intervals, idle CPU and
    frames, GPU, machine, peak RSS). `--gpu low|high`, `--adapter`,
    `--no-vsync`, `--frames N`.
  - Measured on this VM (llvmpipe, Xvfb, release): paint 0.41 ms, encode
    1.57 ms per frame (mean, ~50 cards and 640 grid dots on screen);
    llvmpipe rasterizes in `submit` (24 ms), so frame intervals mean
    nothing here. Idle: 0 frames, 0.0–0.2 % CPU over 5 s (one 10 ms tick
    at most). Main to first frame 262–297 ms (mostly wgpu/Vulkan setup on
    llvmpipe).
  - Found on the way: asking an X11 window for its size costs a server
    round trip (it tripled the paint time; the size now comes from
    `Resized`). winit's default Wayland fallback decorations pull in the
    unmaintained `ttf-parser` (RUSTSEC-2026-0192); we use the variant
    without title text.
  - **Reactive library comparison (decision: keep ours).** A separate
    agent ran our tests, properties and benchmarks against
    `sycamore-reactive` 0.9.3 and `reactive_graph` 0.2.15 (Leptos):
    - Correctness: sycamore fails 6 of 27 tests (eager memos, no
      re-run after writing its own source, poisoned after a panic,
      hidden cycles) and one property. Leptos' synchronous effects are
      not glitch-free, lose updates through memos (also in a batch,
      which every frame is), deadlock on a cycle, and fail two
      properties.
    - Fan-out per write: 10k effects take ours 1.76 ms, sycamore 53 ms,
      Leptos 393 ms. Both candidates are quadratic (20k: 3.3 ms vs
      211 ms vs 1.6 s).
    - Ours has no dependencies and builds in 1.3 CPU-s; Leptos'
      `reactive_graph` is 33 crates and 30 CPU-s, with `Send + Sync`
      bounds and process-global state.
    - The cost of keeping ours: ~1,250 lines to maintain, guarded by the
      tests and property tests.
- [x] **M2.3 Text and the text field** (done 2026-10-13):
  - `imaginarium-render`: glyph runs in scenes (one glyph buffer per
    scene), drawn by both backends with their glyph caches; an optional
    glyph atlas (`GpuOptions::glyph_atlas`; Vello calls it experimental).
  - `imaginarium-text` on Parley 0.12: `Fonts` (the bundled Inter 4.001,
    OFL, then system fonts with per-script fallback; fontconfig loaded at
    run time), `TextBlock`, and `Editor` on `PlainEditor` (`Edit`
    operations, word/line selection by clicks, drag, IME composition,
    per-line data for screen readers). Tests use only the bundled font:
    a CPU golden image, GPU = CPU, editing and the IME sequence winit
    delivers.
  - `imaginarium-ui`: our own `Key`/`Modifiers`, IME events and
    `View::ime_area` (candidate window at the caret), click counting,
    `TextField` (platform editing keys, a caret that blinks 10 s and then
    rests), AccessKit through `accesskit_winit` (`View::accessibility`,
    actions; the tree is sent after frames while a screen reader
    listens).
  - Gate app: text on the cards in nine scripts, search (Ctrl+F) over
    2,000 cards, double-click editing in place at any zoom, status line,
    an accessibility tree, scripted typing in `--measure`, and
    `docs/design/m2-gate.md` (how to run it, the manual checklist).
  - Measured on this VM (llvmpipe, release, ~50 cards of text on
    screen): paint 1.0 ms, encode **12.0 ms without the glyph atlas,
    4.3 ms with it**; a keystroke including the search 0.2 ms (search
    0.08 ms); fonts load in 15 ms; main to first frame 343 ms (the first
    frame lays out the visible cards: paint 23 ms); idle 0 frames, 0 %
    CPU. Text encode is the renderer's biggest cost so far: tiles
    (M2.5) cache it on the canvas, the atlas helps the UI chrome.
  - Found on the way: PNG stores straight alpha, so golden checks allow
    one step per channel; `PlainEditor::set_compose` must not get the
    empty preedit winit sends before a commit; this VM has no Korean,
    Hindi or emoji fonts (boxes or gaps there).
  - **Early test binary** sent to the owner with the checklist (criteria
    4–6 and the numbers so far), ahead of the plan (M2.4).
- [x] **M2.4 Widgets and layout** (done 2026-10-13):
  - `imaginarium-ui`: the widget tree (`Ui`): an arena with generational
    ids, Taffy 0.15 layout (leaves measure their text), a cached scene per
    widget (a frame repaints only invalidated widgets), hit testing,
    hover, pointer capture during presses, click-to-focus, bubbling,
    Tab/Shift+Tab, accessibility from the widgets. Signals drive it:
    `Ui::watch` ties a widget to what a closure reads, and the effect
    invalidates only that widget (disposed with it). Widgets: `Flex`,
    `Label` (fixed or from signals), `Button`, `TextInput`. Clipboard
    (arboard, Wayland data-control) in text fields. Design:
    `docs/design/ui-toolkit.md`.
  - `imaginarium-commands`: word-prefix matches now rank above scattered
    ones ("ins tab" → "Insert table"; found with the palette).
  - Gate app: the window is a widget tree (toolbar, board, status line,
    palette). Command palette (Ctrl+K) over 2,000 synthetic commands with
    the real `SearchIndex`: a memo over the query signal, ↑/↓, Enter,
    Escape, clicks, shortcut chips, accessibility (dialog, list box).
    `--measure` adds 60 palette keystrokes.
  - Measured on this VM (llvmpipe, release): palette keystroke: search
    0.26 ms (p95 0.41 ms: **criterion 3 passes**), handling 0.40 ms,
    Taffy layout 0.42 ms, paint 0.82 ms (5 of 20 widgets repainted),
    encode 5.2 ms. Pan: paint 1.07 ms, encode 4.3 ms. Cold start 348 ms
    (passes; the first frame skips the glyph atlas, whose fill cost 85 ms
    there). Idle: 0 frames, 0 % CPU.
  - Fixed on the way: labels shaped their text for every width Taffy
    asked about (layout 1.26 → 0.42 ms); a timer's frame request could
    wait for the next wake-up; per-frame timings in a signal would have
    caused endless frames (they are read from a cell and refreshed with
    `Ui::refresh`).
- [x] **M2.5 Canvas** (done 2026-10-13):
  - `imaginarium-render`: textures kept across frames (`Scene::
    render_texture`, `draw_texture`, `free_texture`; one GPU submission
    per texture render, external texture bindings; pixmaps on the CPU),
    and `canvas::Canvas`: items in a 512-unit grid, 256-px tiles at
    half-octave levels, old ∪ new invalidation, a budget of 24 renders per
    frame with stand-ins, the 150 ms zoom pause, LRU eviction at 1,000
    tiles, counters, a tile-updates tint, and `draw_direct` without tiles.
    Content paints items on demand (level of detail per level).
  - Gate app: both boards on the tile cache (hover and editing are live
    overlays); a 10,000-shape board (Ctrl+2) with dragging; Tile updates
    (Ctrl+T); `--measure` pans, zooms and drags the shapes and checks
    criteria 1 and 2; `--no-tiles` to compare.
  - Measured on this VM (llvmpipe): a drag renders at most 2 tiles per
    frame (**criterion 2 passes**); panning renders 0.3 tiles per frame on
    average. Encode per pan frame 1.4 ms with tiles vs 4.7 ms without
    (shapes), 1.6 vs 4.4 ms (cards). Criterion 1 can't be judged here:
    llvmpipe rasterizes inside `submit` (~23 ms of a 27 ms frame); the
    UI thread's own work is 2.5–5 ms. Cold start 378 ms, idle 0 %.
  - Then: shape editing (selection by click, Shift+click, rubber band,
    Ctrl+A; drag, nudge, delete, undo/redo). A drag lifts the selection
    out of the tiles and draws it live, so it renders tiles only at its
    start and end (scripted drag: 2 tile renders in 240 frames). The
    board's first frame is drawn without tiles (cold start 324–351 ms
    instead of 404–428 ms with 24 tile submissions on llvmpipe).
  - Property tests: no valid tile ever shows stale content (a planted
    missing invalidation fails it), and panning invalidates nothing.
  - Not yet (design §10): tile rendering on workers, margin prefetch.
  - Disk full during M2.5 (2026-10-13 ~01:55 UTC): accumulated Cargo
    output (another project's `~/.cache/somedesk-target*`, 77 GB; ours
    `target/`, 42 GB). Freed our `target/debug/incremental` (9.3 GB).
    Clean `target/debug` between milestones.
- [x] **M2.6 Gate binary** (done along the way; closed 2026-10-15):
  scripted scenarios for criteria 1, 2, 3 and 6, `gate-report.json` with
  toolkit and renderer numbers apart, `--gpu low|high`, `--adapter`,
  `--no-vsync`, `--no-glyph-atlas`, `--no-tiles`, and the manual checklist
  (`docs/design/m2-gate.md`). `--backend cpu` is dropped (owner,
  2026-10-15, PLAN §14: wgpu's software adapters cover machines without a
  GPU driver). Mixed DPI stays manual (checklist items 5 and 6).
- [x] **M2.7 Gate run** (done 2026-10-13) on the owner's laptop: results in
  `docs/design/m2-gate.md`, one focused fix round for failed criteria,
  then the decision.
  - First run (2026-10-13, manual): a crash from tile textures freed while
    drawn, and boards showing each other's tiles after switching. Fixed
    (unique texture keys, stand-ins kept from eviction, texture work before
    the surface is acquired and done once per repaint, missing textures
    draw nothing); property test with two canvases on one renderer. The
    report now has manual sessions' frames and the cold start's window and
    GPU parts.
  - Second run (`--measure`, fix 1): every run panicked on color emoji in
    the first frame (drawn without the glyph atlas). Bitmap glyphs now
    always go through the atlas on the GPU, scaled up past its 128 pixels
    per em; test with a one-glyph cut of Noto Color Emoji.
  - Third run (the five reports): criteria 2 and 3 pass; criterion 1 fails
    only in the zoom phase (p99 26 ms on the iGPU: level switches rendering
    24 tiles at once); criterion 6 fails (cold start 735–1,162 ms, 1–2
    idle frames); and a one-second freeze from font fallback.
  - Owner decision (2026-10-13, ADR-0007 amendment): cold start is judged
    without the GPU's setup.
  - Fix round: font stack without a generic family (the freeze; first
    paint 385 → 24 ms on the VM with 1,537 families), a 6 ms time budget
    for tile renders with finer stand-ins and textureless empty tiles, the
    GPU opened on its own thread (Vulkan only), idle frames by cause and
    no redraw for same-size resizes, `--fullscreen`, the build in the
    report.
  - Fourth run (fix 3): cold start without the GPU's setup 275–347 ms, idle
    0 frames, palette 0.34 ms, no freeze, no crash. The drag phase starts
    with about 190 tiles still owed from the zoom (a fault in the script).
    The iGPU misses vertical sync in one frame in four or five
    fullscreen, and does 50–60 fps without vertical sync. (The reported
    1132×1348 is the first frame's size, before Hyprland made the window
    fullscreen.)
  - The gate evaluates the renderer of ADR-0003 too. A renderer failure
    has other fallbacks than GPUI.
  - **Decision (owner, 2026-10-13): we keep our toolkit.** Criterion 1's
    failure is in the canvas renderer, which a GPUI shell would keep. The
    checklist notes for criteria 4 and 5 hadn't come in yet.
- [x] **M2.8 Canvas on the integrated GPU** (done 2026-10-14; the gate's
  follow-up). The steps:
  - The script settles before each shapes phase, so criterion 2 is
    measured on its own.
  - The report times the GPU and records the monitor's size and scale.
  - Find where the iGPU's time goes, then cut it: the tile renders go
    to the GPU in one submission rather than one per tile, the tiles
    reach the screen without Vello's per-pixel image paint (the
    compositor of ADR-0003), and the tile budget is sized to what's left
    of the frame.
  - Find the slow first tile frame per board on the iGPU (100–144 ms).
  - The owner measures again: criteria 1 and 2 on the iGPU, fullscreen.
  - Done so far (2026-10-13): everything above except the compositor and
    the first-frame cause on the iGPU. On the VM the first text tile frame
    stalled in wgpu's clear of a 64 MB atlas page; it is now a 16 MB
    page.
  - The owner's runs (2026-10-14): C1 passes on the dGPU, the iGPU misses
    only the zoom's p99 (17.7 ms); C2 passes. Tiles filled the screen
    over 22–31 frames, and single tile submissions stalled for 58–117 ms
    on the iGPU.
  - Done (2026-10-14, ADR-0019): tiles on worker threads (`vello_cpu`),
    tile pages on the GPU, smaller GPU memory blocks, nearest sampling at
    1:1, items painted in place, glyph atlas up to 32 pages. Whether
    tiles still need a compositor pass of their own is for the next
    run's GPU times to say.
  - Next: the owner's fourth set of runs, criteria 1 and 2 on the iGPU.
- [x] **M2.9 Wrap-up** (2026-10-15/16):
  - [x] **The review of all M2 code**
    (https://github.com/giosuel/imaginarium/pull/33, `fix/m2.9-review`):
    - Four reviewer agents at medium effort, read-only.
    - Every finding was checked against the code; none was a false
      alarm. Each fix has a test that failed on the old code first,
      except where only a real desktop shows it (those have checklist
      items).
    - **High:**
      - AccessKit panicked on an update before its first-tree request,
        and on a focus without a node: every update is a whole tree now,
        tested with `accesskit_consumer`;
      - two lost-update bugs in the reactive graph: a memo writing during
        a check, and an effect whose update unwound.
    - **Medium:**
      - the clipboard read blocked the UI thread for up to 4 s: it has a
        thread of its own now, and pastes arrive as `Input::Paste`;
      - canvas invalidation scanned every cached tile (moving 10,000
        items: 299 ms, now 8.5 ms; new `benches/canvas.rs`);
      - the blank wait could upload past 16 tiles;
      - a hidden window redrew in a loop;
      - a screen reader could focus removed widgets;
      - the pointer capture ended on any button's release;
      - criterion 1's pass checked the p99 alone, with the present in it:
        it checks the frame rate too now (1 % of intervals over 20 ms at
        most, with vsync);
      - the typing counted the pan's owed tiles.
    - Reactive benchmarks are within noise (median of alternating runs).
      10,000 conditional readers toggling: 18.0 ms, now 1.47 ms.
    - Not changed, with reasons in the PR: tile pages aren't given back
      (M3's memory budget); access ids by slot; a composition's text isn't
      cleaned; the dragged shape's double frame (checklist 5b).
  - [x] **Developer docs** (`docs/m2.9-wrap-up`): the website's "UI
    toolkit" guide (`website/src/content/docs/developers/ui-toolkit.md`),
    the `counter` example (`crates/imaginarium-ui/examples/counter.rs`),
    the M2 crates in the architecture table, `ui-toolkit.md` accepted.
  - [x] `--backend cpu` dropped (owner, PLAN §14).
  - [x] **The owner's last gate runs:** "M2.9 final" and "M2.9 run 2"
    (iGPU, dGPU, no vsync): criteria 1, 2, 3 and 6 pass
    (`docs/design/m2-gate.md`).
  - [ ] **The manual checklist** (criteria 4 and 5): **deferred** by the
    owner (2026-10-16: "lets skip this for now, and I will do a proper
    test later. we can move on and close M2"). See Known issues.
  - [x] The tag `v0.0.3-m2` on `e499dc5`, the merge of #35, the last M2
    code (the owner pushes it).

**Wasm (browser) build: not a goal now** (owner, 2026-10-12: focus on the
native build, keep the browser build in mind). Core and toolkit crates stay
wasm-friendly where that is free: no `SystemTime::now()` or threads in core
paths, pure-Rust dependencies when the choice is otherwise equal. A web
viewer is in PLAN §12 "Later".

## M3: Shell and canvas (checklist)

The first usable editor: a board that opens, edits and saves `.imag`
files. PLAN §12 lists the palette, the contextual toolbar, the inspector,
the settings UI, basic shapes, selection and transform, themes, and local
saving and loading. M3 builds the product crates `imaginarium-canvas`
(the document on the M2 tile canvas) and `imaginarium-shell` (the window
around it), and `imaginarium-app` starts them. The gate app stays as a
benchmark until the app can measure the same (M3.9).

Effort: **xhigh** for the toolkit additions (M3.1) and the core of the
canvas (M3.2: nodes to items, the editing path), **high** for the rest.
One branch and PR per step (or smaller), a review after each, and
screenshots of every visible step (Xvfb, lavapipe).

What M3 builds on:
- **The model** (M1): `Document::transact` (the one way to change a
  document), `DocumentView` (`imbl`, readable on any thread), `ChangeSet`
  and undo per actor. Edits are cheap enough for the UI thread: a drag
  step on a 10,000-node board costs 29 µs, a keystroke in 300 pages
  59 µs (M1 benchmarks).
- **The registries** (M0): commands with keymaps and the palette's
  search, settings with UI metadata.
- **`.imag` files** (M1): opening and saving whole files; no incremental
  saving yet.
- **The toolkit and the tile canvas** (M2; ADR-0007, ADR-0019).

- [x] **M3.0 Design** (done 2026-10-16, #37): `docs/design/shell.md`,
  `docs/design/canvas.md`, ADR-0020 (the document on the UI thread) and
  ADR-0021 (one shape model), both accepted by the owner, and schema
  §5.6.
  - **Mockups first** (owner, 2026-10-16): the start window, the board
    window with its tools, the contextual toolbar, the inspector, the
    palette, menus and the settings, in light and dark, before any UI is
    built. They are HTML pages sent to the owner, not committed; what
    they settle goes into `shell.md`. The owner liked the first set
    (2026-10-16) and asked for more: documents, sheets, presentations
    and the draw tool on the board, and a tool bar that can be moved to
    any edge (PLAN §14), also mocked up before they are designed.
    Then the title bar (the system's), the icons (Lucide) and the extras
    were settled, and a dark set was made. The owner chose the dark theme
    to build first (PLAN §14). The mockups are done.
  - **Where the document lives:** on the UI thread, with opening, saving
    and big pastes on workers, or on a model thread of its own. Measured
    before deciding.
  - **The editing path:**
    - every edit is a command (invariant 2);
    - a drag or a burst of typing is one undo step;
    - text editing on the canvas turns into transactions.
  - **Nodes to canvas items:**
    - each kind and style is painted (schema §5, §8);
    - a `ChangeSet` becomes `upsert`, `remove` and `invalidate` calls;
    - hit testing;
    - the clipboard format for nodes.
  - **Vector shapes** (ADR): one model for simple shapes now and
    path-based shapes later.
    - As in SVG and Inkscape, a rectangle keeps its parameters (size,
      corner radius) and a path its path data, both `shape` nodes with the
      same styles and transform.
    - Every shape becomes a path for painting, hit testing, bounds and
      export, so paths add a kind of data, not a second pipeline.
    - "Convert to path" turns a rectangle into an editable path when the
      user asks, as in a vector tool (owner, 2026-10-16).
    - How path data is stored in the CRDT is designed now, and built when
      paths come.
  - **The window:**
    - the look: Excalidraw's clean, floating style, not its layout;
    - the pill and the tools in it, the contextual toolbar, the inspector,
      the palette and menus, with their layers and the focus between them;
    - the start window.
  - **Themes and icons:**
    - themes as tokens: light, dark, high contrast, following the system,
      reduced motion;
    - the icon set and its license.
  - **Saving:** the mechanics of Google Docs in a desktop application with
    native open and save dialogs (owner, 2026-10-16).
    - Every change is appended (`UPDT`s) on a worker within about a
      second, with crash recovery.
    - A new board starts untitled; its changes are kept for crash
      recovery until the save dialog gives it a place, and from then on
      nothing needs saving by hand. Closing a board with unsaved changes
      works as in any other program: Save…, Don't save or Cancel (owner,
      2026-10-16).
    - Newer files open read-only (`Mode`).
  - **Dependency research** for new crates (file dialogs through the
    desktop portal on Linux, icons), with the two-week rule.
- [x] **M3.1 Toolkit additions** (`imaginarium-ui`, `imaginarium-render`):
  - layers for popovers and menus: outside clicks, Esc, focus return
    (done 2026-10-16: popovers, modal dialogs and tooltips, placed by
    anchor with flipping, focus in and back, Tab kept inside; the gallery
    example shows them);
  - scroll containers with virtualized rows (done 2026-10-16: `Scroll`
    and `VirtualList`, a wheel step on 100,000 rows in 0.12 ms);
  - animation (time-based, and off with reduced motion; done
    2026-10-16: `Transition`, buttons fade between states, layers fade
    in, keyboard scrolls glide; reduced motion is a switch until M3.7
    reads `ui.reduce_motion`);
  - vector icons (done 2026-10-16: `cargo xtask icons` vendors the 98
    listed Lucide 1.50.0 icons, `Icon::named`);
  - theme tokens as signals, so a theme switches live (done 2026-10-16:
    `Theme`, `Token` and `Paint` in `imaginarium-ui`, dark first, the
    widgets on tokens, a contrast test; the counter example switches
    themes live);
  - access ids with generations (from M2.9; done 2026-10-16: a late
    screen reader request about a removed widget is refused).
- [x] **M3.2 `imaginarium-canvas`**: the document on the tile canvas
  (done 2026-10-17: `BoardCanvas`, `canvas.md` "As built" notes;
  benchmarks on this VM, release: a drag step 0.03 ms, hit testing
  0.009 ms, a style used by 1,000 of 10,000 nodes 5.5 ms, a paste of
  1,000 shapes 8 ms on the canvas plus 105 ms in the model; opening
  10,000 nodes 282 ms in the model and 44 ms for the items and the first
  frame, 512 ms and 200 ms with a label each). Not done: reading the
  nodes in bulk on open (below, Known issues); the drag lift wasn't
  needed (a drag redraws 1 to 4 tiles a frame).
  - **Painting:**
    - shapes: rectangle (also rounded), ellipse, line, triangle and
      diamond, each as a path (M3.0's vector model), with fill, stroke,
      radius and opacity from styles;
    - text boxes;
    - frames, which clip their children.
  - **Updates from `ChangeSet`s:** a remote change or an undo repaints only
    its tiles.
  - **Selection and transform:**
    - selection in signals, hit testing and a marquee;
    - handles to move, resize and rotate, and nudging;
    - each gesture is one undo step, through commands.
  - **Text editing in place**: plain text in M3 (rich text is M5), through
    transactions.
  - **Benchmarks:**
    - a 10,000-node board from the model: opening, the first frame, drag
      latency, `ChangeSet` to tiles;
    - opening such a board took 339 ms in M1; read the nodes in bulk.
- [x] **M3.3 Commands and keymaps** (done 2026-10-17, in
  `imaginarium-canvas`: 46 commands in `commands::board_commands()`,
  the default keymap in `keys.rs`, tools, copy and paste; `canvas.md`
  "As built" notes. **File commands moved to M3.6**, which builds files:
  they need the window and its dialogs):
  - **Board commands:**
    - insert each shape and text;
    - delete, duplicate, copy, cut and paste nodes;
    - order (front, back), group and ungroup, select all, nudge;
    - undo and redo;
    - zoom in, out, to fit and to 100 %.
  - **File commands.**
  - **Shortcuts** come from the keymaps of `imaginarium-commands`, replacing
    the gate's hard-coded ones.
  - **The command context:** the document, the selection and the view.
- [x] **M3.4 The shell** (`imaginarium-shell`, `imaginarium-app`), done 2026-10-17:
  - [x] **part 1, the board window** (2026-10-17, `shell.md` §3 "As
    built"): the pill, search, AI and share (placeholders that say what
    is coming), zoom and undo, help, and the tool bar, docked left,
    dragged to any edge or floating (`ui.toolbar.position`). Toolkit:
    hover tooltips, `IconButton`, `ButtonBox`, overlays that let the
    pointer through, clicks that keep the focus, the window's app ID
    (`ch.giosuel.Imaginarium`). Left for later parts: arrow keys in the
    tool bar, `F6`, double-click to keep a tool;
  - [x] **part 2, the command palette** (2026-10-17, `shell.md` §3.7
    "As built"): commands and settings in one search, highlighted
    matches, availability with reasons, settings changed in place,
    recent first. Not yet: arguments typed inline;
  - [x] **parts 3 and 4, the contextual toolbar and the context menu**
    (2026-10-17, `shell.md` §3.5 and §3.8 "As built"): style commands on
    the selection (fill, outline, width, radius, text color), the bar
    above the selection with its popovers, menus of commands for More,
    Arrange and right-clicks;
  - [x] **part 5, the inspector** (2026-10-17, `shell.md` §3.6 "As
    built"), with its commands (opacity, text size, Reset to style, named
    styles, rename, hide, shape type) and a slider in the toolkit;
  - [x] **part 6, the start window and windows** (2026-10-17, `shell.md`
    §4 "As built"): New board, Show at startup
    (`startup.show_start_window`), Open… and Recent waiting for files
    (M3.6); several windows in one event loop sharing the GPU and fonts,
    `Ctrl+N` and `Ctrl+W`, the start window again after the last board;
    the first frame 240–255 ms after start on llvmpipe (budget 400 ms);
  - the palette on the real registry, with commands and settings;
  - the contextual toolbar, built from the commands available for the
    selection;
  - the inspector (`Ctrl+I`: geometry and style);
  - the context menu as in its mockup: items with an icon, a label and
    the shortcut, separators, Delete in the danger color, built from the
    commands (the gallery's menu is a stand-in that shows the layer);
  - the start window, like Blender's splash: new boards, recent files,
    open;
  - one window per document;
  - the app's startup: settings, commands, fonts and the GPU in parallel,
    under 400 ms to an interactive empty board (PLAN §10).
- [ ] **M3.5 Settings UI**, generated from the registry:
  - search, scopes and every kind of setting;
  - changes apply at once;
  - `Ctrl+,`, and settings in the palette.
- [ ] **M3.6 Files:**
  - autosave of every change on a worker, and crash recovery;
  - native open and save dialogs; closing an unsaved board asks Save…,
    Don't save or Cancel; rename, move, save a copy;
  - recent files, and newer files opening read-only;
  - optionally, password-protected files (the crypto exists; it needs the
    prompt).
- [ ] **M3.7 Themes:** dark comes first and is what M3.1 to M3.6 build
  and test (owner, 2026-10-16); then light and high contrast on the same
  tokens, following the system, reduced motion, and their settings.
- [ ] **M3.8 From M2:**
  - **the board's opening:** cover the fill, maybe with an animation
    (owner);
  - the cards' text layout off the UI thread;
  - **the iGPU:** its slow frames and upload stalls;
  - **tiles:**
    - a window-wide memory budget for tile pages;
    - rendering the next zoom level ahead, if needed;
    - a shape shown twice as a drag starts, if seen.
- [ ] **M3.9 Measure and wrap up:**
  - **the app's measurements:**
    - the frame HUD (`F12`);
    - a scripted run like the gate's, folded in from `imaginarium-gate`;
  - the owner's run on the laptop;
  - the review of all M3 code;
  - **docs:** user docs on the website (getting started, the palette,
    shortcuts, files) and developer docs;
  - the tag `v0.0.4-m3`, with a Linux package.

**Order:**
1. M3.0 first.
2. M3.1 and M3.2 side by side (different crates).
3. Then M3.3 and M3.4.
4. M3.5 to M3.7 in any order.
5. M3.8 where it fits, and M3.9 last.

**The owner's answers** (2026-10-16, PLAN §14):
1. **The look:** clean, modern and simple, in the style and feel of
   Excalidraw's interface (not its layout); a highly capable,
   feature-rich multi-editor that stays simple to use.
2. **Windows:** one per document, and a start window like Blender's
   splash.
3. **Saving:** the save mechanics of Google Docs (nobody saves by hand),
   with native open and save dialogs.
4. **Shapes:** the simple ones first, stored and handled the way
   path-based vector shapes (as in Inkscape) will be later. A rectangle
   stays a rectangle and can be converted to a path. Images and
   connectors come later.

## Open questions (owner)

- Hub name: "Imaginarium Hub" is the working name (not yet confirmed).

## Known issues and notes

- **Two render tests are timing-sensitive**: `canvas::tests::
  the_budget_owes_tiles_and_stand_ins_fill_them` and
  `uploads_follow_what_they_cost` failed once in a full workspace run
  while other sessions loaded the machine (load average about 5), and
  passed on their own and in the next full run (2026-10-17).

- **Opening a big board is slow in the model** (M3.2 benchmarks): 282 ms
  for 10,000 shapes (Loro import and reading the view key by key), and
  the canvas lays out every label on the UI thread at open (200 ms for
  10,000). To do before boards that big are common: read the nodes in
  bulk on the opening worker (`LoroTree::get_value_with_meta`, ADR-0020),
  and lay out off-screen labels lazily or on a worker.
- **A gesture right after another edit undoes together with it**: the
  model merges undo steps within 500 ms (`UndoConfig`), groups included.
- **The demo board can be undone** (M3.4): `--demo` builds the Launch
  plan board as the user's edits, so undo takes it apart. Opened files
  (M3.6) start without history.
- **Pasting big copies parses on the UI thread** (M3.3): a payload is
  capped at 64 MB; moving the parsing to a worker is left for when
  pastes get that big. Also not yet: a paste doesn't remap references
  inside the copy (connectors' ends, `gen.inputs`; connectors come in
  M4), carries asset ids to boards without those blobs (images come in
  M4), and leaves a paragraph style (`ps`) dangling on another board.
- **Text editing**: a collaborator typing before the caret moves it by
  their text's length; with tile workers the edited text shows twice for
  a frame when editing starts.

- **The M2 gate's manual checklist hasn't been run** (deferred by the
  owner, 2026-10-16). So criterion 4 (text input with an input method,
  and Orca) and criterion 5 (HiDPI, fractional and mixed scales) are
  unchecked on a real desktop; the automated tests cover the logic
  (IME events, the accessibility tree through `accesskit_consumer`), not
  the desktop integration. The owner runs it later
  (`docs/design/m2-gate.md`, "Manual checklist"); what it finds is fixed
  in the milestone then under way.

- **Third-party notices don't ship yet.** The licenses of what the app
  bundles (the Lucide icons' ISC and MIT notices in `LICENSE-lucide`, the
  bundled fonts, the Rust dependencies) must reach users with the
  binaries: a notices file in the packages or an About screen, before the
  first release with the shell.

- **Signatures are verified on open** (since M1.8: every record a document
  with an ACL chain is read from), but that is slow for big shared
  documents: about 70 µs per Ed25519 record on this VM, so ~0.7 s for 10k
  updates. Decide in M8: verify on receipt and remember it in the local
  cache, use batch verification, or verify only what the newest snapshot
  doesn't cover. The ACL replay (36 ms per 100 changes) can be cached by
  head hash in the same way.
- **Deferred from the M1 review** (for M8, when shared documents are
  written and synced):
  - remember the newest ACL head per document and refuse older ones (a
    rolled-back chain looks genuine, spec §12);
  - writers must compare the newest epoch key they hold with
    `AclState::epoch()` before encrypting (spec §8.3);
  - the app needs the expected owner for each recipient-key document
    (ADR-0018): the account identity, or a value stored with the
    document's library entry.
- `imag inspect`/`verify` read the whole file into memory (about 2× the
  file size at peak). Stream the scan when large files with blobs exist.
- `AtomicFile` keeps the replaced file's permissions, but not its owner,
  extended attributes or ACLs.
- **Loro depends on `im` 15.1.0, which RustSec lists as unmaintained**, and
  so are its `bitmaps` and `sized-chunks` (RUSTSEC-2026-0247/-0248/-0251).
  These are not vulnerabilities. They are ignored in `deny.toml` with
  reasons. Re-check on every Loro update and remove the ignores once Loro
  moves off `im`. Our own code uses `imbl`.
- **Opening a 10k-shape board takes 339 ms** on this VM (Loro import ~90 ms;
  building the view ~25 µs per node). Optimize in M2, when boards render:
  read nodes in bulk (`LoroTree::get_value_with_meta`) instead of key by
  key.
- Typing at one spot copies that run's text per keystroke, which is fine
  up to tens of KB per run. Very long single runs would need a rope.
- HPKE unwrap (303 µs) is 3× the design estimate, because `x-wing` 0.1.0
  expands the private seed (ML-KEM key generation plus X25519) on every
  decapsulation (checked in its source, `DecapsulationKey::decapsulate`). It runs
  once per open, so it isn't worth patching now. Re-check after updating
  `x-wing` (pinned until 0.1.1 is two weeks old).
