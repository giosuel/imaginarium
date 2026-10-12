// SPDX-License-Identifier: AGPL-3.0-or-later

//! The native `.imag` file as a [`Document`]: opening (with keys) and
//! saving, on top of `imaginarium-format`, `imaginarium-crypto` and
//! `imaginarium-model`.
//!
//! # Responsibility
//! - **Open**: unlock an encrypted file ([`Credentials`]), read the `main`
//!   stream (its latest `SNAP` plus the `UPDT`s after it, in index order)
//!   into a model [`Document`], and return the file's metadata, recovery
//!   report and ACL chain alongside ([`Opened`]).
//! - **Save** a new file: `META`, a `SNAP` of the document, and for
//!   password protection a keyring and encryption ([`save_new`],
//!   [`save_new_path`], which replaces the file atomically).
//!
//! In documents with an ACL chain, every record the document is read from
//! must be signed by a device the chain allows to write it (spec §7.3,
//! §8.4). That costs about 70 µs per record (STATUS, known issues).
//!
//! # Non-goals (yet)
//! - Incremental saving (appending `UPDT`s), blobs, thumbnails, the
//!   `discussion` stream and saving Hub documents (account keys, ACL
//!   chains) come with the app (M2) and collaboration (M8).
#![forbid(unsafe_code)]

mod error;

pub use error::{NativeError, Result};

use std::collections::HashMap;
use std::io::{Read, Seek, Write};
use std::path::Path;

use imaginarium_crypto::{
    AclState, Argon2Config, EpochKey, KeyId, OsRandomness, PasswordKeys, Randomness, RecipientKeys,
    Suite1Sealer, XWingKeyPair, keyring_record, wrap_password,
};
use imaginarium_format::{
    AtomicFile, DocId, Durable, FileHeader, ObjectId, OpenOptions, Reader, Record, RecordBuilder,
    RecordType, Recovery, StreamKey, Writer, pb,
};
use imaginarium_model::{CODEC, Document};
use prost::Message;
use zeroize::Zeroizing;

/// How to unlock an encrypted document.
pub enum Credentials {
    /// No key (unencrypted documents).
    None,
    /// A password (password keyring wraps).
    Password(Zeroizing<String>),
    /// A device's or user's X-Wing key pair (HPKE wraps), for documents
    /// whose ACL chain `owner` started (ADR-0018).
    Recipient {
        /// The key pair.
        keypair: XWingKeyPair,
        /// The identity expected to have signed the chain's `Genesis`: the
        /// user's own for account-key files, the inviting owner for shared
        /// documents. `None` accepts any owner, which proves nothing about
        /// who made the document; only inspection tools that show the owner
        /// instead ([`AclState::genesis_owner`]) use it.
        owner: Option<KeyId>,
    },
}

/// An opened document file.
pub struct Opened {
    /// The `main` stream as a document.
    pub document: Document,
    /// The file header.
    pub header: FileHeader,
    /// The latest `META` record, if any.
    pub meta: Option<pb::DocumentMeta>,
    /// How the file was recovered, if its last commit wasn't usable.
    pub recovery: Option<Recovery>,
    /// The replayed ACL chain, for documents that have one.
    pub acl: Option<AclState>,
}

/// Opens the file at `path`.
pub fn open_path(path: &Path, credentials: Credentials) -> Result<Opened> {
    open(std::fs::File::open(path)?, credentials)
}

/// Opens a document from any seekable byte source.
pub fn open<F: Read + Seek>(file: F, credentials: Credentials) -> Result<Opened> {
    let mut reader = open_reader(file, credentials)?;
    let records = IndexedRecords::new(&reader);
    let acl = read_acl(&mut reader)?;
    let document = read_document(&mut reader, &records, acl.as_ref())?;
    let meta = read_meta(&mut reader)?;
    Ok(Opened {
        document,
        header: *reader.header(),
        meta,
        recovery: reader.recovery().cloned(),
        acl,
    })
}

