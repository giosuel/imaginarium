# Status: hand-off log

> **Fresh session?** Read [`MAINTAINERS.md`](../MAINTAINERS.md) first, then this file.
> Continue at **"Next step"**. Update this file at the end of every work
> step and commit it.

Last updated: 2026-10-12

## Current position

- **Phase:** **Phase A, M1 (Format, crypto, model)**. M0 is done (tag `v0.0.1-m0`).
- **Next step:** **M1.4 `imaginarium-crypto`** (see the M1 checklist below). Use **xhigh/max effort** for M1.
- **Code status:** Cargo workspace with 6 crates. Working: `xtask` (repo
  checks and dev-profile generator), `imaginarium-profiling` (tracing,
  Tracy, frame stats), `imaginarium-test-utils` (fixtures) and
  `imaginarium-settings` (typed registry, TOML) and `imaginarium-commands`
  (registry, args/JSON Schema, keymaps, palette search). The app only prints its
  version.
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

## M1: Format, crypto, model (checklist)

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
- [ ] **M1.4 `imaginarium-crypto`** ← **next**:
  - suite 1 wrappers;
  - key hierarchy (epoch keys, labels);
  - keyring (password via Argon2id, account and device keys via X-Wing);
  - record encryption and segmented blobs (STREAM);
  - signatures (Ed25519, hybrid with ML-DSA for authority records), ACL
    chain verification;
  - test vectors in `docs/format/test-vectors/`;
  - benchmarks against the throughput table in the design.
- [ ] **M1.5 CRDT benchmark**: Loro vs Yrs vs Automerge on our workloads
  (10k-shape board, 300-page document, concurrent edits; memory, encode
  size, apply/merge time) using the `test-utils` fixtures. Confirm ADR-0009
  or supersede it.
- [ ] **M1.6 `imaginarium-model`**:
  - schema (§3 of the design) on the chosen CRDT;
  - `imbl` view model, transactions, `ChangeSet`;
  - collaboration-aware undo;
  - migrations and `required_features`;
  - proptests: convergence, undo inverse.
- [ ] **M1.7 `imaginarium-cli` (`imag`)**: `inspect`, `verify`,
  `export-json`.
- [ ] **M1.8 Wrap-up**:
  - code review, publish the test vectors, update the website docs;
  - **automate the two-week rule** (an xtask check comparing new
    `Cargo.lock` entries against crates.io publish dates; done by hand so
    far);
  - a **weekly CI fuzz job** (nightly, time-boxed);
  - tag `v0.0.2-m1`.

## Open questions (owner)

- Hub name: "Imaginarium Hub" is the working name (not yet confirmed).

## Known issues and notes

- None yet.
