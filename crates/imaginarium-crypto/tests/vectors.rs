// SPDX-License-Identifier: AGPL-3.0-or-later

//! Test vectors for encryption, keyrings, signatures and the ACL chain
//! (spec §13, vectors 5–10).
//!
//! The files in `docs/format/test-vectors/` are the reference, and their
//! README lists the fixed keys and seeds. This test checks that this
//! implementation (a) writes them byte-for-byte, using deterministic
//! randomness for nonces, salts and encapsulations, and (b) reads them
//! with the documented results. Independent implementations need only the
//! keys from the README to run the reading checks.
//!
//! Regenerate deliberately (only when the spec changes) with
//! `IMAGINARIUM_UPDATE_VECTORS=1 cargo test -p imaginarium-crypto --test vectors`.

// Test helpers panic on failure by design.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::io::Cursor;
use std::path::PathBuf;

use imaginarium_crypto::{
    AclState, Argon2Config, CryptoError, DeviceKey, EpochKey, EpochKeys, IdentityKey, KeyId,
    KnownKeys, PasswordKeys, RecipientKeys, Role, SeededRandomness, SharedKeys, Signer,
    Suite1Sealer, Verifier, XWingKeyPair, blob_records, genesis_record, identity_pb,
    keyring_record, keyrings, sign_record, unwrap_hpke, verify_record, wrap_hpke, wrap_password,
    wrap_with_key,
};
use imaginarium_format::{
    DocId, FileHeader, KeyProvider, ObjectId, OpenOptions, Reader, Record, RecordBuilder,
    RecordSealer, RecordType, SEGMENT_LEN, Writer, pb,
};
use pb::acl_change::Change;
use prost::Message;

/// The fixed document id of all vectors (as in vectors 1–4).
const DOC: DocId = DocId([
    0x6d, 0x69, 0x6e, 0x69, 0x6d, 0x61, 0x6c, 0x40, 0x89, 0x76, 0x65, 0x63, 0x74, 0x6f, 0x72, 0x73,
]);

/// The epoch keys `DEK_1` and `DEK_2`.
const DEK_1: [u8; 32] = [0xD1; 32];
const DEK_2: [u8; 32] = [0xD2; 32];
/// `DocumentMeta.blob_id_key`.
const BLOB_ID_KEY: [u8; 32] = [0xB1; 32];
/// Folder key of vectors 5 and 6, and the folder id it is addressed to.
const FOLDER_KEY: [u8; 32] = [0xF0; 32];
const FOLDER_ID: &[u8] = b"vector folder";
/// Password of vector 7 (NFC, with a composed "é"), and its Argon2id cost.
const PASSWORD: &str = "caf\u{e9} imaginarium";
const VECTOR_ARGON2: Argon2Config = Argon2Config {
    memory_kib: 64,
    iterations: 1,
    parallelism: 1,
};
/// X-Wing seed of the recipient in vector 8.
const RECIPIENT_SEED: [u8; 32] = [0xA1; 32];
/// Signing seeds of vector 9.
const DEVICE_SEED: [u8; 32] = [0xE1; 32];
const IDENTITY_ED25519_SEED: [u8; 32] = [0xE2; 32];
const IDENTITY_MLDSA_SEED: [u8; 32] = [0xE3; 32];

/// Object id `n`: 32 bytes of value `n` (readable in hex dumps).
fn id(n: u8) -> ObjectId {
    ObjectId([n; 32])
}

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/format/test-vectors")
}

fn dek(epoch: u32) -> EpochKey {
    EpochKey::from_bytes(epoch, if epoch == 1 { DEK_1 } else { DEK_2 })
}

fn rng(vector: &str) -> SeededRandomness {
    SeededRandomness::new(vector)
}

fn meta_content() -> Vec<u8> {
    pb::DocumentMeta {
        format_version: "1.0".into(),
        created_with: "imaginarium test vectors".into(),
        created_unix_ms: 1_760_000_000_000,
        required_features: vec![],
        blob_id_key: BLOB_ID_KEY.to_vec(),
    }
    .encode_to_vec()
}

