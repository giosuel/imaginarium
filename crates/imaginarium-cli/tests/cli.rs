// SPDX-License-Identifier: AGPL-3.0-or-later

//! The `imag` binary, run as a user would run it.

// Test helpers panic on failure by design.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use imaginarium_crypto::{Argon2Config, IdentityKey, XWingKeyPair};
use imaginarium_io_native::{Protection, SaveOptions, save_new_path};
use imaginarium_model::{Actor, Document, DocumentKind, NodeKind, Parent};
use zeroize::Zeroizing;

struct Output {
    code: i32,
    stdout: String,
    stderr: String,
}

fn imag(args: &[&str], stdin: Option<&str>, env: &[(&str, &str)]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_imag"));
    command
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::piped());
    for (key, value) in env {
        command.env(key, value);
    }
    let mut child = command.spawn().unwrap();
    if let Some(input) = stdin {
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
    }
    drop(child.stdin.take());
    let output = child.wait_with_output().unwrap();
    Output {
        code: output.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
}

/// A fresh directory under the workspace's `target/` (never `/tmp`),
/// removed when the test is done.
struct Scratch(PathBuf);

impl std::ops::Deref for Scratch {
    type Target = Path;
    fn deref(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn scratch(name: &str) -> Scratch {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/test-scratch")
        .join(format!("cli-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    Scratch(dir)
}

fn vector(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/format/test-vectors")
        .join(name)
}

fn sample(path: &Path, protection: Protection) {
    let mut doc = Document::new(DocumentKind::Document).unwrap();
    doc.transact(&Actor::User, |t| {
        t.set_title("CLI sample")?;
        let text = t.create_node(Parent::Root, None, &NodeKind::Text)?;
        t.insert_text(text, 0, "Hello from imag")
    })
    .unwrap();
    let options = SaveOptions {
        protection,
        created_with: "imag tests".into(),
    };
    save_new_path(path, &doc, &options).unwrap();
}

fn password_protection() -> Protection {
    Protection::Password {
        password: Zeroizing::new("s3cret".into()),
        argon2: Argon2Config {
            memory_kib: 64,
            iterations: 1,
            parallelism: 1,
        },
    }
}

fn s(path: &Path) -> &str {
    path.to_str().unwrap()
}

#[test]
fn inspect_lists_records_as_text_and_json() {
    let dir = scratch("inspect");
    let file = dir.join("plain.imag");
    sample(&file, Protection::None);
    let out = imag(&["inspect", s(&file)], None, &[]);
    assert_eq!(out.code, 0, "{}", out.stderr);
    for expected in [
        "META",
        "SNAP",
        "INDX",
        "Commits   1",
        "main: snapshot + 0 updates",
        "loro-1",
    ] {
        assert!(
            out.stdout.contains(expected),
            "missing {expected:?} in:\n{}",
            out.stdout
        );
    }
    let out = imag(&["inspect", "--json", s(&file)], None, &[]);
    let json: serde_json::Value = serde_json::from_str(&out.stdout).unwrap();
    assert_eq!(json["records"].as_array().unwrap().len(), 3);
    assert_eq!(json["encrypted"], false);
    assert_eq!(json["records"][1]["type"], "SNAP");
}

#[test]
fn inspect_works_on_encrypted_files_without_a_key() {
    let dir = scratch("inspect-encrypted");
    let file = dir.join("secret.imag");
    sample(&file, password_protection());
    let out = imag(&["inspect", s(&file)], None, &[]);
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(out.stdout.contains("encrypted index; give a key"));
    assert!(out.stdout.contains("KEYR") && out.stdout.contains("CS-E--"));
    assert!(!out.stdout.contains("CLI sample"), "no plaintext");
    let out = imag(
        &["inspect", "--password-env", "IMAG_TEST_PW", s(&file)],
        None,
        &[("IMAG_TEST_PW", "s3cret")],
    );
    assert!(
        out.stdout.contains("main: snapshot + 0 updates"),
        "{}",
        out.stdout
    );
}

#[test]
fn verify_passes_good_files_and_explains_problems() {
    let dir = scratch("verify");
    let plain = dir.join("plain.imag");
    sample(&plain, Protection::None);
    let out = imag(&["verify", s(&plain)], None, &[]);
    assert_eq!(out.code, 0, "{}\n{}", out.stdout, out.stderr);
    assert!(out.stdout.contains("Result: ok"), "{}", out.stdout);
    assert!(out.stdout.contains("1 nodes, editable"), "{}", out.stdout);

    let secret = dir.join("secret.imag");
    sample(&secret, password_protection());
    let out = imag(
        &["verify", "--password-stdin", s(&secret)],
        Some("s3cret\n"),
        &[],
    );
    assert_eq!(out.code, 0, "{}", out.stdout);
    assert!(out.stdout.contains("decrypted"));
    // Without the key some checks can't run: not a failure, but not "ok"
    // either, so `imag verify f && deploy` doesn't pass unchecked files.
    let out = imag(&["verify", s(&secret)], None, &[]);
    assert_eq!(out.code, 3, "{}", out.stdout);
    assert!(out.stdout.contains("Result: incomplete"), "{}", out.stdout);
    let out = imag(
        &["verify", "--password-stdin", s(&secret)],
        Some("wrong\n"),
        &[],
    );
    assert_eq!(out.code, 1, "{}", out.stdout);
    assert!(out.stdout.contains("FAIL  container"), "{}", out.stdout);

    // One flipped byte inside the snapshot: its checksum catches it.
    let mut bytes = std::fs::read(&plain).unwrap();
    let corrupt = dir.join("corrupt.imag");
    let middle = 36 + 116 + 40;
    bytes[middle] ^= 0xff;
    std::fs::write(&corrupt, &bytes).unwrap();
    let out = imag(&["verify", s(&corrupt)], None, &[]);
    assert_eq!(out.code, 1, "{}", out.stdout);
    assert!(out.stdout.contains("FAIL  records"), "{}", out.stdout);

    // An interrupted save leaves bytes after the last commit: a warning.
    let mut torn = std::fs::read(&plain).unwrap();
    torn.extend_from_slice(&[0x04, 0x01, 0x30, 0x00]);
    let torn_path = dir.join("torn.imag");
    std::fs::write(&torn_path, &torn).unwrap();
    let out = imag(&["verify", s(&torn_path)], None, &[]);
    assert!(out.stdout.contains("warn  records"), "{}", out.stdout);

    // A damaged header is a problem in the file, not a failure to run.
    let mut headless = std::fs::read(&plain).unwrap();
    headless[0] ^= 0xff;
    let headless_path = dir.join("headless.imag");
    std::fs::write(&headless_path, &headless).unwrap();
    let out = imag(&["verify", s(&headless_path)], None, &[]);
    assert_eq!(out.code, 1, "{}{}", out.stdout, out.stderr);
    assert!(out.stdout.contains("FAIL  records"), "{}", out.stdout);
}

#[test]
fn verify_tells_corruption_inside_the_file_from_an_interrupted_save() {
    // Vector 2: two commits. Damage the update of the second one: a later
    // commit follows, so this is corruption, not an interrupted save.
    let dir = scratch("midfile");
    let mut bytes = std::fs::read(vector("02-two-commits.imag")).unwrap();
    bytes[427 + 10] ^= 0xff;
    let path = dir.join("midfile.imag");
    std::fs::write(&path, &bytes).unwrap();
    let out = imag(&["verify", s(&path)], None, &[]);
    assert_eq!(out.code, 1, "{}", out.stdout);
    assert!(out.stdout.contains("FAIL  records"), "{}", out.stdout);
}

#[test]
fn verify_reports_each_unreadable_record_once() {
    // Damage META (the first record of vector 1). The other entries must
    // still be content-checked against their own records.
    let dir = scratch("meta");
    let mut bytes = std::fs::read(vector("01-minimal.imag")).unwrap();
    bytes[36 + 20] ^= 0xff;
    let path = dir.join("meta.imag");
    std::fs::write(&path, &bytes).unwrap();
    let out = imag(&["verify", s(&path)], None, &[]);
    assert_eq!(out.code, 1, "{}", out.stdout);
    let contents = out
        .stdout
        .lines()
        .find(|line| line.contains("contents"))
        .unwrap()
        .to_owned();
    assert_eq!(contents.matches("at 36").count(), 1, "{contents}");
    assert!(!contents.contains("SNAP at 36"), "{contents}");
}

#[test]
fn verify_checks_the_acl_chain_without_a_key() {
    // Key records are plaintext and signatures cover the stored bytes: an
    // unauthorized change is caught without the key.
    let out = imag(
        &["verify", s(&vector("10c-acl-unauthorized.imag"))],
        None,
        &[],
    );
    assert_eq!(out.code, 1, "{}", out.stdout);
    assert!(
        out.stdout.contains("not allowed to add a member"),
        "{}",
        out.stdout
    );
    let out = imag(&["verify", s(&vector("10a-acl-chain.imag"))], None, &[]);
    assert_eq!(out.code, 3, "{}", out.stdout);
    assert!(out.stdout.contains("ACL chain valid"), "{}", out.stdout);
}

#[test]
fn inspect_survives_hostile_segment_headers() {
    use imaginarium_format::{DocId, FileHeader, ObjectId, RecordBuilder, RecordType, Writer};
    let dir = scratch("segments");
    let mut writer = Writer::create(
        std::io::Cursor::new(Vec::new()),
        FileHeader::new(DocId([3; 16]), false),
    )
    .unwrap();
    let blob = RecordBuilder::new(RecordType::BLOB, ObjectId([4; 32]))
        .segment(u32::MAX, 1, 3, Vec::new())
        .build(&DocId([3; 16]), vec![1, 2, 3], None)
        .unwrap();
    writer.append(&blob).unwrap();
    writer.commit(None).unwrap();
    let path = dir.join("segments.imag");
    std::fs::write(&path, writer.into_inner().into_inner()).unwrap();
    let out = imag(&["inspect", s(&path)], None, &[]);
    assert_eq!(out.code, 0, "{}{}", out.stdout, out.stderr);
    assert!(
        out.stdout.contains("segment 4294967296/1"),
        "{}",
        out.stdout
    );
}

#[test]
fn verify_checks_the_acl_chain_of_the_test_vectors() {
    let dir = scratch("vectors");
    // The owner of vector 10 (docs/format/test-vectors/README.md): X-Wing seed 0x12…
    let owner = XWingKeyPair::from_seed(&[0x12; 32]);
    let key_file = dir.join("owner.key");
    let hex: String = owner
        .private_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    std::fs::write(&key_file, hex).unwrap();

    let out = imag(
        &[
            "verify",
            "--key-file",
            s(&key_file),
            s(&vector("10a-acl-chain.imag")),
        ],
        None,
        &[],
    );
    // Identity seeds 0x10… and 0x11… (same README).
    let owner_id: String = IdentityKey::from_seeds(&[0x10; 32], &[0x11; 32])
        .public()
        .key_id()
        .0
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let summary = format!("ACL chain valid (5 changes, 1 members, epoch 2, started by {owner_id})");
    assert!(out.stdout.contains(&summary), "{}", out.stdout);
    // Without --owner, nothing shows the chain is the document's real one
    // (ADR-0018): a warning that names the owner.
    assert!(out.stdout.contains("warn  signatures"), "{}", out.stdout);
    assert!(
        out.stdout.contains(&format!("pass --owner {owner_id}")),
        "{}",
        out.stdout
    );
    // The vectors hold placeholder bytes, not real Loro data: the document check fails.
    assert!(out.stdout.contains("FAIL  document"), "{}", out.stdout);

    let vector_10a = vector("10a-acl-chain.imag");
    let args = |owner: &str| {
        [
            "verify",
            "--key-file",
            s(&key_file),
            "--owner",
            owner,
            s(&vector_10a),
        ]
        .map(str::to_owned)
    };
    let pinned = args(&owner_id);
    let out = imag(&pinned.each_ref().map(String::as_str), None, &[]);
    assert!(out.stdout.contains("ok    signatures"), "{}", out.stdout);
    let wrong = args(&"ab".repeat(16));
    let out = imag(&wrong.each_ref().map(String::as_str), None, &[]);
    assert_eq!(out.code, 1);
    assert!(out.stdout.contains("expected owner"), "{}", out.stdout);

    let out = imag(
        &[
            "verify",
            "--key-file",
            s(&key_file),
            s(&vector("10b-acl-fork.imag")),
        ],
        None,
        &[],
    );
    assert_eq!(out.code, 1);
    assert!(out.stdout.contains("forked at index 1"), "{}", out.stdout);

    let out = imag(
        &[
            "verify",
            "--key-file",
            s(&key_file),
            s(&vector("10c-acl-unauthorized.imag")),
        ],
        None,
        &[],
    );
    assert_eq!(out.code, 1);
    assert!(out.stdout.contains("not allowed"), "{}", out.stdout);
}

#[test]
fn export_json_prints_the_decrypted_document() {
    let dir = scratch("export");
    let secret = dir.join("secret.imag");
    sample(&secret, password_protection());
    let out = imag(
        &["export-json", "--password-stdin", s(&secret)],
        Some("s3cret\n"),
        &[],
    );
    assert_eq!(out.code, 0, "{}", out.stderr);
    let json: serde_json::Value = serde_json::from_str(&out.stdout).unwrap();
    assert_eq!(json["encrypted"], true);
    assert_eq!(json["created_with"], "imag tests");
    assert_eq!(json["document"]["meta"]["schema_version"], 1);
    assert_eq!(json["document"]["meta"]["title"], "CLI sample");
    let out = imag(
        &["export-json", "--compact", "--password-stdin", s(&secret)],
        Some("s3cret\n"),
        &[],
    );
    assert_eq!(out.stdout.lines().count(), 1);
}

#[test]
fn bad_arguments_and_missing_keys_are_errors() {
    let out = imag(
        &[
            "export-json",
            "--password-env",
            "IMAG_TEST_UNSET_VAR",
            "x.imag",
        ],
        None,
        &[],
    );
    assert_eq!(out.code, 2);
    assert!(
        out.stderr.contains("IMAG_TEST_UNSET_VAR isn't set"),
        "{}",
        out.stderr
    );
    let out = imag(&["inspect", "/nonexistent/file.imag"], None, &[]);
    assert_eq!(out.code, 2);
    let out = imag(
        &["verify", "--password-stdin", "--key-file", "k", "x.imag"],
        None,
        &[],
    );
    assert_eq!(out.code, 2, "conflicting key options");
    let out = imag(&["--help"], None, &[]);
    assert_eq!(out.code, 0);
    assert!(out.stdout.contains("inspect") && out.stdout.contains("export-json"));
}

#[test]
fn a_closed_pipe_is_not_an_error() {
    let dir = scratch("pipe");
    let file = dir.join("plain.imag");
    sample(&file, Protection::None);
    let mut child = Command::new(env!("CARGO_BIN_EXE_imag"))
        .args(["export-json", s(&file)])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    drop(child.stdout.take()); // like `| head` closing early
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn export_says_when_it_exports_a_recovered_file() {
    let dir = scratch("export-recovered");
    let plain = dir.join("plain.imag");
    sample(&plain, Protection::None);
    let mut torn = std::fs::read(&plain).unwrap();
    torn.extend_from_slice(b"torn");
    let path = dir.join("torn.imag");
    std::fs::write(&path, &torn).unwrap();
    let out = imag(&["export-json", s(&path)], None, &[]);
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(
        out.stderr
            .contains("warning: the file's last commit is unusable"),
        "{}",
        out.stderr
    );
    assert!(out.stdout.contains("\"recovered\": \""), "{}", out.stdout);
    let out = imag(&["export-json", s(&plain)], None, &[]);
    assert!(out.stderr.is_empty(), "{}", out.stderr);
    assert!(out.stdout.contains("\"recovered\": null"), "{}", out.stdout);
}

#[test]
fn key_input_edge_cases() {
    let dir = scratch("key-input");
    let secret = dir.join("secret.imag");
    sample(&secret, password_protection());
    // Nothing on stdin is an error, not the empty password.
    let out = imag(&["verify", "--password-stdin", s(&secret)], Some(""), &[]);
    assert_eq!(out.code, 2, "{}{}", out.stdout, out.stderr);
    assert!(
        out.stderr.contains("no password on standard input"),
        "{}",
        out.stderr
    );

    // A key file saved with a byte-order mark (Windows editors) still reads.
    let owner = XWingKeyPair::from_seed(&[0x12; 32]);
    let hex: String = owner
        .private_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let key_file = dir.join("bom.key");
    std::fs::write(&key_file, format!("\u{feff}{hex}\r\n")).unwrap();
    let out = imag(
        &[
            "inspect",
            "--key-file",
            s(&key_file),
            s(&vector("10a-acl-chain.imag")),
        ],
        None,
        &[],
    );
    assert_eq!(out.code, 0, "{}{}", out.stdout, out.stderr);
}
