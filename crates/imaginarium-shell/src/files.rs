// SPDX-License-Identifier: AGPL-3.0-or-later

//! Files from the user's side (`shell.md` §3.1, §4): the open and save
//! dialogs, opening a board's file, putting a board in a place of its own
//! (Save…, Rename or move…) and Save a copy.
//!
//! Dialogs and file work never run on the UI thread (MAINTAINERS.md
//! invariant 3): each runs on a thread of its own ([`Pending`]), which
//! wakes the window that asked when it is done; the window takes the
//! result at its next frame.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use imaginarium_io_native::{Opening, Session};
use imaginarium_model::{Document, OpenOptions, Version};

/// The file dialogs: the system's ([`SystemDialogs`]), or a stand-in in
/// tests. They are called on a thread of their own and may block.
pub trait Dialogs: Send + Sync {
    /// Asks for a board to open; `None` if the user cancelled.
    fn open_board(&self) -> Option<PathBuf>;

    /// Asks where to save a board: `title` names the dialog, `folder` and
    /// `name` are what it starts with. `None` if the user cancelled.
    fn save_board(&self, title: &str, folder: Option<&Path>, name: &str) -> Option<PathBuf>;
}

/// The system's file dialogs (`rfd`): the desktop portal on Linux, the
/// system's own on Windows and macOS.
pub struct SystemDialogs;

impl Dialogs for SystemDialogs {
    fn open_board(&self) -> Option<PathBuf> {
        rfd::FileDialog::new()
            .set_title("Open a board")
            .add_filter("Imaginarium board", &["imag"])
            .pick_file()
    }

    fn save_board(&self, title: &str, folder: Option<&Path>, name: &str) -> Option<PathBuf> {
        let mut dialog = rfd::FileDialog::new()
            .set_title(title)
            .add_filter("Imaginarium board", &["imag"])
            .set_file_name(name);
        if let Some(folder) = folder {
            dialog = dialog.set_directory(folder);
        }
        dialog.save_file()
    }
}

/// A result a thread of its own is working on.
pub(crate) struct Pending<T> {
    slot: Arc<Mutex<Option<T>>>,
}