fn update_content(n: u8) -> Vec<u8> {
    format!("opaque update {n} ").repeat(16).into_bytes()
}

/// A user of the ACL vector, with all keys derived from one byte.
struct User {
    identity: IdentityKey,
    xwing: XWingKeyPair,
    device: DeviceKey,
    device_xwing: XWingKeyPair,
}

impl User {
    /// Seeds: identity Ed25519 `n0`, identity ML-DSA `n1`, identity X-Wing
    /// `n2`, device Ed25519 `n3`, device X-Wing `n4` (each 32 × that byte).
    fn new(n: u8) -> Self {
        Self {
            identity: IdentityKey::from_seeds(&[n; 32], &[n + 1; 32]),
            xwing: XWingKeyPair::from_seed(&[n + 2; 32]),
            device: DeviceKey::from_seed(&[n + 3; 32]),
            device_xwing: XWingKeyPair::from_seed(&[n + 4; 32]),
        }
    }

    fn id(&self) -> KeyId {
        self.identity.public().key_id()
    }

    fn xwing(&self) -> XWingKeyPair {
        XWingKeyPair::from_private_bytes(&self.xwing.private_bytes()).unwrap()
    }

    /// Keys for documents whose chain [`owner`] started.
    fn reader_keys(&self) -> RecipientKeys {
        RecipientKeys::new(self.xwing(), owner().id())
    }
}

fn owner() -> User {
    User::new(0x10)
}

fn editor() -> User {
    User::new(0x20)
}

fn viewer() -> User {
    User::new(0x30)
}

/// Writes `records` to a new file and commits once with index id `index_id`.
fn file(
    encrypted: bool,
    records: &[Record],
    index_id: u8,
    sealer: Option<&dyn RecordSealer>,
) -> Vec<u8> {
    let mut writer =
        Writer::create(Cursor::new(Vec::new()), FileHeader::new(DOC, encrypted)).unwrap();
    for record in records {
        writer.append(record).unwrap();
    }
    writer.commit_with_index_id(id(index_id), sealer).unwrap();
    writer.into_inner().into_inner()
}

fn folder_keyring(vector: &str) -> Record {
    let wrap = wrap_with_key(
        &DOC,
        &dek(1),
        &FOLDER_KEY,
        pb::WrapKind::FolderKey,
        FOLDER_ID.to_vec(),
        &mut rng(&format!("{vector} wrap")),
    )
    .unwrap();
    let keyring = pb::Keyring {
        epochs: vec![pb::EpochKeys {
            epoch: 1,
            wraps: vec![wrap],
        }],
    };
    keyring_record(&DOC, id(0x51), &keyring, false).unwrap()
}

/// Vector 5: an encrypted document (folder-key keyring, `META`, `SNAP`, a
/// compressed `UPDT`, encrypted index).
fn v5_encrypted() -> Vec<u8> {
    let key = dek(1);
    let sealer = Suite1Sealer::new(&key, rng("vector 5"));
    let records = [
        folder_keyring("vector 5"),
        RecordBuilder::new(RecordType::META, id(0x01))
            .build(&DOC, meta_content(), Some(&sealer))
            .unwrap(),
        RecordBuilder::new(RecordType::SNAP, id(0x02))
            .stream(pb::Stream::Main)
            .codec("loro-1")
            .build(&DOC, b"opaque snapshot bytes".to_vec(), Some(&sealer))
            .unwrap(),
        RecordBuilder::new(RecordType::UPDT, id(0x03))
            .stream(pb::Stream::Main)
            .codec("loro-1")
            .compress(true)
            .build(&DOC, update_content(3), Some(&sealer))
            .unwrap(),
    ];
    file(true, &records, 0xF5, Some(&sealer))
}

/// The blob of vector 6: 1 MiB + 1000 bytes, five segments, compressible.
fn v6_blob() -> Vec<u8> {
    (0..1024 * 1024 + 1000)
        .map(|i| (i * 7 % 256) as u8)
        .collect()
}

