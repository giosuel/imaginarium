// SPDX-License-Identifier: AGPL-3.0-or-later

//! Reading: open a file through its trailer and index, fall back to
//! recovery, and read records and contents (spec §9.2, §9.5).

use std::io::{Read, Seek, SeekFrom};

use prost::Message;

use crate::header::{FileHeader, HEADER_LEN};
use crate::index::IndexState;
use crate::payload::RecordOpener;
use crate::pb;
use crate::record::{Limits, Record, RecordFlags, RecordType};
use crate::recovery::{self, Recovery};
use crate::trailer::{TRAILER_LEN, Trailer};
use crate::{FormatError, ObjectId, Result};

/// Builds a [`RecordOpener`] from a document's key records (`KEYR` and
/// `ACLC`, spec §6.4, §8). It's called while opening an encrypted document,
/// before its index can be decrypted. Implemented by `imaginarium-crypto`
/// (password prompt, device keys, …).
pub trait KeyProvider {
    /// Returns an opener for the document, or an error if no key fits.
    fn opener(
        &mut self,
        header: &FileHeader,
        key_records: &[Record],
    ) -> Result<Box<dyn RecordOpener>>;
}

/// Options for [`Reader::open_with`].
#[derive(Clone, Copy, Debug)]
pub struct OpenOptions {
    /// Size limits (spec §4.6).
    pub limits: Limits,
    /// Whether to run recovery (spec §9.5) when the trailer or index is
    /// invalid. If `false`, such files fail to open.
    pub recover: bool,
}

impl Default for OpenOptions {
    fn default() -> Self {
        Self {
            limits: Limits::default(),
            recover: true,
        }
    }
}

/// An opened `.imag` file.
pub struct Reader<F> {
    file: F,
    header: FileHeader,
    state: IndexState,
    /// Offset of the index record in use (0 if recovered without one).
    index_offset: u64,
    /// End of the last valid commit: where appends would continue.
    end: u64,
    limits: Limits,
    opener: Option<Box<dyn RecordOpener>>,
    recovery: Option<Recovery>,
}

impl<F: Read + Seek> Reader<F> {
    /// Opens an unencrypted document with default options.
    pub fn open(file: F) -> Result<Self> {
        Self::open_with(file, OpenOptions::default(), None)
    }

