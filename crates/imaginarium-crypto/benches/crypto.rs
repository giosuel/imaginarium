// SPDX-License-Identifier: AGPL-3.0-or-later

//! Crypto benchmarks, against the design's cost table
//! (`docs/design/file-format.md` §7): XChaCha20-Poly1305 ~1–2 GB/s per
//! core, Ed25519 sign/verify ~15/40 µs, HPKE unwrap ~100 µs, Argon2id
//! ~250 ms by intent. Plus what the app pays per edit batch and per open.

// Benchmark setup panics on failure by design.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::hint::black_box;
use std::io::Cursor;

use criterion::{BatchSize, Criterion, Throughput, criterion_group, criterion_main};
use imaginarium_crypto::{
    AclState, Argon2Config, DeviceKey, EpochKey, EpochKeys, IdentityKey, KnownKeys, OsRandomness,
    Role, Signer, Suite1Opener, Suite1Sealer, Verifier, XWingKeyPair, blob_id, genesis_record,
    identity_pb, keyring_record, sign_record, unwrap_hpke, unwrap_password, verify_record,
    wrap_hpke, wrap_password,
};
use imaginarium_format::{
    DocId, FileHeader, Limits, ObjectId, OpenOptions, Reader, Record, RecordBuilder, RecordType,
    Writer, pb,
};

const DOC: DocId = DocId([0x22; 16]);
const MIB: usize = 1024 * 1024;

fn key() -> EpochKey {
    EpochKey::from_bytes(1, [0x42; 32])
}

fn opener() -> Suite1Opener {
    let mut keys = EpochKeys::new();
    keys.insert(key());
    Suite1Opener::new(keys)
}

fn bench_aead(c: &mut Criterion) {
    let key = key();
    let sealer = Suite1Sealer::new(&key, OsRandomness);
    let opener = opener();
    let content: Vec<u8> = (0..MIB).map(|i| (i * 31 % 251) as u8).collect();
    let limits = Limits::default();

    let mut group = c.benchmark_group("crypto/aead_1mib");
    group.throughput(Throughput::Bytes(MIB as u64));
    group.bench_function("seal", |b| {
        b.iter(|| {
            black_box(
                RecordBuilder::new(RecordType::BLOB, ObjectId([1; 32]))
                    .build(&DOC, content.clone(), Some(&sealer))
                    .unwrap(),
            )
        });
    });
    let sealed = RecordBuilder::new(RecordType::BLOB, ObjectId([1; 32]))
        .build(&DOC, content.clone(), Some(&sealer))
        .unwrap();
    group.bench_function("open", |b| {
        b.iter(|| black_box(sealed.open_content(&DOC, Some(&opener), &limits).unwrap()));
    });
    group.bench_function("blake3_blob_id", |b| {
        b.iter(|| black_box(blob_id(&[0xB1; 32], &content)));
    });
    group.finish();
}

/// One real-time edit batch (~50 ms of typing, ~200 bytes): encrypt and
/// sign on the writer, verify and decrypt on every reader.
fn bench_edit_batch(c: &mut Criterion) {
    let key = key();
    let sealer = Suite1Sealer::new(&key, OsRandomness);
    let opener = opener();
    let device = DeviceKey::generate(&mut OsRandomness).unwrap();
    let verifier = Verifier::Device(device.public());
    let limits = Limits::default();
    let build = || {
        let record = RecordBuilder::new(RecordType::UPDT, ObjectId([2; 32]))
            .stream(pb::Stream::Main)
            .codec("loro-1")
            .will_be_signed()
            .build(&DOC, vec![7; 200], Some(&sealer))
            .unwrap();
        sign_record(record, &DOC, Signer::Device(&device), &mut OsRandomness).unwrap()
    };

    let mut group = c.benchmark_group("crypto/edit_batch_200b");
    group.bench_function("seal_and_sign", |b| b.iter(|| black_box(build())));
    let record = build();
    group.bench_function("verify_and_open", |b| {
        b.iter(|| {
            verify_record(&record, &DOC, &verifier).unwrap();
            black_box(record.open_content(&DOC, Some(&opener), &limits).unwrap())
        });
    });
    group.finish();
}

fn bench_signatures(c: &mut Criterion) {
    let device = DeviceKey::generate(&mut OsRandomness).unwrap();
    let identity = IdentityKey::generate(&mut OsRandomness).unwrap();
    let unsigned = || {
        RecordBuilder::new(RecordType::UPDT, ObjectId([3; 32]))
            .will_be_signed()
            .build(&DOC, vec![5; 200], None)
            .unwrap()
    };

    let mut group = c.benchmark_group("crypto/signatures");
    group.bench_function("ed25519_sign", |b| {
        b.iter_batched(
            unsigned,
            |record| {
                black_box(
                    sign_record(record, &DOC, Signer::Device(&device), &mut OsRandomness).unwrap(),
                )
            },
            BatchSize::SmallInput,
        );
    });
    let signed = sign_record(unsigned(), &DOC, Signer::Device(&device), &mut OsRandomness).unwrap();
    let verifier = Verifier::Device(device.public());
    group.bench_function("ed25519_verify", |b| {
        b.iter(|| verify_record(black_box(&signed), &DOC, &verifier).unwrap());
    });
    group.bench_function("hybrid_sign", |b| {
        b.iter_batched(
            unsigned,
            |record| {
                black_box(
                    sign_record(record, &DOC, Signer::Identity(&identity), &mut OsRandomness)
                        .unwrap(),
                )
            },
            BatchSize::SmallInput,
        );
    });
    let signed = sign_record(
        unsigned(),
        &DOC,
        Signer::Identity(&identity),
        &mut OsRandomness,
    )
    .unwrap();
    let verifier = Verifier::Identity(identity.public());
    group.bench_function("hybrid_verify", |b| {
        b.iter(|| verify_record(black_box(&signed), &DOC, &verifier).unwrap());
    });
    group.finish();
}

