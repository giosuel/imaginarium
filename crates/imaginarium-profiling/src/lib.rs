// SPDX-License-Identifier: AGPL-3.0-or-later

//! Profiling and instrumentation for Imaginarium.
//!
//! Performance is a feature (PLAN §10), so measurement infrastructure exists
//! before any feature work. Every crate may depend on this one (support
//! layer).
//!
//! # Responsibility
//! - [`subscriber`]: process-wide [`tracing`] setup. It reads the filter
//!   from config or `IMAGINARIUM_LOG`/`RUST_LOG`, and writes compact,
//!   pretty or JSON logs to stderr.
//! - Optional [Tracy](https://github.com/wolfpld/tracy) integration behind
//!   the `tracy` cargo feature: spans stream live to the Tracy GUI.
//! - [`frame`]: frame-time statistics (ring buffer, nearest-rank
//!   percentiles) for the frame-time HUD and the M2 prototype gate.
//! - The conventions below, so traces and benchmarks stay comparable across
//!   crates and releases.
//!
//! # Non-goals
//! - No UI. The HUD is drawn by the shell; this crate only provides data.
//! - No telemetry upload or crash reporting.
//!
//! # Span and event conventions
//! Instrument with plain [`tracing`] macros. The *target* is the module path
//! automatically, so names don't repeat the crate.
//!
//! | Level   | Use for | Examples |
//! |---------|---------|----------|
//! | `error` | Failures the user notices | file failed to save |
//! | `warn`  | Recovered problems | fell back to the CPU renderer |
//! | `info`  | User-level operations, at most a few per second | `open_document`, `export_pdf` |
//! | `debug` | Per-frame phases and background jobs | `frame`, `layout_pass`, `sync_flush` |
//! | `trace` | Per-item hot paths (per node, tile, glyph run) | `render_tile`, `shape_paragraph` |
//!
//! - **Span names** are `snake_case` verb-object phrases (`render_tile`,
//!   `decrypt_record`), constant string literals, and never contain data.
//! - **Data goes into fields**, e.g. `node = %id`, `bytes = len`. Never log
//!   document content, keys or other secrets (MAINTAINERS.md, invariant 7).
//!   Ids and sizes are fine.
//! - **Frame phases** use these span names, so the HUD and Tracy can split a
//!   frame consistently: `frame` (root), `frame_input`, `frame_update`,
//!   `frame_layout`, `frame_render`, `frame_present`.
//! - Spans on hot paths must be `trace` level. Filtered-out spans cost only
//!   a relaxed atomic load, so they are cheap to leave in.
//!
//! # Profiling a run
//! - Logs: `IMAGINARIUM_LOG=debug cargo run -p imaginarium-app`.
//! - Tracy: build with `--features imaginarium-profiling/tracy` and the
//!   `profiling` profile (release with debug info), then connect the Tracy
//!   GUI. The GUI version must match the `tracy-client` protocol version
//!   that `tracing-tracy` uses (see the
//!   [compatibility table](https://github.com/nagisa/rust_tracy_client#version-support-table)).
//!
//! # Benchmark conventions
//! - Benchmarks live in `crates/<crate>/benches/<topic>.rs`, use
//!   [criterion](https://docs.rs/criterion) with `harness = false`, and name
//!   their groups `<crate-short-name>/<topic>` (e.g. `profiling/frame_stats`).
//!   Crates with benches set `[lib] bench = false`, so criterion flags
//!   aren't passed to the libtest harness.
//! - Input data comes from `imaginarium-test-utils` fixtures with their
//!   fixed seeds, never ad-hoc random data, so runs are comparable.
//! - Benchmarks that guard a PLAN §10 budget say so in their doc comment,
//!   with the budget value.
//! - Run them with `cargo bench -p <crate>`. CI compiles them through clippy
//!   `--all-targets`. Running them with regression gates comes later.
//! - Report results as the owner rules require (MAINTAINERS.md): CPU,
//!   machine and load context.

pub mod frame;
pub mod subscriber;

pub use frame::{FrameStats, FrameSummary};
pub use subscriber::{InitError, LogFormat, TracingConfig, init};
