// SPDX-License-Identifier: AGPL-3.0-or-later

//! File-level tests: writing, reading, incremental saves, recovery,
//! compaction and the encryption plumbing (spec §9).

// Test helpers outside `#[test]` functions panic on failure by design.
// clippy's allow-unwrap-in-tests only covers the test functions themselves.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::io::Cursor;

use imaginarium_format::{
    CompactionPlan, DocId, FileHeader, FormatError, HEADER_LEN, KeyProvider, Limits, ObjectId,
    OpenOptions, Reader, Record, RecordBuilder, RecordFlags, RecordOpener, RecordSealer,
    RecordType, RecoveryMethod, Result, SEGMENT_LEN, StreamKey, Writer, compact, compact_file, pb,
};
use proptest::prelude::*;
use prost::Message;

const DOC: DocId = DocId([0x11; 16]);

fn id(n: u8) -> ObjectId {
    ObjectId([n; 32])
}

fn snap(n: u8, content: &[u8]) -> Record {
    RecordBuilder::new(RecordType::SNAP, id(n))
        .stream(pb::Stream::Main)
        .codec("loro-1")
        .build(&DOC, content.to_vec(), None)
        .unwrap()
}

fn update(n: u8, stream: pb::Stream) -> Record {
    RecordBuilder::new(RecordType::UPDT, id(n))
        .stream(stream)
        .codec("loro-1")
        .compress(true)
        .build(&DOC, format!("update {n} ").repeat(20).into_bytes(), None)
        .unwrap()
}

fn meta(n: u8) -> Record {
    RecordBuilder::new(RecordType::META, id(n))
        .build(&DOC, vec![n; 8], None)
        .unwrap()
}

/// A complete small document: META, SNAP, two main updates, one discussion update.
fn sample_file() -> Vec<u8> {
    let mut writer = Writer::create(Cursor::new(Vec::new()), FileHeader::new(DOC, false)).unwrap();
    for record in [
        meta(1),
        snap(2, b"state"),
        update(3, pb::Stream::Main),
        update(4, pb::Stream::Main),
        update(5, pb::Stream::Discussion),
    ] {
        writer.append(&record).unwrap();
    }
    writer.commit(None).unwrap();
    writer.into_inner().into_inner()
}

fn open(bytes: Vec<u8>) -> Reader<Cursor<Vec<u8>>> {
    Reader::open(Cursor::new(bytes)).unwrap()
}

#[test]
fn write_then_read_everything() {
    let mut reader = open(sample_file());
    assert!(reader.recovery().is_none());
    assert_eq!(reader.header().doc_id, DOC);
    assert_eq!(reader.index().entries().len(), 5);

    let main = reader.index().head(&StreamKey::main()).unwrap().clone();
    assert_eq!(main.snapshot_id, id(2).0.to_vec());
    assert_eq!(main.update_ids, vec![id(3).0.to_vec(), id(4).0.to_vec()]);

    let entries = reader.index().entries().to_vec();
    assert_eq!(reader.read_content(&entries[1]).unwrap(), b"state");
    assert_eq!(
        reader.read_content(&entries[2]).unwrap(),
        "update 3 ".repeat(20).into_bytes()
    );
}

#[test]
fn incremental_saves_append_and_chain_indexes() {
    let first = sample_file();
    let first_len = first.len();
    let reader = open(first.clone());
    let mut writer = Writer::resume(reader).unwrap();
    writer.append(&snap(6, b"newer state")).unwrap();
    writer.append(&update(7, pb::Stream::Main)).unwrap();
    writer.commit(None).unwrap();
    let second = writer.into_inner().into_inner();

    assert_eq!(
        &second[..first_len],
        &first[..],
        "append-only: the old bytes are untouched"
    );
    let reader = open(second);
    assert_eq!(reader.index().entries().len(), 7);
    let head = reader.index().head(&StreamKey::main()).unwrap();
    assert_eq!(
        head.snapshot_id,
        id(6).0.to_vec(),
        "a new snapshot starts a new head"
    );
    assert_eq!(head.update_ids, vec![id(7).0.to_vec()]);
}

#[test]
fn torn_autosave_recovers_the_previous_commit_and_adopts_complete_records() {
    let reader = open(sample_file());
    let mut writer = Writer::resume(reader).unwrap();
    writer.append(&update(8, pb::Stream::Main)).unwrap(); // complete, but never committed
    let mut bytes = writer.into_inner().into_inner();
    let partial = update(9, pb::Stream::Main).encode();
    bytes.extend_from_slice(&partial[..partial.len() / 2]); // torn write

    let reader = open(bytes);
    let recovery = reader.recovery().expect("recovered");
    match &recovery.method {
        RecoveryMethod::PreviousCommit {
            adopted_records,
            discarded_bytes,
            ..
        } => {
            assert_eq!(
                *adopted_records, 1,
                "the complete uncommitted update is kept"
            );
            assert_eq!(*discarded_bytes, (partial.len() / 2) as u64);
        }
        other => panic!("unexpected {other:?}"),
    }
    let head = reader.index().head(&StreamKey::main()).unwrap();
    assert_eq!(head.update_ids.last(), Some(&id(8).0.to_vec()));
}