impl<T: Send + 'static> Pending<T> {
    /// Runs `work` on a thread named `name`; the window whose view runs
    /// now is woken when it is done. If no thread can start, or `work`
    /// panics (a bug, a dialog library's), `failed()` is the result: the
    /// window never waits for nothing.
    pub(crate) fn spawn(
        name: &str,
        work: impl FnOnce() -> T + Send + 'static,
        failed: impl Fn() -> T + Send + Sync + 'static,
    ) -> Self {
        let slot = Arc::new(Mutex::new(None));
        let done = slot.clone();
        let wake = imaginarium_ui::window_waker();
        let failed = Arc::new(failed);
        let failed_here = failed.clone();
        let spawned = std::thread::Builder::new()
            .name(name.into())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(work))
                    .unwrap_or_else(|_| failed());
                if let Ok(mut done) = done.lock() {
                    *done = Some(result);
                }
                if let Some(wake) = wake {
                    wake();
                }
            });
        if spawned.is_err()
            && let Ok(mut slot) = slot.lock()
        {
            *slot = Some(failed_here());
        }
        Self { slot }
    }

    /// The result, once it is there (taken once).
    pub(crate) fn take(&self) -> Option<T> {
        self.slot.lock().ok()?.take()
    }

    /// Waits for the result (on a thread that may wait: a closed window's
    /// work finishing).
    pub(crate) fn wait(self) -> Option<T> {
        loop {
            match self.slot.lock() {
                Ok(mut slot) => {
                    if let Some(result) = slot.take() {
                        return Some(result);
                    }
                }
                Err(_) => return None,
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
}

/// `path` with the board extension, if the dialog's answer lacks it.
pub(crate) fn with_extension(path: PathBuf) -> PathBuf {
    if path
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("imag"))
    {
        path
    } else {
        let mut name = path.file_name().map(ToOwned::to_owned).unwrap_or_default();
        name.push(".imag");
        path.with_file_name(name)
    }
}

/// A board's name from its file: the file name without `.imag`.
pub(crate) fn board_name(path: &Path) -> String {
    path.file_stem().map_or_else(
        || "Board".to_owned(),
        |stem| stem.to_string_lossy().into_owned(),
    )
}

/// A file opened from the dialog (or `Err`, as a sentence to show).
pub(crate) type OpenedFile = Result<(PathBuf, Opening), String>;

/// Opens the board at `path` to edit it (read-only if it can't be).
pub(crate) fn open_file(path: PathBuf) -> OpenedFile {
    match Session::open(&path) {
        Ok(opening) => Ok((path, opening)),
        Err(err) => Err(format!("{} can't be opened: {err}", board_name(&path))),
    }
}

/// A board put in a new place: its session there, and the document's
/// version the file holds (what the window has sent since goes next).
pub(crate) struct Placed {
    pub(crate) path: PathBuf,
    pub(crate) session: Session,
    pub(crate) saved: Version,
}

/// Why a board couldn't be put in a new place, and the session on its
/// old file to go on saving into (if there was one and it opens again).
pub(crate) struct NotPlaced {
    pub(crate) reason: String,
    pub(crate) back: Option<Box<(PathBuf, Session, Version)>>,
}

/// Moves the board's file from `from` (its own, or the one keeping it
/// for recovery; its saving has finished) to `to`, keeping the document
/// and its history, and opens it there. A file at `to` that another
/// window or app has open is never replaced. On failure the old file is
/// opened again, so saving goes on there.
pub(crate) fn move_file(from: &Path, to: &Path) -> Result<Placed, NotPlaced> {
    let moved = check_free(to).and_then(|()| {
        if from == to {
            return Ok(());
        }
        match std::fs::rename(from, to) {
            Ok(()) => Ok(()),
            // Another file system: copied, then the old one goes.
            Err(err) if err.kind() == std::io::ErrorKind::CrossesDevices => {
                copy_then_remove(from, to)
            }
            Err(err) => Err(err.to_string()),
        }
    });
    let reopened = |path: &Path| -> Result<(Session, Version), String> {
        match Session::open(path).map_err(|err| err.to_string())? {
            Opening::Editable(opened, session) => Ok((*session, opened.document.version())),
            Opening::ReadOnly(_) => Err("it is open in another window or app".into()),
        }
    };
    match moved.and_then(|()| reopened(to)) {
        Ok((session, saved)) => Ok(Placed {
            path: to.to_owned(),
            session,
            saved,
        }),
        Err(reason) => {
            let at = if from.exists() { from } else { to };
            Err(NotPlaced {
                reason: format!("{} can't be saved there: {reason}", board_name(to)),
                back: reopened(at)
                    .ok()
                    .map(|(session, saved)| Box::new((at.to_owned(), session, saved))),
            })
        }
    }
}

/// Opens the board's old file again, to go on saving into it.
pub(crate) fn reopen(path: &Path) -> Option<Box<(PathBuf, Session, Version)>> {
    match Session::open(path).ok()? {
        Opening::Editable(opened, session) => Some(Box::new((
            path.to_owned(),
            *session,
            opened.document.version(),
        ))),
        Opening::ReadOnly(_) => None,
    }
}

/// Writes a board that has no file at all (`snapshot`) as a new one at
/// `to`, and opens it there.
pub(crate) fn write_file(snapshot: &[u8], to: &Path, created_with: &str) -> Result<Placed, String> {
    check_free(to)
        .and_then(|()| write_new(snapshot, to, created_with))
        .and_then(
            |()| match Session::open(to).map_err(|err| err.to_string())? {
                Opening::Editable(opened, session) => Ok(Placed {
                    path: to.to_owned(),
                    session: *session,
                    saved: opened.document.version(),
                }),
                Opening::ReadOnly(_) => Err("it is open in another window or app".into()),
            },
        )
        .map_err(|reason| format!("{} can't be saved there: {reason}", board_name(to)))
}

/// Save a copy: `snapshot` as a new file at `to` (a new document, with
/// an id of its own); the board stays where it is.
pub(crate) fn write_copy(snapshot: &[u8], to: &Path, created_with: &str) -> Result<(), String> {
    check_free(to)
        .and_then(|()| write_new(snapshot, to, created_with))
        .map_err(|reason| format!("The copy can't be saved: {reason}"))
}

fn write_new(snapshot: &[u8], to: &Path, created_with: &str) -> Result<(), String> {
    let document = Document::open_with(
        &[snapshot],
        &OpenOptions {
            migrations: &[],
            ..OpenOptions::default()
        },
    )
    .map_err(|err| err.to_string())?;
    let options = imaginarium_io_native::SaveOptions {
        protection: imaginarium_io_native::Protection::None,
        created_with: created_with.to_owned(),
    };
    imaginarium_io_native::save_new_path(to, &document, &options).map_err(|err| err.to_string())
}

/// A file at `path` may be replaced: there is none, or nobody has it
/// open.
fn check_free(path: &Path) -> Result<(), String> {
    if !path.exists() {
        return Ok(());
    }
    match Session::open(path) {
        Ok(Opening::ReadOnly(opened))
            if matches!(
                opened.document.mode(),
                imaginarium_model::Mode::ReadOnly(imaginarium_model::ReadOnlyReason::File(reason))
                    if reason == imaginarium_io_native::IN_USE
            ) =>
        {
            Err("a board there is open in another window or app".into())
        }
        // Not a board, or one this app can't write: the user chose to
        // replace it.
        _ => Ok(()),
    }
}

/// Syncs the folder holding `path`, so a rename in it is on the disk
/// (Unix; Windows has no such call, and its renames are durable).
fn sync_folder(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    if let Some(folder) = path.parent() {
        std::fs::File::open(folder)?.sync_all()?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

/// Copies `from` to `to` (replacing it atomically), then removes `from`.
fn copy_then_remove(from: &Path, to: &Path) -> Result<(), String> {
    let name = to.file_name().map(ToOwned::to_owned).unwrap_or_default();
    let mut temp = name.clone();
    temp.push(format!(".{}.tmp", std::process::id()));
    let temp = to.with_file_name(format!(".{}", temp.to_string_lossy()));
    // Synced through a handle that may write (Windows flushes no other),
    // and the folder after the rename (Unix), before the old one goes: two
    // file systems keep their journals apart.
    let copied = std::fs::copy(from, &temp)
        .and_then(|_| {
            std::fs::OpenOptions::new()
                .write(true)
                .open(&temp)?
                .sync_all()
        })
        .and_then(|()| std::fs::rename(&temp, to))
        .and_then(|()| sync_folder(to));
    if let Err(err) = copied {
        let _ = std::fs::remove_file(&temp);
        return Err(err.to_string());
    }
    // The board is in its new place; an old copy left behind isn't lost.
    let _ = std::fs::remove_file(from);
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn a_name_without_the_extension_gets_it() {
        assert_eq!(
            with_extension(PathBuf::from("/a/Plan")),
            PathBuf::from("/a/Plan.imag")
        );
        assert_eq!(
            with_extension(PathBuf::from("/a/Plan.IMAG")),
            PathBuf::from("/a/Plan.IMAG")
        );
        assert_eq!(
            with_extension(PathBuf::from("/a/Plan v2.1")),
            PathBuf::from("/a/Plan v2.1.imag")
        );
        assert_eq!(board_name(Path::new("/a/Plan v2.1.imag")), "Plan v2.1");
    }
}
