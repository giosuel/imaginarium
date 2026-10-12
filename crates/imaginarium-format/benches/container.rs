// SPDX-License-Identifier: AGPL-3.0-or-later

//! Container benchmarks: opening, incremental saving and recovery.
//!
//! Budget context (PLAN §10): opening a large document must show the first
//! page in < 500 ms. The container's share (trailer, index, head snapshot)
//! should be a small fraction of that. An autosave (append plus commit)
//! must stay far below a frame's worth of a background thread.

// Benchmark setup panics on failure by design.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::hint::black_box;
use std::io::Cursor;

use criterion::{BatchSize, Criterion, criterion_group, criterion_main};
use imaginarium_format::{
    DocId, FileHeader, ObjectId, Reader, RecordBuilder, RecordType, Writer, pb,
};
use imaginarium_test_utils::rng::Rng;

const DOC: DocId = DocId([0x22; 16]);
/// Seed for the synthetic update contents. Never change it.
const SEED: u64 = 0x00F0_A3A7_0000_0001;
/// Updates in the "heavily edited, not yet compacted" document.
const UPDATES: usize = 10_000;

fn object_id(rng: &mut Rng) -> ObjectId {
    let mut id = [0u8; 32];
    for chunk in id.chunks_mut(8) {
        chunk.copy_from_slice(&rng.next_u64().to_le_bytes());
    }
    ObjectId(id)
}

/// A document with one snapshot and `UPDATES` CRDT updates of 200 bytes each.
fn heavy_file() -> Vec<u8> {
    let mut rng = Rng::new(SEED);
    let mut writer = Writer::create(Cursor::new(Vec::new()), FileHeader::new(DOC, false)).unwrap();
    let snapshot = RecordBuilder::new(RecordType::SNAP, object_id(&mut rng))
        .stream(pb::Stream::Main)
        .codec("loro-1")
        .build(&DOC, vec![7; 256 * 1024], None)
        .unwrap();
    writer.append(&snapshot).unwrap();
    for _ in 0..UPDATES {
        let content: Vec<u8> = (0..200).map(|_| rng.next_u64() as u8).collect();
        let update = RecordBuilder::new(RecordType::UPDT, object_id(&mut rng))
            .stream(pb::Stream::Main)
            .codec("loro-1")
            .build(&DOC, content, None)
            .unwrap();
        writer.append(&update).unwrap();
    }
    writer.commit(None).unwrap();
    writer.into_inner().into_inner()
}

fn bench_container(c: &mut Criterion) {
    let file = heavy_file();
    let mut group = c.benchmark_group("format/container_10k_updates");
    group.sample_size(20);

    group.bench_function("open", |b| {
        b.iter(|| black_box(Reader::open(Cursor::new(file.as_slice())).unwrap()));
    });

    group.bench_function("read_head_snapshot", |b| {
        let mut reader = Reader::open(Cursor::new(file.as_slice())).unwrap();
        let entry = reader.index().entries()[0].clone();
        b.iter(|| black_box(reader.read_content(&entry).unwrap()));
    });

    group.bench_function("append_and_commit_one_update", |b| {
        let update = RecordBuilder::new(RecordType::UPDT, ObjectId([9; 32]))
            .stream(pb::Stream::Main)
            .codec("loro-1")
            .build(&DOC, vec![1; 200], None)
            .unwrap();
        b.iter_batched(
            || Writer::resume(Reader::open(Cursor::new(file.clone())).unwrap()).unwrap(),
            |mut writer| {
                writer.append(&update).unwrap();
                writer.commit(None).unwrap();
                black_box(writer)
            },
            BatchSize::LargeInput,
        );
    });

    group.bench_function("recover_forward_scan", |b| {
        let mut broken = file.clone();
        let last = broken.len() - 1;
        broken[last] ^= 0xFF; // break the only trailer
        b.iter(|| black_box(Reader::open(Cursor::new(broken.as_slice())).unwrap()));
    });

    group.finish();
}

criterion_group!(benches, bench_container);
criterion_main!(benches);
