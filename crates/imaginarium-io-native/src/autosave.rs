// SPDX-License-Identifier: AGPL-3.0-or-later

//! Saving on a thread of its own (MAINTAINERS.md invariant 3).
//!
//! The UI sends each change's update bytes; the thread:
//! - appends them as `UPDT` records and commits them ([`Session`]),
//!   everything waiting in one commit;
//! - keeps a copy of the document, from which it writes a snapshot every
//!   [`SNAPSHOT_AFTER`] records (so the file opens fast) and after a
//!   recovery;
//! - compacts the file when most of it is superseded (spec §9.4);
//! - says how it goes ([`SaveState`]) and wakes the UI when that changes.
//!
//! A failed save is retried every [`RETRY_AFTER`]: the file is opened again
//! (it must still hold the same document) and a snapshot of everything
//! written, since the failed attempt may have left part of its records.

use std::fs::File;
use std::io::{Read, Seek, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use imaginarium_format::Durable;
use imaginarium_model::Document;

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

/// Compaction once the superseded part is more than half of a file at least
/// this long.
const COMPACT_FROM: u64 = 1 << 20;

/// What the saving thread is sent.
enum Job {
    Update(Vec<u8>),
    Stop,
}

/// The storage a saving thread writes to, and how it does what only a real
/// file can (compaction, noticing the file was deleted).
pub trait Storage: Read + Write + Seek + Durable + Send + 'static {
    /// Whether the file was deleted or replaced while open.
    fn gone(session: &Session<Self>) -> bool
    where
        Self: Sized;
}

impl Storage for File {
    fn gone(session: &Session<Self>) -> bool {
        session.file_gone()
    }
}

/// Opens the storage again (for a retry, or after compaction).
type Reopen<F> = Box<dyn FnMut() -> crate::Result<Session<F>> + Send>;

/// Compacts the storage (for files): `None` where it can't.
type Compact = Option<Box<dyn FnMut() -> crate::Result<()> + Send>>;

/// The saving thread of one open document.
pub struct Autosave {
    sender: Sender<Job>,
    shared: Arc<Shared>,
    thread: Option<std::thread::JoinHandle<()>>,
}

