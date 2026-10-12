// SPDX-License-Identifier: AGPL-3.0-or-later

//! Errors of the crypto layer.

use thiserror::Error;

/// What went wrong in a cryptographic operation.
///
/// Messages never contain key material or plaintext.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum CryptoError {
    /// The OS random number generator failed.
    #[error("OS randomness unavailable: {0}")]
    Random(String),
    /// Authentication failed: wrong key, tampering, or the wrong document.
    #[error("authentication failed (wrong key or tampered data)")]
    Authentication,
    /// The record uses a crypto suite this implementation doesn't support.
    #[error("unsupported crypto suite {0}")]
    UnknownSuite(u32),
    /// No key is available for the record's epoch.
    #[error("no key for epoch {0}")]
    MissingEpoch(u32),
    /// A key or key-derived value has the wrong format or length.
    #[error("invalid key: {0}")]
    InvalidKey(&'static str),
    /// Input that isn't well-formed for the operation.
    #[error("invalid input: {0}")]
    InvalidInput(String),
    /// Password key derivation failed (e.g. parameters out of range).
    #[error("password key derivation failed: {0}")]
    Kdf(String),
    /// No key wrap in the keyring matches the available credentials.
    #[error("no key in the keyring matches these credentials")]
    NoMatchingWrap,
    /// A signature is missing, malformed, from an unexpected key, or invalid.
    #[error("signature invalid: {0}")]
    Signature(String),
    /// The access-control chain is invalid (spec §8).
    #[error("ACL chain invalid: {0}")]
    Acl(String),
}

impl From<CryptoError> for imaginarium_format::FormatError {
    fn from(err: CryptoError) -> Self {
        imaginarium_format::FormatError::Crypto(err.to_string())
    }
}

/// Result alias for this crate.
pub type Result<T, E = CryptoError> = std::result::Result<T, E>;
