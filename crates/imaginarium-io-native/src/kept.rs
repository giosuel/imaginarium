// SPDX-License-Identifier: AGPL-3.0-or-later

//! Boards kept for crash recovery: untitled boards are saved into files of
//! their own in a folder ([`crate::Autosave::create`]), and deleted when
//! they close normally. A file there that nobody has open was left by an
//! app that closed unexpectedly.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::{Opened, Opening, Session};

/// A board an app left behind, opened to edit (its session holds the
/// file, so no other app takes it too).
pub struct KeptBoard {
    /// Its file.
    pub path: PathBuf,
    /// The board.
    pub opened: Opened,
    /// Saving into the file.
    pub session: Box<Session>,
}

/// The boards kept in `folder` that no running app has open, newest
/// first. Files that are open elsewhere, made by a newer version, or
/// can't be read are left as they are. A folder that doesn't exist holds
/// none.
pub fn kept_boards(folder: &Path) -> Vec<KeptBoard> {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return Vec::new();
    };
    let mut found: Vec<(SystemTime, KeptBoard)> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| is_kept_file(path))
        .filter_map(|path| {
            let modified = std::fs::metadata(&path)
                .and_then(|meta| meta.modified())
                .unwrap_or(SystemTime::UNIX_EPOCH);
            match Session::open(&path) {
                Ok(Opening::Editable(opened, session)) => Some((
                    modified,
                    KeptBoard {
                        path,
                        opened,
                        session,
                    },
                )),
                _ => None,
            }
        })
        .collect();
    found.sort_by_key(|(modified, _)| std::cmp::Reverse(*modified));
    found.into_iter().map(|(_, kept)| kept).collect()
}

/// A board's file, not a lock file (`.~name.lock`) or a half-written one.
fn is_kept_file(path: &Path) -> bool {
    path.extension().is_some_and(|ext| ext == "imag")
        && path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| !name.starts_with('.'))
}