/// What the UI and the saving thread share.
struct Shared {
    state: Mutex<SaveState>,
    /// Updates sent and written: Saved only when they are equal.
    sent: AtomicU64,
    written: AtomicU64,
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
    pub fn start(session: Session, id: FileId, notify: impl Fn() + Send + 'static) -> Self {
        let reopen_id = id.clone();
        let compact_path = id.path.clone();
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

    /// [`Autosave::start`] on any storage: `reopen` opens it again before a
    /// retry, `compact` compacts it.
    pub fn start_with<F: Storage>(
        mut session: Session<F>,
        reopen: impl FnMut() -> crate::Result<Session<F>> + Send + 'static,
        compact: Compact,
        notify: impl Fn() + Send + 'static,
    ) -> Self {
        let (sender, receiver) = mpsc::channel();
        let shared = Arc::new(Shared {
            state: Mutex::new(SaveState::Saved),
            sent: AtomicU64::new(0),
            written: AtomicU64::new(0),
        });
        let thread_shared = shared.clone();
        let parts = session.take_parts();
        let thread = std::thread::Builder::new()
            .name("autosave".into())
            .spawn(move || {
                let mut saver = Saver {
                    session: Some(session),
                    replica: replica(&parts),
                    reopen: Box::new(reopen),
                    compact,
                    pending: Vec::new(),
                    snapshot_due: false,
                    retry_at: None,
                    shared: thread_shared,
                    notify: Box::new(notify),
                };
                saver.run(&receiver);
            });
        let thread = match thread {
            Ok(thread) => Some(thread),
            Err(err) => {
                shared.set(SaveState::Failed(format!("no thread to save with: {err}")));
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
        self.shared.sent.fetch_add(1, Ordering::SeqCst);
        if let Ok(mut state) = self.shared.state.lock()
            && *state == SaveState::Saved
        {
            *state = SaveState::Saving(Instant::now());
        }
        let _ = self.sender.send(Job::Update(update));
    }

    /// How saving goes now.
    pub fn state(&self) -> SaveState {
        self.shared.state.lock().map_or_else(
            |_| SaveState::Failed("saving stopped".into()),
            |state| state.clone(),
        )
    }

    /// Writes what is pending and stops; then why the last write failed,
    /// if it did (a failing save is tried once more, not again and again).
    /// Waits for the disk: [`Autosave::finish_in_background`] doesn't.
    pub fn finish(mut self) -> Option<String> {
        self.stop();
        match self.state() {
            SaveState::Failed(reason) => Some(reason),
            SaveState::Saved | SaveState::Saving(_) => None,
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

    fn stop(&mut self) {
        let _ = self.sender.send(Job::Stop);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for Autosave {
    fn drop(&mut self) {
        self.stop();
    }
}

impl Shared {
    /// Sets the state; whether it changed.
    fn set(&self, new: SaveState) -> bool {
        self.state.lock().is_ok_and(|mut state| {
            let changed = *state != new;
            *state = new;
            changed
        })
    }
}

/// A copy of the document, from the file's head, for snapshots.
fn replica(parts: &[Vec<u8>]) -> Option<Document> {
    let slices: Vec<&[u8]> = parts.iter().map(Vec::as_slice).collect();
    Document::open(&slices).ok()
}

/// The saving thread's state.
struct Saver<F: Storage> {
    session: Option<Session<F>>,
    replica: Option<Document>,
    reopen: Reopen<F>,
    compact: Compact,
    /// Updates not yet in the file.
    pending: Vec<Vec<u8>>,
    /// The next write is a snapshot (after a recovery or failure, or a long
    /// head).
    snapshot_due: bool,
    /// After a failure: when to try again.
    retry_at: Option<Instant>,
    shared: Arc<Shared>,
    notify: Box<dyn Fn() + Send>,
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
                        // The copy follows every update, so it can always
                        // make a snapshot of what was sent.
                        if let Some(replica) = &mut self.replica {
                            let _ = replica.apply_remote(&[&update]);
                        }
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

    /// Writes what is pending; says how it went.
    fn save(&mut self) {
        if self.pending.is_empty() && !self.snapshot_due {
            return;
        }
        let count = self.pending.len() as u64;
        let new = match self.write() {
            Ok(()) => {
                self.pending.clear();
                self.shared.written.fetch_add(count, Ordering::SeqCst);
                self.retry_at = None;
                self.compact_if_due();
                if self.shared.written.load(Ordering::SeqCst)
                    == self.shared.sent.load(Ordering::SeqCst)
                {
                    SaveState::Saved
                } else {
                    SaveState::Saving(Instant::now())
                }
            }
            Err(reason) => {
                // Opened again, and everything written as a snapshot, at
                // the retry.
                self.session = None;
                self.snapshot_due = true;
                self.retry_at = Some(Instant::now() + RETRY_AFTER);
                SaveState::Failed(reason)
            }
        };
        let saved = new == SaveState::Saved;
        let failed = matches!(new, SaveState::Failed(_));
        if self.shared.set(new) && (saved || failed) {
            (self.notify)();
        }
    }

    fn write(&mut self) -> Result<(), String> {
        if self.session.is_none() {
            self.session = Some((self.reopen)().map_err(|err| err.to_string())?);
        }
        let Some(session) = self.session.as_mut() else {
            return Err("no file to save into".into());
        };
        if F::gone(session) {
            return Err("the file was deleted or replaced".into());
        }
        session.check_unchanged().map_err(|err| err.to_string())?;
        let snapshot =
            self.snapshot_due || session.head_len() + self.pending.len() > SNAPSHOT_AFTER;
        match (&self.replica, snapshot) {
            // The snapshot holds every pending update.
            (Some(replica), true) => {
                let bytes = replica.export_snapshot().map_err(|err| err.to_string())?;
                session
                    .append_snapshot(bytes)
                    .map_err(|err| err.to_string())?;
            }
            _ => {
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

    /// Compacts when most of the file is superseded, then opens it again
    /// (compaction writes a new file in its place).
    fn compact_if_due(&mut self) {
        let Some(session) = &self.session else {
            return;
        };
        let (live, total) = session.live_and_total();
        if total < COMPACT_FROM || live * 2 > total {
            return;
        }
        let Some(compact) = self.compact.as_mut() else {
            return;
        };
        // The new file takes the old one's place while this session still
        // holds the old one's lock; then it opens (and locks) the new one.
        let compacted = compact().is_ok();
        if compacted {
            self.session = None;
            // If that fails, the next save opens it.
            self.session = (self.reopen)().ok();
        }
    }
}
