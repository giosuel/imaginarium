// SPDX-License-Identifier: AGPL-3.0-or-later

//! File-level tests: writing, reading, incremental saves, recovery,
//! compaction and the encryption plumbing (spec §9).

// Test helpers outside `#[test]` functions panic on failure by design.
// clippy's allow-unwrap-in-tests only covers the test functions themselves.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::io::Cursor;

use imaginarium_format::{
    CompactionPlan, DocId, FileHeader, FormatError, KeyProvider, ObjectId, OpenOptions, Reader,
    Record, RecordBuilder, RecordFlags, RecordOpener, RecordSealer, RecordType, RecoveryMethod,
    Result, SEGMENT_LEN, StreamKey, Writer, compact, compact_file, pb,
};
use proptest::prelude::*;

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
fn corrupted_only_trailer_falls_back_to_a_forward_scan() {
    let mut bytes = sample_file();
    let last = bytes.len() - 1;
    bytes[last - 10] ^= 0xFF; // inside the trailer
    let reader = open(bytes);
    match &reader.recovery().unwrap().method {
        RecoveryMethod::ForwardScan {
            records,
            stopped_at,
            ..
        } => {
            assert_eq!(*records, 5);
            // The scan skips the index record and stops at the broken
            // trailer: the last 24 bytes.
            assert_eq!(*stopped_at, (sample_file().len() - 24) as u64);
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
