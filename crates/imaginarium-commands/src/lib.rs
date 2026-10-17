// SPDX-License-Identifier: AGPL-3.0-or-later

//! The command registry: every action a user can take in Imaginarium.
//!
//! Commands are the backbone of the UX and of extensibility (PLAN §4.4). The
//! command palette, contextual toolbars, keyboard shortcuts, add-ons and the
//! AI assistant all go through this single registry. A feature that is
//! registered here is automatically searchable, bindable, and usable as an
//! AI tool.
//!
//! # Pieces
//! - [`Command`]: id, [`CommandMeta`] (title, description, synonyms,
//!   category, icon), an [`ArgsSchema`], [`Availability`] in a context, and
//!   `run`. Most built-in commands are `static` [`FnCommand`]s.
//! - [`Registry`]: registration, availability snapshots, and **the single
//!   execution path** [`Registry::execute`] (availability and argument
//!   checks included). AI tool definitions and tool calls go through it too
//!   ([`Registry::tool_definitions`], [`Registry::execute_tool_call`]).
//! - [`ArgsSchema`]: typed arguments with limits, which export as JSON Schema
//!   for AI tool use and parse from JSON tool inputs.
//! - [`Keymap`]: platform-neutral shortcuts (`Mod+Shift+K`), matched against
//!   physical key presses and displayed per platform (`Ctrl+Shift+K` /
//!   `⇧⌘K`).
//! - [`SearchIndex`]: the palette's fuzzy search, with synonyms and a
//!   frecency boost from [`UsageStats`]. Budget: < 1 ms per keystroke over
//!   2,000 commands (bench: `cargo bench -p imaginarium-commands`).
//!
//! # Non-goals
//! - No dependency on the document model. The execution context `Cx` is a
//!   type parameter, provided by higher layers. Document mutations happen
//!   through transactions built by those layers (MAINTAINERS.md,
//!   invariant 2).
//! - No UI rendering. The palette and toolbars are drawn by the shell.
//!
//! # Writing a command
//! 1. Pick a stable id (`area.verb_object`, never renamed later).
//! 2. Write a specific description. AI models read it to decide when to
//!    call the command.
//! 3. Add synonyms users might type, including other languages and other
//!    apps' names for it.
//! 4. Declare the arguments with limits. Never parse free-form strings in
//!    `run`.
//! 5. Make `availability` cheap, and give disabled states a helpful reason.

mod args;
mod command;
mod error;
mod id;
mod keymap;
mod registry;
mod search;

pub use args::{ArgKind, ArgSpec, ArgValue, Args, ArgsSchema};
pub use command::{Availability, Command, CommandMeta, FnCommand, Outcome};
pub use error::CommandError;
pub use id::CommandId;
pub use keymap::{Key, KeyPress, Keymap, Modifiers, Platform, Shortcut};
pub use registry::Registry;
pub use search::{Explanation, SearchHit, SearchIndex, Usage, UsageStats};
