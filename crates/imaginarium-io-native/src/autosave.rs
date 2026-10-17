// SPDX-License-Identifier: AGPL-3.0-or-later

//! Saving on a thread of its own (MAINTAINERS.md invariant 3).
//!
//! The UI sends each change's update bytes; the thread:
//! - appends them as `UPDT` records and commits them ([`Session`]),
//!   everything waiting in one commit;
//! - keeps a copy of the document, from which it writes a snapshot once the
//!   head has [`SNAPSHOT_AFTER`] records (so the file opens fast), and after
//!   a recovery or a failed write;
//! - compacts the file when most of it is superseded (spec §9.4), unless
//!   that could lose something;
//! - says how it goes ([`SaveState`]) and wakes the UI when that changes.
//!
//! A snapshot never drops what is in the file: before one, the thread
//! merges the file's head into its copy (another writer may have changed
//! the file while this one couldn't write). A failed save is retried every
//! [`RETRY_AFTER`], on the same handle (and lock) when the file is still
//! there, else by opening the path again, which must still hold the same
//! document.

use std::fs::File;
use std::io::{Read, Seek, Write};
use std::panic::AssertUnwindSafe;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use imaginarium_format::Durable;
use imaginarium_model::{Document, OpenOptions};

use crate::{FileId, NativeError, Session};

/// How a document's file is doing.
#[derive(Clone, Debug, PartialEq)]
pub enum SaveState {
    /// Every change sent so far is in the file.
    Saved,
    /// Changes are being written, since then.
    Saving(Instant),
    /// The last write failed, for this reason; it is retried.
    Failed(String),
}

/// How long to wait before trying a failed save again.
pub const RETRY_AFTER: Duration = Duration::from_secs(2);

/// A snapshot instead of more updates once the head has this many records.
pub const SNAPSHOT_AFTER: usize = 200;

/// Compaction once the superseded part is at least this much, and twice
/// the live part.
const COMPACT_FROM: u64 = 1 << 20;

/// What the saving thread is sent.
enum Job {
    Update(Vec<u8>),
    Stop,
}

/// The storage a saving thread writes to, and what only a real file can
/// say about itself.
pub trait Storage: Read + Write + Seek + Durable + Send + 'static {
    /// Whether the file was deleted or replaced while open.
    fn gone(session: &Session<Self>) -> bool
    where
        Self: Sized;

    /// Whether compacting it could lose something.
    fn compaction_unsafe(session: &Session<Self>) -> bool
    where
        Self: Sized;
}

impl Storage for File {
    fn gone(session: &Session<Self>) -> bool {
        session.file_gone()
    }

    fn compaction_unsafe(session: &Session<Self>) -> bool {
        session.compaction_unsafe()
    }
}

/// Opens the storage again by its name (after it was replaced, or
/// compacted).
type Reopen<F> = Box<dyn FnMut() -> crate::Result<Session<F>> + Send>;

/// Compacts the storage (for files): `None` where it can't.
pub type Compact = Option<Box<dyn FnMut() -> crate::Result<()> + Send>>;

/// The saving thread of one open document.
pub struct Autosave {
    sender: Sender<Job>,
    shared: Arc<Mutex<Shared>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

/// What the UI and the saving thread share, under one lock (Saved must
/// never be set while an update is on its way).
struct Shared {
    state: SaveState,
    /// Updates sent and written: Saved only when they are equal.
    sent: u64,
    written: u64,
}

impl std::fmt::Debug for Autosave {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Autosave")
            .field("state", &self.state())
            .finish_non_exhaustive()
    }
}

impl Autosave {
    /// Saves into `session`, the file `id`; `notify` is called (from the
    /// saving thread) when the state changes, to wake the UI.
    pub fn start(session: Session, id: FileId, notify: impl Fn() + Send + Sync + 'static) -> Self {
        let reopen_id = id.clone();
        let compact_path = id.path;
        Self::start_with(
            session,
            move || Session::reopen(&reopen_id.path, &reopen_id.doc_id),
            Some(Box::new(move || {
                imaginarium_format::compact_file(
                    &compact_path,
                    None,
                    imaginarium_format::CompactionPlan::default(),
                    None,
                )
                .map(|_| ())
                .map_err(NativeError::from)
            })),
            notify,
        )
    }