/// A sealer that encrypts the last segment as if it weren't the last one.
struct DropLastFlag<'a, 'k>(&'a Suite1Sealer<'k, SeededRandomness>);

impl RecordSealer for DropLastFlag<'_, '_> {
    fn suite(&self) -> u32 {
        self.0.suite()
    }
    fn epoch(&self) -> u32 {
        self.0.epoch()
    }
    fn seal(
        &self,
        rtype: RecordType,
        header: &pb::RecordHeader,
        aad: &[u8],
        plaintext: &[u8],
    ) -> imaginarium_format::Result<Vec<u8>> {
        let mut header = header.clone();
        if let Some(segment) = header.segment.as_mut() {
            segment.count += 1;
        }
        self.0.seal(rtype, &header, aad, plaintext)
    }
}

/// Vector 6: a segmented encrypted blob (a), and tampered copies with two
/// segment payloads swapped (b), the last segment missing (c), and the
/// last segment encrypted without the STREAM last flag (d).
fn v6_segmented() -> [Vec<u8>; 4] {
    let key = dek(1);
    let sealer = Suite1Sealer::new(&key, rng("vector 6"));
    let meta = RecordBuilder::new(RecordType::META, id(0x01))
        .build(&DOC, meta_content(), Some(&sealer))
        .unwrap();
    let blob = v6_blob();
    let (blob_id, segments) = blob_records(&DOC, &BLOB_ID_KEY, &blob, true, Some(&sealer)).unwrap();
    assert_eq!(segments.len(), 5);
    let base = |segments: &[Record]| {
        let mut records = vec![folder_keyring("vector 6"), meta.clone()];
        records.extend_from_slice(segments);
        file(true, &records, 0xF6, Some(&sealer))
    };

    let a = base(&segments);

    let mut swapped = segments.clone();
    let (one, two) = (&segments[1], &segments[2]);
    swapped[1] = Record::new(
        one.rtype(),
        one.flags(),
        one.header().clone(),
        two.payload().to_vec(),
    );
    swapped[2] = Record::new(
        two.rtype(),
        two.flags(),
        two.header().clone(),
        one.payload().to_vec(),
    );
    let b = base(&swapped);

    let c = base(&segments[..4]);

    let last = segments[4].header().segment.clone().unwrap();
    let mut unflagged = segments.clone();
    unflagged[4] = RecordBuilder::new(RecordType::BLOB, blob_id)
        .compress(true)
        .segment(last.index, last.count, last.total_len, last.nonce_prefix)
        .build(
            &DOC,
            blob[4 * SEGMENT_LEN..].to_vec(),
            Some(&DropLastFlag(&sealer)),
        )
        .unwrap();
    let d = base(&unflagged);
    [a, b, c, d]
}

/// Vector 7: a password keyring (Argon2id with tiny, vector-only costs).
fn v7_password() -> Vec<u8> {
    let key = dek(1);
    let mut wrap_rng = rng("vector 7 wrap");
    let wrap = wrap_password(&DOC, &key, PASSWORD, VECTOR_ARGON2, &mut wrap_rng).unwrap();
    let keyring = pb::Keyring {
        epochs: vec![pb::EpochKeys {
            epoch: 1,
            wraps: vec![wrap],
        }],
    };
    let sealer = Suite1Sealer::new(&key, rng("vector 7"));
    let records = [
        keyring_record(&DOC, id(0x51), &keyring, false).unwrap(),
        RecordBuilder::new(RecordType::META, id(0x01))
            .build(&DOC, meta_content(), Some(&sealer))
            .unwrap(),
    ];
    file(true, &records, 0xF7, Some(&sealer))
}

