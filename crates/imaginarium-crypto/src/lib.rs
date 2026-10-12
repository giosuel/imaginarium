// SPDX-License-Identifier: AGPL-3.0-or-later

//! Suite 1 cryptography for `.imag` documents ([`docs/format/spec-v1.md`]
//! §6–§8): record encryption, keyrings, signatures and the ACL chain.
//!
//! `imaginarium-format` moves bytes and calls hooks. This crate fills them:
//! [`Suite1Sealer`] and [`Suite1Opener`] implement the record encryption
//! hooks, and the key providers ([`PasswordKeys`], [`RecipientKeys`],
//! [`SharedKeys`], [`KnownKeys`]) unlock a document while it is opened.
//!
//! # Responsibility
//! - **Keys** (spec §6.2): epoch keys `DEK_e` ([`EpochKey`]) and their
//!   subkeys for content, blobs and presence; content-addressed blob ids
//!   ([`blob_id`]).
//! - **Records** (§6.3): XChaCha20-Poly1305 with the record AAD, STREAM
//!   segments for large blobs ([`blob_records`]).
//! - **Keyrings** (§6.4): wrapping `DEK_e` with a password (Argon2id), to
//!   X-Wing recipients (HPKE, post-quantum hybrid), or with folder and
//!   share-link keys.
//! - **Signatures** (§7): Ed25519 device keys ([`DeviceKey`]) for frequent
//!   records, hybrid Ed25519 + ML-DSA-65 identities ([`IdentityKey`]) for
//!   authority records ([`sign_record`], [`verify_record`]).
//! - **ACL chain** (§8): replaying `ACLC` records into members, roles,
//!   devices and the current epoch, and authorizing records against it
//!   ([`AclState`]).
//!
//! # Non-goals
//! - **No storage of secrets.** Key bundles, the OS keychain and the
//!   account's key backup belong to the app and the Hub client. This crate
//!   takes keys and returns keys.
//! - **No transport security.** TLS and the sync protocol are elsewhere.
//!
//! # Invariants
//! - Secret keys are zeroized on drop, and `Debug` never prints them.
//!   Error messages never contain key material or plaintext.
//! - All randomness comes through [`Randomness`]: the OS generator in
//!   production ([`OsRandomness`]), a seed only for the published test
//!   vectors (`SeededRandomness`, behind the `test-vectors` feature).
//! - Untrusted input is bounded before expensive work: Argon2 parameters
//!   (§6.4.1), key and signature lengths, and the limits of the format.
//! - `#![forbid(unsafe_code)]`. The primitives come from the RustCrypto
//!   and dalek crates; this crate composes them and adds none of its own.
//!   The post-quantum crates are pre-1.0 and not independently audited,
//!   which is why every post-quantum step is hybrid with a classical one
//!   (`docs/design/m1-dependencies.md`).
//!
//! [`docs/format/spec-v1.md`]: https://github.com/giosuel/imaginarium/blob/main/docs/format/spec-v1.md
#![forbid(unsafe_code)]

mod acl;
mod error;
mod keyring;
mod keys;
mod record;
mod rng;
mod sign;

pub use acl::*;
pub use error::{CryptoError, Result};
pub use keyring::*;
pub use keys::*;
pub use record::*;
pub use rng::*;
pub use sign::*;
