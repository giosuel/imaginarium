// SPDX-License-Identifier: AGPL-3.0-or-later

//! Saving into an opened file: updates appended and committed, snapshots,
//! recovery of a torn append, and the saving thread.

// Test helpers panic on failure by design.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::io::Write;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use imaginarium_io_native::{
    Autosave, Credentials, NativeError, Protection, SaveOptions, SaveState, Session, open_path,
    save_new_path,
};
use imaginarium_model::{Actor, Document, DocumentKind, Geometry, NodeKind, Parent};

fn scratch_dir(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/test-scratch")
        .join(format!("autosave-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn new_file(dir: &std::path::Path) -> PathBuf {
    let path = dir.join("board.imag");
    let document = Document::new(DocumentKind::Board).unwrap();
    save_new_path(
        &path,
        &document,
        &SaveOptions {
            protection: Protection::None,
            created_with: "imaginarium tests".into(),
        },
    )
    .unwrap();
    path
}

/// Adds a shape; returns the update bytes of that change.
fn add_shape(document: &mut Document, x: f64) -> Vec<u8> {
    let before = document.version();
    document
        .transact(&Actor::User, |t| {
            let node = t.create_node(Parent::Root, None, &NodeKind::Shape)?;
            t.set_geometry(node, Geometry::new(x, 0.0, 10.0, 10.0))
        })
        .unwrap();
    document.export_updates_since(&before).unwrap()
}

#[test]
fn every_change_reaches_the_file() {
    let dir = scratch_dir("changes");
    let path = new_file(&dir);
    let (opened, mut session) = Session::open(&path).unwrap();
    let mut document = opened.document;
    for n in 0..5 {
        let update = add_shape(&mut document, f64::from(n));
        session.append_update(update).unwrap();
        session.commit().unwrap();
    }
    assert_eq!(session.head_len(), 6, "the snapshot and five updates");
    let reopened = open_path(&path, Credentials::None).unwrap();
    assert_eq!(reopened.document.view(), document.view());
    assert!(reopened.recovery.is_none());
    // A snapshot starts the head again.
    session
        .append_snapshot(document.export_snapshot().unwrap())
        .unwrap();
    session.commit().unwrap();
    assert_eq!(session.head_len(), 1);
    assert_eq!(
        open_path(&path, Credentials::None).unwrap().document.view(),
        document.view()
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_torn_append_is_recovered_and_saving_goes_on() {
    let dir = scratch_dir("torn");
    let path = new_file(&dir);
    let (opened, mut session) = Session::open(&path).unwrap();
    let mut document = opened.document;
    session
        .append_update(add_shape(&mut document, 1.0))
        .unwrap();
    session.commit().unwrap();
    // A crash in the middle of the next append: half a record, no commit.
    drop(session);
    std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(b"IMG\x01half a record")
        .unwrap();
    let (opened, mut session) = Session::open(&path).unwrap();
    assert!(opened.recovery.is_some());
    let mut document = opened.document;
    // As the spec asks, a recovered file goes on from a snapshot.
    session
        .append_snapshot(document.export_snapshot().unwrap())
        .unwrap();
    session
        .append_update(add_shape(&mut document, 2.0))
        .unwrap();
    session.commit().unwrap();
    let reopened = open_path(&path, Credentials::None).unwrap();
    assert!(reopened.recovery.is_none());
    assert_eq!(reopened.document.view(), document.view());
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn encrypted_files_cant_be_saved_into_yet() {
    let dir = scratch_dir("encrypted");
    let path = dir.join("secret.imag");
    save_new_path(
        &path,
        &Document::new(DocumentKind::Board).unwrap(),
        &SaveOptions {
            protection: Protection::Password {
                password: zeroize::Zeroizing::new("pw".into()),
                argon2: imaginarium_crypto::Argon2Config {
                    memory_kib: 64,
                    iterations: 1,
                    parallelism: 1,
                },
            },
            created_with: "imaginarium tests".into(),
        },
    )
    .unwrap();
    assert!(matches!(
        Session::open(&path),
        Err(NativeError::CantSave(_))
    ));
    std::fs::remove_dir_all(&dir).unwrap();
}

fn wait_for(autosave: &Autosave, want: impl Fn(&SaveState) -> bool) -> SaveState {
    let start = Instant::now();
    loop {
        let state = autosave.state();
        if want(&state) || start.elapsed() > Duration::from_secs(10) {
            return state;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn the_saving_thread_writes_updates_and_says_when_it_is_done() {
    let dir = scratch_dir("thread");
    let path = new_file(&dir);
    let (opened, session) = Session::open(&path).unwrap();
    let mut document = opened.document;
    let woken = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let count = woken.clone();
    let autosave = Autosave::start(session, path.clone(), move || {
        count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    });
    for n in 0..20 {
        autosave.update(add_shape(&mut document, f64::from(n)));
    }
    assert_eq!(
        wait_for(&autosave, |state| *state == SaveState::Saved),
        SaveState::Saved
    );
    assert!(woken.load(std::sync::atomic::Ordering::Relaxed) >= 1);
    // What's sent just before finishing is written too.
    autosave.update(add_shape(&mut document, 99.0));
    assert_eq!(autosave.finish(), None);
    assert_eq!(
        open_path(&path, Credentials::None).unwrap().document.view(),
        document.view()
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Storage that fails every write while `failing` is set (a full disk).
struct Flaky {
    bytes: std::sync::Arc<std::sync::Mutex<Vec<u8>>>,
    at: u64,
    failing: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl Flaky {
    fn check(&self) -> std::io::Result<()> {
        if self.failing.load(std::sync::atomic::Ordering::Relaxed) {
            Err(std::io::Error::other("the disk is full"))
        } else {
            Ok(())
        }
    }
}

impl std::io::Read for Flaky {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let bytes = self.bytes.lock().unwrap();
        let start = usize::try_from(self.at).unwrap().min(bytes.len());
        let n = buf.len().min(bytes.len() - start);
        buf[..n].copy_from_slice(&bytes[start..start + n]);
        self.at += n as u64;
        Ok(n)
    }
}

impl std::io::Write for Flaky {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.check()?;
        let mut bytes = self.bytes.lock().unwrap();
        let start = usize::try_from(self.at).unwrap();
        if bytes.len() < start + buf.len() {
            bytes.resize(start + buf.len(), 0);
        }
        bytes[start..start + buf.len()].copy_from_slice(buf);
        self.at += buf.len() as u64;
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.check()
    }
}

impl std::io::Seek for Flaky {
    fn seek(&mut self, pos: std::io::SeekFrom) -> std::io::Result<u64> {
        let len = self.bytes.lock().unwrap().len() as u64;
        self.at = match pos {
            std::io::SeekFrom::Start(at) => at,
            std::io::SeekFrom::End(back) => len.saturating_add_signed(back),
            std::io::SeekFrom::Current(by) => self.at.saturating_add_signed(by),
        };
        Ok(self.at)
    }
}

impl imaginarium_format::Durable for Flaky {
    fn sync(&mut self) -> std::io::Result<()> {
        self.check()
    }
    fn set_len(&mut self, len: u64) -> std::io::Result<()> {
        self.check()?;
        self.bytes
            .lock()
            .unwrap()
            .resize(usize::try_from(len).unwrap(), 0);
        Ok(())
    }
}

#[test]
fn a_failed_save_is_retried_until_it_works() {
    let document = Document::new(DocumentKind::Board).unwrap();
    let saved = imaginarium_io_native::save_new(
        std::io::Cursor::new(Vec::new()),
        &document,
        &SaveOptions {
            protection: Protection::None,
            created_with: "imaginarium tests".into(),
        },
    )
    .unwrap()
    .into_inner();
    let bytes = std::sync::Arc::new(std::sync::Mutex::new(saved));
    let failing = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
    let storage = {
        let (bytes, failing) = (bytes.clone(), failing.clone());
        move || Flaky {
            bytes: bytes.clone(),
            at: 0,
            failing: failing.clone(),
        }
    };
    let (opened, session) = Session::from_file(storage()).unwrap();
    let mut document = opened.document;
    let reopens = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counted = reopens.clone();
    let autosave = Autosave::start_with(
        session,
        move || {
            counted.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            Session::from_file(storage()).map(|(_, session)| session)
        },
        || {},
    );
    autosave.update(add_shape(&mut document, 1.0));
    assert!(matches!(
        wait_for(&autosave, |state| matches!(state, SaveState::Failed(_))),
        SaveState::Failed(_)
    ));
    // The disk has room again: the next try writes the change.
    failing.store(false, std::sync::atomic::Ordering::Relaxed);
    assert_eq!(
        wait_for(&autosave, |state| *state == SaveState::Saved),
        SaveState::Saved
    );
    assert!(reopens.load(std::sync::atomic::Ordering::Relaxed) >= 1);
    assert_eq!(autosave.finish(), None);
    let reopened = imaginarium_io_native::open(
        std::io::Cursor::new(bytes.lock().unwrap().clone()),
        Credentials::None,
    )
    .unwrap();
    assert_eq!(reopened.document.view(), document.view());
}

/// Writes an empty board to `$IMAGINARIUM_SAMPLE` (for trying the app by
/// hand): `IMAGINARIUM_SAMPLE=board.imag cargo test -p imaginarium-io-native
/// --test autosave write_a_sample -- --ignored`.
#[test]
#[ignore = "writes a file for trying the app"]
fn write_a_sample() {
    let path = PathBuf::from(std::env::var_os("IMAGINARIUM_SAMPLE").unwrap());
    save_new_path(
        &path,
        &Document::new(DocumentKind::Board).unwrap(),
        &SaveOptions {
            protection: Protection::None,
            created_with: "imaginarium tests".into(),
        },
    )
    .unwrap();
}