/// Vector 8: an HPKE (X-Wing) keyring with two epochs; a snapshot under
/// epoch 1 and an update under epoch 2.
fn v8_hpke() -> Vec<u8> {
    let recipient = XWingKeyPair::from_seed(&RECIPIENT_SEED);
    let mut wrap_rng = rng("vector 8 wrap");
    let keyring = pb::Keyring {
        epochs: [1, 2]
            .map(|epoch| pb::EpochKeys {
                epoch,
                wraps: vec![
                    wrap_hpke(&DOC, &dek(epoch), &recipient.public_bytes(), &mut wrap_rng).unwrap(),
                ],
            })
            .to_vec(),
    };
    let (k1, k2) = (dek(1), dek(2));
    let s1 = Suite1Sealer::new(&k1, rng("vector 8 epoch 1"));
    let s2 = Suite1Sealer::new(&k2, rng("vector 8 epoch 2"));
    let records = [
        keyring_record(&DOC, id(0x51), &keyring, false).unwrap(),
        RecordBuilder::new(RecordType::META, id(0x01))
            .build(&DOC, meta_content(), Some(&s1))
            .unwrap(),
        RecordBuilder::new(RecordType::SNAP, id(0x02))
            .stream(pb::Stream::Main)
            .codec("loro-1")
            .build(&DOC, b"opaque snapshot bytes".to_vec(), Some(&s1))
            .unwrap(),
        RecordBuilder::new(RecordType::UPDT, id(0x03))
            .stream(pb::Stream::Main)
            .codec("loro-1")
            .build(&DOC, update_content(3), Some(&s2))
            .unwrap(),
    ];
    file(true, &records, 0xF8, Some(&s2))
}

/// Vector 9: an unencrypted document with an Ed25519-signed update and a
/// hybrid-signed update.
fn v9_signatures() -> Vec<u8> {
    let device = DeviceKey::from_seed(&DEVICE_SEED);
    let identity = IdentityKey::from_seeds(&IDENTITY_ED25519_SEED, &IDENTITY_MLDSA_SEED);
    let mut sign_rng = rng("vector 9");
    let update = |n: u8| {
        RecordBuilder::new(RecordType::UPDT, id(n))
            .stream(pb::Stream::Main)
            .codec("loro-1")
            .will_be_signed()
            .build(&DOC, update_content(n), None)
            .unwrap()
    };
    let records = [
        RecordBuilder::new(RecordType::META, id(0x01))
            .build(&DOC, meta_content(), None)
            .unwrap(),
        sign_record(update(0x03), &DOC, Signer::Device(&device), &mut sign_rng).unwrap(),
        sign_record(
            update(0x04),
            &DOC,
            Signer::Identity(&identity),
            &mut sign_rng,
        )
        .unwrap(),
    ];
    file(false, &records, 0xF9, None)
}

fn add_member(user: &User, role: Role) -> Change {
    Change::AddMember(pb::AddMember {
        member: Some(identity_pb(
            &user.identity.public(),
            user.xwing.public_bytes(),
        )),
        role: role.to_pb() as i32,
    })
}

fn add_device(user: &User) -> Change {
    Change::AddDevice(pb::AddDevice {
        member_id: user.id().0.to_vec(),
        device: Some(pb::Device {
            ed25519_public_key: user.device.public().0.to_vec(),
            xwing_public_key: user.device_xwing.public_bytes(),
        }),
    })
}

/// A KEYR wrapping each epoch to each user's identity, signed by `signer`.
fn signed_keyring(
    epochs: &[u32],
    to: &[&User],
    signer: &User,
    object: u8,
    rng: &mut SeededRandomness,
) -> Record {
    let keyring = pb::Keyring {
        epochs: epochs
            .iter()
            .map(|&epoch| pb::EpochKeys {
                epoch,
                wraps: to
                    .iter()
                    .map(|user| {
                        wrap_hpke(&DOC, &dek(epoch), &user.xwing.public_bytes(), rng).unwrap()
                    })
                    .collect(),
            })
            .collect(),
    };
    let record = keyring_record(&DOC, id(object), &keyring, true).unwrap();
    sign_record(record, &DOC, Signer::Identity(&signer.identity), rng).unwrap()
}

/// An encrypted update signed by `user`'s device.
fn signed_update(
    user: &User,
    n: u8,
    sealer: &dyn RecordSealer,
    rng: &mut SeededRandomness,
) -> Record {
    let record = RecordBuilder::new(RecordType::UPDT, id(n))
        .stream(pb::Stream::Main)
        .codec("loro-1")
        .will_be_signed()
        .build(&DOC, update_content(n), Some(sealer))
        .unwrap();
    sign_record(record, &DOC, Signer::Device(&user.device), rng).unwrap()
}

