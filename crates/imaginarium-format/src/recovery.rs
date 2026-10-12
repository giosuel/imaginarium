// SPDX-License-Identifier: AGPL-3.0-or-later

//! Recovery of files whose last commit is unusable (spec §9.5).
//!
//! Recovery builds on the newest commit whose index loads: one with an
//! intact trailer (searched backwards from the end), or a newer index
//! record whose trailer was lost (found scanning forward from there).
//! Complete records after that commit are adopted if they authenticate
//! under its keys. Without any usable index, one is rebuilt from the
//! records after the header.
//!
//! An index with a valid CRC that fails to decrypt is reported, never
//! skipped. Otherwise a reader without the newest epoch's key could read
//! around it.

use std::io::{Read, Seek, SeekFrom};

use crate::header::{FileHeader, HEADER_LEN};
use crate::index::IndexState;
use crate::payload::RecordOpener;
use crate::reader::{KeyProvider, load_from_trailer, load_index, read_exact_at, read_record_at};
use crate::record::{Limits, Record, RecordType};
use crate::trailer::{TRAILER_LEN, TRAILER_MAGIC, Trailer};
use crate::{FormatError, Result};

/// How a file was recovered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RecoveryMethod {
    /// An earlier commit was intact: an index with its trailer, or one
    /// whose trailer was lost. It ends at `commit_end`. Complete records
    /// found after it (e.g. an autosave that crashed before committing)
    /// were adopted if they authenticate, and `discarded_bytes` at the end
    /// were not.
    PreviousCommit {
        /// End offset of the commit used.
        commit_end: u64,
        /// Complete records after the commit that were added back.
        adopted_records: usize,
        /// Bytes at the end that couldn't be read as records.
        discarded_bytes: u64,
    },
    /// No usable index: one was rebuilt from the records after the header,
    /// up to the first one that is invalid or doesn't authenticate.
    ForwardScan {
        /// Records adopted (index records and trailers are not counted).
        records: usize,
        /// End of the last adopted record.
        stopped_at: u64,
        /// Why it stopped (end of file, or the error found there).
        reason: String,
    },
}

/// What recovery did and why it was needed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Recovery {
    /// Why the normal open failed.
    pub cause: String,
    /// How the index was obtained.
    pub method: RecoveryMethod,
}

pub(crate) struct Recovered {
    pub(crate) state: IndexState,
    pub(crate) index_offset: u64,
    pub(crate) end: u64,
    pub(crate) opener: Option<Box<dyn RecordOpener>>,
    pub(crate) report: Recovery,
}

/// Chunk size of the backward search for trailer magics.
const CHUNK: u64 = 64 * 1024;

/// Trailers with a valid CRC that the backward search tries. Each one
/// costs a read of its index, so a crafted file full of them can't make
/// recovery quadratic. Real files need one or two.
const MAX_TRAILERS: usize = 16;

/// Index records without a usable trailer that are tried, newest first.
const MAX_LOST_INDEXES: usize = 4;

/// A commit that recovery builds on.
struct Commit {
    state: IndexState,
    opener: Option<Box<dyn RecordOpener>>,
    index_offset: u64,
    /// Where the commit ends: after its trailer, or after its index if the
    /// trailer was lost.
    end: u64,
}