    /// [`Autosave::start`] on any storage: `reopen` opens it again by its
    /// name, `compact` compacts it.
    pub fn start_with<F: Storage>(
        mut session: Session<F>,
        reopen: impl FnMut() -> crate::Result<Session<F>> + Send + 'static,
        compact: Compact,
        notify: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        let (sender, receiver) = mpsc::channel();
        let shared = Arc::new(Mutex::new(Shared {
            state: SaveState::Saved,
            sent: 0,
            written: 0,
        }));
        let thread_shared = shared.clone();
        let parts = session.take_parts();
        let thread = std::thread::Builder::new()
            .name("autosave".into())
            .spawn(move || {
                let crashed = thread_shared.clone();
                let notify: Arc<dyn Fn() + Send + Sync> = Arc::new(notify);
                let wake = notify.clone();
                let ran = std::panic::catch_unwind(AssertUnwindSafe(move || {
                    let mut saver = Saver {
                        session: Some(session),
                        broken: false,
                        replica: replica(&parts),
                        reopen: Box::new(reopen),
                        compact,
                        pending: Vec::new(),
                        snapshot_due: false,
                        retry_at: None,
                        shared: thread_shared,
                        notify: wake,
                    };
                    saver.run(&receiver);
                }));
                // A bug in saving must not look like a save.
                if ran.is_err() {
                    set(
                        &crashed,
                        SaveState::Failed("saving stopped after an error".into()),
                    );
                    notify();
                }
            });
        let thread = match thread {
            Ok(thread) => Some(thread),
            Err(err) => {
                set(
                    &shared,
                    SaveState::Failed(format!("no thread to save with: {err}")),
                );
                None
            }
        };
        Self {
            sender,
            shared,
            thread,
        }
    }

    /// Saves an update of the `main` stream.
    pub fn update(&self, update: Vec<u8>) {
        if let Ok(mut shared) = self.shared.lock() {
            shared.sent += 1;
            if shared.state == SaveState::Saved {
                shared.state = SaveState::Saving(Instant::now());
            }
        }
        if self.sender.send(Job::Update(update)).is_err() {
            set(&self.shared, SaveState::Failed("saving stopped".into()));
        }
    }

    /// How saving goes now.
    pub fn state(&self) -> SaveState {
        self.shared.lock().map_or_else(
            |_| SaveState::Failed("saving stopped".into()),
            |shared| shared.state.clone(),
        )
    }

    /// Writes what is pending and stops; then why the last changes aren't
    /// all in the file, if they aren't (a failing save is tried once more,
    /// not again and again). Waits for the disk:
    /// [`Autosave::finish_in_background`] doesn't.
    pub fn finish(mut self) -> Option<String> {
        let joined = self.stop();
        match self.state() {
            _ if !joined => Some("saving stopped after an error".into()),
            SaveState::Failed(reason) => Some(reason),
            // Stopped without writing everything (it can't, normally).
            SaveState::Saving(_) => Some("the last changes may not be in the file".into()),
            SaveState::Saved => None,
        }
    }

    /// [`Autosave::finish`] on a thread of its own, so closing a window
    /// doesn't wait for the disk. Join it before the program exits.
    pub fn finish_in_background(self) -> std::thread::JoinHandle<Option<String>> {
        std::thread::Builder::new()
            .name("autosave-finish".into())
            .spawn(move || self.finish())
            .unwrap_or_else(|_| std::thread::spawn(|| Some("no thread to finish saving".into())))
    }

    /// Stops the thread; whether it ended well.
    fn stop(&mut self) -> bool {
        let _ = self.sender.send(Job::Stop);
        self.thread
            .take()
            .is_none_or(|thread| thread.join().is_ok())
    }
}

impl Drop for Autosave {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Sets the state; whether it changed.
fn set(shared: &Mutex<Shared>, new: SaveState) -> bool {
    shared.lock().is_ok_and(|mut shared| {
        let changed = shared.state != new;
        shared.state = new;
        changed
    })
}

/// A copy of the document, from the file's head, for snapshots. It never
/// runs migrations of its own: the UI's document sends them.
fn replica(parts: &[Vec<u8>]) -> Option<Document> {
    let slices: Vec<&[u8]> = parts.iter().map(Vec::as_slice).collect();
    Document::open_with(
        &slices,
        &OpenOptions {
            migrations: &[],
            ..OpenOptions::default()
        },
    )
    .ok()
}

/// The saving thread's state.
struct Saver<F: Storage> {
    session: Option<Session<F>>,
    /// The last write failed: the session's state isn't trusted.
    broken: bool,
    /// The copy of the document (`None` if it couldn't follow: then only
    /// updates are written, never a snapshot).
    replica: Option<Document>,
    reopen: Reopen<F>,
    compact: Compact,
    /// Updates not yet in the file.
    pending: Vec<Vec<u8>>,
    /// The next write is a snapshot (after a recovery or a failure).
    snapshot_due: bool,
    /// After a failure: when to try again.
    retry_at: Option<Instant>,
    shared: Arc<Mutex<Shared>>,
    notify: Arc<dyn Fn() + Send + Sync>,
}

impl<F: Storage> Saver<F> {
    fn run(&mut self, receiver: &Receiver<Job>) {
        self.snapshot_due = self.session.as_ref().is_some_and(Session::recovered);
        if self.snapshot_due {
            self.save();
        }
        let mut stopping = false;
        loop {
            let first = match self.retry_at {
                None => receiver.recv().unwrap_or(Job::Stop),
                Some(at) => {
                    match receiver.recv_timeout(at.saturating_duration_since(Instant::now())) {
                        Ok(job) => job,
                        Err(RecvTimeoutError::Timeout) => {
                            self.retry_at = None;
                            self.save();
                            continue;
                        }
                        Err(RecvTimeoutError::Disconnected) => Job::Stop,
                    }
                }
            };
            for job in std::iter::once(first).chain(receiver.try_iter()) {
                match job {
                    Job::Stop => stopping = true,
                    Job::Update(update) => {
                        // The copy follows every update, so a snapshot of
                        // it holds what was sent.
                        self.follow(&update);
                        self.pending.push(update);
                    }
                }
            }
            // While a failure waits for its retry, new changes wait too;
            // at exit what is pending is tried once, not forever.
            if self.retry_at.is_none() || stopping {
                self.save();
            }
            if stopping {
                break;
            }
        }
    }