#[test]
fn corrupted_only_trailer_uses_its_index() {
    // The index before the broken trailer is intact, so it's used as is.
    let mut bytes = sample_file();
    let last = bytes.len() - 1;
    bytes[last - 10] ^= 0xFF; // inside the trailer
    let reader = open(bytes);
    match &reader.recovery().unwrap().method {
        RecoveryMethod::PreviousCommit {
            commit_end,
            adopted_records,
            discarded_bytes,
        } => {
            assert_eq!(*commit_end, (sample_file().len() - 24) as u64);
            assert_eq!(*adopted_records, 0);
            assert_eq!(*discarded_bytes, 24, "only the broken trailer");
        }
        other => panic!("unexpected {other:?}"),
    }
    assert_eq!(
        reader
            .index()
            .head(&StreamKey::main())
            .unwrap()
            .update_ids
            .len(),
        2
    );
}

#[test]
fn without_any_usable_index_the_records_are_rebuilt() {
    let mut bytes = sample_file();
    let index_offset = imaginarium_format::Trailer::decode(&bytes[bytes.len() - 24..])
        .unwrap()
        .index_offset;
    bytes[index_offset as usize + 3] ^= 0xFF; // the only index
    let reader = open(bytes);
    match &reader.recovery().unwrap().method {
        RecoveryMethod::ForwardScan {
            records,
            stopped_at,
            ..
        } => {
            assert_eq!(*records, 5);
            assert_eq!(*stopped_at, index_offset);
        }
        other => panic!("unexpected {other:?}"),
    }
    assert_eq!(
        reader
            .index()
            .head(&StreamKey::main())
            .unwrap()
            .update_ids
            .len(),
        2
    );
}

#[test]
fn corrupted_latest_index_uses_the_previous_commit() {
    let reader = open(sample_file());
    let mut writer = Writer::resume(reader).unwrap();
    writer.append(&update(8, pb::Stream::Main)).unwrap();
    writer.commit(None).unwrap();
    let mut bytes = writer.into_inner().into_inner();
    // Corrupt the last index record (just before the final trailer).
    let at = bytes.len() - 24 - 5;
    bytes[at] ^= 0xFF;
    let reader = open(bytes);
    assert!(matches!(
        reader.recovery().unwrap().method,
        RecoveryMethod::PreviousCommit { .. }
    ));
    assert_eq!(
        reader.index().entries().len(),
        6,
        "5 committed + the adopted update"
    );
}

#[test]
fn resuming_after_recovery_produces_a_clean_file() {
    let mut bytes = sample_file();
    bytes.extend_from_slice(b"garbage that is not a record at all");
    let reader = open(bytes);
    assert!(reader.recovery().is_some());
    let mut writer = Writer::resume(reader).unwrap();
    writer.append(&update(9, pb::Stream::Main)).unwrap();
    writer.commit(None).unwrap();
    let reader = open(writer.into_inner().into_inner());
    assert!(
        reader.recovery().is_none(),
        "the junk was truncated by the commit"
    );
    assert_eq!(reader.index().entries().len(), 6);
}

#[test]
fn files_without_records_or_header_fail_cleanly() {
    assert!(matches!(
        Reader::open(Cursor::new(vec![0u8; 10])),
        Err(FormatError::Truncated { .. })
    ));
    let header_only = FileHeader::new(DOC, false).encode().to_vec();
    assert!(matches!(
        Reader::open(Cursor::new(header_only.clone())),
        Err(FormatError::NoValidIndex(_))
    ));
    let strict = OpenOptions {
        recover: false,
        ..OpenOptions::default()
    };
    assert!(Reader::open_with(Cursor::new(header_only), strict, None).is_err());
}

