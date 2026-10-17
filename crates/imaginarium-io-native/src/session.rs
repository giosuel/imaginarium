// SPDX-License-Identifier: AGPL-3.0-or-later

//! An opened file kept open for saving: each change is appended as an
//! `UPDT` record and committed with a new index (spec §9.3), so every
//! change reaches the file on its own (PLAN §14, "Saving").
//!
//! A file is only written when this version may. A second writer (another
//! window or app) is kept out by a lock: on Unix an advisory lock on the
//! file itself, on Windows a lock on a file next to it (a lock on the file
//! itself would keep everyone from reading it), and none where the file
//! system can't lock. It opens read-only when it can't be written, is in
//! use, comes from a newer format, or needs features or records this
//! version doesn't know (spec §4.3, §10.2, §10.3). Encrypted and Hub files
//! can't be opened for saving yet (their keys aren't kept).

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
    /// Where the file is (none for storage that isn't a file).
    path: Option<PathBuf>,
    /// The file's head as read (snapshot, then updates), once: for the
    /// saving thread's copy of the document.
    parts: Vec<Vec<u8>>,
    /// The last save was torn and the file was recovered (spec §9.5).
    recovered: bool,
    /// The file's length as this session last left it (a torn tail after
    /// a recovery stays until the next commit cuts it off).
    known_len: u64,
    /// Keeps other writers out while it lives (dropped after the writer).
    lock: Lock,
}

/// How a file opened.
pub enum Opening {
    /// To edit: its changes are saved through the session.
    Editable(Opened, Box<Session>),
    /// To read only: the document is read-only (its mode says why).
    ReadOnly(Opened),
}

/// What keeps a second writer out of a file.
enum Lock {
    /// An advisory lock on the file's own handle (Unix).
    Handle,
    /// Nothing locked: storage that isn't a file, or a file system that
    /// can't lock (NFS without lockd, some FUSE and SMB mounts).
    None,
    /// A locked file next to it, removed with the lock (Windows).
    #[cfg_attr(not(windows), allow(dead_code))]
    Beside(File, PathBuf),
}

impl Drop for Lock {
    fn drop(&mut self) {
        if let Self::Beside(_, path) = self {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// Why a file opens read-only when another writer holds it
/// ([`imaginarium_model::ReadOnlyReason::File`]).
pub const IN_USE: &str = "it is open in another window or app";

/// Locks the file at `path` (open as `file`) against other writers.
/// `Err(IN_USE)` if another writer has it.
fn lock(file: &File, path: &Path) -> std::result::Result<Lock, &'static str> {
    #[cfg(windows)]
    {
        let _ = file;
        let name = path.file_name().map_or_else(
            || "board".into(),
            |name| name.to_string_lossy().into_owned(),
        );
        let beside = path.with_file_name(format!(".~{name}.lock"));
        let Ok(holder) = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&beside)
        else {
            // A folder where nothing can be created: no lock to take.
            return Ok(Lock::None);
        };
        match holder.try_lock() {
            Ok(()) => Ok(Lock::Beside(holder, beside)),
            Err(std::fs::TryLockError::WouldBlock) => Err(IN_USE),
            Err(std::fs::TryLockError::Error(_)) => Ok(Lock::None),
        }
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        match file.try_lock() {
            Ok(()) => Ok(Lock::Handle),
            Err(std::fs::TryLockError::WouldBlock) => Err(IN_USE),
            // A file system without locks (NFS without lockd, some FUSE and
            // SMB mounts): it opens, with nothing to keep a second writer
            // out.
            Err(std::fs::TryLockError::Error(_)) => Ok(Lock::None),
        }
    }
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
        let (file, held) = match writable {
            Ok(file) => {
                let held = lock(&file, path);
                (file, held)
            }
            Err(err)
                if matches!(
                    err.kind(),
                    std::io::ErrorKind::PermissionDenied | std::io::ErrorKind::ReadOnlyFilesystem
                ) =>
            {
                (File::open(path)?, Err("the file can't be written"))
            }
            Err(err) => return Err(err.into()),
        };
        let (mut opened, session) = Self::from_file(file)?;
        match (held, session) {
            (Err(reason), _) => {
                opened
                    .document
                    .set_read_only(ReadOnlyReason::File(reason.to_owned()));
                Ok(Opening::ReadOnly(opened))
            }
            (Ok(lock), Some(mut session)) => {
                session.path = Some(path.to_owned());
                session.lock = lock;
                Ok(Opening::Editable(opened, Box::new(session)))
            }
            (Ok(_), None) => Ok(Opening::ReadOnly(opened)),
        }
    }

