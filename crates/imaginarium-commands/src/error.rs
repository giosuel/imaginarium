// SPDX-License-Identifier: AGPL-3.0-or-later

//! Errors of the command system.

use thiserror::Error;

/// What went wrong with a command operation.
#[derive(Debug, Error, PartialEq)]
pub enum CommandError {
    /// The id is malformed (see [`CommandId::validate`](crate::CommandId::validate)).
    #[error("invalid command id `{id}`: {reason}")]
    InvalidId {
        /// The rejected id.
        id: &'static str,
        /// Why it was rejected.
        reason: String,
    },
    /// A command with this id is already registered.
    #[error("command `{0}` is already registered")]
    Duplicate(&'static str),
    /// No command with this id is registered.
    #[error("unknown command `{0}`")]
    Unknown(String),
    /// The command can't run in the current context (hidden or disabled).
    #[error("command `{id}` is not available: {reason}")]
    Unavailable {
        /// The command.
        id: &'static str,
        /// User-facing reason (e.g. "Select some text first").
        reason: String,
    },
    /// The arguments don't match the command's schema.
    #[error("invalid arguments for `{id}`: {reason}")]
    InvalidArgs {
        /// The command.
        id: &'static str,
        /// What is wrong, phrased for users and AI tool callers alike.
        reason: String,
    },
    /// The command ran and failed.
    #[error("command `{id}` failed: {message}")]
    Failed {
        /// The command.
        id: &'static str,
        /// User-facing message.
        message: String,
    },
    /// A keyboard shortcut string couldn't be parsed.
    #[error("invalid shortcut `{shortcut}`: {reason}")]
    InvalidShortcut {
        /// The rejected shortcut text.
        shortcut: String,
        /// Why it was rejected.
        reason: String,
    },
}
