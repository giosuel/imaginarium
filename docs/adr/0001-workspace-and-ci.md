# 0001. Rust workspace, toolchain and CI

- Status: Accepted
- Date: 2026-10-12

## Context
The project will grow large, and both humans and AI agents will maintain it.
Compile times, clear boundaries and reproducible builds on three OSes all
matter.

## Decision
- Use one Cargo workspace with many small crates (`crates/imaginarium-*`) and a
  shared `[workspace.dependencies]` table.
- Use the stable toolchain, pinned via `rust-toolchain.toml`. The MSRV is
  bumped deliberately.
- Run CI on GitHub Actions with a matrix of ubuntu-latest, windows-latest and
  macos-latest: fmt, clippy (`-D warnings`), tests, `cargo deny`, and docs
  (`-D rustdoc::broken_intra_doc_links`).
- Use Conventional Commits.

## Consequences
- Small crates give incremental builds and enforce boundaries.
- Having more crates means more `Cargo.toml` boilerplate. That is accepted.
