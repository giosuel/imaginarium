// SPDX-License-Identifier: AGPL-3.0-or-later

//! Errors of opening and saving native files.

use thiserror::Error;

/// What went wrong reading or writing a `.imag` document.
#[derive(Debug, Error)]
pub enum NativeError {
    /// Reading or writing the file failed.
    #[error("file error: {0}")]
    Io(#[from] std::io::Error),
    /// The container is damaged or not an `.imag` file.
    #[error(transparent)]
    Format(#[from] imaginarium_format::FormatError),
    /// Keys, decryption, signatures or the ACL chain.
    #[error(transparent)]
    Crypto(#[from] imaginarium_crypto::CryptoError),
    /// The CRDT content.
    #[error(transparent)]
    Model(#[from] imaginarium_model::ModelError),
    /// The file has no `main` stream (no `SNAP` or `UPDT` of it).
    #[error("the file contains no document (no main stream)")]
    NoDocument,
    /// A stream record uses a codec this version can't read.
    #[error("unsupported codec `{0}` (this version reads `loro-1`)")]
    UnsupportedCodec(String),
    /// The index names a record that isn't there.
    #[error("the index refers to a missing record {0}")]
    MissingRecord(String),
}

/// Result alias for this crate.
pub type Result<T, E = NativeError> = std::result::Result<T, E>;
