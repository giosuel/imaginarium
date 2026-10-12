# 0017. Dev build profile: optimized dependencies, unoptimized build-time crates

- Status: Accepted
- Date: 2026-10-12

## Context
Debug builds must be usable: rendering, text shaping, compression and CRDT
code are far too slow at `opt-level = 0`. So the dev profile optimizes all
dependencies (`[profile.dev.package."*"] opt-level = 2`). The development
machine is CPU-bound (see docs/STATUS.md, "Development machine"), and
agents often do clean builds in fresh worktrees, so clean-build cost
matters.

Measurements on a clean dev build of the planned heavy stack (vello/wgpu,
winit, parley, taffy, accesskit, loro, crypto):

| Variant | CPU-s |
|---|---|
| deps opt 2, full debug info | 2,355 |
| + deps line tables only | 2,099 |
| + codegen-units = 16 for deps | 2,116 (no effect: already the default) |
| + build-time-only crates at opt 0 | **1,415** |

`package."*"` takes precedence over `build-override`, so it also optimizes
proc macros and build scripts. `syn` alone cost 354 s versus 72 s in
release. Unoptimized macros make incremental rebuilds of a crate with 450
serde/thiserror derives 21.7 s versus 22.4 s, which is within noise.

## Decision
1. Dependencies: `opt-level = 2`, `debug = "line-tables-only"` (backtraces
   keep `file:line`).
2. Crates used **only at compile time** get `opt-level = 0` through named
   overrides in a **generated block** in the root `Cargo.toml`.
   `cargo xtask dev-profile --write` computes it from the dependency graph
   of all platforms. `cargo xtask check` (and therefore CI) fails when it is
   stale.
3. `syn`, `quote`, `proc-macro2` and `unicode-ident` always count as
   build-time, even though two crates use them trivially at runtime.

## Consequences
- Clean dev builds take about a third less CPU (this workspace: −47 %).
- After adding or updating dependencies, run `cargo xtask dev-profile
  --write` and commit `Cargo.toml` together with `Cargo.lock`.
- If a runtime-relevant crate is wrongly classified as build-time, it would
  run unoptimized in debug builds. The classification follows the actual
  graph, and the `ALWAYS_BUILD_TIME` exceptions are listed and justified in
  `xtask/src/dev_profile.rs`.

## Alternatives considered
- **Keep optimizing everything:** ~33 % slower clean builds for no benefit.
- **opt 0 by default, opt 2 only for named hot crates:** missing a crate
  would make debug builds slow at runtime, which is a worse failure mode than
  a slightly slower build. The list would also need manual curation.
- **Hand-maintained opt-0 list:** goes stale with every dependency update,
  and Cargo warns about stale names. Generation and a CI check solve that.