/// Starts an ACL chain for the owner and applies `changes`.
fn chain(changes: Vec<(Change, &User)>, rng: &mut SeededRandomness) -> (AclState, Vec<Record>) {
    let owner = owner();
    let genesis = genesis_record(
        &DOC,
        &owner.identity,
        owner.xwing.public_bytes(),
        &owner.device,
        owner.device_xwing.public_bytes(),
        rng,
    )
    .unwrap();
    let mut state = AclState::genesis(&DOC, &genesis).unwrap();
    let mut records = vec![genesis];
    for (change, signer) in changes {
        let record = state.next_change(change, &signer.identity, rng).unwrap();
        state.apply(&record).unwrap();
        records.push(record);
    }
    (state, records)
}

/// Vector 10a: an ACL chain over two epochs: genesis, add an editor, the
/// editor adds a device and writes, the owner removes them, rotates to
/// epoch 2, re-wraps the keys for themselves only, and writes.
fn v10a_acl_chain() -> Vec<u8> {
    let (owner, editor) = (owner(), editor());
    let mut rng = rng("vector 10a");
    let (k1, k2) = (dek(1), dek(2));
    let s1 = Suite1Sealer::new(&k1, SeededRandomness::new("vector 10a epoch 1"));
    let s2 = Suite1Sealer::new(&k2, SeededRandomness::new("vector 10a epoch 2"));

    let (mut state, mut records) = chain(
        vec![
            (add_member(&editor, Role::Editor), &owner),
            (add_device(&editor), &editor),
        ],
        &mut rng,
    );
    records.push(signed_keyring(
        &[1],
        &[&owner, &editor],
        &owner,
        0x51,
        &mut rng,
    ));
    records.push(
        RecordBuilder::new(RecordType::META, id(0x01))
            .build(&DOC, meta_content(), Some(&s1))
            .unwrap(),
    );
    records.push(signed_update(&editor, 0x03, &s1, &mut rng));
    let remove = Change::RemoveMember(pb::RemoveMember {
        member_id: editor.id().0.to_vec(),
    });
    for change in [remove, Change::RotateEpoch(pb::RotateEpoch { epoch: 2 })] {
        let record = state
            .next_change(change, &owner.identity, &mut rng)
            .unwrap();
        state.apply(&record).unwrap();
        records.push(record);
    }
    records.push(signed_keyring(&[1, 2], &[&owner], &owner, 0x52, &mut rng));
    records.push(signed_update(&owner, 0x04, &s2, &mut rng));
    file(true, &records, 0xFA, Some(&s2))
}

/// The start of vectors 10b and 10c: genesis, editor added, a keyring for
/// the owner, and an encrypted META.
fn acl_base(rng: &mut SeededRandomness) -> (AclState, Vec<Record>) {
    let (owner, editor) = (owner(), editor());
    let (state, mut records) = chain(vec![(add_member(&editor, Role::Editor), &owner)], rng);
    records.push(signed_keyring(&[1], &[&owner], &owner, 0x51, rng));
    (state, records)
}

fn acl_file(records: &[Record], index_id: u8, label: &str) -> Vec<u8> {
    let key = dek(1);
    let sealer = Suite1Sealer::new(&key, SeededRandomness::new(label));
    let mut records = records.to_vec();
    records.push(
        RecordBuilder::new(RecordType::META, id(0x01))
            .build(&DOC, meta_content(), Some(&sealer))
            .unwrap(),
    );
    file(true, &records, index_id, Some(&sealer))
}

/// Vector 10b: a fork: a second, different change with index 1.
fn v10b_acl_fork() -> Vec<u8> {
    let mut rng = rng("vector 10b");
    let (_, mut records) = acl_base(&mut rng);
    let genesis = AclState::genesis(&DOC, &records[0]).unwrap();
    let fork = genesis
        .next_change(
            add_member(&viewer(), Role::Viewer),
            &owner().identity,
            &mut rng,
        )
        .unwrap();
    records.push(fork);
    acl_file(&records, 0xFB, "vector 10b epoch 1")
}

