// SPDX-License-Identifier: AGPL-3.0-or-later

//! Profiling and instrumentation for Imaginarium.
//!
//! Performance is a feature (PLAN §10), so measurement infrastructure exists
//! before any feature work. Every crate may depend on this one (support
//! layer).
//!
//! # Responsibility
//! - Process-wide [`tracing`](https://docs.rs/tracing) subscriber setup:
//!   environment-based filtering, human-readable or JSON output.
//! - Optional [Tracy](https://github.com/wolfpld/tracy) integration behind
//!   the `tracy` cargo feature, for frame-level profiling.
//! - `FrameStats`: a ring buffer of frame times with p50/p95/p99, which
//!   feeds the frame-time HUD (`F12`) and the M2 prototype-gate measurements.
//! - Span naming conventions, so traces stay comparable across releases.
//!
//! # Non-goals
//! - No UI. The HUD is drawn by the shell; this crate only provides data.
//! - No telemetry upload or crash reporting.
//!
//! Status: skeleton (M0.1). Implemented in M0.4.
