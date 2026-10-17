// SPDX-License-Identifier: AGPL-3.0-or-later

//! The in-memory index: record locations and per-stream heads (spec §9.1).

use crate::pb;
use crate::record::{Record, RecordType};

/// Which CRDT document a head belongs to: stream plus add-on id.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct StreamKey {
    /// `pb::Stream` as its integer value.
    pub stream: i32,
    /// Add-on id for extension streams, otherwise empty.
    pub extension_id: String,
}

impl StreamKey {
    /// The main document stream.
    pub fn main() -> Self {
        Self {
            stream: pb::Stream::Main as i32,
            extension_id: String::new(),
        }
    }

    pub(crate) fn of(header: &pb::RecordHeader) -> Self {
        Self {
            stream: header.stream,
            extension_id: header.extension_id.clone(),
        }
    }
}

/// Index of a file: every record with its location, plus the heads.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct IndexState {
    pub(crate) entries: Vec<pb::IndexEntry>,
    pub(crate) heads: Vec<pb::StreamHead>,
    pub(crate) previous_index_offset: u64,
}

impl IndexState {
    /// All indexed records, in file order.
    pub fn entries(&self) -> &[pb::IndexEntry] {
        &self.entries
    }

    /// Per-stream heads: the latest snapshot and the updates after it.
    pub fn heads(&self) -> &[pb::StreamHead] {
        &self.heads
    }

    /// The head of one stream, if it has any SNAP/UPDT.
    pub fn head(&self, key: &StreamKey) -> Option<&pb::StreamHead> {
        self.heads
            .iter()
            .find(|h| h.stream == key.stream && h.extension_id == key.extension_id)
    }

    /// Records a record appended at `offset` with encoded `length`. A
    /// snapshot supersedes its stream's head: the old head's records leave
    /// the index (it lists live records, spec §9.1), so an often-saved file's
    /// index stays as small as its head.
    pub(crate) fn add(&mut self, record: &Record, offset: u64, length: u64) {
        let header = record.header();
        if record.rtype() == RecordType::SNAP {
            let key = StreamKey::of(header);
            if let Some(head) = self
                .heads
                .iter()
                .find(|h| h.stream == key.stream && h.extension_id == key.extension_id)
            {
                // A set: a long head would make this quadratic.
                let superseded: std::collections::HashSet<Vec<u8>> =
                    std::iter::once(head.snapshot_id.clone())
                        .chain(head.update_ids.iter().cloned())
                        .filter(|id| !id.is_empty())
                        .collect();
                self.entries.retain(|entry| {
                    let in_stream = (entry.rtype == RecordType::SNAP.0
                        || entry.rtype == RecordType::UPDT.0)
                        && entry.stream == key.stream
                        && entry.extension_id == key.extension_id;
                    !(in_stream && superseded.contains(&entry.object_id))
                });
            }
        }
        self.entries.push(pb::IndexEntry {
            rtype: record.rtype().0,
            object_id: header.object_id.clone(),
            stream: header.stream,
            extension_id: header.extension_id.clone(),
            offset,
            length,
            rflags: u32::from(record.flags().0),
            epoch: header.epoch,
        });
        let rtype = record.rtype();
        if rtype == RecordType::SNAP || rtype == RecordType::UPDT {
            let key = StreamKey::of(header);
            let index = match self
                .heads
                .iter()
                .position(|h| h.stream == key.stream && h.extension_id == key.extension_id)
            {
                Some(index) => index,
                None => {
                    self.heads.push(pb::StreamHead {
                        stream: key.stream,
                        extension_id: key.extension_id,
                        snapshot_id: Vec::new(),
                        update_ids: Vec::new(),
                    });
                    self.heads.len() - 1
                }
            };
            let head = &mut self.heads[index];
            if rtype == RecordType::SNAP {
                head.snapshot_id = header.object_id.clone();
                head.update_ids.clear();
            } else {
                head.update_ids.push(header.object_id.clone());
            }
        }
    }

    /// KEYR and ACLC locations, for the INDX header of encrypted documents.
    pub(crate) fn key_records(&self) -> Vec<pb::RecordRef> {
        self.entries
            .iter()
            .filter(|e| e.rtype == RecordType::KEYR.0 || e.rtype == RecordType::ACLC.0)
            .map(|e| pb::RecordRef {
                offset: e.offset,
                length: e.length,
            })
            .collect()
    }

    pub(crate) fn to_pb(&self) -> pb::Index {
        pb::Index {
            entries: self.entries.clone(),
            heads: self.heads.clone(),
            previous_index_offset: self.previous_index_offset,
        }
    }

    pub(crate) fn from_pb(index: pb::Index) -> Self {
        Self {
            entries: index.entries,
            heads: index.heads,
            previous_index_offset: index.previous_index_offset,
        }
    }
}
