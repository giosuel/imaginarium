// SPDX-License-Identifier: AGPL-3.0-or-later

//! Writing: create a file, append records, commit (spec §9.3).

use std::io::{Seek, SeekFrom, Write};

use prost::Message;

use crate::header::{FileHeader, HEADER_LEN};
use crate::index::IndexState;
use crate::payload::{RecordBuilder, RecordSealer};
use crate::pb;
use crate::record::{Record, RecordType};
use crate::trailer::{TRAILER_LEN, Trailer};
use crate::{FormatError, ObjectId, Reader, Result};

/// Storage that can be truncated and flushed to stable storage, so a commit
/// survives a crash and the trailer is always the last bytes (spec §9.3).
pub trait Durable {
    /// Makes all written data durable (e.g. `fsync`).
    fn sync(&mut self) -> std::io::Result<()>;
    /// Truncates (or extends) the storage to `len` bytes.
    fn set_len(&mut self, len: u64) -> std::io::Result<()>;
}

impl Durable for std::fs::File {
    fn sync(&mut self) -> std::io::Result<()> {
        self.sync_data()
    }
    fn set_len(&mut self, len: u64) -> std::io::Result<()> {
        std::fs::File::set_len(self, len)
    }
}

impl Durable for std::io::Cursor<Vec<u8>> {
    fn sync(&mut self) -> std::io::Result<()> {
        Ok(())
    }
    fn set_len(&mut self, len: u64) -> std::io::Result<()> {
        let len =
            usize::try_from(len).map_err(|_| std::io::Error::other("length exceeds memory"))?;
        self.get_mut().resize(len, 0);
        Ok(())
    }
}

impl<T: Durable + ?Sized> Durable for &mut T {
    fn sync(&mut self) -> std::io::Result<()> {
        (**self).sync()
    }
    fn set_len(&mut self, len: u64) -> std::io::Result<()> {
        (**self).set_len(len)
    }
}

/// Appends records to an `.imag` file.
///
/// Records are appended immediately. They become part of the file's state
/// only when [`Writer::commit`] writes a new index and trailer. A crash
/// before that leaves the previous commit intact (spec §9.5).
///
/// ```
/// use std::io::Cursor;
/// use imaginarium_format::{pb, DocId, FileHeader, ObjectId, Reader, RecordBuilder, RecordType, Writer};
///
/// let doc = DocId([1; 16]);
/// let mut writer = Writer::create(Cursor::new(Vec::new()), FileHeader::new(doc, false))?;
/// let snap = RecordBuilder::new(RecordType::SNAP, ObjectId([2; 32]))
///     .stream(pb::Stream::Main)
///     .codec("loro-1")
///     .build(&doc, b"snapshot".to_vec(), None)?;
/// writer.append(&snap)?;
/// writer.commit(None)?;
///
/// let mut reader = Reader::open(Cursor::new(writer.into_inner().into_inner()))?;
/// assert_eq!(reader.index().entries().len(), 1);
/// let entry = reader.index().entries()[0].clone();
/// assert_eq!(reader.read_content(&entry)?, b"snapshot");
/// # Ok::<(), imaginarium_format::FormatError>(())
/// ```
pub struct Writer<F> {
    file: F,
    header: FileHeader,
    state: IndexState,
    end: u64,
    last_index_offset: u64,
    pending: usize,
}

impl<F: Write + Seek + Durable> Writer<F> {
    /// Starts a new file: writes the header at offset 0. The storage should
    /// be empty (e.g. a newly created file).
    ///
    /// The header written is this implementation's: its minor version, and
    /// only the flags it knows (spec §4.1: writers write 0 for the ignorable
    /// bits). A header copied from a newer file doesn't claim more than this
    /// writer produces.
    pub fn create(mut file: F, header: FileHeader) -> Result<Self> {
        let header = FileHeader {
            minor: crate::header::MINOR,
            flags: header.flags & crate::header::FLAG_ENCRYPTED,
            ..header
        };
        file.seek(SeekFrom::Start(0))?;
        file.write_all(&header.encode())?;
        Ok(Self {
            file,
            header,
            state: IndexState::default(),
            end: HEADER_LEN as u64,
            last_index_offset: 0,
            pending: 0,
        })
    }

    /// Continues an existing file after its last valid commit. Bytes after
    /// that commit (from a crashed append) are overwritten.
    pub fn resume(reader: Reader<F>) -> Result<Self>
    where
        F: std::io::Read,
    {
        let (file, header, state, index_offset, end) = reader.into_parts();
        let mut state = state;
        state.previous_index_offset = index_offset;
        Ok(Self {
            file,
            header,
            state,
            end,
            last_index_offset: index_offset,
            pending: 0,
        })
    }

    /// The file header.
    pub fn header(&self) -> &FileHeader {
        &self.header
    }

    /// The index as it will be written by the next commit.
    pub fn index(&self) -> &IndexState {
        &self.state
    }

    /// Number of records appended since the last commit.
    pub fn pending(&self) -> usize {
        self.pending
    }

