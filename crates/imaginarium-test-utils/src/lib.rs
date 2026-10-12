// SPDX-License-Identifier: AGPL-3.0-or-later

//! Shared test and benchmark utilities for Imaginarium crates.
//!
//! # Responsibility
//! - **Deterministic fixture generators** for benchmarks and tests, seeded so
//!   runs are comparable: the 10,000-shape board, the 300-page document and
//!   the 100-slide deck used by the performance budgets (PLAN §10).
//! - Later: golden-image comparison (CPU renderer), format round-trip
//!   harnesses, and multi-client sync simulation helpers.
//!
//! # Non-goals
//! - This crate is **only ever a dev-dependency**. Production crates must
//!   never depend on it; `cargo xtask check-layers` enforces this.
//!
//! Status: skeleton (M0.1). Fixture API stubs arrive in M0.4.
