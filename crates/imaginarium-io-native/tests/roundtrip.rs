// SPDX-License-Identifier: AGPL-3.0-or-later

//! Saving documents as `.imag` files and opening them again.

// Test helpers panic on failure by design.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::io::Cursor;
use std::path::PathBuf;

use imaginarium_crypto::Argon2Config;
use imaginarium_io_native::{
    Credentials, NativeError, Protection, SaveOptions, open, open_path, save_new, save_new_path,
};
use imaginarium_model::{Actor, Document, DocumentKind, Geometry, NodeKind, Parent, Value};
use zeroize::Zeroizing;

/// Fast parameters for tests only.
const FAST: Argon2Config = Argon2Config {
    memory_kib: 64,
    iterations: 1,
    parallelism: 1,
};

fn sample() -> Document {
    let mut doc = Document::new(DocumentKind::Board).unwrap();
    doc.transact(&Actor::User, |t| {
        t.set_title("Roundtrip")?;
        let frame = t.create_node(Parent::Root, None, &NodeKind::Frame)?;
        let text = t.create_node(Parent::Node(frame), None, &NodeKind::Text)?;
        t.set_geometry(text, Geometry::new(10.0, 20.0, 300.0, 40.0))?;
        t.insert_text(text, 0, "Hello, file\nSecond paragraph")?;
        t.set_paragraph_mark(text, 0, "h", Value::Int(1))?;
        Ok(())
    })
    .unwrap();
    doc
}

fn options(protection: Protection) -> SaveOptions {
    SaveOptions {
        protection,
        created_with: "imaginarium tests".into(),
    }
}

fn password(text: &str) -> Zeroizing<String> {
    Zeroizing::new(text.to_owned())
}

#[test]
fn unencrypted_files_round_trip() {
    let doc = sample();
    let bytes = save_new(Cursor::new(Vec::new()), &doc, &options(Protection::None))
        .unwrap()
        .into_inner();
    let opened = open(Cursor::new(bytes), Credentials::None).unwrap();
    assert_eq!(opened.document.view(), doc.view());
    assert!(!opened.header.is_encrypted());
    assert!(opened.recovery.is_none());
    assert!(opened.acl.is_none());
    let meta = opened.meta.unwrap();
    assert_eq!(meta.created_with, "imaginarium tests");
    assert_eq!(meta.blob_id_key.len(), 32);
}

#[test]
fn password_files_round_trip_and_hide_the_content() {
    let doc = sample();
    let protection = Protection::Password {
        password: password("correct horse"),
        argon2: FAST,
    };
    let bytes = save_new(Cursor::new(Vec::new()), &doc, &options(protection))
        .unwrap()
        .into_inner();
    assert!(
        !bytes.windows(9).any(|w| w == b"Roundtrip"),
        "no plaintext in an encrypted file"
    );
    let opened = open(
        Cursor::new(bytes.clone()),
        Credentials::Password(password("correct horse")),
    )
    .unwrap();
    assert!(opened.header.is_encrypted());
    assert_eq!(opened.document.view(), doc.view());
    assert!(
        open(
            Cursor::new(bytes.clone()),
            Credentials::Password(password("wrong"))
        )
        .is_err()
    );
    assert!(matches!(
        open(Cursor::new(bytes), Credentials::None),
        Err(NativeError::Format(
            imaginarium_format::FormatError::NeedsKey
        ))
    ));
}

#[test]
fn saving_to_a_path_replaces_the_file_atomically() {
    let dir = scratch_dir("save-path");
    let path = dir.join("doc.imag");
    std::fs::write(&path, b"old contents").unwrap();
    let doc = sample();
    save_new_path(&path, &doc, &options(Protection::None)).unwrap();
    let opened = open_path(&path, Credentials::None).unwrap();
    assert_eq!(opened.document.view(), doc.view());
    let leftovers: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(
        leftovers,
        vec![std::ffi::OsString::from("doc.imag")],
        "no temporary file is left"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn files_without_a_document_fail_cleanly() {
    let vectors = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/format/test-vectors");
    // META only (vector 7): no main stream.
    assert!(matches!(
        open_path(
            &vectors.join("07-password.imag"),
            Credentials::Password(password("café imaginarium"))
        ),
        Err(NativeError::NoDocument)
    ));
    // Placeholder bytes instead of Loro data (vector 1): a model error.
    assert!(matches!(
        open_path(&vectors.join("01-minimal.imag"), Credentials::None),
        Err(NativeError::Model(_))
    ));
}

/// A fresh directory under the workspace's `target/` (never `/tmp`, see
/// MAINTAINERS.md).
fn scratch_dir(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/test-scratch")
        .join(format!("{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}