/// Opens the container with the given credentials, without reading the
/// document.
pub fn open_reader<F: Read + Seek>(file: F, credentials: Credentials) -> Result<Reader<F>> {
    let options = OpenOptions::default();
    Ok(match credentials {
        Credentials::None => Reader::open_with(file, options, None)?,
        Credentials::Password(password) => {
            Reader::open_with(file, options, Some(&mut PasswordKeys::new(&password)))?
        }
        Credentials::Recipient { keypair, owner } => {
            let mut keys = match owner {
                Some(owner) => RecipientKeys::new(keypair, owner),
                None => RecipientKeys::trusting_any_owner(keypair),
            };
            Reader::open_with(file, options, Some(&mut keys))?
        }
    })
}

/// Index entries by record type and object id.
pub struct IndexedRecords(HashMap<(u32, Vec<u8>), pb::IndexEntry>);

impl IndexedRecords {
    /// Indexes the reader's entries. A later entry with the same id wins.
    pub fn new<F: Read + Seek>(reader: &Reader<F>) -> Self {
        Self(
            reader
                .index()
                .entries()
                .iter()
                .map(|e| ((e.rtype, e.object_id.clone()), e.clone()))
                .collect(),
        )
    }

    fn get(&self, rtype: RecordType, id: &[u8]) -> Result<&pb::IndexEntry> {
        self.0
            .get(&(rtype.0, id.to_vec()))
            .ok_or_else(|| NativeError::MissingRecord(hex(id)))
    }
}

/// The payloads of a stream's head (snapshot, then updates), decrypted and
/// decompressed. `None` if the file has no such stream. With `acl`, every
/// record must be signed by a device the chain allows to write it.
pub fn stream_parts<F: Read + Seek>(
    reader: &mut Reader<F>,
    records: &IndexedRecords,
    stream: &StreamKey,
    acl: Option<&AclState>,
) -> Result<Option<Vec<Vec<u8>>>> {
    let Some(head) = reader.index().head(stream).cloned() else {
        return Ok(None);
    };
    let mut wanted = Vec::new();
    if !head.snapshot_id.is_empty() {
        wanted.push((RecordType::SNAP, head.snapshot_id));
    }
    wanted.extend(head.update_ids.into_iter().map(|id| (RecordType::UPDT, id)));
    if wanted.is_empty() {
        return Ok(None);
    }
    let mut parts = Vec::with_capacity(wanted.len());
    for (rtype, id) in wanted {
        let entry = records.get(rtype, &id)?.clone();
        let record = reader.read_record(&entry)?;
        if record.header().codec != CODEC {
            return Err(NativeError::UnsupportedCodec(record.header().codec.clone()));
        }
        if let Some(acl) = acl {
            acl.verify_content(&record)?;
        }
        parts.push(reader.read_content(&entry)?);
    }
    Ok(Some(parts))
}

/// Reads the `main` stream into a document, checking its records against
/// `acl` (from [`read_acl`]) if the file has an ACL chain.
pub fn read_document<F: Read + Seek>(
    reader: &mut Reader<F>,
    records: &IndexedRecords,
    acl: Option<&AclState>,
) -> Result<Document> {
    let parts =
        stream_parts(reader, records, &StreamKey::main(), acl)?.ok_or(NativeError::NoDocument)?;
    let parts: Vec<&[u8]> = parts.iter().map(Vec::as_slice).collect();
    Ok(Document::open(&parts)?)
}

/// The latest `META` record.
pub fn read_meta<F: Read + Seek>(reader: &mut Reader<F>) -> Result<Option<pb::DocumentMeta>> {
    let Some(entry) = reader
        .index()
        .entries()
        .iter()
        .rev()
        .find(|e| e.rtype == RecordType::META.0)
        .cloned()
    else {
        return Ok(None);
    };
    let content = reader.read_content(&entry)?;
    Ok(Some(pb::DocumentMeta::decode(content.as_slice()).map_err(
        |err| imaginarium_format::FormatError::Protobuf {
            what: "DocumentMeta",
            source: err,
        },
    )?))
}

/// Replays the ACL chain, if the file has one.
pub fn read_acl<F: Read + Seek>(reader: &mut Reader<F>) -> Result<Option<AclState>> {
    let entries: Vec<pb::IndexEntry> = reader
        .index()
        .entries()
        .iter()
        .filter(|e| e.rtype == RecordType::ACLC.0)
        .cloned()
        .collect();
    if entries.is_empty() {
        return Ok(None);
    }
    let records: Vec<Record> = entries
        .iter()
        .map(|e| reader.read_record(e))
        .collect::<Result<_, _>>()?;
    Ok(Some(AclState::replay(&reader.header().doc_id, &records)?))
}

