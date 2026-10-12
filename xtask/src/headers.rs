// SPDX-License-Identifier: AGPL-3.0-or-later

//! SPDX license header check (`cargo xtask check-headers`).
//!
//! Every Rust source file must start with the line
//! `// SPDX-License-Identifier: AGPL-3.0-or-later` (MAINTAINERS.md, "Code
//! conventions"). This makes the license of every file unambiguous, even
//! when it is copied out of the repository.

use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

/// The exact first line every `.rs` file must have.
pub(crate) const HEADER: &str = "// SPDX-License-Identifier: AGPL-3.0-or-later";

/// Directory names that are never scanned: build output, VCS data,
/// generated artifacts and third-party code.
const SKIP_DIRS: &[&str] = &["target", "node_modules", "artifacts", "profiling"];

/// Returns all `.rs` files below `root` (sorted) whose first line is not
/// [`HEADER`]. Hidden directories (`.git`, `.cargo`, …) are skipped.
pub(crate) fn check(root: &Path) -> Result<Vec<PathBuf>, String> {
    let mut files = Vec::new();
    collect_rust_files(root, &mut files)?;
    files.sort();
    let mut missing = Vec::new();
    for file in files {
        if !has_header(&file)? {
            missing.push(file);
        }
    }
    Ok(missing)
}

/// Recursively collects `.rs` files, skipping [`SKIP_DIRS`] and hidden directories.
fn collect_rust_files(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
    let entries =
        fs::read_dir(dir).map_err(|err| format!("cannot read {}: {err}", dir.display()))?;
    for entry in entries {
        let entry = entry.map_err(|err| format!("cannot read {}: {err}", dir.display()))?;
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let file_type = entry
            .file_type()
            .map_err(|err| format!("cannot stat {}: {err}", path.display()))?;
        if file_type.is_dir() {
            if !name.starts_with('.') && !SKIP_DIRS.contains(&name.as_ref()) {
                collect_rust_files(&path, out)?;
            }
        } else if file_type.is_file() && name.ends_with(".rs") {
            out.push(path);
        }
    }
    Ok(())
}

/// Whether the file's first line is exactly [`HEADER`] (a trailing `\r` is tolerated).
fn has_header(file: &Path) -> Result<bool, String> {
    let handle =
        fs::File::open(file).map_err(|err| format!("cannot open {}: {err}", file.display()))?;
    let mut first = String::new();
    BufReader::new(handle)
        .read_line(&mut first)
        .map_err(|err| format!("cannot read {}: {err}", file.display()))?;
    Ok(first.trim_end_matches(['\n', '\r']) == HEADER)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A unique, empty temporary directory, removed on drop.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "imaginarium-xtask-{tag}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            TempDir(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn write(path: &Path, contents: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }

    #[test]
    fn reports_only_files_without_header() {
        let tmp = TempDir::new("headers");
        let root = &tmp.0;
        write(
            &root.join("src/good.rs"),
            &format!("{HEADER}\n\nfn f() {{}}\n"),
        );
        write(
            &root.join("src/crlf.rs"),
            &format!("{HEADER}\r\nfn f() {{}}\r\n"),
        );
        write(&root.join("src/bad.rs"), "fn f() {}\n");
        write(&root.join("src/empty.rs"), "");
        write(&root.join("README.md"), "no header needed\n");

        let missing = check(root).unwrap();
        assert_eq!(
            missing,
            vec![root.join("src/bad.rs"), root.join("src/empty.rs")]
        );
    }

    #[test]
    fn skips_build_output_and_hidden_directories() {
        let tmp = TempDir::new("skips");
        let root = &tmp.0;
        write(&root.join("target/debug/build/gen.rs"), "generated\n");
        write(&root.join(".git/hooks/x.rs"), "x\n");
        write(&root.join("website/node_modules/pkg/y.rs"), "y\n");
        write(&root.join("artifacts/z.rs"), "z\n");

        assert_eq!(check(root).unwrap(), Vec::<PathBuf>::new());
    }

    #[test]
    fn header_must_be_the_first_line() {
        let tmp = TempDir::new("first-line");
        let root = &tmp.0;
        write(&root.join("late.rs"), &format!("//! docs\n{HEADER}\n"));
        assert_eq!(check(root).unwrap(), vec![root.join("late.rs")]);
    }
}
