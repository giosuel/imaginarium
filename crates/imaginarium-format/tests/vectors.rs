// SPDX-License-Identifier: AGPL-3.0-or-later

//! Test vectors for the unencrypted container (spec §13, vectors 1–4).
//!
//! The files in `docs/format/test-vectors/` are the reference. This test
//! checks that this implementation (a) writes them byte-for-byte and (b)
//! reads them with the documented results. Independent implementations
//! can run the same checks.
//!
//! Regenerate deliberately (only when the spec changes) with
//! `IMAGINARIUM_UPDATE_VECTORS=1 cargo test -p imaginarium-format --test vectors`.

// Test helpers panic on failure by design.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::io::Cursor;
use std::path::PathBuf;

use imaginarium_format::{
    CompactionPlan, DocId, FileHeader, ObjectId, Reader, Record, RecordBuilder, RecordFlags,
    RecordType, RecoveryMethod, StreamKey, Writer, compact, pb,
};
use prost::Message;

/// The fixed document id of all vectors.
const DOC: DocId = DocId([
    0x6d, 0x69, 0x6e, 0x69, 0x6d, 0x61, 0x6c, 0x40, 0x89, 0x76, 0x65, 0x63, 0x74, 0x6f, 0x72, 0x73,
]);

/// Object id `n`: 32 bytes of value `n` (readable in hex dumps).
fn id(n: u8) -> ObjectId {
    ObjectId([n; 32])
}

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/format/test-vectors")
}

fn meta() -> Record {
    let meta = pb::DocumentMeta {
        format_version: "1.0".into(),
        created_with: "imaginarium test vectors".into(),
        created_unix_ms: 1_760_000_000_000,
        required_features: vec![],
        blob_id_key: vec![0xB1; 32],
    };
    RecordBuilder::new(RecordType::META, id(0x01))
        .build(&DOC, meta.encode_to_vec(), None)
        .unwrap()
}

fn snapshot() -> Record {
    RecordBuilder::new(RecordType::SNAP, id(0x02))
        .stream(pb::Stream::Main)
        .codec("loro-1")
        .build(&DOC, b"opaque snapshot bytes".to_vec(), None)
        .unwrap()
}

fn update(n: u8) -> Record {
    RecordBuilder::new(RecordType::UPDT, id(n))
        .stream(pb::Stream::Main)
        .codec("loro-1")
        .seq(u64::from(n))
        .build(&DOC, format!("opaque update {n}").into_bytes(), None)
        .unwrap()
}

/// Vector 1: header, META, SNAP, index, trailer.
fn v1_minimal() -> Vec<u8> {
    let mut writer = Writer::create(Cursor::new(Vec::new()), FileHeader::new(DOC, false)).unwrap();
    writer.append(&meta()).unwrap();
    writer.append(&snapshot()).unwrap();
    writer.commit_with_index_id(id(0xF1), None).unwrap();
    writer.into_inner().into_inner()
}

/// Vector 2: vector 1 plus one incremental save with an update.
fn v2_two_commits() -> Vec<u8> {
    let reader = Reader::open(Cursor::new(v1_minimal())).unwrap();
    let mut writer = Writer::resume(reader).unwrap();
    writer.append(&update(0x03)).unwrap();
    writer.commit_with_index_id(id(0xF2), None).unwrap();
    writer.into_inner().into_inner()
}

/// Vector 3a: vector 2, plus a complete uncommitted update and half of another.
fn v3a_torn_append() -> Vec<u8> {
    let mut bytes = v2_two_commits();
    bytes.extend_from_slice(&update(0x04).encode());
    let torn = update(0x05).encode();
    bytes.extend_from_slice(&torn[..torn.len() / 2]);
    bytes
}

/// Vector 3b: vector 1 with one corrupted trailer byte (in its CRC).
fn v3b_corrupt_trailer() -> Vec<u8> {
    let mut bytes = v1_minimal();
    let crc_at = bytes.len() - 24 + 12;
    bytes[crc_at] ^= 0xFF;
    bytes
}

/// Vector 3c: header and records only, no index and no trailer.
fn v3c_no_index() -> Vec<u8> {
    let mut bytes = FileHeader::new(DOC, false).encode().to_vec();
    for record in [meta(), snapshot(), update(0x03)] {
        bytes.extend_from_slice(&record.encode());
    }
    bytes
}

/// Vector 4: unknown records: a SAFE_TO_COPY extension (0x4001) and a
/// non-safe one (0x4002).
fn v4_unknown_records() -> Vec<u8> {
    let mut writer = Writer::create(Cursor::new(Vec::new()), FileHeader::new(DOC, false)).unwrap();
    writer.append(&meta()).unwrap();
    let safe = RecordBuilder::new(RecordType(0x4001), id(0x41))
        .extension_id("org.example.safe")
        .flags(RecordFlags::SAFE_TO_COPY)
        .build(&DOC, b"copy me".to_vec(), None)
        .unwrap();
    let fragile = RecordBuilder::new(RecordType(0x4002), id(0x42))
        .extension_id("org.example.fragile")
        .build(&DOC, b"drop me".to_vec(), None)
        .unwrap();
    writer.append(&safe).unwrap();
    writer.append(&fragile).unwrap();
    writer.commit_with_index_id(id(0xF4), None).unwrap();
    writer.into_inner().into_inner()
}

