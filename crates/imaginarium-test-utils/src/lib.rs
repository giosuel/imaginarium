// SPDX-License-Identifier: AGPL-3.0-or-later

//! Shared test and benchmark utilities for Imaginarium crates.
//!
//! # Responsibility
//! - [`rng`]: a deterministic, platform-independent random generator, so
//!   generated data is identical on every machine and in every release.
//! - [`fixtures`]: generators for the data behind the performance budgets
//!   (PLAN §10): the 10,000-shape board ([`fixtures::BoardFixture::LARGE`]),
//!   the 300-page document ([`fixtures::DocumentFixture::LARGE`]) and the
//!   100-slide deck ([`fixtures::DeckFixture::LARGE`]). They produce plain
//!   data, independent of the document model.
//! - Later: golden-image comparison (CPU renderer), format round-trip
//!   harnesses, and multi-client sync simulation helpers.
//!
//! # Non-goals
//! - This crate is **only ever a dev-dependency**. Production crates must
//!   never depend on it; `cargo xtask check-layers` enforces this.
//!
//! # Example
//! ```
//! use imaginarium_test_utils::fixtures::BoardFixture;
//!
//! let shapes: Vec<_> = BoardFixture::SMALL.shapes().collect();
//! assert_eq!(shapes.len(), 100);
//! ```

pub mod fixtures;
pub mod rng;
