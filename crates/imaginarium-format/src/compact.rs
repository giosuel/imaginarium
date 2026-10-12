// SPDX-License-Identifier: AGPL-3.0-or-later

//! Compaction: rewrite a file with only its live records (spec §9.4).

use std::collections::HashSet;
use std::io::{Read, Seek, Write};
use std::path::Path;

use crate::atomic::AtomicFile;
use crate::header::FileHeader;
use crate::index::StreamKey;
use crate::payload::RecordSealer;
use crate::pb;
use crate::reader::{KeyProvider, OpenOptions};
use crate::record::{Record, RecordFlags, RecordType};
use crate::writer::Durable;
use crate::{Reader, Result, Writer};

/// Decides whether a record is kept during compaction.
pub type KeepFn = Box<dyn Fn(&pb::IndexEntry) -> bool>;

/// What to change while compacting.
#[derive(Default)]
pub struct CompactionPlan {
    /// Records to add, written after the kept ones. Typically fresh
    /// snapshots from the document model.
    pub new_records: Vec<Record>,
    /// Streams whose existing snapshots and updates are dropped, because
    /// `new_records` contains a new snapshot for them.
    pub replaced_streams: Vec<StreamKey>,
    /// An extra veto on top of the default policy: return `false` to drop
    /// a record that would otherwise be kept (e.g. blobs that no longer
    /// referenced by the document).
    pub keep: Option<KeepFn>,
}

/// What compaction did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CompactionReport {
    /// Records copied byte-for-byte.
    pub kept: usize,
    /// New records written.
    pub added: usize,
    /// Superseded records dropped.
    pub dropped: usize,
    /// Record types that were dropped because they are unknown and not
    /// `SAFE_TO_COPY`. Spec §9.4 says the user should be warned.
    pub dropped_unknown: Vec<u32>,
}

/// Compacts `reader` into `out`, which must be empty, and commits it.
/// Kept records are copied byte-for-byte (spec §6.3.1).
///
/// The default policy keeps: the latest `META` and `THMB`; for each stream,
/// the head snapshot and its updates; every `KEYR`, `ACLC`, `BLOB`, `VERS`
/// and `DERV`; and unknown records marked `SAFE_TO_COPY`.
pub fn compact<F, G>(
    reader: &mut Reader<F>,
    out: G,
    plan: CompactionPlan,
    index_sealer: Option<&dyn RecordSealer>,
) -> Result<(Writer<G>, CompactionReport)>
where
    F: Read + Seek,
    G: Write + Seek + Durable,
{
    let header = FileHeader { ..*reader.header() };
    let mut writer = Writer::create(out, header)?;
    let mut report = CompactionReport::default();

    let entries = reader.index().entries().to_vec();
    let latest = |rtype: RecordType| {
        entries
            .iter()
            .rev()
            .find(|e| e.rtype == rtype.0)
            .map(|e| e.offset)
    };
    let latest_meta = latest(RecordType::META);
    let latest_thumbnail = latest(RecordType::THMB);
    let head_ids: HashSet<Vec<u8>> = reader
        .index()
        .heads()
        .iter()
        .filter(|head| {
            !plan
                .replaced_streams
                .iter()
                .any(|k| k.stream == head.stream && k.extension_id == head.extension_id)
        })
        .flat_map(|head| {
            std::iter::once(head.snapshot_id.clone()).chain(head.update_ids.iter().cloned())
        })
        .filter(|id| !id.is_empty())
        .collect();

    for entry in &entries {
        let rtype = RecordType(entry.rtype);
        let keep_by_policy = match rtype {
            RecordType::META => Some(entry.offset) == latest_meta,
            RecordType::THMB => Some(entry.offset) == latest_thumbnail,
            RecordType::SNAP | RecordType::UPDT => head_ids.contains(&entry.object_id),
            RecordType::KEYR
            | RecordType::ACLC
            | RecordType::BLOB
            | RecordType::VERS
            | RecordType::DERV => true,
            _ => {
                let flags = RecordFlags(u8::try_from(entry.rflags).unwrap_or(0));
                if !flags.contains(RecordFlags::SAFE_TO_COPY) {
                    report.dropped_unknown.push(entry.rtype);
                }
                flags.contains(RecordFlags::SAFE_TO_COPY)
            }
        };
        let vetoed = plan.keep.as_ref().is_some_and(|keep| !keep(entry));
        if keep_by_policy && !vetoed {
            let record = reader.read_record(entry)?;
            writer.append(&record)?;
            report.kept += 1;
        } else {
            report.dropped += 1;
        }
    }
    for record in &plan.new_records {
        writer.append(record)?;
        report.added += 1;
    }
    writer.commit(index_sealer)?;
    Ok((writer, report))
}

/// Compacts the file at `path` in place, atomically: writes a temporary
/// file in the same directory, syncs it, renames it over the original, and
/// syncs the directory (spec §9.4). A crash at any point leaves either the
/// old or the new file, never a mix.
pub fn compact_file(
    path: &Path,
    keys: Option<&mut (dyn KeyProvider + '_)>,
    plan: CompactionPlan,
    index_sealer: Option<&dyn RecordSealer>,
) -> Result<CompactionReport> {
    let source = std::fs::File::open(path)?;
    let mut reader = Reader::open_with(source, OpenOptions::default(), keys)?;
    let mut out = AtomicFile::create(path)?;
    let (writer, report) = compact(&mut reader, out.file(), plan, index_sealer)?;
    drop(writer); // already synced by commit
    drop(reader);
    out.commit()?;
    Ok(report)
}
