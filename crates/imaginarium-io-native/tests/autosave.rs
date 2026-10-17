// SPDX-License-Identifier: AGPL-3.0-or-later

//! Saving into an opened file: updates appended and committed, snapshots,
//! compaction, recovery, locking and read-only files, and the saving
//! thread with its retries.

// Test helpers panic on failure by design.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use imaginarium_format::{ObjectId, RecordBuilder, RecordFlags, RecordType, Writer};
use imaginarium_io_native::{
    Autosave, Credentials, FileId, NativeError, Opening, Protection, SaveOptions, SaveState,
    Session, Storage, open_path, save_new, save_new_path,
};
use imaginarium_model::{Actor, Document, DocumentKind, Geometry, Mode, NodeKind, Parent};

/// A fresh directory under the workspace's `target/` (never `/tmp`).
fn scratch_dir(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/test-scratch")
        .join(format!("autosave-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn options() -> SaveOptions {
    SaveOptions {
        protection: Protection::None,
        created_with: "imaginarium tests".into(),
    }
}

fn new_file(dir: &Path) -> PathBuf {
    let path = dir.join("board.imag");
    save_new_path(
        &path,
        &Document::new(DocumentKind::Board).unwrap(),
        &options(),
    )
    .unwrap();
    path
}

/// Opens `path` to edit it; panics if it opens read-only.
fn editable(path: &Path) -> (Document, Session) {
    match Session::open(path).unwrap() {
        Opening::Editable(opened, session) => (opened.document, *session),
        Opening::ReadOnly(opened) => panic!("read-only: {:?}", opened.document.mode()),
    }
}

fn file_id(path: &Path, session: &Session) -> FileId {
    FileId {
        path: path.to_owned(),
        doc_id: session.doc_id(),
    }
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

fn reopened(path: &Path) -> Document {
    open_path(path, Credentials::None).unwrap().document
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
fn every_change_reaches_the_file_and_a_snapshot_shortens_the_head() {
    let dir = scratch_dir("changes");
    let path = new_file(&dir);
    let (mut document, mut session) = editable(&path);
    for n in 0..5 {
        session
            .append_update(add_shape(&mut document, f64::from(n)))
            .unwrap();
        session.commit().unwrap();
    }
    assert_eq!(session.head_len(), 6, "the snapshot and five updates");
    assert_eq!(reopened(&path).view(), document.view());
    session
        .append_snapshot(document.export_snapshot().unwrap())
        .unwrap();
    session.commit().unwrap();
    assert_eq!(session.head_len(), 1);
    // The old head left the index: it lists only what is live.
    let (live, total) = session.live_and_total();
    assert!(live < total, "{live} of {total}");
    drop(session);
    assert_eq!(reopened(&path).view(), document.view());
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_file_open_elsewhere_opens_read_only() {
    let dir = scratch_dir("locked");
    let path = new_file(&dir);
    let (_document, session) = editable(&path);
    let Opening::ReadOnly(second) = Session::open(&path).unwrap() else {
        panic!("a second writer");
    };
    assert!(matches!(second.document.mode(), Mode::ReadOnly(_)));
    // Free again once the first one is closed.
    drop(session);
    let _ = editable(&path);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[cfg(unix)]
#[test]
fn a_file_that_cant_be_written_opens_read_only() {
    use std::os::unix::fs::PermissionsExt;
    let dir = scratch_dir("permission");
    let path = new_file(&dir);
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o444)).unwrap();
    // Root may write anyway: then there is nothing to test.
    let root = std::fs::OpenOptions::new().write(true).open(&path).is_ok();
    let opening = Session::open(&path).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    if !root {
        let Opening::ReadOnly(opened) = opening else {
            panic!("a file that can't be written opened for writing");
        };
        assert!(matches!(opened.document.mode(), Mode::ReadOnly(_)));
    }
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_file_with_records_this_version_doesnt_know_opens_read_only() {
    let dir = scratch_dir("unknown");
    let path = new_file(&dir);
    let (_, session) = editable(&path);
    drop(session);
    // A newer version wrote a record type this one doesn't know, which
    // readers must understand to edit (CRITICAL, spec §10.2).
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .unwrap();
    let reader = imaginarium_format::Reader::open(file).unwrap();
    let doc_id = reader.header().doc_id;
    let mut writer = Writer::resume(reader).unwrap();
    let record = RecordBuilder::new(RecordType(0x4100), ObjectId::random().unwrap())
        .extension_id("org.example.newer")
        .flags(RecordFlags::CRITICAL)
        .build(&doc_id, b"newer data".to_vec(), None)
        .unwrap();
    writer.append(&record).unwrap();
    writer.commit(None).unwrap();
    drop(writer);
    let before = std::fs::read(&path).unwrap();
    let Opening::ReadOnly(opened) = Session::open(&path).unwrap() else {
        panic!("opened for writing");
    };
    assert!(matches!(opened.document.mode(), Mode::ReadOnly(_)));
    drop(opened);
    assert_eq!(std::fs::read(&path).unwrap(), before, "never written");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_change_appended_but_not_committed_survives_a_crash() {
    let dir = scratch_dir("uncommitted");
    let path = new_file(&dir);
    let (mut document, mut session) = editable(&path);
    session
        .append_update(add_shape(&mut document, 1.0))
        .unwrap();
    session.commit().unwrap();
    // A crash after the record, before its commit: the record is
    // complete, the index doesn't list it.
    session
        .append_update(add_shape(&mut document, 2.0))
        .unwrap();
    drop(session);
    let (recovered, mut session) = editable(&path);
    assert!(session.recovered());
    assert_eq!(
        recovered.view(),
        document.view(),
        "the newest change is kept"
    );
    session.commit().unwrap();
    drop(session);
    // And a torn half record after it is dropped.
    std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(b"IMG\x01half a record")
        .unwrap();
    let (again, _) = editable(&path);
    assert_eq!(again.view(), document.view());
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

#[test]
fn the_saving_thread_writes_updates_and_says_when_it_is_done() {
    let dir = scratch_dir("thread");
    let path = new_file(&dir);
    let (mut document, session) = editable(&path);
    let woken = Arc::new(AtomicUsize::new(0));
    let count = woken.clone();
    let id = file_id(&path, &session);
    let autosave = Autosave::start(session, id, move || {
        count.fetch_add(1, Ordering::Relaxed);
    });
    for n in 0..20 {
        autosave.update(add_shape(&mut document, f64::from(n)));
    }
    assert_eq!(
        wait_for(&autosave, |state| *state == SaveState::Saved),
        SaveState::Saved
    );
    assert!(woken.load(Ordering::Relaxed) >= 1);
    // What's sent just before finishing is written too.
    autosave.update(add_shape(&mut document, 99.0));
    assert_eq!(autosave.finish_in_background().join().unwrap(), None);
    assert_eq!(reopened(&path).view(), document.view());
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn many_saves_keep_the_file_small_and_quick_to_open() {
    let dir = scratch_dir("growth");
    let path = new_file(&dir);
    let (mut document, session) = editable(&path);
    let id = file_id(&path, &session);
    let autosave = Autosave::start(session, id, || {});
    // A thousand changes, each saved on its own (as when edits come
    // slowly): snapshots keep the head short, compaction the file small.
    for n in 0..1000 {
        autosave.update(add_shape(&mut document, f64::from(n % 50)));
        wait_for(&autosave, |state| *state == SaveState::Saved);
    }
    assert_eq!(autosave.finish(), None);
    let size = std::fs::metadata(&path).unwrap().len();
    let snapshot = document.export_snapshot().unwrap().len() as u64;
    assert!(
        size < 2 * 1024 * 1024,
        "{size} bytes (a snapshot is {snapshot})"
    );
    let (opened, session) = editable(&path);
    assert_eq!(opened.view(), document.view());
    assert!(session.head_len() <= imaginarium_io_native::SNAPSHOT_AFTER + 1);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[cfg(unix)]
#[test]
fn a_deleted_file_is_a_failed_save_not_a_silent_one() {
    let dir = scratch_dir("deleted");
    let path = new_file(&dir);
    let (mut document, session) = editable(&path);
    let id = file_id(&path, &session);
    let autosave = Autosave::start(session, id, || {});
    std::fs::remove_file(&path).unwrap();
    autosave.update(add_shape(&mut document, 1.0));
    assert!(matches!(
        wait_for(&autosave, |state| matches!(state, SaveState::Failed(_))),
        SaveState::Failed(_)
    ));
    assert!(autosave.finish().is_some(), "closing says it failed");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn another_document_in_the_files_place_is_never_written() {
    let dir = scratch_dir("replaced");
    let path = new_file(&dir);
    let (_, session) = editable(&path);
    let id = file_id(&path, &session);
    drop(session);
    // Something else saved another board over it.
    save_new_path(
        &path,
        &Document::new(DocumentKind::Board).unwrap(),
        &options(),
    )
    .unwrap();
    let before = std::fs::read(&path).unwrap();
    assert!(matches!(
        Session::reopen(&id.path, &id.doc_id),
        Err(NativeError::CantSave(_))
    ));
    assert_eq!(std::fs::read(&path).unwrap(), before);
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Storage in memory that fails writes while `failing` is set, after
/// `fail_after` more bytes (a disk that fills up in the middle of a write).
struct Flaky {
    bytes: Arc<Mutex<Vec<u8>>>,
    at: u64,
    failing: Arc<AtomicBool>,
    fail_after: Arc<AtomicU64>,
}

impl Flaky {
    fn check(&self, len: usize) -> std::io::Result<usize> {
        if !self.failing.load(Ordering::Relaxed) {
            return Ok(len);
        }
        let room = self.fail_after.load(Ordering::Relaxed);
        if room == 0 {
            return Err(std::io::Error::other("the disk is full"));
        }
        let n = len.min(usize::try_from(room).unwrap());
        self.fail_after.fetch_sub(n as u64, Ordering::Relaxed);
        Ok(n)
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
        let n = self.check(buf.len())?;
        let mut bytes = self.bytes.lock().unwrap();
        let start = usize::try_from(self.at).unwrap();
        if bytes.len() < start + n {
            bytes.resize(start + n, 0);
        }
        bytes[start..start + n].copy_from_slice(&buf[..n]);
        self.at += n as u64;
        Ok(n)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.check(0).map(|_| ())
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
        self.check(0).map(|_| ())
    }
    fn set_len(&mut self, len: u64) -> std::io::Result<()> {
        self.check(0)?;
        self.bytes
            .lock()
            .unwrap()
            .resize(usize::try_from(len).unwrap(), 0);
        Ok(())
    }
}

impl Storage for Flaky {
    fn gone(_session: &Session<Self>) -> bool {
        false
    }

    fn compaction_unsafe(_session: &Session<Self>) -> bool {
        true
    }
}

#[test]
fn a_save_that_breaks_off_halfway_is_retried_as_one_snapshot() {
    let saved = save_new(
        std::io::Cursor::new(Vec::new()),
        &Document::new(DocumentKind::Board).unwrap(),
        &options(),
    )
    .unwrap()
    .into_inner();
    let bytes = Arc::new(Mutex::new(saved));
    let failing = Arc::new(AtomicBool::new(true));
    // The disk fills up 100 bytes into the next write.
    let fail_after = Arc::new(AtomicU64::new(100));
    let storage = {
        let (bytes, failing, fail_after) = (bytes.clone(), failing.clone(), fail_after.clone());
        move || Flaky {
            bytes: bytes.clone(),
            at: 0,
            failing: failing.clone(),
            fail_after: fail_after.clone(),
        }
    };
    let (opened, session) = Session::from_file(storage()).unwrap();
    let mut document = opened.document;
    let session = session.unwrap();
    let reopens = Arc::new(AtomicUsize::new(0));
    let counted = reopens.clone();
    let autosave = Autosave::start_with(
        session,
        move || {
            counted.fetch_add(1, Ordering::Relaxed);
            Session::from_file(storage()).map(|(_, session)| session.unwrap())
        },
        None,
        || {},
    );
    for n in 0..3 {
        autosave.update(add_shape(&mut document, f64::from(n)));
    }
    assert!(matches!(
        wait_for(&autosave, |state| matches!(state, SaveState::Failed(_))),
        SaveState::Failed(_)
    ));
    // The disk has room again: the retry writes everything.
    failing.store(false, Ordering::Relaxed);
    assert_eq!(
        wait_for(&autosave, |state| *state == SaveState::Saved),
        SaveState::Saved
    );
    assert_eq!(autosave.finish(), None);
    let reopened = imaginarium_io_native::open(
        std::io::Cursor::new(bytes.lock().unwrap().clone()),
        Credentials::None,
    )
    .unwrap();
    assert_eq!(reopened.document.view(), document.view());
    // One snapshot holds it all (the broken-off write left no duplicates
    // in the head).
    let mut storage = Flaky {
        bytes: bytes.clone(),
        at: 0,
        failing: Arc::new(AtomicBool::new(false)),
        fail_after: Arc::new(AtomicU64::new(0)),
    };
    let _ = std::io::Seek::rewind(&mut storage);
    let (_, session) = Session::from_file(storage).unwrap();
    assert_eq!(session.unwrap().head_len(), 1);
    let _ = reopens;
}

#[cfg(unix)]
#[test]
fn a_retry_merges_what_another_writer_put_in_the_file() {
    let dir = scratch_dir("merge");
    let path = new_file(&dir);
    let (mut document, session) = editable(&path);
    let id = file_id(&path, &session);
    let autosave = Autosave::start(session, id, || {});
    autosave.update(add_shape(&mut document, 1.0));
    wait_for(&autosave, |state| *state == SaveState::Saved);
    // Another device's copy gets a change of its own and a sync tool puts
    // it in the file's place.
    let copy = dir.join("synced.imag");
    std::fs::copy(&path, &copy).unwrap();
    {
        let (mut other, mut session) = editable(&copy);
        session.append_update(add_shape(&mut other, 50.0)).unwrap();
        session.commit().unwrap();
    }
    // First the file is missing (a folder in its place): the next save
    // fails, and its retry will write one snapshot.
    let aside = dir.join("aside.imag");
    std::fs::rename(&path, &aside).unwrap();
    std::fs::create_dir(&path).unwrap();
    autosave.update(add_shape(&mut document, 2.0));
    assert!(matches!(
        wait_for(&autosave, |state| matches!(state, SaveState::Failed(_))),
        SaveState::Failed(_)
    ));
    // Then the sync tool puts the other device's copy there: the retry
    // merges its change into the snapshot instead of writing over it.
    std::fs::remove_dir(&path).unwrap();
    std::fs::rename(&copy, &path).unwrap();
    assert_eq!(
        wait_for(&autosave, |state| *state == SaveState::Saved),
        SaveState::Saved
    );
    assert_eq!(autosave.finish(), None);
    assert_eq!(
        reopened(&path).view().node_count(),
        3,
        "both writers' shapes"
    );
    std::fs::remove_dir_all(&dir).unwrap();
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
        &options(),
    )
    .unwrap();
}

/// A new board's snapshot, for keeping it.
fn new_board() -> (Document, Vec<u8>) {
    let document = Document::new(DocumentKind::Board).unwrap();
    let snapshot = document.export_snapshot().unwrap();
    (document, snapshot)
}

#[test]
fn an_untitled_board_is_kept_in_a_file_made_with_its_first_change() {
    let dir = scratch_dir("kept");
    let folder = dir.join("recovery");
    let path = folder.join("untitled.imag");
    let (mut document, snapshot) = new_board();
    let autosave = Autosave::create(path.clone(), &snapshot, "tests".into(), || {});
    // Nothing yet: an empty board leaves nothing behind.
    std::thread::sleep(Duration::from_millis(50));
    assert!(!folder.exists());
    autosave.update(add_shape(&mut document, 0.0));
    assert_eq!(
        wait_for(&autosave, |s| *s == SaveState::Saved),
        SaveState::Saved
    );
    assert_eq!(reopened(&path).view().node_count(), 1);
    // Later changes go into the same file.
    autosave.update(add_shape(&mut document, 20.0));
    assert!(autosave.finish().is_none());
    assert_eq!(reopened(&path).view().node_count(), 2);
}

#[test]
fn a_discarded_board_leaves_no_file_and_writes_nothing_more() {
    let dir = scratch_dir("discard");
    let path = dir.join("untitled.imag");
    let (mut document, snapshot) = new_board();
    let autosave = Autosave::create(path.clone(), &snapshot, "tests".into(), || {});
    autosave.update(add_shape(&mut document, 0.0));
    wait_for(&autosave, |s| *s == SaveState::Saved);
    assert!(path.exists());
    autosave.update(add_shape(&mut document, 20.0));
    assert!(autosave.discard_in_background().join().unwrap().is_none());
    assert!(!path.exists());
    // Discarding a board that never changed: nothing to delete, no error.
    let (_, snapshot) = new_board();
    let untouched = Autosave::create(dir.join("other.imag"), &snapshot, "tests".into(), || {});
    assert!(untouched.discard_in_background().join().unwrap().is_none());
}

#[test]
fn boards_left_behind_are_found_but_not_those_still_open() {
    let dir = scratch_dir("found");
    let (mut first, snapshot) = new_board();
    let left = Autosave::create(dir.join("left.imag"), &snapshot, "tests".into(), || {});
    left.update(add_shape(&mut first, 0.0));
    // The app "crashes": the saving stops, the file stays.
    assert!(left.finish().is_none());
    let (mut second, snapshot) = new_board();
    let open = Autosave::create(dir.join("open.imag"), &snapshot, "tests".into(), || {});
    open.update(add_shape(&mut second, 0.0));
    wait_for(&open, |s| *s == SaveState::Saved);
    // Neither lock files nor temporary files count.
    std::fs::write(dir.join(".~left.imag.lock"), "").unwrap();
    std::fs::write(dir.join(".left.imag.1234.tmp"), "").unwrap();
    let kept = imaginarium_io_native::kept_boards(&dir);
    let names: Vec<_> = kept
        .iter()
        .map(|kept| kept.path.file_name().unwrap().to_owned())
        .collect();
    assert_eq!(names, ["left.imag"]);
    assert_eq!(kept[0].opened.document.view().node_count(), 1);
    // Found means held: a second look (another app) doesn't take it too.
    assert!(imaginarium_io_native::kept_boards(&dir).is_empty());
    drop(kept);
    assert_eq!(imaginarium_io_native::kept_boards(&dir).len(), 1);
    assert!(imaginarium_io_native::kept_boards(&dir.join("missing")).is_empty());
    drop(open);
}

#[test]
fn a_board_that_cant_be_kept_yet_is_tried_again() {
    let dir = scratch_dir("blocked");
    // A file where the folder should be.
    let blocked = dir.join("recovery");
    std::fs::write(&blocked, "").unwrap();
    let path = blocked.join("untitled.imag");
    let (mut document, snapshot) = new_board();
    let autosave = Autosave::create(path.clone(), &snapshot, "tests".into(), || {});
    autosave.update(add_shape(&mut document, 0.0));
    assert!(matches!(
        wait_for(&autosave, |s| matches!(s, SaveState::Failed(_))),
        SaveState::Failed(_)
    ));
    std::fs::remove_file(&blocked).unwrap();
    autosave.update(add_shape(&mut document, 20.0));
    let start = Instant::now();
    while wait_for(&autosave, |s| *s == SaveState::Saved) != SaveState::Saved
        && start.elapsed() < Duration::from_secs(10)
    {}
    assert_eq!(autosave.state(), SaveState::Saved);
    assert_eq!(reopened(&path).view().node_count(), 2);
}