    /// Opens the file at `path` again for saving (after it was replaced or
    /// compacted): it must still be document `doc_id`, not in use
    /// elsewhere, and writable by this version. Its head is read
    /// ([`Session::take_parts`]), so changes made to it meanwhile can be
    /// merged before anything replaces them.
    ///
    /// # Errors
    /// If it can't be opened, locked or read, or holds another document.
    pub fn reopen(path: &Path, doc_id: &DocId) -> Result<Self> {
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)?;
        let held = lock(&file, path).map_err(NativeError::CantSave)?;
        let mut session = Self::resume(file, doc_id)?;
        session.path = Some(path.to_owned());
        session.lock = held;
        Ok(session)
    }

    /// Makes a new file at `path` holding `document` (or writes it over
    /// a broken one this app left, at a retry), locked before anything is
    /// in it: another app looking for kept boards never takes it half
    /// made, or between its making and its locking.
    ///
    /// # Errors
    /// If it can't be made or written, or another writer has the file.
    pub fn create(path: &Path, document: &Document, options: &crate::SaveOptions) -> Result<Self> {
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)?;
        let held = lock(&file, path).map_err(NativeError::CantSave)?;
        file.set_len(0)?;
        let file = crate::save_new(file, document, options)?;
        let (_, session) = Self::from_file(file)?;
        let mut session = session.ok_or(NativeError::CantSave("the new file can't be written"))?;
        session.path = Some(path.to_owned());
        session.lock = held;
        Ok(session)
    }

    /// Whether the file is locked against other writers (it can't be on
    /// some file systems).
    pub fn is_locked(&self) -> bool {
        matches!(self.lock, Lock::Handle | Lock::Beside(..))
    }

    /// Deletes the file, while still holding its lock, so no other app
    /// takes it in between; a file already gone is fine.
    ///
    /// # Errors
    /// If it can't be deleted.
    pub fn delete(self) -> std::io::Result<()> {
        let Self {
            writer, path, lock, ..
        } = self;
        let Some(path) = path else {
            return Ok(());
        };
        // Windows deletes no file that is open, so the handle closes first
        // (the lock beside it stays until the end).
        let writer = if cfg!(windows) {
            drop(writer);
            None
        } else {
            Some(writer)
        };
        let removed = match std::fs::remove_file(&path) {
            Err(err) if err.kind() != std::io::ErrorKind::NotFound => Err(err),
            _ => Ok(()),
        };
        drop(writer);
        drop(lock);
        removed
    }

    /// Whether the file this session writes into was deleted or replaced:
    /// writes would go where nobody can open them again.
    pub fn file_gone(&self) -> bool {
        let Some(path) = &self.path else {
            return false;
        };
        let Ok(on_path) = std::fs::metadata(path) else {
            return true;
        };
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let Ok(open) = self.writer.storage().metadata() else {
                return true;
            };
            open.nlink() == 0 || (open.dev(), open.ino()) != (on_path.dev(), on_path.ino())
        }
        #[cfg(not(unix))]
        {
            // Without a file id to compare: another file there has
            // another length (this session keeps the length it wrote).
            on_path.len() != self.known_len
        }
    }

    /// Whether compacting this file could lose something: records this
    /// version doesn't know (compaction drops some of them, spec §9.4), or
    /// other names for the file (hard links would keep the old one).
    pub fn compaction_unsafe(&self) -> bool {
        let unknown = self.writer.index().entries().iter().any(|entry| {
            let flags = RecordFlags(u8::try_from(entry.rflags).unwrap_or(0xFF));
            flags.has_reserved() || !RecordType(entry.rtype).is_known()
        });
        #[cfg(unix)]
        let linked = {
            use std::os::unix::fs::MetadataExt;
            self.writer
                .storage()
                .metadata()
                .is_ok_and(|meta| meta.nlink() > 1)
        };
        #[cfg(not(unix))]
        let linked = false;
        unknown || linked
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
    // A reserved flag bit makes a record one of unknown type (spec §4.3).
    let unknown_critical = reader.index().entries().iter().any(|entry| {
        let flags = RecordFlags(u8::try_from(entry.rflags).unwrap_or(0xFF));
        (flags.has_reserved() || !RecordType(entry.rtype).is_known())
            && flags.contains(RecordFlags::CRITICAL)
    });
    unknown_critical.then_some("it holds data this version doesn't know")
}