fn vectors() -> Vec<(&'static str, Vec<u8>)> {
    vec![
        ("01-minimal.imag", v1_minimal()),
        ("02-two-commits.imag", v2_two_commits()),
        ("03a-torn-append.imag", v3a_torn_append()),
        ("03b-corrupt-trailer.imag", v3b_corrupt_trailer()),
        ("03c-no-index.imag", v3c_no_index()),
        ("04-unknown-records.imag", v4_unknown_records()),
    ]
}

#[test]
fn files_match_the_published_vectors() {
    let update = std::env::var_os("IMAGINARIUM_UPDATE_VECTORS").is_some();
    for (name, bytes) in vectors() {
        let path = dir().join(name);
        if update {
            std::fs::create_dir_all(dir()).unwrap();
            std::fs::write(&path, &bytes).unwrap();
        }
        let published = std::fs::read(&path)
            .unwrap_or_else(|_| panic!("missing vector {name}; see the module docs"));
        assert!(
            published == bytes,
            "{name}: this implementation writes different bytes than the published vector"
        );
    }
}

fn read(name: &str) -> Reader<Cursor<Vec<u8>>> {
    Reader::open(Cursor::new(std::fs::read(dir().join(name)).unwrap())).unwrap()
}

#[test]
fn v1_reads_as_documented() {
    let mut reader = read("01-minimal.imag");
    assert!(reader.recovery().is_none());
    assert_eq!(reader.header().doc_id, DOC);
    let entries = reader.index().entries().to_vec();
    assert_eq!(
        entries.iter().map(|e| e.rtype).collect::<Vec<_>>(),
        vec![0x01, 0x03]
    );
    assert_eq!(
        entries[0].offset, 36,
        "the first record follows the 36-byte header"
    );
    let meta =
        pb::DocumentMeta::decode(reader.read_content(&entries[0]).unwrap().as_slice()).unwrap();
    assert_eq!(meta.format_version, "1.0");
    assert_eq!(
        reader.read_content(&entries[1]).unwrap(),
        b"opaque snapshot bytes"
    );
}

#[test]
fn v2_has_both_commits_and_chained_indexes() {
    let reader = read("02-two-commits.imag");
    let v1_len = std::fs::read(dir().join("01-minimal.imag")).unwrap().len();
    assert_eq!(reader.index().entries().len(), 3);
    let head = reader.index().head(&StreamKey::main()).unwrap();
    assert_eq!(head.update_ids, vec![id(0x03).0.to_vec()]);
    let bytes = std::fs::read(dir().join("02-two-commits.imag")).unwrap();
    assert_eq!(
        &bytes[..v1_len],
        std::fs::read(dir().join("01-minimal.imag"))
            .unwrap()
            .as_slice(),
        "append-only"
    );
}

#[test]
fn v3_recovery_cases_behave_as_documented() {
    let torn = read("03a-torn-append.imag");
    match &torn.recovery().unwrap().method {
        RecoveryMethod::PreviousCommit {
            adopted_records, ..
        } => assert_eq!(*adopted_records, 1),
        other => panic!("03a: {other:?}"),
    }
    assert_eq!(
        torn.index()
            .head(&StreamKey::main())
            .unwrap()
            .update_ids
            .len(),
        2
    );

    // The trailer is broken, but the index before it is intact.
    let trailer = read("03b-corrupt-trailer.imag");
    assert!(matches!(
        trailer.recovery().unwrap().method,
        RecoveryMethod::PreviousCommit {
            adopted_records: 0,
            discarded_bytes: 24,
            ..
        }
    ));

    let no_index = read("03c-no-index.imag");
    assert!(matches!(
        no_index.recovery().unwrap().method,
        RecoveryMethod::ForwardScan { records: 3, .. }
    ));
}

#[test]
fn v4_unknown_records_follow_the_copy_rules() {
    let mut reader = read("04-unknown-records.imag");
    let (out, report) = compact(
        &mut reader,
        Cursor::new(Vec::new()),
        CompactionPlan::default(),
        None,
    )
    .unwrap();
    assert_eq!(report.dropped_unknown, vec![0x4002]);
    let compacted = Reader::open(Cursor::new(out.into_inner().into_inner())).unwrap();
    let kinds: Vec<u32> = compacted
        .index()
        .entries()
        .iter()
        .map(|e| e.rtype)
        .collect();
    assert_eq!(kinds, vec![0x01, 0x4001]);
}