pub(crate) fn recover<F: Read + Seek>(
    file: &mut F,
    header: &FileHeader,
    file_len: u64,
    limits: &Limits,
    mut keys: Option<&mut (dyn KeyProvider + '_)>,
    cause: &FormatError,
) -> Result<Recovered> {
    let cause = cause.to_string();

    // 1. Backwards: the newest earlier commit whose trailer and index verify.
    let mut commit = None;
    let mut search = MagicSearch::new(file_len);
    let mut tried = 0;
    while let Some(magic_at) = search.next(file)? {
        let trailer_end = magic_at + TRAILER_MAGIC.len() as u64;
        if trailer_end == file_len {
            continue; // that's the trailer that already failed
        }
        // Stray magics (in blob data, say) mostly fail the trailer CRC,
        // which is cheap to check.
        let mut bytes = [0u8; TRAILER_LEN];
        read_exact_at(
            file,
            trailer_end - TRAILER_LEN as u64,
            &mut bytes,
            "trailer",
        )?;
        if Trailer::decode(&bytes).is_err() {
            continue;
        }
        if tried == MAX_TRAILERS {
            break;
        }
        tried += 1;
        match load_from_trailer(
            file,
            header,
            file_len,
            trailer_end,
            limits,
            keys.as_deref_mut(),
        ) {
            Ok((state, opener, index_offset)) => {
                commit = Some(Commit {
                    state,
                    opener,
                    index_offset,
                    end: trailer_end,
                });
                break;
            }
            Err(err) if err.is_key_problem() => return Err(err),
            Err(_) => {}
        }
    }

    // 2. Forwards from there (or from the header). A newer index whose
    //    trailer was lost is a commit as well.
    let start = commit.as_ref().map_or(HEADER_LEN as u64, |c| c.end);
    let scan = scan_forward(file, header, start, file_len, limits)?;
    for &(offset, length) in scan.indexes.iter().rev().take(MAX_LOST_INDEXES) {
        match load_index(file, header, offset, length, limits, keys.as_deref_mut()) {
            Ok((state, opener)) => {
                let mut end = offset + length;
                if is_trailer_for(file, end, file_len, offset)? {
                    end += TRAILER_LEN as u64;
                }
                commit = Some(Commit {
                    state,
                    opener,
                    index_offset: offset,
                    end,
                });
                break;
            }
            Err(err) if err.is_key_problem() => return Err(err),
            Err(_) => {}
        }
    }

    match commit {
        Some(commit) => adopt_after(file, header, file_len, limits, commit, &scan, cause),
        None => rebuild(file, header, file_len, limits, keys, &scan, cause),
    }
}

/// Adopts the complete records after `commit` that authenticate under its
/// keys, up to the first one that doesn't. Uncommitted `KEYR` and `ACLC`
/// records are left out: keys and access changes count only once they are
/// committed.
fn adopt_after<F: Read + Seek>(
    file: &mut F,
    header: &FileHeader,
    file_len: u64,
    limits: &Limits,
    commit: Commit,
    scan: &Scan,
    cause: String,
) -> Result<Recovered> {
    let Commit {
        mut state,
        opener,
        index_offset,
        end: commit_end,
    } = commit;
    let mut end = commit_end;
    let mut adopted = 0;
    for found in scan.records.iter().filter(|f| f.offset >= commit_end) {
        if matches!(found.rtype, RecordType::KEYR | RecordType::ACLC) {
            continue;
        }
        let record = read_record_at(file, found.offset, found.length, file_len, limits)?;
        if record
            .open_content(&header.doc_id, opener.as_deref(), limits)
            .is_err()
        {
            break;
        }
        state.add(&record, found.offset, found.length);
        adopted += 1;
        end = found.offset + found.length;
    }
    Ok(Recovered {
        state,
        index_offset,
        end,
        opener,
        report: Recovery {
            cause,
            method: RecoveryMethod::PreviousCommit {
                commit_end,
                adopted_records: adopted,
                discarded_bytes: file_len - end,
            },
        },
    })
}

/// Without a usable index: rebuilds one from the records after the header,
/// up to the first one that doesn't authenticate under the keys found among
/// them.
fn rebuild<F: Read + Seek>(
    file: &mut F,
    header: &FileHeader,
    file_len: u64,
    limits: &Limits,
    keys: Option<&mut (dyn KeyProvider + '_)>,
    scan: &Scan,
    cause: String,
) -> Result<Recovered> {
    let opener = if header.is_encrypted() {
        let keys = keys.ok_or(FormatError::NeedsKey)?;
        let mut key_records = Vec::new();
        for found in &scan.records {
            if matches!(found.rtype, RecordType::KEYR | RecordType::ACLC) {
                key_records.push(read_record_at(
                    file,
                    found.offset,
                    found.length,
                    file_len,
                    limits,
                )?);
            }
        }
        Some(keys.opener(header, &key_records)?)
    } else {
        None
    };
    let mut state = IndexState::default();
    let mut end = HEADER_LEN as u64;
    let mut records = 0;
    let mut reason = scan.reason.clone();
    for found in &scan.records {
        let record = read_record_at(file, found.offset, found.length, file_len, limits)?;
        if let Err(err) = record.open_content(&header.doc_id, opener.as_deref(), limits) {
            reason = format!("the record at {} can't be read: {err}", found.offset);
            break;
        }
        state.add(&record, found.offset, found.length);
        records += 1;
        end = found.offset + found.length;
    }
    if records == 0 {
        return Err(FormatError::NoValidIndex(format!(
            "{cause}; no readable records"
        )));
    }
    Ok(Recovered {
        state,
        index_offset: 0,
        end,
        opener,
        report: Recovery {
            cause,
            method: RecoveryMethod::ForwardScan {
                records,
                stopped_at: end,
                reason,
            },
        },
    })
}

/// A record found by [`scan_forward`].
struct Found {
    offset: u64,
    length: u64,
    rtype: RecordType,
}

struct Scan {
    /// Every record except index records, in file order.
    records: Vec<Found>,
    /// Index records as (offset, length), in file order.
    indexes: Vec<(u64, u64)>,
    /// Why scanning stopped (end of file, or the error found there).
    reason: String,
}

/// Lists every complete record from `start` up to the first unreadable
/// one, skipping the trailers after index records. A record that breaks
/// the encryption policy (spec §5.1) counts as unreadable.
fn scan_forward<F: Read + Seek>(
    file: &mut F,
    header: &FileHeader,
    start: u64,
    file_len: u64,
    limits: &Limits,
) -> Result<Scan> {
    let mut scan = Scan {
        records: Vec::new(),
        indexes: Vec::new(),
        reason: String::new(),
    };
    let mut pos = start;
    scan.reason = loop {
        if pos >= file_len {
            break "end of file".to_owned();
        }
        file.seek(SeekFrom::Start(pos))?;
        let mut reader = (&mut *file).take(file_len - pos);
        let (record, len) = match Record::read_from(&mut reader, limits) {
            Ok(found) => found,
            Err(err) => break format!("at {pos}: {err}"),
        };
        let len = len as u64;
        if let Some(problem) = header.encryption_violation(&record) {
            break format!("at {pos}: {problem}");
        }
        if record.rtype() == RecordType::INDX {
            scan.indexes.push((pos, len));
            pos += len;
            // An index is followed by its trailer, which is not a record.
            if is_trailer_for(file, pos, file_len, pos - len)? {
                pos += TRAILER_LEN as u64;
            }
            continue;
        }
        scan.records.push(Found {
            offset: pos,
            length: len,
            rtype: record.rtype(),
        });
        pos += len;
    };
    Ok(scan)
}

/// Whether a valid trailer for the index at `index_offset` starts at `pos`.
fn is_trailer_for<F: Read + Seek>(
    file: &mut F,
    pos: u64,
    file_len: u64,
    index_offset: u64,
) -> Result<bool> {
    if pos + TRAILER_LEN as u64 > file_len {
        return Ok(false);
    }
    let mut bytes = [0u8; TRAILER_LEN];
    read_exact_at(file, pos, &mut bytes, "trailer")?;
    Ok(Trailer::decode(&bytes).is_ok_and(|t| t.index_offset == index_offset))
}

/// Trailer-magic offsets from the end of the file towards the header,
/// newest first. Reads one chunk at a time, so a good trailer near the end
/// is found without reading the whole file.
struct MagicSearch {
    /// Offsets found in the current chunk, ascending, so `pop` is newest.
    found: Vec<u64>,
    /// `HEADER_LEN..end` hasn't been searched yet.
    end: u64,
    file_len: u64,
}

impl MagicSearch {
    fn new(file_len: u64) -> Self {
        Self {
            found: Vec::new(),
            end: file_len,
            file_len,
        }
    }

    fn next<F: Read + Seek>(&mut self, file: &mut F) -> Result<Option<u64>> {
        let overlap = TRAILER_MAGIC.len() - 1;
        let floor = HEADER_LEN as u64;
        loop {
            if let Some(at) = self.found.pop() {
                return Ok(Some(at));
            }
            if self.end <= floor {
                return Ok(None);
            }
            let start = self.end.saturating_sub(CHUNK).max(floor);
            let read_end = (self.end + overlap as u64).min(self.file_len);
            let mut buf = vec![0u8; usize::try_from(read_end - start).unwrap_or(0)];
            read_exact_at(file, start, &mut buf, "recovery scan")?;
            for i in 0..buf.len().saturating_sub(overlap) {
                let at = start + i as u64;
                if at < self.end && buf[i..].starts_with(&TRAILER_MAGIC) {
                    self.found.push(at);
                }
            }
            self.end = start;
        }
    }
}