#[test]
fn segmented_blobs_are_reassembled_and_validated() {
    let data: Vec<u8> = (0..SEGMENT_LEN * 2 + 1234)
        .map(|i| (i % 251) as u8)
        .collect();
    let chunks: Vec<&[u8]> = data.chunks(SEGMENT_LEN).collect();
    let count = chunks.len() as u32;
    let segment = |i: usize| {
        RecordBuilder::new(RecordType::BLOB, id(42))
            .segment(i as u32, count, data.len() as u64, Vec::new())
            .build(&DOC, chunks[i].to_vec(), None)
            .unwrap()
    };
    let mut writer = Writer::create(Cursor::new(Vec::new()), FileHeader::new(DOC, false)).unwrap();
    // Out of order on purpose.
    for i in [2, 0, 1] {
        writer.append(&segment(i)).unwrap();
    }
    writer.commit(None).unwrap();
    let mut reader = open(writer.into_inner().into_inner());
    assert_eq!(reader.read_blob(&id(42)).unwrap(), data);

    // A missing segment is an error, not silently short data.
    let mut writer = Writer::create(Cursor::new(Vec::new()), FileHeader::new(DOC, false)).unwrap();
    writer.append(&segment(0)).unwrap();
    writer.append(&segment(2)).unwrap();
    writer.commit(None).unwrap();
    let mut reader = open(writer.into_inner().into_inner());
    assert!(reader.read_blob(&id(42)).is_err());
}

/// One `BLOB` segment with the given claims.
fn blob_segment(index: u32, count: u32, total_len: u64, content: Vec<u8>) -> Record {
    RecordBuilder::new(RecordType::BLOB, id(42))
        .segment(index, count, total_len, Vec::new())
        .compress(true)
        .build(&DOC, content, None)
        .unwrap()
}

fn file_with(records: &[Record]) -> Vec<u8> {
    let mut writer = Writer::create(Cursor::new(Vec::new()), FileHeader::new(DOC, false)).unwrap();
    for record in records {
        writer.append(record).unwrap();
    }
    writer.commit(None).unwrap();
    writer.into_inner().into_inner()
}

#[test]
fn blob_lengths_are_checked_before_anything_is_allocated() {
    // A segment header is untrusted input: a claimed length of 2^62 must
    // be an error, not a capacity-overflow panic or an abort.
    let mut reader = open(file_with(&[blob_segment(0, 1, 1 << 62, vec![1; 10])]));
    assert!(reader.read_blob(&id(42)).is_err());

    // Every segment but the last holds exactly SEGMENT_LEN bytes (spec
    // §4.6), so the size of the whole blob is known before decompressing.
    let short_first = [
        blob_segment(0, 2, 1000 + SEGMENT_LEN as u64, vec![0; 1000]),
        blob_segment(1, 2, 1000 + SEGMENT_LEN as u64, vec![0; SEGMENT_LEN]),
    ];
    assert!(open(file_with(&short_first)).read_blob(&id(42)).is_err());

    // Highly compressible segments add up fast; the reader's limit caps
    // the blob as a whole, not just each record.
    let segments: Vec<Record> = (0..5)
        .map(|i| blob_segment(i, 5, 5 * SEGMENT_LEN as u64, vec![0; SEGMENT_LEN]))
        .collect();
    let options = OpenOptions {
        limits: Limits {
            max_payload: 4 * SEGMENT_LEN as u64,
            ..Limits::default()
        },
        ..OpenOptions::default()
    };
    let mut reader = Reader::open_with(Cursor::new(file_with(&segments)), options, None).unwrap();
    assert!(reader.read_blob(&id(42)).is_err());
}

#[test]
fn duplicate_blob_segments_are_harmless() {
    // An autosave that crashed after writing segments, then a save that
    // wrote the blob again: the index lists some segments twice.
    let data: Vec<u8> = (0..SEGMENT_LEN * 2 + 77).map(|i| (i % 13) as u8).collect();
    let chunks: Vec<&[u8]> = data.chunks(SEGMENT_LEN).collect();
    let segment = |i: usize| blob_segment(i as u32, 3, data.len() as u64, chunks[i].to_vec());
    let bytes = file_with(&[segment(0), segment(1), segment(1), segment(2), segment(0)]);
    assert_eq!(open(bytes).read_blob(&id(42)).unwrap(), data);
}

