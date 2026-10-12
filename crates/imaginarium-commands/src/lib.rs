// SPDX-License-Identifier: AGPL-3.0-or-later

//! The command registry: every action a user can take in Imaginarium.
//!
//! Commands are the backbone of the UX and of extensibility (PLAN §4.4). The
//! command palette, contextual toolbars, keyboard shortcuts, add-ons and the
//! AI assistant all read from this single registry. A feature that is
//! registered here is automatically searchable, bindable, and usable as an
//! AI tool.
//!
//! # Responsibility
//! - The `Command` trait: id, metadata (title, description, synonyms, icon,
//!   category), argument schema (exportable as JSON Schema for AI tool use),
//!   availability for a given context, and execution.
//! - The registry, with lookup by id and by context.
//! - The keymap model, with a platform-aware modifier (`Mod` = Ctrl on
//!   Linux/Windows, Cmd on macOS).
//! - A fuzzy search index with synonyms and recency/frequency boosting.
//!   Budget: under 1 ms per keystroke over 2,000 commands.
//!
//! # Non-goals
//! - No dependency on the document model. The execution context is
//!   abstract, and higher layers provide the concrete context. Document
//!   mutations happen through transactions built by those layers
//!   (MAINTAINERS.md invariant 2).
//! - No UI rendering. The palette and toolbars are drawn by the shell.
//!
//! Status: skeleton (M0.1). Implemented in M0.6.
