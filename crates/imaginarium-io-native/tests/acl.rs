// SPDX-License-Identifier: AGPL-3.0-or-later

//! Opening documents with an ACL chain: every record the document is read
//! from must be signed by a device allowed to write it (spec §7.3, §8.4).

// Test helpers panic on failure by design.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::io::Cursor;

use imaginarium_crypto::{
    CryptoError, DeviceKey, EpochKey, IdentityKey, KeyId, OsRandomness, Signer, Suite1Sealer,
    XWingKeyPair, genesis_record, keyring_record, sign_record, wrap_hpke,
};
use imaginarium_format::{
    DocId, FileHeader, ObjectId, Record, RecordBuilder, RecordType, Writer, pb,
};
use imaginarium_io_native::{Credentials, NativeError, open};
use imaginarium_model::{Actor, CODEC, Document, DocumentKind, NodeKind, Parent};

/// An owner with one device, and an encrypted document whose chain they
/// started.
struct Owned {
    doc_id: DocId,
    owner: IdentityKey,
    owner_xwing: XWingKeyPair,
    device: DeviceKey,
    key: EpochKey,
    document: Document,
}

impl Owned {
    fn new() -> Self {
        let rng = &mut OsRandomness;
        let mut document = Document::new(DocumentKind::Board).unwrap();
        document
            .transact(&Actor::User, |t| {
                t.create_node(Parent::Root, None, &NodeKind::Shape)
            })
            .unwrap();
        Self {
            doc_id: DocId::random().unwrap(),
            owner: IdentityKey::generate(rng).unwrap(),
            owner_xwing: XWingKeyPair::generate(rng).unwrap(),
            device: DeviceKey::generate(rng).unwrap(),
            key: EpochKey::generate(1, rng).unwrap(),
            document,
        }
    }

    fn credentials(&self) -> Credentials {
        Credentials::Recipient {
            keypair: XWingKeyPair::from_private_bytes(&self.owner_xwing.private_bytes()).unwrap(),
            owner: Some(self.owner.public().key_id()),
        }
    }

    /// A main-stream record of `rtype` holding `content`, encrypted, and
    /// signed by `device` if given.
    fn content(&self, rtype: RecordType, content: Vec<u8>, device: Option<&DeviceKey>) -> Record {
        let sealer = Suite1Sealer::new(&self.key, OsRandomness);
        let mut builder = RecordBuilder::new(rtype, ObjectId::random().unwrap())
            .stream(pb::Stream::Main)
            .codec(CODEC);
        if device.is_some() {
            builder = builder.will_be_signed();
        }
        let record = builder.build(&self.doc_id, content, Some(&sealer)).unwrap();
        match device {
            Some(device) => sign_record(
                record,
                &self.doc_id,
                Signer::Device(device),
                &mut OsRandomness,
            )
            .unwrap(),
            None => record,
        }
    }

    /// The file: genesis, a signed keyring for the owner, the owner's signed
    /// snapshot, then `extra` records in the main stream's head.
    fn file(&self, extra: &[Record]) -> Vec<u8> {
        let rng = &mut OsRandomness;
        let genesis = genesis_record(
            &self.doc_id,
            &self.owner,
            self.owner_xwing.public_bytes(),
            &self.device,
            XWingKeyPair::generate(rng).unwrap().public_bytes(),
            rng,
        )
        .unwrap();
        let keyring = pb::Keyring {
            epochs: vec![pb::EpochKeys {
                epoch: 1,
                wraps: vec![
                    wrap_hpke(
                        &self.doc_id,
                        &self.key,
                        &self.owner_xwing.public_bytes(),
                        rng,
                    )
                    .unwrap(),
                ],
            }],
        };
        let keyring = sign_record(
            keyring_record(&self.doc_id, ObjectId::random().unwrap(), &keyring, true).unwrap(),
            &self.doc_id,
            Signer::Identity(&self.owner),
            rng,
        )
        .unwrap();
        let snapshot = self.content(
            RecordType::SNAP,
            self.document.export_snapshot().unwrap(),
            Some(&self.device),
        );
        let mut writer =
            Writer::create(Cursor::new(Vec::new()), FileHeader::new(self.doc_id, true)).unwrap();
        for record in [genesis, keyring, snapshot].iter().chain(extra) {
            writer.append(record).unwrap();
        }
        writer
            .commit(Some(&Suite1Sealer::new(&self.key, OsRandomness)))
            .unwrap();
        writer.into_inner().into_inner()
    }

    /// A real update of the document (one more node), as an attacker with
    /// the epoch key could write it.
    fn update(&self) -> Vec<u8> {
        let mut document = Document::open(&[&self.document.export_snapshot().unwrap()]).unwrap();
        let before = document.version();
        document
            .transact(&Actor::User, |t| {
                t.create_node(Parent::Root, None, &NodeKind::Text)
            })
            .unwrap();
        document.export_updates_since(&before).unwrap()
    }
}

#[test]
fn signed_documents_open() {
    let owned = Owned::new();
    let opened = open(Cursor::new(owned.file(&[])), owned.credentials()).unwrap();
    assert_eq!(opened.document.view(), owned.document.view());
    assert_eq!(
        opened.acl.unwrap().genesis_owner(),
        owned.owner.public().key_id()
    );
}

#[test]
fn records_the_chain_does_not_authorize_are_refused() {
    // Someone with the epoch key (a viewer, or a member removed later)
    // adds an update to the head: unsigned, or signed by a device the
    // chain doesn't know. Opening must not apply it.
    let owned = Owned::new();
    let stranger = DeviceKey::generate(&mut OsRandomness).unwrap();
    for device in [None, Some(&stranger)] {
        let forged = owned.content(RecordType::UPDT, owned.update(), device);
        let err = open(Cursor::new(owned.file(&[forged])), owned.credentials())
            .err()
            .expect("the forged update was applied");
        assert!(
            matches!(
                err,
                NativeError::Crypto(CryptoError::Signature(_) | CryptoError::Acl(_))
            ),
            "{err}"
        );
    }
    // The owner's own device may write it.
    let genuine = owned.content(RecordType::UPDT, owned.update(), Some(&owned.device));
    let opened = open(Cursor::new(owned.file(&[genuine])), owned.credentials()).unwrap();
    assert_eq!(opened.document.view().node_count(), 2);
}

#[test]
fn a_chain_started_by_someone_else_is_refused() {
    let owned = Owned::new();
    let Credentials::Recipient { keypair, .. } = owned.credentials() else {
        unreachable!()
    };
    let wrong = Credentials::Recipient {
        keypair,
        owner: Some(KeyId([7; 16])),
    };
    assert!(open(Cursor::new(owned.file(&[])), wrong).is_err());
}
