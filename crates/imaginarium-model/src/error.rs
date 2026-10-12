// SPDX-License-Identifier: AGPL-3.0-or-later

//! Errors of the document model.

use thiserror::Error;

use crate::ids::NodeId;

/// Why a document is open read-only (schema-v1.md §10.2, §10.3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReadOnlyReason {
    /// The document lists features this version doesn't know.
    MissingFeatures(Vec<String>),
    /// The document's schema is newer than this version knows.
    NewerSchema(i64),
    /// The document's schema is older, and there is no migration path.
    NoMigration(i64),
    /// Migrating from schema version `from` failed (and was rolled back).
    MigrationFailed {
        /// The version the failed migration started from.
        from: i64,
        /// Why it failed.
        reason: String,
    },
}

impl std::fmt::Display for ReadOnlyReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingFeatures(features) => {
                write!(
                    f,
                    "it needs features this version lacks: {}",
                    features.join(", ")
                )
            }
            Self::NewerSchema(version) => {
                write!(
                    f,
                    "it uses schema version {version}, which is newer than this version"
                )
            }
            Self::NoMigration(version) => {
                write!(f, "there is no migration from its schema version {version}")
            }
            Self::MigrationFailed { from, reason } => {
                write!(
                    f,
                    "migrating it from schema version {from} failed: {reason}"
                )
            }
        }
    }
}

/// What went wrong in a model operation.
#[derive(Clone, Debug, Error, PartialEq)]
pub enum ModelError {
    /// The node doesn't exist, or was deleted.
    #[error("no node {0}")]
    NoSuchNode(NodeId),
    /// A move would make a node its own ancestor.
    #[error("moving {0} there would create a cycle")]
    Cycle(NodeId),
    /// A position or range is outside the text or sibling list.
    #[error("{what} {value} is out of range (length {len})")]
    OutOfRange {
        /// What was out of range.
        what: &'static str,
        /// The value given.
        value: usize,
        /// The valid length.
        len: usize,
    },
    /// A key or value that the schema doesn't allow here.
    #[error("invalid: {0}")]
    Invalid(String),
    /// The document is read-only, so it can't be edited.
    #[error("the document is read-only because {0}")]
    ReadOnly(ReadOnlyReason),
    /// The bytes aren't a valid `loro-1` document or update.
    #[error("can't decode the document: {0}")]
    Decode(String),
    /// An error reported by the CRDT library.
    #[error("CRDT error: {0}")]
    Crdt(String),
}

impl From<loro::LoroError> for ModelError {
    fn from(err: loro::LoroError) -> Self {
        ModelError::Crdt(err.to_string())
    }
}

/// Result alias for this crate.
pub type Result<T, E = ModelError> = std::result::Result<T, E>;