    /// Takes `bytes` (an update, or a file's head) into the copy; a copy
    /// that can't follow is dropped.
    fn follow(&mut self, bytes: &[u8]) {
        if let Some(replica) = &mut self.replica
            && replica.apply_remote(&[bytes]).is_err()
        {
            self.replica = None;
        }
    }

    /// Writes what is pending; says how it went.
    fn save(&mut self) {
        if self.pending.is_empty() && !self.snapshot_due {
            return;
        }
        let count = self.pending.len() as u64;
        let result = self.write();
        let failed = match result {
            Ok(()) => {
                self.pending.clear();
                self.retry_at = None;
                None
            }
            Err(reason) => {
                // The retry starts from the file as it is, and writes one
                // snapshot (a broken-off write may have left part of its
                // records).
                self.broken = true;
                self.snapshot_due = true;
                self.retry_at = Some(Instant::now() + RETRY_AFTER);
                Some(SaveState::Failed(reason))
            }
        };
        let changed = self.shared.lock().is_ok_and(|mut shared| {
            let state = match failed {
                Some(failed) => failed,
                None => {
                    shared.written += count;
                    if shared.written == shared.sent {
                        SaveState::Saved
                    } else {
                        SaveState::Saving(Instant::now())
                    }
                }
            };
            let changed =
                shared.state != state && matches!(state, SaveState::Saved | SaveState::Failed(_));
            shared.state = state;
            changed
        });
        if changed {
            (self.notify)();
        }
        if self.retry_at.is_none() {
            self.compact_if_due();
        }
    }

    /// A trusted session: the one there is, read afresh after a failure
    /// (its handle and lock stay), or the file opened again by its name if
    /// it was replaced. Whatever the file holds is merged into the copy.
    fn trusted_session(&mut self) -> Result<(), String> {
        let gone = self.session.as_ref().is_none_or(|session| F::gone(session));
        if gone {
            self.session = None;
            self.session = Some((self.reopen)().map_err(|err| err.to_string())?);
            self.broken = false;
        } else if self.broken
            && let Some(session) = self.session.take()
        {
            self.session = Some(match session.rebuild() {
                Ok(session) => session,
                Err(_) => (self.reopen)().map_err(|err| err.to_string())?,
            });
            self.broken = false;
        }
        let parts = self
            .session
            .as_mut()
            .map(Session::take_parts)
            .unwrap_or_default();
        for part in &parts {
            self.follow(part);
        }
        Ok(())
    }

    fn write(&mut self) -> Result<(), String> {
        self.trusted_session()?;
        let snapshot_due = self.snapshot_due;
        let pending_len = self.pending.len();
        let Some(session) = self.session.as_mut() else {
            return Err("no file to save into".into());
        };
        if F::gone(session) {
            return Err("the file was deleted or replaced".into());
        }
        session.check_unchanged().map_err(|err| err.to_string())?;
        let snapshot = snapshot_due || session.head_len() + pending_len > SNAPSHOT_AFTER;
        let bytes = match (&self.replica, snapshot) {
            // The copy holds the file's head and every pending update.
            (Some(replica), true) => {
                Some(replica.export_snapshot().map_err(|err| err.to_string())?)
            }
            _ => None,
        };
        match bytes {
            Some(bytes) => session
                .append_snapshot(bytes)
                .map_err(|err| err.to_string())?,
            None => {
                for update in &self.pending {
                    session
                        .append_update(update.clone())
                        .map_err(|err| err.to_string())?;
                }
            }
        }
        session.commit().map_err(|err| err.to_string())?;
        self.snapshot_due = false;
        Ok(())
    }

    /// Compacts when most of the file is superseded and compacting can't
    /// lose anything, then opens it again (compaction writes a new file in
    /// its place). A compaction that fails isn't tried again.
    fn compact_if_due(&mut self) {
        let Some(session) = &self.session else {
            return;
        };
        let (live, total) = session.live_and_total();
        let dead = total.saturating_sub(live);
        if dead < COMPACT_FROM || dead < 2 * live || F::compaction_unsafe(session) {
            return;
        }
        let Some(compact) = self.compact.as_mut() else {
            return;
        };
        // The new file takes the old one's place while this session still
        // holds the old one's lock; then it opens (and locks) the new one.
        match compact() {
            Ok(()) => {
                self.session = None;
                // If that fails, the next save opens it.
                self.session = (self.reopen)().ok();
                if let Some(session) = &mut self.session {
                    session.take_parts();
                }
            }
            Err(_) => self.compact = None,
        }
    }
}