#[test]
fn compaction_keeps_live_records_byte_for_byte() {
    // Two generations of META and snapshots, an unknown SAFE_TO_COPY
    // record, and an unknown non-safe one.
    let mut writer = Writer::create(Cursor::new(Vec::new()), FileHeader::new(DOC, false)).unwrap();
    let safe = RecordBuilder::new(RecordType(0x4001), id(50))
        .extension_id("org.example.safe")
        .flags(RecordFlags::SAFE_TO_COPY)
        .build(&DOC, b"keep me".to_vec(), None)
        .unwrap();
    let unsafe_unknown = RecordBuilder::new(RecordType(0x4002), id(51))
        .extension_id("org.example.fragile")
        .build(&DOC, b"drop me".to_vec(), None)
        .unwrap();
    for record in [
        meta(1),
        snap(2, b"old"),
        update(3, pb::Stream::Main),
        safe.clone(),
        unsafe_unknown,
        meta(4),
        snap(5, b"new"),
        update(6, pb::Stream::Main),
    ] {
        writer.append(&record).unwrap();
    }
    writer.commit(None).unwrap();
    let mut reader = open(writer.into_inner().into_inner());

    let (compacted, report) = compact(
        &mut reader,
        Cursor::new(Vec::new()),
        CompactionPlan::default(),
        None,
    )
    .unwrap();
    assert_eq!(report.kept, 4, "{report:?}"); // META 4, SNAP 5, UPDT 6, safe extension
    assert_eq!(report.dropped, 4);
    assert_eq!(report.dropped_unknown, vec![0x4002]);

    let mut reader = open(compacted.into_inner().into_inner());
    let entries = reader.index().entries().to_vec();
    let copied = entries.iter().find(|e| e.rtype == 0x4001).unwrap();
    assert_eq!(
        reader.read_record(copied).unwrap().encode(),
        safe.encode(),
        "byte-for-byte"
    );
    assert_eq!(
        reader.index().head(&StreamKey::main()).unwrap().snapshot_id,
        id(5).0.to_vec()
    );
}

#[test]
fn compaction_can_replace_a_stream_and_veto_records() {
    let mut reader = open(sample_file());
    let plan = CompactionPlan {
        new_records: vec![snap(30, b"merged state")],
        replaced_streams: vec![StreamKey::main()],
        keep: Some(Box::new(|entry: &pb::IndexEntry| {
            entry.rtype != RecordType::META.0
        })),
    };
    let (compacted, report) = compact(&mut reader, Cursor::new(Vec::new()), plan, None).unwrap();
    assert_eq!(report.added, 1);
    let reader = open(compacted.into_inner().into_inner());
    let main = reader.index().head(&StreamKey::main()).unwrap();
    assert_eq!(
        (main.snapshot_id.clone(), main.update_ids.len()),
        (id(30).0.to_vec(), 0)
    );
    assert!(
        reader
            .index()
            .entries()
            .iter()
            .all(|e| e.rtype != RecordType::META.0),
        "vetoed"
    );
    let discussion = StreamKey {
        stream: pb::Stream::Discussion as i32,
        extension_id: String::new(),
    };
    assert_eq!(
        reader.index().head(&discussion).unwrap().update_ids.len(),
        1,
        "other streams untouched"
    );
}

