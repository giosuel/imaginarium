// SPDX-License-Identifier: AGPL-3.0-or-later

//! Replacing a file atomically (spec §9.4): write a temporary file next to
//! it, sync, rename it over the original, sync the directory.

use std::fs::File;
use std::path::{Path, PathBuf};

use crate::ids::ObjectId;
use crate::{FormatError, Result};

/// Attempts at finding an unused temporary name.
const TEMP_ATTEMPTS: usize = 8;

/// A new file that replaces `target` when [`AtomicFile::commit`] succeeds.
/// Dropped without committing, it removes itself and leaves `target`
/// alone.
///
/// - The temporary file gets a fresh random name, created exclusively. A
///   file or symlink someone left at a predictable name is never written
///   through, and two saves never share one temporary file.
/// - It takes the replaced file's permissions, so a private, unencrypted
///   document doesn't become readable by others. (Owner, extended
///   attributes and ACLs aren't copied.)
/// - A `target` that is a symlink is resolved: the file it points to is
///   replaced, and the link stays a link.
pub struct AtomicFile {
    file: Option<File>,
    temp: PathBuf,
    target: PathBuf,
}

impl AtomicFile {
    /// Creates the temporary file for replacing (or creating) `target`.
    pub fn create(target: &Path) -> Result<Self> {
        let target = std::fs::canonicalize(target).unwrap_or_else(|_| target.to_path_buf());
        let name = target
            .file_name()
            .filter(|name| !name.is_empty())
            .ok_or_else(|| FormatError::Invalid(format!("{} has no file name", target.display())))?
            .to_string_lossy()
            .into_owned();
        let existing = std::fs::metadata(&target).ok();
        for _ in 0..TEMP_ATTEMPTS {
            let suffix: String = ObjectId::random()?.0[..6]
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect();
            let temp = target.with_file_name(format!(".{name}.{suffix}.tmp"));
            let mut options = std::fs::OpenOptions::new();
            options.read(true).write(true).create_new(true);
            // Private until it gets the replaced file's permissions.
            #[cfg(unix)]
            if existing.is_some() {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            match options.open(&temp) {
                Ok(file) => {
                    if let Some(existing) = &existing {
                        file.set_permissions(existing.permissions())?;
                    }
                    return Ok(Self {
                        file: Some(file),
                        temp,
                        target,
                    });
                }
                Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(err) => return Err(err.into()),
            }
        }
        Err(FormatError::Invalid(format!(
            "no free temporary name next to {}",
            target.display()
        )))
    }

    /// The temporary file, to write into.
    pub fn file(&mut self) -> &mut File {
        // Only `commit` takes it, and that consumes `self`.
        self.file
            .as_mut()
            .unwrap_or_else(|| unreachable!("the file is only taken by commit"))
    }

    /// The file `commit` replaces (with symlinks resolved).
    pub fn target(&self) -> &Path {
        &self.target
    }

    /// Syncs the temporary file, renames it over the target and syncs the
    /// directory, so the rename survives a crash too.
    pub fn commit(mut self) -> Result<()> {
        if let Some(file) = self.file.take() {
            file.sync_all()?;
        }
        std::fs::rename(&self.temp, &self.target)?;
        sync_parent_dir(&self.target)
    }
}

impl Drop for AtomicFile {
    fn drop(&mut self) {
        // Not committed (an error, or the rename failed): clean up.
        if self.file.is_some() || self.temp.exists() {
            self.file = None;
            let _ = std::fs::remove_file(&self.temp);
        }
    }
}

/// Makes a rename durable. On Unix this fsyncs the directory. Other
/// platforms don't support or need it.
fn sync_parent_dir(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        File::open(parent)?.sync_all()?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}
