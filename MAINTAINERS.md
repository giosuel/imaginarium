# Maintainer guide

Rules and conventions for everyone who works on Imaginarium: human
maintainers and AI coding agents alike. Agent tooling files (`CLAUDE.md`,
`AGENTS.md`, `.claude/`, …) are **never committed**. They are git-ignored
and at most contain a local pointer to this file.

Imaginarium is a cross-platform (Linux, Windows, macOS) office suite in Rust.
It covers boards/diagrams, documents, presentations, page layout and paint
on one whiteboard-like surface, with built-in generative AI and end-to-end
encrypted real-time collaboration through a self-hostable server ("Hub").
License: AGPL-3.0-or-later.

## Start here (every session)

Read these in order before doing anything:

1. **[`docs/STATUS.md`](docs/STATUS.md)**: where we are and the **next
   concrete step**. This is the hand-off point between sessions.
2. **[`docs/PLAN.md`](docs/PLAN.md)**: vision, architecture, roadmap,
   decisions log.
3. **[`docs/adr/README.md`](docs/adr/README.md)**: index of architecture
   decisions. Read the ADRs relevant to your task.
4. **[`docs/design/`](docs/design/)**: detailed designs (start with
   `file-format.md` for anything touching model, format, crypto or sync).
5. This file's rules (below), and [`CONTRIBUTING.md`](CONTRIBUTING.md).

**At the end of every work step**, update `docs/STATUS.md`: tick what's
done, write down the next step and any open problems, then commit. A fresh
session must be able to continue from `STATUS.md` alone.

## Working with the owner

- The owner (Giosuel) decides product and architecture questions. **Answer
  questions before starting work** when they ask something. Give a clear
  recommendation with trade-offs, not an exhaustive survey.
- Don't silently reverse a decision recorded in an ADR. Propose a
  superseding ADR and ask.
- Ask when a decision is genuinely the owner's (naming, licensing, product
  scope). Otherwise choose sensible defaults and mention them.

## Git policy (mandatory)

- Repo-local identity: `user.name = Giosuel`, `user.email = me@giosuel.ch`.
  Check this with `git config user.name` before committing. If it is
  missing, set it locally (never globally).
- **Commit after every module, refactor and decision.** Small, focused
  commits. Don't batch unrelated work.
- **Conventional Commits**: `feat(model): …`, `fix(format): …`,
  `docs(adr): …`, `refactor(ui): …`, `chore: …`, `test: …`, `perf: …`,
  `ci: …`.
- Always commit with **`-s`** (DCO `Signed-off-by`, see `CONTRIBUTING.md`).
- AI agents keep their co-author trailer (`Co-Authored-By: Claude …`), plus
  any session attribution line provided by the harness.
- **Never commit agent or tool files.** This includes `CLAUDE.md`,
  `CLAUDE.local.md`, `AGENTS.md`, `.claude/`, `.cursor/` and the other tool
  directories listed in `.gitignore`. Check `git status` before every commit.
- **Never force-push or rewrite published history.**

## Pull request workflow (mandatory)

Remote: `origin` = <https://github.com/giosuel/imaginarium> (public).

The branch rules for `main` are versioned in
[`.github/rulesets/main.json`](.github/rulesets/main.json). To change a rule,
edit the file in a PR. The owner then imports it under Settings → Rules →
Rulesets. Agents push with the owner's token, so the ruleset requires 0
approvals (GitHub forbids self-approval) and has no bypass actors. Any
bypass would apply to agents too.

1. **Never commit directly to `main`.** Start each unit of work on a branch
   cut from an up-to-date `main` (`git switch main && git pull && git switch
   -c <branch>`).
   - Branch names: `<type>/<short-topic>`, e.g. `feat/m0.2-xtask-layers`,
     `docs/sync-protocol`, `fix/format-trailer-crc`.
   - One branch and PR per STATUS step (e.g. M0.2), or smaller.
2. Inside the branch, still **commit after every module, refactor and
   decision**. Don't squash locally: the commit history is part of the
   record.
3. Before opening the PR, run every check from "Build and check commands",
   and update `docs/STATUS.md` in the same PR (tick the step, set the next
   one).
