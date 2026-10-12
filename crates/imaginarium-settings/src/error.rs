// SPDX-License-Identifier: AGPL-3.0-or-later

//! Errors of the settings registry.

use thiserror::Error;

/// What went wrong with a settings operation.
#[derive(Debug, Error, PartialEq)]
pub enum SettingsError {
    /// The setting was used without being registered first. This is a
    /// programming error: every setting must be registered at startup, so
    /// it appears in the settings UI (ADR-0014).
    #[error(
        "setting `{0}` is not registered; register it at startup so it appears in the settings UI"
    )]
    NotRegistered(String),
    /// Another setting with this key is already registered.
    #[error("setting `{0}` is already registered")]
    DuplicateKey(&'static str),
    /// The key is malformed, or clashes structurally with another key.
    #[error("invalid setting key `{key}`: {reason}")]
    InvalidKey {
        /// The rejected key.
        key: &'static str,
        /// Why it was rejected.
        reason: String,
    },
    /// The value doesn't fit the setting's kind, constraint or validator.
    #[error("invalid value for `{key}`: {reason}")]
    Invalid {
        /// The setting's key.
        key: &'static str,
        /// User-facing explanation.
        reason: String,
    },
    /// A settings file couldn't be parsed as TOML.
    #[error("settings file is not valid TOML: {0}")]
    Parse(String),
    /// Reading or writing a settings file failed.
    #[error("settings file I/O failed: {0}")]
    Io(String),
}
