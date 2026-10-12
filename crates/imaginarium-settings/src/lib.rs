// SPDX-License-Identifier: AGPL-3.0-or-later

//! The typed settings registry (ADR-0014).
//!
//! Every setting in Imaginarium is declared here with enough metadata to
//! generate its UI. The settings screen, the command palette's "Settings: …"
//! entries and the Hub admin panel are all built from these declarations.
//! No setting may exist only in a config file.
//!
//! # Responsibility
//! - Setting declarations: key, type, default, validation, **scope** (app,
//!   user, workspace, document, hub) and UI metadata (title, description,
//!   synonyms, group, advanced flag).
//! - Reading and writing values, with validation.
//! - Change notifications. These start as a simple observer and are later
//!   bridged to reactive signals.
//! - Persistence for the app scope (TOML). Other scopes are persisted by
//!   their owners (user/workspace via sync, document in the CRDT).
//!
//! # Non-goals
//! - No UI. The shell generates it from the registry metadata.
//! - No secret storage. Secrets (e.g. AI API keys) are stored as references
//!   to OS-keychain entries, which a higher layer resolves. Plaintext secrets
//!   never appear in settings files.
//!
//! Status: skeleton (M0.1). Implemented in M0.5.
