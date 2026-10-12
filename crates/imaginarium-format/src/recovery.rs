// SPDX-License-Identifier: AGPL-3.0-or-later

//! Recovery of files whose last commit is unusable (spec §9.5).

use std::io::{Read, Seek, SeekFrom};

use crate::header::{FileHeader, HEADER_LEN};
use crate::index::IndexState;
use crate::payload::RecordOpener;
use crate::reader::{KeyProvider, load_from_trailer, read_exact_at};
use crate::record::{Limits, Record, RecordType};
use crate::trailer::{TRAILER_LEN, TRAILER_MAGIC, Trailer};
use crate::{FormatError, Result};

/// How a file was recovered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RecoveryMethod {
    /// An earlier commit was intact. Its trailer ends at `commit_end`.
    /// Complete records found after it (e.g. an autosave that crashed
    /// before committing) were adopted, and `discarded_bytes` at the end
    /// were unreadable.
    PreviousCommit {
        /// End offset of the commit used.
        commit_end: u64,
        /// Complete records after the commit that were added back.
        adopted_records: usize,
        /// Bytes at the end that couldn't be read as records.
        discarded_bytes: u64,
    },
    /// No intact commit: the index was rebuilt by scanning records
    /// forward from the header until the first invalid one.
    ForwardScan {
        /// Records found (index records and trailers are not counted).
        records: usize,
        /// Offset where scanning stopped.
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

pub(crate) fn recover<F: Read + Seek>(
    file: &mut F,
    header: &FileHeader,
    file_len: u64,
    limits: &Limits,
    mut keys: Option<&mut (dyn KeyProvider + '_)>,
    cause: &FormatError,
) -> Result<Recovered> {
    let cause = cause.to_string();

    // 1. Backwards: the newest earlier commit whose trailer and index verify,
    //    plus any complete records appended after it.
    for magic_at in find_magics_backwards(file, file_len)? {
        let trailer_end = magic_at + 8;
        if trailer_end == file_len {
            continue; // that's the trailer that already failed
        }
        if let Ok((mut state, opener, index_offset)) = load_from_trailer(
            file,
            header,
            file_len,
            trailer_end,
            limits,
            keys.as_deref_mut(),
        ) {
            let scan = scan_forward(file, header, trailer_end, file_len, limits, &mut state)?;
            return Ok(Recovered {
                state,
                index_offset,
                end: scan.stopped_at,
                opener,
                report: Recovery {
                    cause,
                    method: RecoveryMethod::PreviousCommit {
                        commit_end: trailer_end,
                        adopted_records: scan.records,
                        discarded_bytes: file_len - scan.stopped_at,
                    },
                },
            });
        }
    }

    // 2. Forwards from the header: rebuild the index from the records.
    let mut state = IndexState::default();
    let scan = scan_forward(
        file,
        header,
        HEADER_LEN as u64,
        file_len,
        limits,
        &mut state,
    )?;
    if scan.records == 0 {
        return Err(FormatError::NoValidIndex(format!(
            "{cause}; no readable records"
        )));
    }
    let opener = match (header.is_encrypted(), keys) {
        (true, Some(keys)) => Some(keys.opener(header, &scan.key_records)?),
        (true, None) => return Err(FormatError::NeedsKey),
        (false, _) => None,
    };
    Ok(Recovered {
        state,
        index_offset: scan.last_index,
        end: scan.stopped_at,
        opener,
        report: Recovery {
            cause,
            method: RecoveryMethod::ForwardScan {
                records: scan.records,
                stopped_at: scan.stopped_at,
                reason: scan.reason,
            },
        },
    })
}

struct Scan {
    records: usize,
    stopped_at: u64,
    reason: String,
    last_index: u64,
    key_records: Vec<Record>,
}

/// Adds every complete record from `start` up to the first unreadable one
/// to `state`, skipping index records and their trailers. A record that
/// breaks the encryption policy (spec §5.1) counts as unreadable.
fn scan_forward<F: Read + Seek>(
    file: &mut F,
    header: &FileHeader,
    start: u64,
    file_len: u64,
    limits: &Limits,
    state: &mut IndexState,
) -> Result<Scan> {
    let mut scan = Scan {
        records: 0,
        stopped_at: start,
        reason: String::new(),
        last_index: 0,
        key_records: Vec::new(),
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
            Err(err) => break err.to_string(),
        };
        let len = len as u64;
        if record.rtype() == RecordType::INDX {
            scan.last_index = pos;
            pos += len;
            // An index is followed by its trailer, which is not a record.
            if is_trailer_for(file, pos, file_len, scan.last_index)? {
                pos += TRAILER_LEN as u64;
            }
            continue;
        }
        if let Some(problem) = header.encryption_violation(&record) {
            break problem;
        }
        if matches!(record.rtype(), RecordType::KEYR | RecordType::ACLC) {
            scan.key_records.push(record.clone());
        }
        state.add(&record, pos, len);
        scan.records += 1;
        pos += len;
    };
    scan.stopped_at = pos;
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

/// Offsets of every trailer-magic occurrence, newest (highest) first.
fn find_magics_backwards<F: Read + Seek>(file: &mut F, file_len: u64) -> Result<Vec<u64>> {
    let mut found = Vec::new();
    let overlap = (TRAILER_MAGIC.len() - 1) as u64;
    let mut end = file_len;
    let floor = HEADER_LEN as u64;
    while end > floor {
        let start = end.saturating_sub(CHUNK).max(floor);
        let read_end = (end + overlap).min(file_len);
        let mut buf = vec![0u8; usize::try_from(read_end - start).unwrap_or(0)];
        read_exact_at(file, start, &mut buf, "recovery scan")?;
        for i in (0..buf.len().saturating_sub(TRAILER_MAGIC.len() - 1)).rev() {
            let at = start + i as u64;
            if at < end && buf[i..].starts_with(&TRAILER_MAGIC) {
                found.push(at);
            }
        }
        end = start;
    }
    Ok(found)
}
