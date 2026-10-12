---
title: Developer documentation
description: How Imaginarium is built, and how to contribute.
---

Imaginarium is a Rust workspace (AGPL-3.0-or-later). These pages explain
**how it is built and why**. Most of them are synced from the repository's
`docs/` folder, which stays the single source of truth.

## Start here

1. [Architecture](/imaginarium/developers/architecture/): the crate layers,
   data flow and key mechanisms.
2. [Project plan](/imaginarium/developers/reference/plan/): vision, UX
   principles, roadmap.
3. [Architecture decisions](/imaginarium/developers/reference/adr/0001-workspace-and-ci/):
   one short record per significant decision, superseded rather than edited.
4. [`.imag` file format](/imaginarium/developers/reference/design/file-format/):
   the encrypted, CRDT-based document format.
5. [Contributing](/imaginarium/developers/contributing/): workflow, DCO,
   checks.

## Building and checking

```sh
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo xtask check        # layering, licenses, SPDX headers, tracked files, dev profile
cargo deny check         # dependency licenses and advisories
```

Rust API docs: `cargo doc --workspace --no-deps --open`. They will be
published here later.