/// Vector 10c: an editor (not allowed to manage members) adds a member.
fn v10c_acl_unauthorized() -> Vec<u8> {
    let mut rng = rng("vector 10c");
    let (state, mut records) = acl_base(&mut rng);
    let rogue = state
        .next_change(
            add_member(&viewer(), Role::Viewer),
            &editor().identity,
            &mut rng,
        )
        .unwrap();
    records.push(rogue);
    acl_file(&records, 0xFC, "vector 10c epoch 1")
}

fn vectors() -> Vec<(&'static str, Vec<u8>)> {
    let [v6a, v6b, v6c, v6d] = v6_segmented();
    vec![
        ("05-encrypted.imag", v5_encrypted()),
        ("06a-segmented-blob.imag", v6a),
        ("06b-segments-swapped.imag", v6b),
        ("06c-segment-missing.imag", v6c),
        ("06d-last-flag-missing.imag", v6d),
        ("07-password.imag", v7_password()),
        ("08-hpke.imag", v8_hpke()),
        ("09-signatures.imag", v9_signatures()),
        ("10a-acl-chain.imag", v10a_acl_chain()),
        ("10b-acl-fork.imag", v10b_acl_fork()),
        ("10c-acl-unauthorized.imag", v10c_acl_unauthorized()),
    ]
}

