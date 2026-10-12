# Contributing to Imaginarium

Thanks for your interest! This document explains how to contribute. The
architecture and roadmap are in [`docs/PLAN.md`](docs/PLAN.md). The current
progress is in [`docs/STATUS.md`](docs/STATUS.md).

## License and Developer Certificate of Origin (DCO)

Imaginarium is licensed under **AGPL-3.0-or-later**. By contributing, you
agree that your contribution is licensed under the same terms.

We use the [Developer Certificate of Origin](https://developercertificate.org/)
instead of a CLA. Every commit must carry a sign-off line:

```
Signed-off-by: Your Name <your.email@example.com>
```

Add it automatically with `git commit -s`. With the sign-off you certify
that you wrote the change or otherwise have the right to submit it under
the project's license. CI rejects commits without a sign-off.

Add-ons must also be AGPL-compatible (ADR-0008). Template and asset
packages may use content licenses such as CC-BY, declared in their
`package.toml`.

## Workflow

1. For anything larger than a bug fix, open an issue or discussion first.
   Significant design changes need an **ADR** (`docs/adr/`, see its README).
2. Keep commits small and focused, and use **Conventional Commits**
   (`feat(scope): …`, `fix(scope): …`, `docs: …`, `refactor: …`, `perf: …`,
   `test: …`, `ci: …`, `chore: …`).
3. Before pushing, run:
   ```sh
   cargo fmt --all
   cargo clippy --workspace --all-targets -- -D warnings
   cargo test --workspace
   cargo deny check
   cargo xtask check-layers
   ```
4. Update docs with your change (user guide and/or developer docs). Docs
   are part of done.

## Code guidelines

The full list is in [`CLAUDE.md`](CLAUDE.md#architecture-invariants-do-not-break).
It applies to humans too. The most important rules:

- Respect crate layering, and never mutate documents outside the
  command/transaction path.
- Never block the UI thread.
- Every feature is a command, and every setting is registered with UI
  metadata.
- Keep the file format backwards compatible: additive changes only, and
  preserve unknown data.
- No home-grown cryptography.
- Every source file starts with `// SPDX-License-Identifier: AGPL-3.0-or-later`.
- No `unwrap()` outside tests, and no `unsafe` without an ADR.

## Reporting security issues

Please **don't** open public issues for vulnerabilities. Until a
`SECURITY.md` with a contact address exists, email the maintainer at
me@giosuel.ch.