4. Push (`git push -u origin <branch>`) and open the PR with `gh pr create`
   against `main`. Fill in the template (`.github/pull_request_template.md`).
   AI agents end the PR description with the attribution lines provided by
   the harness.
   - Gotcha: `gh pr edit` fails with gh 2.46 ("Projects (classic) is being
     deprecated"). Use the REST API instead:
     `gh api -X PATCH repos/giosuel/imaginarium/pulls/<n> -f title=… -F body=@file.md`.
5. **The owner reviews and merges.** Agents don't merge their own PRs unless
   the owner explicitly says so for that PR.
   - Merge strategy: **merge commit** (no squash, no rebase-merge). This keeps
     the per-module commits and their SHAs, which `STATUS.md` references.
6. After the merge: `git switch main && git pull`, delete the local branch,
   and continue with the next step on a new branch. The next step may be
   started on a branch based on the unmerged PR branch if it depends on it.
   Say so in the PR description.

## Reporting to the owner (mandatory)

1. **Every change goes into `main` through a PR** (see above). There are no
   exceptions, not even for docs.
2. **Screenshots:** whenever there is a runnable version with something
   visible (from the M2 prototype onwards), take screenshots and **send them
   to the owner directly**. Never commit them. For UI changes, send a
   before/after pair, and mention the screenshots in the PR description.
3. **Profile heavy tasks.** Measure every heavy task (clean and incremental
   builds, release builds, the test suite, clippy, benchmarks, website
   builds) and report:
   - wall time,
   - CPU utilization (are all cores saturated?),
   - peak memory and swap,
   - the machine specs.

   Report this in the PR description and to the owner, so they can judge
   whether the development machine is a bottleneck. Flag a bottleneck
   explicitly, for example "clean release build takes more than 10 minutes
   with all cores at 100 %: more cores would help", or "the linker hits the
   RAM limit and swaps: more RAM is needed". Use `cargo build --timings` for
   per-crate build cost.
   - Compare **CPU-seconds** (user + sys), not only wall time. The machine
     may be shared, which skews wall time.
   - Scratch builds and experiments use a subdirectory of the repo's
     `target/` (e.g. `CARGO_TARGET_DIR=target/scratch-<name>`), **never
     `/tmp`**. It may be a small RAM-backed tmpfs. Clean them up afterwards
     (`cargo clean --target-dir …`).
4. **Never commit generated artifacts:** screenshots, profiling outputs
   (timings HTML, tracy captures, flamegraphs, perf data, reports), or any
   agent or tool files. Keep them in the agent's scratch directory or in the
   git-ignored `artifacts/` and `profiling/` directories. The only shared
   rules file is this one, `MAINTAINERS.md`. Test fixtures that the test
   suite needs (e.g. golden images under a crate's `tests/`) are source code,
   not artifacts, and are committed.

## Architecture invariants (do not break)

1. **Layering:** dependencies point downwards only
   (apps → product → features → services → toolkit → core → support; see
   PLAN §4.1). Core crates (`model`, `format`, `crypto`, `commands`,
   `protocol`) must never depend on UI, networking, async runtimes or AI.
2. **One mutation path:** every document change is a **command → CRDT
   transaction**. This applies to the user, AI agent, add-ons and remote
   peers alike. There is no other way to mutate the model.
3. **Never block the UI thread:** IO, crypto, compression, parsing, layout
   of off-screen content, AI and network calls run in the worker pool or
   async runtime.
4. **Every setting is registered** in the typed settings registry with UI
   metadata (ADR-0014). There are no config-file-only settings.
5. **Every feature is a command** with metadata and an argument schema. That
   makes it searchable in the palette and usable as an AI tool.
6. **File format compatibility** (`docs/design/file-format.md` §3.4, §8):
   additive changes only, unknown data preserved, no reused protobuf field
   numbers, record type ids never reused.
7. **Crypto:** never implement primitives yourself, and use only the crates
   named in the format design. Never log keys, plaintext or secrets.
   Crypto-related changes need tests against test vectors.
8. **Performance budgets** (PLAN §10) are requirements. Add or extend a
   benchmark when you touch a hot path.

## Code conventions

- Rust edition 2024, stable toolchain pinned in `rust-toolchain.toml`.
- Each file starts with `// SPDX-License-Identifier: AGPL-3.0-or-later`.
- Each crate has a crate-level `//!` doc stating its responsibility, its
  boundaries ("does NOT do …") and its key types. `#![warn(missing_docs)]`.
- Errors: `thiserror` in libraries, `anyhow` only in binaries. No
  `unwrap()`/`expect()` outside tests (clippy-enforced). `unsafe` is
  forbidden unless an ADR justifies it.
- Tests live next to the code. Add proptests for invariants (CRDT
  convergence, undo, format round-trip) and fuzz targets for parsers.
- Comments explain *why*, not *what*. Public APIs have doc examples.
- Crate naming: `imaginarium-<name>` (directory `crates/imaginarium-<name>`).
- New crates only when a milestone needs them. Register each new crate in
  the PLAN crate map if it isn't already listed.

## Build and check commands

The toolchain is pinned in `rust-toolchain.toml`, and rustup installs it
automatically on first use.

```sh
cargo build --workspace                                   # debug build
cargo run -p imaginarium-app                              # run the app (binary: `imaginarium`)
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps
cargo xtask check                                         # layering, dependency rules, licenses, SPDX headers, tracked files
cargo deny check                                          # dependency licenses, advisories, bans, sources (deny.toml)
cargo machete                                             # unused dependencies
cargo xtask help                                          # list repo automation tasks
cargo build --profile profiling                           # release + debug info, for tracy/perf
```

Website (`website/`, see its README): `npm ci`, then `npm run dev` or
`npm run build`. Developer reference pages are generated from `docs/`, so
edit the originals there.

CI (`.github/workflows/ci.yml`) runs all of the above on every PR, with
tests on Linux, Windows and macOS. Only the aggregate **"CI result"** job is
required by the ruleset, so jobs can be added without a ruleset change. Pin
new or updated actions to a full commit SHA, with the tag in a comment.

Workspace layout: product crates live in `crates/imaginarium-<name>/`,
repo tooling in `xtask/`. Shared lints and profiles are in the root
`Cargo.toml`. Every crate sets `[lints] workspace = true`.

**Dependency versions** (Rust and npm): adopt a version only once it has
been public for **at least 14 days**, transitive dependencies included
(`npm install --before=<date>`; for Cargo, check `cargo tree` and pin with
`cargo update -p <crate> --precise <version>` if needed). Exception:
security fixes, documented in the commit. Rationale: freshly published
releases are the usual window for compromised packages.

**After adding or updating dependencies:** run `cargo xtask dev-profile
--write` and commit `Cargo.toml` together with `Cargo.lock`. It regenerates
the dev-profile overrides for compile-time-only crates (ADR-0017). `cargo
xtask check` and CI fail while they are stale.

**Profiling:** see the `imaginarium-profiling` crate docs.
- Logs: `IMAGINARIUM_LOG=debug`.
- Tracy: `--features imaginarium-profiling/tracy` with
  `--profile profiling`.
- Benchmarks: `cargo bench -p <crate>`, following the conventions in those
  docs.

**Test vectors** (`docs/format/test-vectors/`, spec §13): the `vectors`
tests of `imaginarium-format` (1–4) and `imaginarium-crypto` (5–10) check
that we write the published files byte for byte. The crypto ones use
seeded randomness from the `test-vectors` feature, which only the test
build enables (the crate dev-depends on itself). Never enable it anywhere
else. Regenerate only together with a deliberate spec change:
`IMAGINARIUM_UPDATE_VECTORS=1 cargo test -p <crate> --test vectors`, then
run the tests again without the variable.

**Fuzzing** (parsers of untrusted input: the format and later the sync
protocol): the targets live in `crates/<crate>/fuzz/`, a separate workspace
that needs a nightly toolchain.
- Run: `cd crates/imaginarium-format/fuzz && cargo +nightly-2026-09-25 fuzz
  run open_file -- -max_total_time=120`.
- Seed new targets with the test vectors.
- A crash found by fuzzing becomes a regression test in the crate before
  it is fixed.

**Declaring a setting:** add a `static Setting<T>` next to the code that
uses it. Title and description are required, and the key is dotted
`snake_case` and stable forever. Add it to the crate's settings list, which
the app registers at startup. Secrets are always `SecretRef` (OS
keychain). See the `imaginarium-settings` crate docs.

**Writing a command:** declare a `static FnCommand` with a stable id
(`area.verb_object`), a *specific* description (AI models read it),
synonyms, typed arguments with limits, and a cheap availability check. See
"Writing a command" in the `imaginarium-commands` crate docs.

**Adding a crate:** create `crates/imaginarium-<name>/` with the workspace
package fields and `[lints] workspace = true`. Then check that it is in
the layer table (`LAYERS` in `xtask/src/layers.rs`) and in the PLAN §4.1
crate map; most planned crates are listed there already. `cargo xtask
check` fails for crates without a layer.

## Decisions and docs

- New significant decision → new ADR (`docs/adr/NNNN-title.md`, from the
  template) + index row in `docs/adr/README.md` + a commit of its own.
- **Docs are part of done:** a milestone is not finished until its user and
  developer docs exist (website lives in `website/`, from M0).
- Owner decisions are logged in PLAN §14.

## AI agent usage (for the orchestrating session)

- One main session acts as architect and integrator. Subagents handle
  bounded tasks: research, codebase exploration, independent crates in
  separate worktrees, and reviews.
- Don't have several agents edit the same crate concurrently.
- Suggested effort: **xhigh or max** for format, crypto, CRDT model, ACL and
  UI-toolkit core; **high** for feature milestones; **medium** for docs,
  CI and mechanical work.
- Run a code review (`/code-review`) after each module before moving on.
