// SPDX-License-Identifier: AGPL-3.0-or-later

//! Errors of the container layer.

use thiserror::Error;

/// Everything that can go wrong reading or writing an `.imag` container.
#[derive(Debug, Error)]
pub enum FormatError {
    /// An I/O error from the underlying file or stream.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// The data ended before a complete structure could be read.
    #[error("unexpected end of data while reading {what}")]
    Truncated {
        /// What was being read.
        what: &'static str,
    },
    /// Not an `.imag` file (wrong header or trailer magic).
    #[error("not an .imag file: bad {what} magic")]
    BadMagic {
        /// `"header"` or `"trailer"`.
        what: &'static str,
    },
    /// The file's major version isn't supported by this implementation.
    #[error("unsupported .imag major version {0} (this reader supports 1)")]
    UnsupportedMajor(u16),
    /// Reserved critical header flags are set (spec §4.1).
    #[error("unsupported critical header flags {0:#06x}")]
    UnknownCriticalFlags(u32),
    /// A varint was malformed: too long, overflowing, or not minimal (spec §1).
    #[error("malformed varint at {what}")]
    Varint {
        /// Which field.
        what: &'static str,
    },
    /// A size exceeded a limit (spec §4.6, or the reader's own limits).
    #[error("{what} of {value} bytes exceeds the limit of {limit}")]
    Limit {
        /// Which size.
        what: &'static str,
        /// The size found.
        value: u64,
        /// The allowed maximum.
        limit: u64,
    },
    /// A CRC32C check failed, which means a torn write or corruption.
    #[error("CRC mismatch in {what}")]
    Crc {
        /// Which structure.
        what: &'static str,
    },
    /// A protobuf message couldn't be decoded.
    #[error("invalid {what} protobuf: {source}")]
    Protobuf {
        /// Which message.
        what: &'static str,
        /// Decoder error.
        #[source]
        source: prost::DecodeError,
    },
    /// The structure is well-formed but violates a rule of the spec.
    #[error("invalid record: {0}")]
    Invalid(String),
    /// zstd decompression failed.
    #[error("decompression failed: {0}")]
    Decompress(String),
    /// The plaintext length differs from `RecordHeader.plain_len` (spec §4.4).
    #[error("plaintext is {actual} bytes, header says {expected}")]
    PlainLen {
        /// `plain_len` from the header.
        expected: u64,
        /// The actual length.
        actual: u64,
    },
    /// The record is encrypted but no opener (key) was provided.
    #[error("record is encrypted; a key is required")]
    NeedsKey,
    /// Encryption or decryption failed (wrong key, tampering, unknown suite).
    #[error("crypto error: {0}")]
    Crypto(String),
    /// Neither the trailer nor recovery found a usable index (spec §9.5).
    #[error("no valid index found: {0}")]
    NoValidIndex(String),
}

/// Result alias for this crate.
pub type Result<T, E = FormatError> = std::result::Result<T, E>;
