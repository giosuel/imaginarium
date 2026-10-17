// SPDX-License-Identifier: AGPL-3.0-or-later

//! An opened file kept open for saving: each change is appended as an
//! `UPDT` record and committed with a new index (spec §9.3), so every
//! change reaches the file on its own (PLAN §14, "Saving").
//!
//! A file is only written when this version may. It is locked against a
//! second writer (another window or app). It opens read-only when it can't
//! be written, is in use, comes from a newer format, or needs features or
//! records this version doesn't know (spec §10.2, §10.3). Encrypted and Hub
//! files can't be opened for saving yet (their keys aren't kept).

use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use imaginarium_format::{
    DocId, Durable, FileHeader, HEADER_LEN, MINOR, ObjectId, Reader, Record, RecordBuilder,
    RecordFlags, RecordType, StreamKey, Writer, pb,
};
use imaginarium_model::{CODEC, Document, KNOWN_FEATURES, ReadOnlyReason};

use crate::{
    Credentials, IndexedRecords, NativeError, Opened, Result, open_reader, read_acl, read_meta,
    snapshot_record, stream_parts,
};

/// A document file open for saving into.
pub struct Session<F = File> {
    writer: Writer<F>,
    doc_id: DocId,
    /// The file's head as it was opened (snapshot, then updates), for a
    /// copy of the document on the saving thread.
    parts: Vec<Vec<u8>>,
    /// The last save was torn and the file was recovered (spec §9.5).
    recovered: bool,
    /// The file's length as this session last left it (a torn tail after
    /// a recovery stays until the next commit cuts it off).
    known_len: u64,
}

/// How a file opened.
pub enum Opening {
    /// To edit: its changes are saved through the session.
    Editable(Opened, Session),
    /// To read only: the document is read-only (its mode says why).
    ReadOnly(Opened),
}

impl Session<File> {
    /// Opens the file at `path` to edit it, locked against other writers,
    /// or read-only when it can't be written (see the module docs).
    ///
    /// # Errors
    /// If the file can't be read at all, or is encrypted or a Hub's.
    pub fn open(path: &Path) -> Result<Opening> {
        let writable = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(path);
        let (file, refused) = match writable {
            Ok(file) => match file.try_lock() {
                Ok(()) => (file, None),
                Err(std::fs::TryLockError::WouldBlock) => {
                    (file, Some("it is open in another window or app"))
                }
                Err(std::fs::TryLockError::Error(err)) => return Err(err.into()),
            },
            Err(err)
                if matches!(
                    err.kind(),
                    std::io::ErrorKind::PermissionDenied | std::io::ErrorKind::ReadOnlyFilesystem
                ) =>
            {
                (File::open(path)?, Some("the file can't be written"))
            }
            Err(err) => return Err(err.into()),
        };
        let (mut opened, session) = Self::from_file(file)?;
        if let Some(reason) = refused {
            opened
                .document
                .set_read_only(ReadOnlyReason::File(reason.to_owned()));
            return Ok(Opening::ReadOnly(opened));
        }
        Ok(match session {
            Some(session) => Opening::Editable(opened, session),
            None => Opening::ReadOnly(opened),
        })
    }

    /// Opens the file at `path` again for saving (a retry), without
    /// reading the document: it must still be document `doc_id`, and not
    /// in use elsewhere.
    ///
    /// # Errors
    /// If it can't be opened, locked or read, or holds another document.
    pub fn reopen(path: &Path, doc_id: &DocId) -> Result<Self> {
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)?;
        match file.try_lock() {
            Ok(()) => {}
            Err(std::fs::TryLockError::WouldBlock) => {
                return Err(NativeError::CantSave("it is open in another window or app"));
            }
            Err(std::fs::TryLockError::Error(err)) => return Err(err.into()),
        }
        let reader: Reader<File> = open_reader(file, Credentials::None)?;
        if reader.header().doc_id != *doc_id {
            return Err(NativeError::CantSave(
                "another document is in its place now",
            ));
        }
        if let Some(reason) = refusal(&reader, None) {
            return Err(NativeError::CantSave(reason));
        }
        let recovered = reader.recovery().is_some();
        let mut writer = Writer::resume(reader)?;
        let known_len = writer.len_on_disk()?;
        Ok(Self {
            doc_id: *doc_id,
            writer,
            parts: Vec::new(),
            recovered,
            known_len,
        })
    }
}

/// Why this version must not write into the file, if it mustn't.
fn refusal<F: Read + Seek>(
    reader: &Reader<F>,
    meta: Option<&pb::DocumentMeta>,
) -> Option<&'static str> {
    if reader.header().minor > MINOR {
        return Some("it was saved by a newer version of the file format");
    }
    if meta.is_some_and(|meta| {
        meta.required_features
            .iter()
            .any(|feature| !KNOWN_FEATURES.contains(&feature.as_str()))
    }) {
        return Some("it needs features this version lacks");
    }
    let unknown_critical = reader.index().entries().iter().any(|entry| {
        let flags = RecordFlags(u8::try_from(entry.rflags).unwrap_or(0xFF));
        !RecordType(entry.rtype).is_known() && flags.contains(RecordFlags::CRITICAL)
    });
    unknown_critical.then_some("it holds data this version doesn't know")
}

