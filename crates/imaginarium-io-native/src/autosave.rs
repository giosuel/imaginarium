// SPDX-License-Identifier: AGPL-3.0-or-later

//! Saving on a thread of its own (MAINTAINERS.md invariant 3): the UI sends
//! each change's update bytes, the thread appends and commits them
//! ([`Session`]), and says how it went ([`SaveState`]) for the document
//! pill. A failed save is retried: the file is opened again (the session's
//! state after a failed write isn't trusted) and everything not yet saved
//! is written again.

use std::io::{Read, Seek, Write};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use imaginarium_format::Durable;

use crate::Session;

/// Opens the file again for a retry.
type Reopen<F> = Box<dyn FnMut() -> crate::Result<Session<F>> + Send>;

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

/// What the saving thread is sent.
enum Job {
    Update(Vec<u8>),
    Snapshot(Vec<u8>),
    Stop,
    /// Time to try a failed save again.
    Retry,
}

/// The saving thread of one open document.
pub struct Autosave {
    sender: Sender<Job>,
    state: Arc<Mutex<SaveState>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl std::fmt::Debug for Autosave {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Autosave")
            .field("state", &self.state())
            .finish_non_exhaustive()
    }
}

impl Autosave {
    /// Saves into `session`, the file at `path`; `notify` is called (from
    /// the saving thread) when the state changes, to wake the UI.
    pub fn start(session: Session, path: PathBuf, notify: impl Fn() + Send + 'static) -> Self {
        Self::start_with(
            session,
            move || Session::open(&path).map(|(_, session)| session),
            notify,
        )
    }

    /// [`Autosave::start`] on any storage: `reopen` opens it again before a
    /// retry.
    pub fn start_with<F>(
        session: Session<F>,
        reopen: impl FnMut() -> crate::Result<Session<F>> + Send + 'static,
        notify: impl Fn() + Send + 'static,
    ) -> Self
    where
        F: Read + Write + Seek + Durable + Send + 'static,
    {
        let (sender, receiver) = mpsc::channel();
        let state = Arc::new(Mutex::new(SaveState::Saved));
        let shared = state.clone();
        let thread = std::thread::Builder::new()
            .name("autosave".into())
            .spawn(move || run(session, Box::new(reopen), &receiver, &shared, &notify));
        let thread = match thread {
            Ok(thread) => Some(thread),
            Err(err) => {
                if let Ok(mut state) = state.lock() {
                    *state = SaveState::Failed(format!("no thread to save with: {err}"));
                }
                None
            }
        };
        Self {
            sender,
            state,
            thread,
        }
    }

    /// Saves an update of the `main` stream.
    pub fn update(&self, update: Vec<u8>) {
        self.mark_saving();
        let _ = self.sender.send(Job::Update(update));
    }

    /// Saves a snapshot: the stream's head starts again from it.
    pub fn snapshot(&self, snapshot: Vec<u8>) {
        self.mark_saving();
        let _ = self.sender.send(Job::Snapshot(snapshot));
    }

    /// How saving goes now.
    pub fn state(&self) -> SaveState {
        self.state.lock().map_or_else(
            |_| SaveState::Failed("saving stopped".into()),
            |state| state.clone(),
        )
    }

    /// Writes what is pending and stops; then why the last write failed,
    /// if it did (a failing save is tried once more, not again and again).
    pub fn finish(mut self) -> Option<String> {
        self.stop();
        match self.state() {
            SaveState::Failed(reason) => Some(reason),
            SaveState::Saved | SaveState::Saving(_) => None,
        }
    }

    fn mark_saving(&self) {
        if let Ok(mut state) = self.state.lock()
            && *state == SaveState::Saved
        {
            *state = SaveState::Saving(Instant::now());
        }
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

/// The saving thread: writes jobs as they come (all waiting ones in one
/// commit), and retries what failed.
fn run<F: Read + Write + Seek + Durable>(
    session: Session<F>,
    mut reopen: Reopen<F>,
    receiver: &Receiver<Job>,
    state: &Mutex<SaveState>,
    notify: &dyn Fn(),
) {
    let mut session = Some(session);
    // Not yet in the file: kept until a commit succeeds.
    let mut pending: Vec<Job> = Vec::new();
    let mut stopping = false;
    loop {
        // Wait for a job; after a failure, at most until the retry.
        let first = if pending.is_empty() {
            receiver.recv().unwrap_or(Job::Stop)
        } else {
            match receiver.recv_timeout(RETRY_AFTER) {
                Ok(job) => job,
                Err(RecvTimeoutError::Timeout) => Job::Retry,
                Err(RecvTimeoutError::Disconnected) => Job::Stop,
            }
        };
        // Everything waiting goes into one commit.
        for job in std::iter::once(first).chain(receiver.try_iter()) {
            match job {
                Job::Stop => stopping = true,
                Job::Retry => {}
                job => pending.push(job),
            }
        }
        if !pending.is_empty() {
            let new = match write(&mut session, &mut reopen, &pending) {
                Ok(()) => {
                    pending.clear();
                    SaveState::Saved
                }
                Err(reason) => {
                    // Opened again before the retry.
                    session = None;
                    SaveState::Failed(reason)
                }
            };
            let changed = state.lock().is_ok_and(|mut state| {
                let changed = *state != new;
                *state = new;
                changed
            });
            if changed {
                notify();
            }
        }
        // At exit what is pending is tried once, not forever.
        if stopping {
            break;
        }
    }
}

/// Writes `jobs` and commits them, opening the file again if needed.
fn write<F: Read + Write + Seek + Durable>(
    session: &mut Option<Session<F>>,
    reopen: &mut Reopen<F>,
    jobs: &[Job],
) -> Result<(), String> {
    if session.is_none() {
        *session = Some(reopen().map_err(|err| err.to_string())?);
    }
    let Some(session) = session.as_mut() else {
        return Err("no file to save into".into());
    };
    for job in jobs {
        match job {
            Job::Update(update) => session.append_update(update.clone()),
            Job::Snapshot(snapshot) => session.append_snapshot(snapshot.clone()),
            Job::Stop | Job::Retry => Ok(()),
        }
        .map_err(|err| err.to_string())?;
    }
    session.commit().map_err(|err| err.to_string())
}
