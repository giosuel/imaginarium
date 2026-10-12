---
title: Contributing
description: How to contribute to Imaginarium.
---

Contributions are welcome. The full rules live in the repository:

- [`CONTRIBUTING.md`](https://github.com/giosuel/imaginarium/blob/main/CONTRIBUTING.md):
  workflow, DCO, checks.
- [`MAINTAINERS.md`](https://github.com/giosuel/imaginarium/blob/main/MAINTAINERS.md):
  architecture invariants and conventions for maintainers and AI agents.

## In short

- **License:** AGPL-3.0-or-later. Every commit is signed off under the
  [DCO](https://developercertificate.org/) (`git commit -s`), and CI checks it.
- **Pull requests** go into `main` and are merged with merge commits. Keep
  commits small and meaningful ([Conventional Commits](https://www.conventionalcommits.org/)).
- **Before pushing:**
  - `cargo fmt --all`
  - `cargo clippy --workspace --all-targets -- -D warnings`
  - `cargo test --workspace`
  - `cargo xtask check`
  - `cargo deny check`
- **Bigger changes** start with an issue or discussion. Lasting design
  decisions get an ADR.
- **Docs are part of done:** update the user guide or developer docs with
  your change.