/// How a new file is protected.
pub enum Protection {
    /// Unencrypted (the default for local files, ADR-0016).
    None,
    /// Encrypted, with the key wrapped by a password.
    Password {
        /// The password.
        password: Zeroizing<String>,
        /// Argon2id cost. [`Argon2Config::default`] in production.
        argon2: Argon2Config,
    },
}

/// Options for saving a new file.
pub struct SaveOptions {
    /// Encryption.
    pub protection: Protection,
    /// `DocumentMeta.created_with`, e.g. "Imaginarium 0.1.0".
    pub created_with: String,
}

/// Writes `document` as a new file into `file`: `META`, one compressed
/// `SNAP` of the `main` stream, and the index. Password protection adds a
/// keyring and encrypts every record (spec §5.1).
pub fn save_new<F: Read + Write + Seek + Durable>(
    file: F,
    document: &Document,
    options: &SaveOptions,
) -> Result<F> {
    let doc_id = DocId::random()?;
    let mut blob_id_key = Zeroizing::new([0u8; 32]);
    OsRandomness.fill(blob_id_key.as_mut())?;
    let meta = pb::DocumentMeta {
        format_version: "1.0".into(),
        created_with: options.created_with.clone(),
        created_unix_ms: now_ms(),
        required_features: document.view().meta().required_features().to_vec(),
        blob_id_key: blob_id_key.to_vec(),
    };
    let snapshot = document.export_snapshot()?;
    let encrypted = !matches!(options.protection, Protection::None);
    let mut writer = Writer::create(file, FileHeader::new(doc_id, encrypted))?;
    match &options.protection {
        Protection::None => {
            writer.append(&meta_record(&doc_id, &meta, None)?)?;
            writer.append(&snapshot_record(&doc_id, snapshot, None)?)?;
            writer.commit(None)?;
        }
        Protection::Password { password, argon2 } => {
            let key = EpochKey::generate(1, &mut OsRandomness)?;
            let wrap = wrap_password(&doc_id, &key, password, *argon2, &mut OsRandomness)?;
            let keyring = pb::Keyring {
                epochs: vec![pb::EpochKeys {
                    epoch: 1,
                    wraps: vec![wrap],
                }],
            };
            writer.append(&keyring_record(
                &doc_id,
                ObjectId::random()?,
                &keyring,
                false,
            )?)?;
            let sealer = Suite1Sealer::new(&key, OsRandomness);
            writer.append(&meta_record(&doc_id, &meta, Some(&sealer))?)?;
            writer.append(&snapshot_record(&doc_id, snapshot, Some(&sealer))?)?;
            writer.commit(Some(&sealer))?;
        }
    }
    Ok(writer.into_inner())
}

/// Saves `document` as a new file at `path`, replacing an existing one
/// atomically ([`AtomicFile`]: a temporary file, synced, renamed over it,
/// keeping its permissions).
///
/// The file gets a new document id (spec §4.1), so this is for new files
/// and "Save as", not for saving an opened file again: that appends to it
/// (incremental saving, M2).
pub fn save_new_path(path: &Path, document: &Document, options: &SaveOptions) -> Result<()> {
    let mut out = AtomicFile::create(path)?;
    save_new(out.file(), document, options)?;
    out.commit()?;
    Ok(())
}

fn meta_record(
    doc_id: &DocId,
    meta: &pb::DocumentMeta,
    sealer: Option<&dyn imaginarium_format::RecordSealer>,
) -> Result<Record> {
    Ok(
        RecordBuilder::new(RecordType::META, ObjectId::random()?).build(
            doc_id,
            meta.encode_to_vec(),
            sealer,
        )?,
    )
}

fn snapshot_record(
    doc_id: &DocId,
    snapshot: Vec<u8>,
    sealer: Option<&dyn imaginarium_format::RecordSealer>,
) -> Result<Record> {
    Ok(RecordBuilder::new(RecordType::SNAP, ObjectId::random()?)
        .stream(pb::Stream::Main)
        .codec(CODEC)
        .compress(true)
        .build(doc_id, snapshot, sealer)?)
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
