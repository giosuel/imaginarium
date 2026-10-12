# Status: hand-off log

> **Fresh session?** Read [`MAINTAINERS.md`](../MAINTAINERS.md) first, then this file.
> Continue at **"Next step"**. Update this file at the end of every work
> step and commit it.

Last updated: 2026-10-12

## Current position

- **Phase:** **Phase A, M0 (Foundations and profiling)**
- **Next step:** **M0.2 xtask checks** (see the checklist below)
- **Code status:** Cargo workspace with 6 skeleton crates. Builds, tests,
  clippy, fmt and docs are all green. There is no functionality yet beyond
  the app printing its version and `cargo xtask help`.
- **Remote:** none yet. The owner adds it later. Commit locally.
- **Environment seen in the planning session:** Linux, Rust 1.99.0 stable,
  Node 22 / npm 10 (for the website). `cargo-deny` and `cargo-machete` are
  not installed yet (`cargo install --locked cargo-deny cargo-machete`).
  The pinned toolchain `1.99.0` (with rustfmt and clippy) was auto-installed
  by rustup next to `stable`.

## Completed

- [x] Plan v2 (`docs/PLAN.md`), ADRs 0001–0016, file-format design (accepted)
- [x] Project name **Imaginarium**, license **AGPL-3.0-or-later**, **DCO**
- [x] `MAINTAINERS.md`, `CONTRIBUTING.md`, this status file (agent files are git-ignored)

## M0: Foundations and profiling (checklist)

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
    - The workspace version is `0.0.1`. `repository`/`homepage` are added
      to the workspace package once the remote exists.
- [ ] **M0.2 xtask checks** ← **next**
  - `cargo xtask check-layers`: verify that crate dependencies respect the
    layer map (define the layer table in `xtask`, mirroring PLAN §4.1) and
    that every `.rs` file has an SPDX header.
  - Also enforce that `imaginarium-test-utils` is only a dev-dependency and
    that nothing depends on `xtask`.
  - Implementation hint: read `cargo metadata --format-version 1 --no-deps`
    and parse it. Consider adding `serde_json` to `xtask` only, which keeps
    product crates dependency-free.
  - AC: a deliberately wrong dependency makes it fail (unit test).
- [ ] **M0.3 CI** (`.github/workflows/ci.yml`)
  - Matrix: ubuntu, windows, macos.
  - Jobs: fmt, clippy, test, doc (`-D warnings`), `cargo deny`
    (`deny.toml` with an AGPL-compatible license allowlist: MIT, Apache-2.0,
    BSD-2/3, ISC, Zlib, MPL-2.0, Unicode-3.0, LGPL/GPL-3.0-compatible as
    reviewed), `cargo machete`, `xtask check-layers`, and a DCO sign-off
    check.
  - AC: the workflow YAML is valid (it can't run until there is a remote;
    note that here).
- [ ] **M0.4 `imaginarium-profiling`**
  - `tracing` subscriber setup (env filter, pretty/JSON).
  - Optional `tracy` feature (`tracing-tracy`).
  - `FrameStats` ring buffer (p50/p95/p99 frame time) for the later HUD.
  - Span naming conventions documented.
  - Benchmark harness conventions (criterion), with a shared fixture API in
    `test-utils` (stub generators for "10k-shape board" and "300-page
    document", filled in later).
  - AC: unit tests for the percentile maths, and a criterion example bench
    that runs.
- [ ] **M0.5 `imaginarium-settings`**
  - Typed registry: key, type (bool/int/float/string/enum/color/path/
    secret-ref/list), default, validation, **scope** (app, user, workspace,
    document, hub), UI metadata (title, description, synonyms, group,
    advanced).
  - Change notifications (a simple observer for now; it becomes signals
    later).
  - App-scope TOML persistence.
  - Secrets are never stored in TOML, only as a keychain reference.
  - AC: tests for registration, validation, persistence round-trip, and a
    "read of an unregistered key is a compile or runtime error" test
    (ADR-0014).
- [ ] **M0.6 `imaginarium-commands`**
  - `Command` trait (id, meta, `ArgsSchema` exportable as JSON Schema,
    availability, run). The `Context` stays abstract or generic so no model
    dependency exists yet.
  - Registry and keymap model (platform-aware `Mod` = Ctrl/Cmd).
  - **Fuzzy search index** with synonyms and a recency/frequency boost.
  - AC: a benchmark searching **2,000 commands in < 1 ms per keystroke**
    (one of the M2 gate criteria), plus tests for ranking.
- [ ] **M0.7 Website skeleton** (`website/`, Astro Starlight)
  - Sections: Home, Download (placeholder), User Guide, Self-hosting,
    Developer Docs (Architecture overview with a crate-map diagram, ADR
    index, File format, Contributing), and API reference link.
  - CI build job.
  - AC: `npm run build` succeeds locally.
- [ ] **M0.8 Wrap-up**
  - Fill in the build commands in `MAINTAINERS.md`.
  - Code review of M0.
  - Update this file, then tag `v0.0.1-m0`.

## After M0 (preview)

**M1, Format, crypto, model** (use xhigh/max effort). See PLAN §12.

1. Write the normative spec `docs/format/spec-v1.md` and the `.proto` files,
   derived from `docs/design/file-format.md`.
2. Write `docs/design/sync-protocol.md`.
3. `imaginarium-format`: records, index, append, compaction, recovery, and
   fuzz targets.
4. `imaginarium-crypto`: suite 1 (hybrid PQ), keyring, ACL chain, test
   vectors.
5. `imaginarium-model`: Loro schema, view model, undo, migrations, and the
   Loro/Yrs/Automerge benchmark.
6. `imag` CLI (`inspect`, `verify`, `export-json`).

## Open questions (owner)

- Hub name: "Imaginarium Hub" is the working name (not yet confirmed).

## Known issues and notes

- None yet.
