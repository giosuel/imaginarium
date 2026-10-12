// SPDX-License-Identifier: AGPL-3.0-or-later

//! Minimal, typed view of `cargo metadata` output.
//!
//! Only the fields the checks need are deserialized. Everything else in the
//! JSON is ignored, so new cargo versions don't break the tasks.

use std::path::PathBuf;
use std::process::Command;

use serde::Deserialize;

/// The subset of `cargo metadata --format-version 1 --no-deps` we use.
#[derive(Debug, Deserialize)]
pub(crate) struct Metadata {
    /// Workspace member packages. With `--no-deps`, only members are listed.
    pub(crate) packages: Vec<Package>,
    /// Absolute path of the workspace root directory.
    pub(crate) workspace_root: PathBuf,
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

/// The full dependency graph: `cargo metadata --format-version 1` (with
/// dependencies, all platforms).
#[derive(Debug, Deserialize)]
pub(crate) struct FullMetadata {
    /// Every package in the graph, including workspace members.
    pub(crate) packages: Vec<FullPackage>,
    /// Package ids of the workspace members.
    pub(crate) workspace_members: Vec<String>,
    /// The resolved dependency graph.
    pub(crate) resolve: Resolve,
}

/// A package in the full graph.
#[derive(Debug, Deserialize)]
pub(crate) struct FullPackage {
    /// Opaque, unique package id.
    pub(crate) id: String,
    /// Package name (not unique: several versions may exist).
    pub(crate) name: String,
    /// Build targets (lib, bin, proc-macro, …).
    pub(crate) targets: Vec<Target>,
}

impl FullPackage {
    /// Whether the package is a procedural-macro crate.
    pub(crate) fn is_proc_macro(&self) -> bool {
        self.targets
            .iter()
            .any(|target| target.kind.iter().any(|kind| kind == "proc-macro"))
    }
}

/// A build target of a package.
#[derive(Debug, Deserialize)]
pub(crate) struct Target {
    /// Target kinds, e.g. `["lib"]` or `["proc-macro"]`.
    pub(crate) kind: Vec<String>,
}

/// The resolved dependency graph.
#[derive(Debug, Deserialize)]
pub(crate) struct Resolve {
    /// One node per package.
    pub(crate) nodes: Vec<Node>,
}

/// Resolved dependencies of one package.
#[derive(Debug, Deserialize)]
pub(crate) struct Node {
    /// Package id.
    pub(crate) id: String,
    /// Edges to dependencies.
    pub(crate) deps: Vec<NodeDep>,
}

/// One resolved dependency edge.
#[derive(Debug, Deserialize)]
pub(crate) struct NodeDep {
    /// Package id of the dependency.
    pub(crate) pkg: String,
    /// How it is depended on (normal, dev, build). One edge can be several.
    pub(crate) dep_kinds: Vec<DepKind>,
}

/// The kind of a resolved dependency edge.
#[derive(Debug, Deserialize)]
pub(crate) struct DepKind {
    /// `None` for normal dependencies, or `"dev"` / `"build"`.
    pub(crate) kind: Option<String>,
}

/// Runs `cargo metadata` for the current workspace and parses it.
pub(crate) fn load() -> Result<Metadata, String> {
    run_cargo_metadata(&["--no-deps"])
}

/// Runs `cargo metadata` with the full dependency graph (all platforms).
pub(crate) fn load_full() -> Result<FullMetadata, String> {
    run_cargo_metadata(&[])
}

/// Runs `cargo metadata --format-version 1 <extra>` and deserializes it.
fn run_cargo_metadata<T: serde::de::DeserializeOwned>(extra: &[&str]) -> Result<T, String> {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned());
    let output = Command::new(cargo)
        .args(["metadata", "--format-version", "1"])
        .args(extra)
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