impl<F: Read + Write + Seek + Durable> Session<F> {
    /// Opens `file` to edit it: the document, and a session if this
    /// version may write into it (else the document is read-only). The
    /// file isn't locked: [`Session::open`] locks files.
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
        if let Some(reason) = refusal(&reader, meta.as_ref()) {
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
                path: None,
                parts,
                recovered,
                known_len,
                lock: Lock::None,
            }),
        ))
    }

    /// Continues `file` for saving without reading the document into a
    /// model: it must be document `doc_id` and writable by this version.
    /// Its head is read for [`Session::take_parts`].
    ///
    /// # Errors
    /// If it can't be read, or holds another document or one this
    /// version mustn't write.
    pub fn resume(file: F, doc_id: &DocId) -> Result<Self> {
        let mut reader: Reader<F> = open_reader(file, Credentials::None)?;
        if reader.header().doc_id != *doc_id {
            return Err(NativeError::CantSave(
                "another document is in its place now",
            ));
        }
        let meta = read_meta(&mut reader)?;
        if let Some(reason) = refusal(&reader, meta.as_ref()) {
            return Err(NativeError::CantSave(reason));
        }
        let records = IndexedRecords::new(&reader);
        let parts =
            stream_parts(&mut reader, &records, &StreamKey::main(), None)?.unwrap_or_default();
        let recovered = reader.recovery().is_some();
        let mut writer = Writer::resume(reader)?;
        let known_len = writer.len_on_disk()?;
        Ok(Self {
            doc_id: *doc_id,
            writer,
            path: None,
            parts,
            recovered,
            known_len,
            lock: Lock::None,
        })
    }

    /// The same file again, read afresh from its own handle after a failed
    /// write (the writer's state isn't trusted then). The handle, and with
    /// it the lock, stays.
    ///
    /// # Errors
    /// If it can't be read.
    pub fn rebuild(self) -> Result<Self> {
        let Self {
            writer,
            doc_id,
            path,
            lock,
            ..
        } = self;
        let mut session = Self::resume(writer.into_inner(), &doc_id)?;
        session.path = path;
        session.lock = lock;
        Ok(session)
    }

    /// The document's id (spec §4.1).
    pub fn doc_id(&self) -> DocId {
        self.doc_id
    }

    /// Whether the file was recovered from a torn save when read.
    pub fn recovered(&self) -> bool {
        self.recovered
    }

    /// The file's head as read, once (for a copy of the document, or to
    /// merge changes made to the file meanwhile).
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
    /// It must hold everything the file's head does.
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

/// Where a session's file is, for reopening it: the path, and the
/// document it must hold.
#[derive(Clone, Debug)]
pub struct FileId {
    /// The file.
    pub path: PathBuf,
    /// The document in it.
    pub doc_id: DocId,
}