#[test]
fn compact_file_replaces_the_file_atomically_on_disk() {
    let dir =
        std::env::temp_dir().join(format!("imaginarium-format-compact-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("doc.imag");
    std::fs::write(&path, sample_file()).unwrap();
    let report = compact_file(&path, None, CompactionPlan::default(), None).unwrap();
    assert_eq!(report.kept, 5);
    assert!(!dir.join("doc.imag.compact.tmp").exists());
    let reader = Reader::open(std::fs::File::open(&path).unwrap()).unwrap();
    assert!(reader.recovery().is_none());
    let _ = std::fs::remove_dir_all(&dir);
}

// --- Encryption plumbing (real crypto comes from imaginarium-crypto) -------

/// Toy cipher: XOR plus a CRC over AAD and plaintext as a "tag". Like an
/// AEAD, it detects changes to the AAD *and* the ciphertext. Tests the
/// plumbing only, not cryptography.
struct ToyCipher(u8);

fn tag(aad: &[u8], plaintext: &[u8]) -> [u8; 4] {
    crc32c::crc32c_append(crc32c::crc32c(aad), plaintext).to_le_bytes()
}

impl RecordSealer for ToyCipher {
    fn suite(&self) -> u32 {
        77
    }
    fn epoch(&self) -> u32 {
        1
    }
    fn seal(
        &self,
        _: RecordType,
        _: &pb::RecordHeader,
        aad: &[u8],
        plaintext: &[u8],
    ) -> Result<Vec<u8>> {
        let mut out = tag(aad, plaintext).to_vec();
        out.extend(plaintext.iter().map(|b| b ^ self.0));
        Ok(out)
    }
}

impl RecordOpener for ToyCipher {
    fn open(
        &self,
        _: RecordType,
        _: &pb::RecordHeader,
        aad: &[u8],
        payload: &[u8],
    ) -> Result<Vec<u8>> {
        let (t, body) = payload.split_at(4);
        let plaintext: Vec<u8> = body.iter().map(|b| b ^ self.0).collect();
        if t != tag(aad, &plaintext) {
            return Err(FormatError::Crypto("authentication failed".into()));
        }
        Ok(plaintext)
    }
}

/// Unlocks the document if the keyring record contains the expected byte.
struct ToyKeys {
    seen_key_records: usize,
}

impl KeyProvider for ToyKeys {
    fn opener(&mut self, _: &FileHeader, key_records: &[Record]) -> Result<Box<dyn RecordOpener>> {
        self.seen_key_records = key_records.len();
        let keyring = key_records
            .iter()
            .find(|r| r.rtype() == RecordType::KEYR)
            .ok_or(FormatError::NeedsKey)?;
        Ok(Box::new(ToyCipher(keyring.payload()[0])))
    }
}

fn encrypted_file() -> Vec<u8> {
    let cipher = ToyCipher(0x42);
    let mut writer = Writer::create(Cursor::new(Vec::new()), FileHeader::new(DOC, true)).unwrap();
    let keyring = RecordBuilder::new(RecordType::KEYR, id(1))
        .build(&DOC, vec![0x42], None)
        .unwrap();
    writer.append(&keyring).unwrap();
    let secret = RecordBuilder::new(RecordType::SNAP, id(2))
        .stream(pb::Stream::Main)
        .codec("loro-1")
        .build(&DOC, b"top secret".to_vec(), Some(&cipher))
        .unwrap();
    writer.append(&secret).unwrap();
    // Unencrypted content in an encrypted document is rejected.
    assert!(writer.append(&snap(3, b"plain")).is_err());
    assert!(writer.commit(None).is_err(), "the index must be encrypted");
    writer.commit(Some(&cipher)).unwrap();
    writer.into_inner().into_inner()
}

#[test]
fn encrypted_documents_need_keys_found_via_key_records() {
    let bytes = encrypted_file();
    assert!(
        !bytes.windows(10).any(|w| w == b"top secret"),
        "no plaintext in the file"
    );
    assert!(matches!(
        Reader::open(Cursor::new(bytes.clone())),
        Err(FormatError::NeedsKey)
    ));

    let mut keys = ToyKeys {
        seen_key_records: 0,
    };
    let mut reader =
        Reader::open_with(Cursor::new(bytes), OpenOptions::default(), Some(&mut keys)).unwrap();
    assert_eq!(
        keys.seen_key_records, 1,
        "the KEYR came from INDX.key_records"
    );
    let entries = reader.index().entries().to_vec();
    assert_eq!(reader.read_content(&entries[1]).unwrap(), b"top secret");
}

#[test]
fn tampering_with_an_encrypted_index_is_detected() {
    let mut bytes = encrypted_file();
    // Flip a payload byte of the index and fix up its CRC. A "smart"
    // attacker passes the CRC, but not the AEAD.
    let trailer = imaginarium_format::Trailer::decode(&bytes[bytes.len() - 24..]).unwrap();
    let start = trailer.index_offset as usize;
    let end = start + trailer.index_len as usize;
    bytes[end - 6] ^= 0x01;
    let crc = crc32c::crc32c(&bytes[start..end - 4]);
    bytes[end - 4..end].copy_from_slice(&crc.to_le_bytes());
    let mut keys = ToyKeys {
        seen_key_records: 0,
    };
    let strict = OpenOptions {
        recover: false,
        ..OpenOptions::default()
    };
    assert!(Reader::open_with(Cursor::new(bytes.clone()), strict, Some(&mut keys)).is_err());
    // Recovery doesn't paper over it either: a wrong key or tampering is
    // reported, not "repaired" by scanning around the index (spec §9.5).
    let err = Reader::open_with(Cursor::new(bytes), OpenOptions::default(), Some(&mut keys))
        .err()
        .unwrap();
    assert!(err.is_key_problem(), "{err}");
}

/// Opens with the toy key no matter which key records the index lists.
struct FixedKeys;

impl KeyProvider for FixedKeys {
    fn opener(&mut self, _: &FileHeader, _: &[Record]) -> Result<Box<dyn RecordOpener>> {
        Ok(Box::new(ToyCipher(0x42)))
    }
}

/// The location of the `KEYR` that [`encrypted_file`] writes first.
fn keyring_ref(bytes: &[u8]) -> pb::RecordRef {
    let (_, len) = Record::decode(&bytes[HEADER_LEN..], &Limits::default()).unwrap();
    pb::RecordRef {
        offset: HEADER_LEN as u64,
        length: len as u64,
    }
}

/// Appends `record`, then an index with `entries` (plus one for `record`)
/// that makes it the main snapshot, then a trailer. Without `sealer` this
/// needs no key: anyone holding the file can do it.
fn append_forged_commit(
    bytes: &mut Vec<u8>,
    record: &Record,
    mut entries: Vec<pb::IndexEntry>,
    key_records: Vec<pb::RecordRef>,
    sealer: Option<&ToyCipher>,
) {
    let at = bytes.len() as u64;
    bytes.extend(record.encode());
    entries.push(pb::IndexEntry {
        rtype: record.rtype().0,
        object_id: record.header().object_id.clone(),
        stream: pb::Stream::Main as i32,
        offset: at,
        length: record.encoded_len() as u64,
        rflags: u32::from(record.flags().0),
        ..Default::default()
    });
    let index = pb::Index {
        entries,
        heads: vec![pb::StreamHead {
            stream: pb::Stream::Main as i32,
            snapshot_id: record.header().object_id.clone(),
            ..Default::default()
        }],
        previous_index_offset: 0,
    };
    let index = RecordBuilder::new(RecordType::INDX, id(99))
        .key_records(key_records)
        .build(
            &DOC,
            index.encode_to_vec(),
            sealer.map(|s| s as &dyn RecordSealer),
        )
        .unwrap();
    let index_offset = bytes.len() as u64;
    let encoded = index.encode();
    bytes.extend(&encoded);
    bytes.extend(
        imaginarium_format::Trailer {
            index_offset,
            index_len: encoded.len() as u32,
        }
        .encode(),
    );
}

fn keyring_entry(location: &pb::RecordRef) -> pb::IndexEntry {
    pb::IndexEntry {
        rtype: RecordType::KEYR.0,
        object_id: id(1).0.to_vec(),
        offset: location.offset,
        length: location.length,
        rflags: u32::from(RecordType::KEYR.required_flags().0),
        ..Default::default()
    }
}

#[test]
fn plaintext_records_appended_to_an_encrypted_document_are_not_adopted() {
    // Spec §5.1: in an encrypted document every record except KEYR/ACLC is
    // encrypted. Recovery must not adopt a plaintext SNAP that someone
    // without the key appended, or it replaces the document's content.
    let mut bytes = encrypted_file();
    let committed = bytes.len();
    bytes.extend(snap(9, b"forged").encode());
    let mut reader = Reader::open_with(
        Cursor::new(bytes),
        OpenOptions::default(),
        Some(&mut FixedKeys),
    )
    .unwrap();
    match &reader.recovery().expect("the trailer is gone").method {
        RecoveryMethod::PreviousCommit {
            commit_end,
            adopted_records,
            ..
        } => {
            assert_eq!(*commit_end, committed as u64);
            assert_eq!(*adopted_records, 0, "the plaintext SNAP was adopted");
        }
        other => panic!("{other:?}"),
    }
    let head = reader.index().head(&StreamKey::main()).unwrap().clone();
    assert_eq!(head.snapshot_id, id(2).0.to_vec());
    let entry = reader.index().entries()[1].clone();
    assert_eq!(reader.read_content(&entry).unwrap(), b"top secret");
}

#[test]
fn a_plaintext_index_in_an_encrypted_document_is_reported() {
    // A key-less attacker appends a plaintext index that keeps the real
    // KEYR (so the key provider still unlocks) but points the main stream
    // at a plaintext SNAP of their own. That's tampering, not a torn
    // write: report it, don't open the forgery or "recover" around it.
    let mut bytes = encrypted_file();
    let keyring = keyring_ref(&bytes);
    append_forged_commit(
        &mut bytes,
        &snap(9, b"forged"),
        vec![keyring_entry(&keyring)],
        vec![keyring],
        None,
    );
    let err = Reader::open_with(
        Cursor::new(bytes),
        OpenOptions::default(),
        Some(&mut FixedKeys),
    )
    .err()
    .expect("the forged index was accepted");
    assert!(err.is_key_problem(), "{err}");
}

#[test]
fn an_encrypted_index_must_list_every_key_record() {
    // Spec §9.1: the INDX header's key_records must match the KEYR/ACLC
    // entries, also when it lists none (a key provider that doesn't need
    // the list would otherwise never see the keyrings at all).
    let cipher = ToyCipher(0x42);
    let secret = RecordBuilder::new(RecordType::SNAP, id(9))
        .stream(pb::Stream::Main)
        .codec("loro-1")
        .build(&DOC, b"newer".to_vec(), Some(&cipher))
        .unwrap();
    let mut bytes = encrypted_file();
    let keyring = keyring_ref(&bytes);
    append_forged_commit(
        &mut bytes,
        &secret,
        vec![keyring_entry(&keyring)],
        Vec::new(),
        Some(&cipher),
    );
    let strict = OpenOptions {
        recover: false,
        ..OpenOptions::default()
    };
    assert!(Reader::open_with(Cursor::new(bytes), strict, Some(&mut FixedKeys)).is_err());
}

/// [`encrypted_file`] plus a second commit whose SNAP and index are sealed
/// with `key`: a newer epoch, if `key` isn't 0x42.
fn encrypted_file_with_second_commit(key: u8) -> Vec<u8> {
    let reader = Reader::open_with(
        Cursor::new(encrypted_file()),
        OpenOptions::default(),
        Some(&mut FixedKeys),
    )
    .unwrap();
    let cipher = ToyCipher(key);
    let mut writer = Writer::resume(reader).unwrap();
    let newer = RecordBuilder::new(RecordType::SNAP, id(5))
        .stream(pb::Stream::Main)
        .codec("loro-1")
        .build(&DOC, b"newer secret".to_vec(), Some(&cipher))
        .unwrap();
    writer.append(&newer).unwrap();
    writer.commit(Some(&cipher)).unwrap();
    writer.into_inner().into_inner()
}

#[test]
fn recovery_reports_a_newer_index_it_cannot_decrypt() {
    // The newest commit is under an epoch this reader has no key for (a
    // removed member, say). Damaging its trailer must not let the reader
    // fall back to the older commit and read around it (spec §9.5).
    let mut bytes = encrypted_file_with_second_commit(0x43);
    let last = bytes.len() - 1;
    bytes[last - 12] ^= 0x01; // the trailer CRC
    let err = Reader::open_with(
        Cursor::new(bytes),
        OpenOptions::default(),
        Some(&mut FixedKeys),
    )
    .err()
    .expect("recovery read around the newer index");
    assert!(err.is_key_problem(), "{err}");
}

#[test]
fn recovery_does_not_fall_back_past_an_index_it_cannot_decrypt() {
    // Same, with the newest trailer intact and a torn write after it.
    let mut bytes = encrypted_file_with_second_commit(0x43);
    bytes.extend_from_slice(b"torn");
    let err = Reader::open_with(
        Cursor::new(bytes),
        OpenOptions::default(),
        Some(&mut FixedKeys),
    )
    .err()
    .expect("recovery fell back to the older commit");
    assert!(err.is_key_problem(), "{err}");
}

#[test]
fn recovery_uses_a_newer_index_whose_trailer_was_lost() {
    let reader = open(sample_file());
    let mut writer = Writer::resume(reader).unwrap();
    writer.append(&update(8, pb::Stream::Main)).unwrap();
    writer.commit(None).unwrap();
    let mut bytes = writer.into_inner().into_inner();
    let len = bytes.len();
    bytes[len - 12] ^= 0x01; // the trailer CRC
    let reader = open(bytes);
    match &reader.recovery().unwrap().method {
        RecoveryMethod::PreviousCommit {
            commit_end,
            adopted_records,
            discarded_bytes,
        } => {
            assert_eq!(*commit_end, (len - 24) as u64, "the newest index");
            assert_eq!(*adopted_records, 0);
            assert_eq!(*discarded_bytes, 24);
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(reader.index().entries().len(), 6);
}

#[test]
fn recovery_adopts_only_records_that_authenticate() {
    // After the last commit: a genuine uncommitted update, then a record
    // with a valid CRC that the document's key doesn't open (forged, or
    // from an epoch the commit doesn't know). Adopting the second would
    // make the document fail to open.
    let mut bytes = encrypted_file();
    let committed = bytes.len() as u64;
    for key in [0x42, 0x43] {
        let record = RecordBuilder::new(RecordType::UPDT, id(key))
            .stream(pb::Stream::Main)
            .codec("loro-1")
            .build(&DOC, b"edit".to_vec(), Some(&ToyCipher(key)))
            .unwrap();
        bytes.extend(record.encode());
    }
    let mut reader = Reader::open_with(
        Cursor::new(bytes),
        OpenOptions::default(),
        Some(&mut FixedKeys),
    )
    .unwrap();
    match &reader.recovery().unwrap().method {
        RecoveryMethod::PreviousCommit {
            commit_end,
            adopted_records,
            ..
        } => {
            assert_eq!(*commit_end, committed);
            assert_eq!(*adopted_records, 1);
        }
        other => panic!("{other:?}"),
    }
    let head = reader.index().head(&StreamKey::main()).unwrap().clone();
    assert_eq!(head.update_ids, vec![id(0x42).0.to_vec()]);
    for entry in reader.index().entries().to_vec() {
        reader.read_content(&entry).unwrap();
    }
}

#[test]
fn recovery_does_not_adopt_uncommitted_key_records() {
    // A KEYR after the last commit could carry keys nobody authorized.
    // Only committed key records count. Content after it is still adopted
    // if it authenticates.
    let mut bytes = encrypted_file();
    let keyring = RecordBuilder::new(RecordType::KEYR, id(7))
        .build(&DOC, vec![0x43], None)
        .unwrap();
    bytes.extend(keyring.encode());
    let update = RecordBuilder::new(RecordType::UPDT, id(8))
        .stream(pb::Stream::Main)
        .codec("loro-1")
        .build(&DOC, b"edit".to_vec(), Some(&ToyCipher(0x42)))
        .unwrap();
    bytes.extend(update.encode());
    let reader = Reader::open_with(
        Cursor::new(bytes),
        OpenOptions::default(),
        Some(&mut FixedKeys),
    )
    .unwrap();
    match &reader.recovery().unwrap().method {
        RecoveryMethod::PreviousCommit {
            adopted_records, ..
        } => assert_eq!(*adopted_records, 1),
        other => panic!("{other:?}"),
    }
    let keyrings = reader
        .index()
        .entries()
        .iter()
        .filter(|e| e.rtype == RecordType::KEYR.0)
        .count();
    assert_eq!(keyrings, 1);
}

// --- Crash safety, property-based ------------------------------------------

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// Truncating a multi-commit file at any byte never panics. Whenever
    /// the cut is after the first commit, at least that commit's records
    /// are recovered.
    #[test]
    fn truncation_at_any_point_is_safe(cut_fraction in 0.0f64..1.0) {
        let reader = open(sample_file());
        let first_commit_end = sample_file().len();
        let mut writer = Writer::resume(reader).unwrap();
        writer.append(&update(10, pb::Stream::Main)).unwrap();
        writer.append(&update(11, pb::Stream::Main)).unwrap();
        writer.commit(None).unwrap();
        let bytes = writer.into_inner().into_inner();
        let cut = (bytes.len() as f64 * cut_fraction) as usize;
        let truncated = bytes[..cut].to_vec();
        match Reader::open(Cursor::new(truncated)) {
            Ok(reader) => {
                if cut >= first_commit_end {
                    prop_assert!(reader.index().entries().len() >= 5);
                }
            }
            Err(_) => prop_assert!(cut < first_commit_end, "a committed file must stay readable (cut {cut})"),
        }
    }

    /// Random bytes after a valid file never panic, and the committed
    /// records survive.
    #[test]
    fn appended_garbage_never_loses_committed_data(garbage in proptest::collection::vec(any::<u8>(), 0..300)) {
        let mut bytes = sample_file();
        bytes.extend(garbage);
        let reader = Reader::open(Cursor::new(bytes)).unwrap();
        prop_assert!(reader.index().entries().len() >= 5);
    }

    /// Fully random input never panics.
    #[test]
    fn random_files_never_panic(bytes in proptest::collection::vec(any::<u8>(), 0..400)) {
        let _ = Reader::open(Cursor::new(bytes));
    }
}

/// Storage that logs every write, sync and truncation.
#[derive(Default)]
struct Logged {
    inner: Cursor<Vec<u8>>,
    log: Vec<Op>,
}

#[derive(Debug, PartialEq)]
enum Op {
    Write(usize),
    Sync,
    SetLen,
}

impl std::io::Write for Logged {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.log.push(Op::Write(buf.len()));
        self.inner.write(buf)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl std::io::Seek for Logged {
    fn seek(&mut self, pos: std::io::SeekFrom) -> std::io::Result<u64> {
        self.inner.seek(pos)
    }
}

impl imaginarium_format::Durable for Logged {
    fn sync(&mut self) -> std::io::Result<()> {
        self.log.push(Op::Sync);
        Ok(())
    }
    fn set_len(&mut self, len: u64) -> std::io::Result<()> {
        self.log.push(Op::SetLen);
        imaginarium_format::Durable::set_len(&mut self.inner, len)
    }
}

#[test]
fn a_commit_makes_its_records_durable_before_its_trailer() {
    // If the trailer could reach the disk before the records it commits,
    // a crash would leave a valid trailer over missing data (spec §9.3).
    let mut writer = Writer::create(Logged::default(), FileHeader::new(DOC, false)).unwrap();
    writer.append(&snap(1, b"state")).unwrap();
    writer.commit(None).unwrap();
    let log = writer.into_inner().log;
    let trailer = log
        .iter()
        .rposition(|op| *op == Op::Write(imaginarium_format::TRAILER_LEN))
        .expect("the trailer was written");
    assert!(
        log[..trailer].contains(&Op::Sync),
        "no sync before the trailer: {log:?}"
    );
    assert_eq!(log.last(), Some(&Op::Sync), "the trailer itself is synced");
}