    /// Opens a document. Encrypted documents need `keys`.
    pub fn open_with(
        mut file: F,
        options: OpenOptions,
        mut keys: Option<&mut (dyn KeyProvider + '_)>,
    ) -> Result<Self> {
        let file_len = file.seek(SeekFrom::End(0))?;
        let mut header_bytes = [0u8; HEADER_LEN];
        read_exact_at(&mut file, 0, &mut header_bytes, "file header")?;
        let header = FileHeader::decode(&header_bytes)?;
        if header.is_encrypted() && keys.is_none() {
            return Err(FormatError::NeedsKey);
        }
        let limits = options.limits;

        let primary = load_from_trailer(
            &mut file,
            &header,
            file_len,
            file_len,
            &limits,
            keys.as_deref_mut(),
        );
        let (state, index_offset, end, opener, recovery) = match primary {
            Ok((state, opener, index_offset)) => (state, index_offset, file_len, opener, None),
            // Recovery is for torn writes. A wrong key or tampered ciphertext
            // (behind a valid CRC) is reported, not "repaired" (spec §9.5).
            Err(err) if options.recover && !err.is_key_problem() => {
                let found = recovery::recover(&mut file, &header, file_len, &limits, keys, &err)?;
                (
                    found.state,
                    found.index_offset,
                    found.end,
                    found.opener,
                    Some(found.report),
                )
            }
            Err(err) => return Err(err),
        };
        Ok(Self {
            file,
            header,
            state,
            index_offset,
            end,
            limits,
            opener,
            recovery,
        })
    }

    /// The file header.
    pub fn header(&self) -> &FileHeader {
        &self.header
    }

    /// The index: entries and heads.
    pub fn index(&self) -> &IndexState {
        &self.state
    }

    /// How the file was recovered, if the last commit wasn't usable.
    pub fn recovery(&self) -> Option<&Recovery> {
        self.recovery.as_ref()
    }

    /// Reads and verifies the record described by an index entry.
    pub fn read_record(&mut self, entry: &pb::IndexEntry) -> Result<Record> {
        let record = read_record_at(
            &mut self.file,
            entry.offset,
            entry.length,
            self.end,
            &self.limits,
        )?;
        if record.rtype().0 != entry.rtype || record.header().object_id != entry.object_id {
            return Err(FormatError::Invalid(format!(
                "index entry at {} doesn't match the record there",
                entry.offset
            )));
        }
        check_encryption(&self.header, &record)?;
        Ok(record)
    }

    /// Reads a record and returns its plaintext content (decrypted and
    /// decompressed, spec §4.4).
    pub fn read_content(&mut self, entry: &pb::IndexEntry) -> Result<Vec<u8>> {
        let record = self.read_record(entry)?;
        record.open_content(&self.header.doc_id, self.opener.as_deref(), &self.limits)
    }

    /// Reads a whole blob by id, joining its segments (spec §5.3).
    pub fn read_blob(&mut self, object_id: &ObjectId) -> Result<Vec<u8>> {
        let entries: Vec<pb::IndexEntry> = self
            .state
            .entries()
            .iter()
            .filter(|e| e.rtype == RecordType::BLOB.0 && e.object_id == object_id.0)
            .cloned()
            .collect();
        let Some(first) = entries.first() else {
            return Err(FormatError::Invalid(format!("no blob {object_id}")));
        };
        if first.rflags & u32::from(RecordFlags::SEGMENTED.0) == 0 {
            return self.read_content(first);
        }
        let mut segments: Vec<(pb::Segment, Vec<u8>)> = Vec::with_capacity(entries.len());
        for entry in &entries {
            let record = self.read_record(entry)?;
            let segment = record.header().segment.clone().ok_or_else(|| {
                FormatError::Invalid("SEGMENTED blob without segment info".into())
            })?;
            let content =
                record.open_content(&self.header.doc_id, self.opener.as_deref(), &self.limits)?;
            segments.push((segment, content));
        }
        assemble_segments(segments)
    }

    /// Takes the reader apart (for [`crate::Writer::resume`]).
    pub(crate) fn into_parts(self) -> (F, FileHeader, IndexState, u64, u64) {
        (
            self.file,
            self.header,
            self.state,
            self.index_offset,
            self.end,
        )
    }
}

/// Validates and joins blob segments.
fn assemble_segments(mut segments: Vec<(pb::Segment, Vec<u8>)>) -> Result<Vec<u8>> {
    segments.sort_by_key(|(segment, _)| segment.index);
    let count = segments.first().map_or(0, |(s, _)| s.count);
    let total = segments.first().map_or(0, |(s, _)| s.total_len);
    let consistent = segments
        .iter()
        .enumerate()
        .all(|(i, (s, _))| s.index as usize == i && s.count == count && s.total_len == total);
    if !consistent || segments.len() != count as usize {
        return Err(FormatError::Invalid(format!(
            "blob segments incomplete or inconsistent: {} of {count}",
            segments.len()
        )));
    }
    let mut out = Vec::with_capacity(usize::try_from(total).unwrap_or(0));
    for (_, content) in segments {
        out.extend_from_slice(&content);
    }
    if out.len() as u64 != total {
        return Err(FormatError::PlainLen {
            expected: total,
            actual: out.len() as u64,
        });
    }
    Ok(out)
}

/// The opened index, its opener (encrypted documents) and its offset.
pub(crate) type LoadedIndex = (IndexState, Option<Box<dyn RecordOpener>>, u64);

/// Loads the index named by the trailer that ends at `trailer_end`.
pub(crate) fn load_from_trailer<F: Read + Seek>(
    file: &mut F,
    header: &FileHeader,
    file_len: u64,
    trailer_end: u64,
    limits: &Limits,
    keys: Option<&mut (dyn KeyProvider + '_)>,
) -> Result<LoadedIndex> {
    if trailer_end < (HEADER_LEN + TRAILER_LEN) as u64 || trailer_end > file_len {
        return Err(FormatError::Truncated { what: "trailer" });
    }
    let trailer_start = trailer_end - TRAILER_LEN as u64;
    let mut bytes = [0u8; TRAILER_LEN];
    read_exact_at(file, trailer_start, &mut bytes, "trailer")?;
    let trailer = Trailer::decode(&bytes)?;
    if trailer.index_offset < HEADER_LEN as u64
        || trailer
            .index_offset
            .checked_add(u64::from(trailer.index_len))
            != Some(trailer_start)
    {
        return Err(FormatError::Invalid(
            "trailer doesn't point to the record before it".into(),
        ));
    }
    let (state, opener) = load_index(
        file,
        header,
        trailer.index_offset,
        u64::from(trailer.index_len),
        limits,
        keys,
    )?;
    Ok((state, opener, trailer.index_offset))
}

/// Reads, decrypts (if needed) and validates an index record.
pub(crate) fn load_index<F: Read + Seek>(
    file: &mut F,
    header: &FileHeader,
    offset: u64,
    length: u64,
    limits: &Limits,
    keys: Option<&mut (dyn KeyProvider + '_)>,
) -> Result<(IndexState, Option<Box<dyn RecordOpener>>)> {
    let index_record = read_record_at(file, offset, length, offset + length, limits)?;
    if index_record.rtype() != RecordType::INDX {
        return Err(FormatError::Invalid(
            "trailer points to a non-index record".into(),
        ));
    }
    check_encryption(header, &index_record)?;
    let opener = if header.is_encrypted() {
        let keys = keys.ok_or(FormatError::NeedsKey)?;
        let mut key_records = Vec::with_capacity(index_record.header().key_records.len());
        for location in &index_record.header().key_records {
            let record = read_record_at(file, location.offset, location.length, offset, limits)?;
            if !matches!(record.rtype(), RecordType::KEYR | RecordType::ACLC) {
                return Err(FormatError::Invalid(
                    "key_records lists a non-key record".into(),
                ));
            }
            check_encryption(header, &record)?;
            key_records.push(record);
        }
        Some(keys.opener(header, &key_records)?)
    } else {
        if !index_record.header().key_records.is_empty() {
            return Err(FormatError::Invalid(
                "unencrypted documents must not list key_records".into(),
            ));
        }
        None
    };
    let content = index_record.open_content(&header.doc_id, opener.as_deref(), limits)?;
    let index = pb::Index::decode(content.as_slice()).map_err(|source| FormatError::Protobuf {
        what: "Index",
        source,
    })?;
    let state = IndexState::from_pb(index);
    validate_index(header, &state, offset, &index_record)?;
    Ok((state, opener))
}

/// Spec §5.1 for a record read from the file. A violation means someone
/// changed the file without the key, so it's reported like a failed
/// decryption (a key problem), which recovery doesn't paper over.
pub(crate) fn check_encryption(header: &FileHeader, record: &Record) -> Result<()> {
    match header.encryption_violation(record) {
        Some(problem) => Err(FormatError::Crypto(problem)),
        None => Ok(()),
    }
}

/// Entries must lie inside the records area before the index, and in
/// encrypted documents the INDX header's `key_records` must match the
/// KEYR/ACLC entries (spec §9.1).
fn validate_index(
    header: &FileHeader,
    state: &IndexState,
    index_offset: u64,
    index_record: &Record,
) -> Result<()> {
    for entry in state.entries() {
        let end = entry.offset.checked_add(entry.length);
        if entry.offset < HEADER_LEN as u64
            || end.is_none_or(|end| end > index_offset)
            || entry.length == 0
        {
            return Err(FormatError::Invalid(format!(
                "index entry at {} lies outside the file",
                entry.offset
            )));
        }
    }
    if header.is_encrypted() && index_record.header().key_records != state.key_records() {
        return Err(FormatError::Invalid(
            "key_records don't match the index".into(),
        ));
    }
    Ok(())
}

/// Reads exactly `length` bytes at `offset` (bounded by `limit_end`) and
/// decodes one record that must span them exactly.
pub(crate) fn read_record_at<F: Read + Seek>(
    file: &mut F,
    offset: u64,
    length: u64,
    limit_end: u64,
    limits: &Limits,
) -> Result<Record> {
    let end = offset
        .checked_add(length)
        .filter(|&end| offset >= HEADER_LEN as u64 && end <= limit_end)
        .ok_or_else(|| {
            FormatError::Invalid(format!(
                "record at {offset} (+{length}) lies outside the file"
            ))
        })?;
    let max_record = limits.max_header + limits.max_payload + limits.max_signature + 64;
    if length > max_record {
        return Err(FormatError::Limit {
            what: "record",
            value: length,
            limit: max_record,
        });
    }
    let mut buf =
        vec![0u8; usize::try_from(length).map_err(|_| FormatError::Truncated { what: "record" })?];
    read_exact_at(file, offset, &mut buf, "record")?;
    let (record, consumed) = Record::decode(&buf, limits)?;
    if consumed as u64 != end - offset {
        return Err(FormatError::Invalid(format!(
            "record at {offset} has trailing bytes"
        )));
    }
    Ok(record)
}

pub(crate) fn read_exact_at<F: Read + Seek>(
    file: &mut F,
    offset: u64,
    buf: &mut [u8],
    what: &'static str,
) -> Result<()> {
    file.seek(SeekFrom::Start(offset))?;
    file.read_exact(buf).map_err(|err| match err.kind() {
        std::io::ErrorKind::UnexpectedEof => FormatError::Truncated { what },
        _ => FormatError::Io(err),
    })
}