    /// Appends a record. Returns its location.
    pub fn append(&mut self, record: &Record) -> Result<pb::RecordRef> {
        if record.rtype() == RecordType::INDX {
            return Err(FormatError::Invalid(
                "INDX records are written by commit".into(),
            ));
        }
        if record.flags().contains(crate::RecordFlags::SIGNED) && record.signature_bytes().is_none()
        {
            return Err(FormatError::Invalid(
                "record has the SIGNED flag but no signature".into(),
            ));
        }
        check_limits(record)?;
        self.check_encryption(record)?;
        let bytes = record.encode();
        self.file.seek(SeekFrom::Start(self.end))?;
        self.file.write_all(&bytes)?;
        let location = pb::RecordRef {
            offset: self.end,
            length: bytes.len() as u64,
        };
        self.state.add(record, location.offset, location.length);
        self.end += location.length;
        self.pending += 1;
        Ok(location)
    }

    /// Commits: writes the index record, syncs, writes the trailer and syncs
    /// again. The first sync keeps the trailer from reaching the disk before
    /// the records it commits, so a crash never leaves a valid trailer over
    /// missing data (spec §9.3). Encrypted documents need `index_sealer`
    /// (the index is encrypted under `K_content`, spec §9.1).
    pub fn commit(&mut self, index_sealer: Option<&dyn RecordSealer>) -> Result<()> {
        self.commit_with_index_id(ObjectId::random()?, index_sealer)
    }

    /// Like [`Writer::commit`], with a given object id for the index record
    /// instead of a random one. Use it only for reproducible output, such as
    /// test vectors. Normal saves use [`Writer::commit`].
    pub fn commit_with_index_id(
        &mut self,
        index_id: ObjectId,
        index_sealer: Option<&dyn RecordSealer>,
    ) -> Result<()> {
        let encrypted = self.header.is_encrypted();
        if encrypted != index_sealer.is_some() {
            return Err(FormatError::Invalid(if encrypted {
                "encrypted documents need a sealer for the index".into()
            } else {
                "unencrypted documents must not encrypt the index".into()
            }));
        }
        self.state.previous_index_offset = self.last_index_offset;
        let mut builder = RecordBuilder::new(RecordType::INDX, index_id);
        if encrypted {
            builder = builder.key_records(self.state.key_records());
        }
        let index = builder.build(
            &self.header.doc_id,
            self.state.to_pb().encode_to_vec(),
            index_sealer,
        )?;
        let bytes = index.encode();
        let index_len = u32::try_from(bytes.len()).map_err(|_| FormatError::Limit {
            what: "index",
            value: bytes.len() as u64,
            limit: u64::from(u32::MAX),
        })?;
        let trailer = Trailer {
            index_offset: self.end,
            index_len,
        };
        self.file.seek(SeekFrom::Start(self.end))?;
        self.file.write_all(&bytes)?;
        self.file.flush()?;
        self.file.sync()?;
        self.file.write_all(&trailer.encode())?;
        self.file.flush()?;
        let new_end = self.end + u64::from(index_len) + TRAILER_LEN as u64;
        // Drop leftovers of a crashed save beyond the new trailer, so the
        // trailer is the last 24 bytes of the file.
        self.file.set_len(new_end)?;
        self.file.sync()?;
        self.last_index_offset = self.end;
        self.end = new_end;
        self.pending = 0;
        Ok(())
    }

    /// The underlying storage.
    pub fn into_inner(self) -> F {
        self.file
    }

    /// Spec §5.1: in encrypted documents everything except KEYR/ACLC is
    /// encrypted. In unencrypted documents nothing is.
    fn check_encryption(&self, record: &Record) -> Result<()> {
        match self.header.encryption_violation(record) {
            Some(problem) => Err(FormatError::Invalid(problem)),
            None => Ok(()),
        }
    }
}

/// Spec §4.6 and §5.3: writers MUST NOT exceed what readers must support,
/// or the file holds records its own reader rejects.
fn check_limits(record: &Record) -> Result<()> {
    use crate::record::{Limits, RecordFlags};
    let limits = Limits::default();
    let over = |what: &'static str, value: u64, limit: u64| {
        (value > limit).then_some(FormatError::Limit { what, value, limit })
    };
    let header = record.header();
    let segment_limit = if record.flags().contains(RecordFlags::SEGMENTED) {
        crate::SEGMENT_LEN as u64
    } else if record.rtype() == RecordType::BLOB {
        crate::SEGMENT_THRESHOLD as u64 // larger blobs are segmented
    } else {
        limits.max_payload
    };
    let problem = over(
        "hlen",
        record.header_bytes().len() as u64,
        limits.max_header,
    )
    .or_else(|| over("plen", record.payload().len() as u64, limits.max_payload))
    .or_else(|| over("plain_len", header.plain_len, segment_limit))
    .or_else(|| {
        let signature = record.signature_bytes().map_or(0, <[u8]>::len);
        over("slen", signature as u64, limits.max_signature)
    });
    problem.map_or(Ok(()), Err)
}
