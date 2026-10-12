// SPDX-License-Identifier: AGPL-3.0-or-later

//! Reference implementation of the `.imag` container format
//! ([`docs/format/spec-v1.md`]).
//!
//! An `.imag` file is a pack of immutable **records** (CRDT snapshots and
//! updates, blobs, keyring, metadata) followed by an **index** and a
//! **trailer**. Saving appends: new records, a new index, a new trailer.
//! The same record bytes travel to the sync server unchanged.
//!
//! # Responsibility
//! - Byte layout: header, record framing, flags, trailer, limits
//!   ([`FileHeader`], [`Record`], [`Limits`]).
//! - Payload processing: zstd compression, `plain_len` checks against
//!   decompression bombs, and the encryption *hooks*. [`RecordSealer`] and
//!   [`RecordOpener`] receive the exact AAD of spec §6.3.
//! - Files: [`Writer`] (create, append, commit with fsync), [`Reader`]
//!   (open through trailer and index, read records, blobs and contents),
//!   recovery of torn or damaged files ([`Recovery`]), and compaction with
//!   atomic rename ([`compact`], [`compact_file`]).
//! - The protobuf messages, generated from the normative schema ([`pb`]).
//!
//! # Non-goals
//! - **No cryptography.** Encryption, signatures and key handling are in
//!   `imaginarium-crypto`, which implements [`RecordSealer`],
//!   [`RecordOpener`] and [`KeyProvider`]. Unencrypted files can be read
//!   and written with this crate alone.
//! - **No document semantics.** What a CRDT payload means is the model's
//!   business (`schema-v1.md`). This crate moves bytes.
//!
//! # Invariants
//! - Record headers and signatures are kept **verbatim** and never
//!   re-encoded (spec §6.3.1), so copies are byte-identical, and unknown
//!   fields of newer versions survive.
//! - `#![forbid(unsafe_code)]`. Every parser path is fuzzed (`fuzz/`) and
//!   property-tested.
//!
//! [`docs/format/spec-v1.md`]: https://github.com/giosuel/imaginarium/blob/main/docs/format/spec-v1.md
#![forbid(unsafe_code)]

mod atomic;
mod compact;
mod error;
mod header;
mod ids;
mod index;
mod payload;
pub mod pb;
mod reader;
mod record;
mod recovery;
mod trailer;
mod varint;
mod writer;

pub use atomic::AtomicFile;
pub use compact::{CompactionPlan, CompactionReport, KeepFn, compact, compact_file};
pub use error::{FormatError, Result};
pub use header::{FLAG_ENCRYPTED, FileHeader, HEADER_LEN, MAGIC, MAJOR, MINOR};
pub use ids::{DocId, ObjectId};
pub use index::{IndexState, StreamKey};
pub use payload::{DEFAULT_ZSTD_LEVEL, RecordBuilder, RecordOpener, RecordSealer};
pub use reader::{KeyProvider, OpenOptions, Reader};
pub use record::{Limits, Record, RecordFlags, RecordType};
pub use recovery::{Recovery, RecoveryMethod};
pub use trailer::{TRAILER_LEN, TRAILER_MAGIC, Trailer};
pub use writer::{Durable, Writer};

/// Size of blob segments in bytes (spec §5.3).
pub const SEGMENT_LEN: usize = 256 * 1024;

/// Blobs larger than this must be segmented (spec §5.3).
pub const SEGMENT_THRESHOLD: usize = 1024 * 1024;