#[test]
fn files_match_the_published_vectors() {
    let update = std::env::var_os("IMAGINARIUM_UPDATE_VECTORS").is_some();
    for (name, bytes) in vectors() {
        let path = dir().join(name);
        if update {
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

fn bytes(name: &str) -> Vec<u8> {
    std::fs::read(dir().join(name)).unwrap()
}

fn open(
    name: &str,
    keys: &mut dyn KeyProvider,
) -> imaginarium_format::Result<Reader<Cursor<Vec<u8>>>> {
    Reader::open_with(Cursor::new(bytes(name)), OpenOptions::default(), Some(keys))
}

fn content(reader: &mut Reader<Cursor<Vec<u8>>>, object: u8) -> Vec<u8> {
    let entry = reader
        .index()
        .entries()
        .iter()
        .find(|entry| entry.object_id == id(object).0)
        .cloned()
        .unwrap();
    reader.read_content(&entry).unwrap()
}

fn records(reader: &mut Reader<Cursor<Vec<u8>>>) -> Vec<Record> {
    let entries = reader.index().entries().to_vec();
    entries
        .iter()
        .map(|entry| reader.read_record(entry).unwrap())
        .collect()
}

fn known(keys: &[EpochKey]) -> KnownKeys {
    let mut set = EpochKeys::new();
    keys.iter().cloned().for_each(|key| set.insert(key));
    KnownKeys::new(set)
}

#[test]
fn v5_opens_with_the_folder_key_or_the_dek() {
    let mut reader = open(
        "05-encrypted.imag",
        &mut SharedKeys::folder(FOLDER_ID.to_vec(), FOLDER_KEY),
    )
    .unwrap();
    assert!(reader.header().is_encrypted());
    assert_eq!(content(&mut reader, 0x02), b"opaque snapshot bytes");
    assert_eq!(
        content(&mut reader, 0x03),
        update_content(3),
        "decrypted, then decompressed"
    );
    let mut reader = open("05-encrypted.imag", &mut known(&[dek(1)])).unwrap();
    assert_eq!(content(&mut reader, 0x01), meta_content());

    assert!(
        open(
            "05-encrypted.imag",
            &mut SharedKeys::folder(FOLDER_ID.to_vec(), [0xF1; 32])
        )
        .is_err()
    );
    assert!(
        open(
            "05-encrypted.imag",
            &mut SharedKeys::folder(b"other".to_vec(), FOLDER_KEY)
        )
        .is_err()
    );
    assert!(
        open(
            "05-encrypted.imag",
            &mut known(&[EpochKey::from_bytes(1, [0; 32])])
        )
        .is_err()
    );
    assert!(
        Reader::open(Cursor::new(bytes("05-encrypted.imag"))).is_err(),
        "needs a key"
    );
}

#[test]
fn v6_segmented_blob_and_tampering() {
    let blob_id = imaginarium_crypto::blob_id(&BLOB_ID_KEY, &v6_blob());
    let read = |name: &str| {
        let mut reader = open(name, &mut known(&[dek(1)])).unwrap();
        reader.read_blob(&blob_id)
    };
    assert_eq!(read("06a-segmented-blob.imag").unwrap(), v6_blob());
    // Swapped payloads and a missing last flag fail authentication. A
    // missing segment is caught before decryption, by the segment count.
    for (name, authentication) in [
        ("06b-segments-swapped.imag", true),
        ("06c-segment-missing.imag", false),
        ("06d-last-flag-missing.imag", true),
    ] {
        let err = read(name).err().unwrap();
        assert_eq!(err.is_key_problem(), authentication, "{name}: {err}");
    }
}

#[test]
fn v7_opens_with_the_password_in_any_normalization() {
    let mut reader = open("07-password.imag", &mut PasswordKeys::new(PASSWORD)).unwrap();
    assert_eq!(content(&mut reader, 0x01), meta_content());
    assert!(
        open(
            "07-password.imag",
            &mut PasswordKeys::new("cafe\u{301} imaginarium")
        )
        .is_ok(),
        "NFD input"
    );
    assert!(
        open(
            "07-password.imag",
            &mut PasswordKeys::new("cafe imaginarium")
        )
        .is_err()
    );
}

#[test]
fn v8_wraps_unwrap_but_the_file_needs_an_acl_chain() {
    let recipient = XWingKeyPair::from_seed(&RECIPIENT_SEED);
    // The wraps themselves are valid HPKE (spec §6.4.2), one per epoch:
    // the keys they hold open both epochs' records.
    let mut reader = open("08-hpke.imag", &mut known(&[dek(1), dek(2)])).unwrap();
    let key_records: Vec<Record> = records(&mut reader)
        .into_iter()
        .filter(|r| r.rtype() == RecordType::KEYR)
        .collect();
    let unwrapped: Vec<EpochKey> = keyrings(&DOC, &key_records).unwrap()[0]
        .epochs
        .iter()
        .map(|epoch| unwrap_hpke(&DOC, epoch.epoch, &epoch.wraps[0], &recipient).unwrap())
        .collect();
    let mut reader = open("08-hpke.imag", &mut known(&unwrapped)).unwrap();
    assert_eq!(content(&mut reader, 0x02), b"opaque snapshot bytes");
    assert_eq!(content(&mut reader, 0x03), update_content(3));
    // But without an ACL chain anyone could have made them, so the file
    // doesn't open with a recipient key (ADR-0018).
    for mut keys in [
        RecipientKeys::trusting_any_owner(XWingKeyPair::from_seed(&RECIPIENT_SEED)),
        RecipientKeys::new(XWingKeyPair::from_seed(&RECIPIENT_SEED), owner().id()),
    ] {
        let err = open("08-hpke.imag", &mut keys).err().unwrap();
        assert!(err.to_string().contains("need an ACL chain"), "{err}");
    }
}

#[test]
fn v9_signatures_verify() {
    let mut reader = Reader::open(Cursor::new(bytes("09-signatures.imag"))).unwrap();
    let records = records(&mut reader);
    let device = Verifier::Device(DeviceKey::from_seed(&DEVICE_SEED).public());
    let identity = Verifier::Identity(
        IdentityKey::from_seeds(&IDENTITY_ED25519_SEED, &IDENTITY_MLDSA_SEED).public(),
    );
    let (ed25519, hybrid) = (&records[1], &records[2]);
    verify_record(ed25519, &DOC, &device).unwrap();
    verify_record(hybrid, &DOC, &identity).unwrap();
    assert!(verify_record(ed25519, &DOC, &identity).is_err());
    assert!(verify_record(hybrid, &DOC, &device).is_err());
    let tampered = Record::new(
        hybrid.rtype(),
        hybrid.flags(),
        hybrid.header().clone(),
        b"forged".to_vec(),
    )
    .with_signature(hybrid.signature_bytes().unwrap().to_vec());
    assert!(verify_record(&tampered, &DOC, &identity).is_err());
}

#[test]
fn v10a_acl_chain_replays_and_authorizes_every_record() {
    let (owner, editor) = (owner(), editor());
    let mut reader = open("10a-acl-chain.imag", &mut owner.reader_keys()).unwrap();
    let records = records(&mut reader);
    let acl = AclState::replay(&DOC, &records).unwrap();
    assert_eq!(acl.owner(), owner.id());
    assert_eq!(acl.members().len(), 1, "the editor was removed");
    assert_eq!(acl.epoch(), 2);
    assert_eq!(acl.next_index(), 5);
    assert!(!acl.rotation_required());
    for record in &records {
        match record.rtype() {
            RecordType::KEYR => acl.verify_keyring(record).unwrap(),
            RecordType::ACLC => {}
            _ => acl.verify_content(record).unwrap(),
        }
    }
    assert_eq!(
        content(&mut reader, 0x03),
        update_content(3),
        "the editor's edit, epoch 1"
    );
    assert_eq!(
        content(&mut reader, 0x04),
        update_content(4),
        "the owner's edit, epoch 2"
    );
    // The removed editor can't open the current version: its index is
    // encrypted under epoch 2, which was never wrapped for them.
    assert!(open("10a-acl-chain.imag", &mut editor.reader_keys()).is_err());
}

#[test]
fn v10a_opens_only_for_the_expected_owner() {
    let owner = owner();
    let mut wrong = RecipientKeys::new(owner.xwing(), editor().id());
    let err = open("10a-acl-chain.imag", &mut wrong).err().unwrap();
    assert!(err.to_string().contains("expected owner"), "{err}");
    let mut any = RecipientKeys::trusting_any_owner(owner.xwing());
    assert!(open("10a-acl-chain.imag", &mut any).is_ok());
}

#[test]
fn v10b_and_v10c_are_rejected_when_opening() {
    let fork = open("10b-acl-fork.imag", &mut owner().reader_keys())
        .err()
        .unwrap();
    assert!(
        fork.to_string()
            .contains(&CryptoError::AclFork(1).to_string()),
        "{fork}"
    );
    let rogue = open("10c-acl-unauthorized.imag", &mut owner().reader_keys())
        .err()
        .unwrap();
    assert!(
        rogue.to_string().contains("not allowed to add a member"),
        "{rogue}"
    );
}

#[test]
fn a_chain_replaced_by_someone_else_is_refused() {
    // ADR-0018: whoever can write to the file replaces the ACL chain with
    // one they started and wraps a key they know to the owner's public
    // key. The chain is internally valid; only the expected owner tells it
    // apart.
    let (owner, attacker) = (owner(), User::new(0x40));
    let mut rng = rng("replaced chain");
    let genesis = genesis_record(
        &DOC,
        &attacker.identity,
        attacker.xwing.public_bytes(),
        &attacker.device,
        attacker.device_xwing.public_bytes(),
        &mut rng,
    )
    .unwrap();
    let keyring = signed_keyring(&[1], &[&owner], &attacker, 0x51, &mut rng);
    let key = dek(1);
    let sealer = Suite1Sealer::new(&key, SeededRandomness::new("replaced chain epoch 1"));
    let meta = RecordBuilder::new(RecordType::META, id(0x01))
        .build(&DOC, meta_content(), Some(&sealer))
        .unwrap();
    let forged = file(true, &[genesis, keyring, meta], 0xF0, Some(&sealer));
    let err = Reader::open_with(
        Cursor::new(forged),
        OpenOptions::default(),
        Some(&mut owner.reader_keys()),
    )
    .err()
    .expect("the forged chain was accepted");
    assert!(err.to_string().contains("expected owner"), "{err}");
}