impl<F: Read + Write + Seek + Durable> Session<F> {
    /// Opens `file` to edit it: the document, and a session if this
    /// version may write into it (else the document is read-only).
    ///
    /// # Errors
    /// If it can't be read, or is encrypted or a Hub's.
    pub fn from_file(mut file: F) -> Result<(Opened, Option<Self>)> {
        // Without its key an encrypted file doesn't even open: say why.
        let mut header = [0u8; HEADER_LEN];
        file.seek(SeekFrom::Start(0))?;
        if file.read_exact(&mut header).is_ok()
            && FileHeader::decode(&header).is_ok_and(|header| header.is_encrypted())
        {
            return Err(NativeError::CantSave("it is encrypted"));
        }
        let mut reader: Reader<F> = open_reader(file, Credentials::None)?;
        let records = IndexedRecords::new(&reader);
        let acl = read_acl(&mut reader)?;
        if acl.is_some() {
            return Err(NativeError::CantSave("it belongs to a Hub"));
        }
        let parts = stream_parts(&mut reader, &records, &StreamKey::main(), None)?
            .ok_or(NativeError::NoDocument)?;
        let slices: Vec<&[u8]> = parts.iter().map(Vec::as_slice).collect();
        let mut document = Document::open(&slices)?;
        let meta = read_meta(&mut reader)?;
        let header = *reader.header();
        let recovery = reader.recovery().cloned();
        let refused = refusal(&reader, meta.as_ref());
        if let Some(reason) = refused {
            document.set_read_only(ReadOnlyReason::File(reason.to_owned()));
        }
        let editable = matches!(document.mode(), imaginarium_model::Mode::Editable);
        let recovered = recovery.is_some();
        let opened = Opened {
            document,
            header,
            meta,
            recovery,
            acl,
        };
        if !editable {
            return Ok((opened, None));
        }
        let mut writer = Writer::resume(reader)?;
        let known_len = writer.len_on_disk()?;
        Ok((
            opened,
            Some(Self {
                writer,
                doc_id: header.doc_id,
                parts,
                recovered,
                known_len,
            }),
        ))
    }

    /// The document's id (spec §4.1).
    pub fn doc_id(&self) -> DocId {
        self.doc_id
    }

    /// Whether the file was recovered from a torn save when opened.
    pub fn recovered(&self) -> bool {
        self.recovered
    }

    /// The file's head as opened, once (for a copy of the document).
    pub fn take_parts(&mut self) -> Vec<Vec<u8>> {
        std::mem::take(&mut self.parts)
    }

    /// Appends one update of the `main` stream (from
    /// `Document::export_updates_since`). It is in the file once
    /// committed.
    ///
    /// # Errors
    /// If it can't be written.
    pub fn append_update(&mut self, update: Vec<u8>) -> Result<()> {
        let record: Record = RecordBuilder::new(RecordType::UPDT, ObjectId::random()?)
            .stream(pb::Stream::Main)
            .codec(CODEC)
            .compress(update.len() > 512)
            .build(&self.doc_id, update, None)?;
        self.writer.append(&record)?;
        Ok(())
    }

    /// Appends a snapshot of the whole document: the stream's head starts
    /// again from it, so the file opens without replaying every update.
    ///
    /// # Errors
    /// If it can't be written.
    pub fn append_snapshot(&mut self, snapshot: Vec<u8>) -> Result<()> {
        self.writer
            .append(&snapshot_record(&self.doc_id, snapshot, None)?)?;
        Ok(())
    }

    /// Checks that nobody else wrote to the file since this session last
    /// did (another program, or an editor's save over it).
    ///
    /// # Errors
    /// [`NativeError::CantSave`] if the file changed.
    pub fn check_unchanged(&mut self) -> Result<()> {
        if self.writer.len_on_disk()? != self.known_len {
            return Err(NativeError::CantSave("another program changed the file"));
        }
        Ok(())
    }

    /// Commits what was appended: a new index and trailer, synced to disk.
    ///
    /// # Errors
    /// If it can't be written.
    pub fn commit(&mut self) -> Result<()> {
        if self.writer.pending() > 0 {
            self.writer.commit(None)?;
            self.known_len = self.writer.end();
        }
        Ok(())
    }

    /// How many records the stream's head has: its snapshot and the updates
    /// since (a new snapshot is due when there are many).
    pub fn head_len(&self) -> usize {
        self.writer
            .index()
            .head(&StreamKey::main())
            .map_or(0, |head| {
                head.update_ids.len() + usize::from(!head.snapshot_id.is_empty())
            })
    }

    /// The bytes of the records the index lists, and the file's length:
    /// the rest is superseded (compaction reclaims it).
    pub fn live_and_total(&self) -> (u64, u64) {
        let live = self
            .writer
            .index()
            .entries()
            .iter()
            .map(|entry| entry.length)
            .sum();
        (live, self.writer.end())
    }
}

impl Session<File> {
    /// Whether the file this session writes into was deleted or replaced
    /// (writes would go to a file nobody can open again).
    pub fn file_gone(&self) -> bool {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            self.writer
                .storage()
                .metadata()
                .is_ok_and(|meta| meta.nlink() == 0)
        }
        #[cfg(not(unix))]
        {
            // Windows doesn't delete a file that is open.
            false
        }
    }
}

/// Where a session's file is, for reopening it: the path, and the
/// document it must hold.
#[derive(Clone, Debug)]
pub struct FileId {
    /// The file.
    pub path: PathBuf,
    /// The document in it.
    pub doc_id: DocId,
}