fn bench_keyrings(c: &mut Criterion) {
    let key = key();
    let recipient = XWingKeyPair::generate(&mut OsRandomness).unwrap();
    let public = recipient.public_bytes();

    let mut group = c.benchmark_group("crypto/keyring");
    group.bench_function("hpke_wrap", |b| {
        b.iter(|| black_box(wrap_hpke(&DOC, &key, &public, &mut OsRandomness).unwrap()));
    });
    let wrap = wrap_hpke(&DOC, &key, &public, &mut OsRandomness).unwrap();
    group.bench_function("hpke_unwrap", |b| {
        b.iter(|| black_box(unwrap_hpke(&DOC, 1, &wrap, &recipient).unwrap()));
    });
    group.sample_size(10);
    let wrap = wrap_password(
        &DOC,
        &key,
        "benchmark",
        Argon2Config::default(),
        &mut OsRandomness,
    )
    .unwrap();
    group.bench_function("argon2id_unwrap_default_cost", |b| {
        b.iter(|| black_box(unwrap_password(&DOC, 1, &wrap, "benchmark").unwrap()));
    });
    group.finish();
}

/// A shared document's chain: genesis plus 99 changes (members join, get a
/// device each), all hybrid-signed. Replayed on every open.
fn acl_chain() -> Vec<Record> {
    let rng = &mut OsRandomness;
    let owner = IdentityKey::generate(rng).unwrap();
    let xwing = XWingKeyPair::generate(rng).unwrap().public_bytes();
    let device = DeviceKey::generate(rng).unwrap();
    let genesis = genesis_record(&DOC, &owner, xwing.clone(), &device, xwing.clone(), rng).unwrap();
    let mut state = AclState::genesis(&DOC, &genesis).unwrap();
    let mut records = vec![genesis];
    while records.len() < 100 {
        let member = IdentityKey::generate(rng).unwrap();
        let add = pb::acl_change::Change::AddMember(pb::AddMember {
            member: Some(identity_pb(&member.public(), xwing.clone())),
            role: Role::Editor.to_pb() as i32,
        });
        let device = pb::acl_change::Change::AddDevice(pb::AddDevice {
            member_id: member.public().key_id().0.to_vec(),
            device: Some(pb::Device {
                ed25519_public_key: DeviceKey::generate(rng).unwrap().public().0.to_vec(),
                xwing_public_key: xwing.clone(),
            }),
        });
        for (change, signer) in [(add, &owner), (device, &member)] {
            let record = state.next_change(change, signer, rng).unwrap();
            state.apply(&record).unwrap();
            records.push(record);
        }
    }
    records
}

fn bench_acl(c: &mut Criterion) {
    let chain = acl_chain();
    let mut group = c.benchmark_group("crypto/acl");
    group.sample_size(20);
    group.bench_function("replay_100_changes", |b| {
        b.iter(|| black_box(AclState::replay(&DOC, &chain).unwrap()));
    });
    group.finish();
}

/// The encrypted counterpart of `format/container_10k_updates/open`: the
/// index (10 000 entries) is decrypted on open.
fn bench_open_encrypted(c: &mut Criterion) {
    let key = key();
    let sealer = Suite1Sealer::new(&key, OsRandomness);
    let mut writer = Writer::create(Cursor::new(Vec::new()), FileHeader::new(DOC, true)).unwrap();
    let keyring = pb::Keyring {
        epochs: vec![pb::EpochKeys {
            epoch: 1,
            wraps: vec![
                wrap_hpke(
                    &DOC,
                    &key,
                    &XWingKeyPair::generate(&mut OsRandomness)
                        .unwrap()
                        .public_bytes(),
                    &mut OsRandomness,
                )
                .unwrap(),
            ],
        }],
    };
    writer
        .append(&keyring_record(&DOC, ObjectId::random().unwrap(), &keyring, false).unwrap())
        .unwrap();
    for i in 0..10_000u32 {
        let update = RecordBuilder::new(RecordType::UPDT, ObjectId::random().unwrap())
            .stream(pb::Stream::Main)
            .codec("loro-1")
            .build(&DOC, i.to_le_bytes().repeat(50), Some(&sealer))
            .unwrap();
        writer.append(&update).unwrap();
    }
    writer.commit(Some(&sealer)).unwrap();
    let file = writer.into_inner().into_inner();

    let mut group = c.benchmark_group("crypto/encrypted_10k_updates");
    group.sample_size(20);
    group.bench_function("open_with_known_key", |b| {
        b.iter(|| {
            let mut keys = EpochKeys::new();
            keys.insert(key.clone());
            let mut provider = KnownKeys::new(keys);
            black_box(
                Reader::open_with(
                    Cursor::new(file.as_slice()),
                    OpenOptions::default(),
                    Some(&mut provider),
                )
                .unwrap(),
            )
        });
    });
    group.finish();
}

criterion_group!(
    benches,
    bench_aead,
    bench_edit_batch,
    bench_signatures,
    bench_keyrings,
    bench_acl,
    bench_open_encrypted
);
criterion_main!(benches);
