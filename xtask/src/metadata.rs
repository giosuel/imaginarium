// SPDX-License-Identifier: AGPL-3.0-or-later

//! Minimal, typed view of `cargo metadata` output.
//!
//! Only the fields the checks need are deserialized. Everything else in the
//! JSON is ignored, so new cargo versions don't break the tasks.

use std::process::Command;

use serde::Deserialize;

/// The subset of `cargo metadata --format-version 1 --no-deps` we use.
#[derive(Debug, Deserialize)]
pub(crate) struct Metadata {
    /// Workspace member packages. With `--no-deps`, only members are listed.
    pub(crate) packages: Vec<Package>,
}

/// A workspace member package.
#[derive(Debug, Deserialize)]
pub(crate) struct Package {
    /// Package name, e.g. `imaginarium-model`.
    pub(crate) name: String,
    /// SPDX license expression from `Cargo.toml`, if set.
    pub(crate) license: Option<String>,
    /// Declared dependencies of all kinds (normal, dev, build).
    pub(crate) dependencies: Vec<Dependency>,
}

/// A dependency declaration of a package.
#[derive(Debug, Deserialize)]
pub(crate) struct Dependency {
    /// Name of the depended-on package (not the rename).
    pub(crate) name: String,
    /// `None` for normal dependencies, or `"dev"` / `"build"`.
    pub(crate) kind: Option<String>,
}

impl Dependency {
    /// Whether this is a dev-dependency (only used by tests, benches, examples).
    pub(crate) fn is_dev(&self) -> bool {
        self.kind.as_deref() == Some("dev")
    }
}

/// Runs `cargo metadata` for the current workspace and parses it.
pub(crate) fn load() -> Result<Metadata, String> {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned());
    let output = Command::new(cargo)
        .args(["metadata", "--format-version", "1", "--no-deps"])
        .output()
        .map_err(|err| format!("failed to run `cargo metadata`: {err}"))?;
    if !output.status.success() {
        return Err(format!(
            "`cargo metadata` failed:\n{}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    serde_json::from_slice(&output.stdout)
        .map_err(|err| format!("failed to parse `cargo metadata` output: {err}"))
}
