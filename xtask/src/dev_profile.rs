// SPDX-License-Identifier: AGPL-3.0-or-later

//! Generated dev-profile overrides for build-time-only crates
//! (`cargo xtask dev-profile [--write]`, ADR-0017).
//!
//! The dev profile optimizes all dependencies (`[profile.dev.package."*"]
//! opt-level = 2`), so the app is usable in debug builds. That override also
//! hits crates that only run **at compile time**: proc macros, build
//! scripts and their dependencies (`syn`, `serde_derive`, …). Cargo's
//! `build-override` can't undo it, because `package."*"` takes precedence.
//! Optimizing those crates cost about a third of a clean dev build and
//! bought nothing measurable. Incremental rebuilds of a crate with 450
//! derives were within noise.
//!
//! This task computes the build-time-only crates from the dependency graph
//! (all platforms, all features)
//! and keeps a generated block of `opt-level = 0` overrides in the root
//! `Cargo.toml` in sync, like a lockfile. `cargo xtask check` fails when the
//! block is stale (e.g. after a dependency update), and `--write` fixes it.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::fs;
use std::path::Path;

use crate::metadata::FullMetadata;

/// First line of the generated block in the root `Cargo.toml`.
pub(crate) const BEGIN_MARKER: &str =
    "# BEGIN GENERATED: build-time-only crates (`cargo xtask dev-profile --write`)";

/// Last line of the generated block.
pub(crate) const END_MARKER: &str = "# END GENERATED: build-time-only crates";

/// Crates treated as build-time-only even though something uses them at
/// runtime. Their runtime use in our graph is trivial (e.g. `zvariant_utils`
/// parsing D-Bus signatures, `generic-btree` using `proc-macro2` types), and
/// almost all their cost is macro expansion. Remove an entry if a runtime
/// user ever becomes performance-relevant.
const ALWAYS_BUILD_TIME: &[&str] = &["proc-macro2", "quote", "syn", "unicode-ident"];

/// Names of all non-workspace crates that are only used at build time.
///
/// A crate is a *runtime* crate if it is reachable from a workspace member
/// through normal edges (or dev edges directly from a member, since tests
/// and benches run that code), without passing through a proc-macro crate.
/// Overrides are per name, so a name counts as runtime if *any* version is.
/// [`ALWAYS_BUILD_TIME`] entries that are present are added back.
pub(crate) fn build_time_only(metadata: &FullMetadata) -> BTreeSet<String> {
    let packages: HashMap<&str, &crate::metadata::FullPackage> = metadata
        .packages
        .iter()
        .map(|package| (package.id.as_str(), package))
        .collect();
    let edges: HashMap<&str, &[crate::metadata::NodeDep]> = metadata
        .resolve
        .nodes
        .iter()
        .map(|node| (node.id.as_str(), node.deps.as_slice()))
        .collect();
    let members: HashSet<&str> = metadata
        .workspace_members
        .iter()
        .map(String::as_str)
        .collect();

    let mut runtime: HashSet<&str> = HashSet::new();
    let mut stack: Vec<&str> = members.iter().copied().collect();
    while let Some(id) = stack.pop() {
        if !runtime.insert(id) {
            continue;
        }
        let is_member = members.contains(id);
        for dep in edges.get(id).copied().unwrap_or_default() {
            let followed = dep
                .dep_kinds
                .iter()
                .any(|dep_kind| match dep_kind.kind.as_deref() {
                    None => true,
                    Some("dev") => is_member,
                    Some(_) => false,
                });
            let into_proc_macro = packages
                .get(dep.pkg.as_str())
                .is_some_and(|package| package.is_proc_macro());
            if followed && !into_proc_macro {
                stack.push(dep.pkg.as_str());
            }
        }
    }

    let runtime_names: HashSet<&str> = runtime
        .iter()
        .filter(|id| !members.contains(*id))
        .filter_map(|id| packages.get(id).map(|package| package.name.as_str()))
        .collect();
    let all_names: BTreeSet<&str> = metadata
        .packages
        .iter()
        .filter(|package| !members.contains(package.id.as_str()))
        .map(|package| package.name.as_str())
        .collect();

    all_names
        .into_iter()
        .filter(|name| !runtime_names.contains(name) || ALWAYS_BUILD_TIME.contains(name))
        .map(str::to_owned)
        .collect()
}

/// Renders the generated block (markers included, trailing newline).
pub(crate) fn render_block(crates: &BTreeSet<String>) -> String {
    let mut block = String::new();
    block.push_str(BEGIN_MARKER);
    block.push('\n');
    block.push_str(
        "# Crates that only run at compile time (proc macros, build scripts and\n\
         # their dependencies) are built unoptimized: optimizing them cost ~1/3 of a\n\
         # clean dev build for no runtime benefit (ADR-0017). Don't edit by hand.\n",
    );
    for name in crates {
        block.push_str(&format!("[profile.dev.package.{name}]\nopt-level = 0\n"));
    }
    block.push_str(END_MARKER);
    block.push('\n');
    block
}

/// Returns `manifest` with the generated block replaced by `block`, or with
/// `block` appended (after a blank line) if there is no block yet.
pub(crate) fn splice_block(manifest: &str, block: &str) -> Result<String, String> {
    match (manifest.find(BEGIN_MARKER), manifest.find(END_MARKER)) {
        (Some(begin), Some(end)) if begin < end => {
            let after_end = manifest[end..]
                .find('\n')
                .map_or(manifest.len(), |newline| end + newline + 1);
            Ok(format!(
                "{}{block}{}",
                &manifest[..begin],
                &manifest[after_end..]
            ))
        }
        (None, None) => {
            let separator = if manifest.ends_with("\n\n") {
                ""
            } else if manifest.ends_with('\n') {
                "\n"
            } else {
                "\n\n"
            };
            Ok(format!("{manifest}{separator}{block}"))
        }
        _ => {
            Err("Cargo.toml has a broken generated block (markers missing or out of order)".into())
        }
    }
}

/// Outcome of [`run`].
pub(crate) enum Outcome {
    /// The block matches the dependency graph.
    UpToDate { crates: usize },
    /// The block was (re)written.
    Written { crates: usize },
    /// The block is stale. Lists names to add and to remove.
    Stale {
        missing: Vec<String>,
        extra: Vec<String>,
    },
}

/// Checks (or with `write`, updates) the generated block in `root/Cargo.toml`.
pub(crate) fn run(root: &Path, metadata: &FullMetadata, write: bool) -> Result<Outcome, String> {
    let path = root.join("Cargo.toml");
    let manifest = fs::read_to_string(&path)
        .map_err(|err| format!("cannot read {}: {err}", path.display()))?;
    let crates = build_time_only(metadata);
    let block = render_block(&crates);
    let updated = splice_block(&manifest, &block)?;

    if updated == manifest {
        return Ok(Outcome::UpToDate {
            crates: crates.len(),
        });
    }
    if write {
        fs::write(&path, updated)
            .map_err(|err| format!("cannot write {}: {err}", path.display()))?;
        return Ok(Outcome::Written {
            crates: crates.len(),
        });
    }
    let current = current_names(&manifest);
    Ok(Outcome::Stale {
        missing: crates.difference(&current).cloned().collect(),
        extra: current.difference(&crates).cloned().collect(),
    })
}

/// Crate names in the manifest's generated block (empty if there is none).
fn current_names(manifest: &str) -> BTreeSet<String> {
    let (Some(begin), Some(end)) = (manifest.find(BEGIN_MARKER), manifest.find(END_MARKER)) else {
        return BTreeSet::new();
    };
    manifest[begin..end.max(begin)]
        .lines()
        .filter_map(|line| line.strip_prefix("[profile.dev.package."))
        .filter_map(|rest| rest.strip_suffix(']'))
        .map(str::to_owned)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metadata::{DepKind, FullPackage, Node, NodeDep, Resolve, Target};

    fn package(name: &str, proc_macro: bool) -> FullPackage {
        let kind = if proc_macro { "proc-macro" } else { "lib" };
        FullPackage {
            id: format!("{name}-id"),
            name: name.to_owned(),
            targets: vec![Target {
                kind: vec![kind.to_owned()],
            }],
        }
    }

    fn edge(to: &str, kind: Option<&str>) -> NodeDep {
        NodeDep {
            pkg: format!("{to}-id"),
            dep_kinds: vec![DepKind {
                kind: kind.map(str::to_owned),
            }],
        }
    }

    fn node(id: &str, deps: Vec<NodeDep>) -> Node {
        Node {
            id: format!("{id}-id"),
            deps,
        }
    }

    /// member ─normal→ runtime_lib ─normal→ shared
    ///        │                    └normal→ derive (proc macro) ─→ syn, shared
    ///        ├─build→ cc ─→ jobserver
    ///        └─dev→ criterion ─→ plotters, (build) build_helper
    /// runtime_lib ─normal→ syn (a runtime user of syn, as in zvariant_utils)
    fn sample() -> FullMetadata {
        FullMetadata {
            packages: vec![
                package("member", false),
                package("runtime_lib", false),
                package("shared", false),
                package("derive", true),
                package("syn", false),
                package("cc", false),
                package("jobserver", false),
                package("criterion", false),
                package("plotters", false),
                package("build_helper", false),
            ],
            workspace_members: vec!["member-id".into()],
            resolve: Resolve {
                nodes: vec![
                    node(
                        "member",
                        vec![
                            edge("runtime_lib", None),
                            edge("cc", Some("build")),
                            edge("criterion", Some("dev")),
                        ],
                    ),
                    node(
                        "runtime_lib",
                        vec![
                            edge("shared", None),
                            edge("derive", None),
                            edge("syn", None),
                        ],
                    ),
                    node("derive", vec![edge("syn", None), edge("shared", None)]),
                    node("cc", vec![edge("jobserver", None)]),
                    node(
                        "criterion",
                        vec![edge("plotters", None), edge("build_helper", Some("build"))],
                    ),
                    node("shared", vec![]),
                    node("syn", vec![]),
                    node("jobserver", vec![]),
                    node("plotters", vec![]),
                    node("build_helper", vec![]),
                ],
            },
        }
    }

    #[test]
    fn classifies_build_time_crates() {
        let names: Vec<String> = build_time_only(&sample()).into_iter().collect();
        // derive: proc macro. cc, jobserver: build deps. build_helper: build
        // dep of a dev dep. syn: runtime-used, but in ALWAYS_BUILD_TIME.
        // Runtime: runtime_lib, shared (used by both), criterion and
        // plotters (benches run them). The member itself is never listed.
        assert_eq!(names, ["build_helper", "cc", "derive", "jobserver", "syn"]);
    }

    #[test]
    fn dev_edges_count_only_from_members() {
        // A dev-dependency of a *non-member* never runs in our builds, so it
        // isn't a runtime crate.
        let mut metadata = sample();
        metadata.packages.push(package("lib_test_helper", false));
        metadata.resolve.nodes.push(node("lib_test_helper", vec![]));
        if let Some(runtime_lib) = metadata
            .resolve
            .nodes
            .iter_mut()
            .find(|n| n.id == "runtime_lib-id")
        {
            runtime_lib.deps.push(edge("lib_test_helper", Some("dev")));
        }
        assert!(build_time_only(&metadata).contains("lib_test_helper"));
    }

    #[test]
    fn render_and_splice_round_trip() {
        let crates: BTreeSet<String> = ["cc", "syn"].iter().map(|s| (*s).to_owned()).collect();
        let block = render_block(&crates);
        assert!(block.starts_with(BEGIN_MARKER));
        assert!(block.contains("[profile.dev.package.cc]\nopt-level = 0\n"));
        assert!(block.ends_with(&format!("{END_MARKER}\n")));

        let manifest = "[workspace]\nmembers = []\n";
        let first = splice_block(manifest, &block).unwrap();
        assert_eq!(first, format!("{manifest}\n{block}"));
        // Splicing again is a no-op, and a changed block replaces the old one.
        assert_eq!(splice_block(&first, &block).unwrap(), first);
        let other = render_block(&["autocfg".to_owned()].into_iter().collect());
        let second = splice_block(&first, &other).unwrap();
        assert_eq!(second, format!("{manifest}\n{other}"));
        assert_eq!(
            current_names(&second).into_iter().collect::<Vec<_>>(),
            ["autocfg"]
        );
    }

    #[test]
    fn content_after_the_block_is_kept() {
        let block = render_block(&["cc".to_owned()].into_iter().collect());
        let manifest = format!("[a]\n\n{block}\n[b]\nx = 1\n");
        let new_block = render_block(&["syn".to_owned()].into_iter().collect());
        let spliced = splice_block(&manifest, &new_block).unwrap();
        assert_eq!(spliced, format!("[a]\n\n{new_block}\n[b]\nx = 1\n"));
    }

    #[test]
    fn broken_markers_are_an_error() {
        let manifest = format!("{BEGIN_MARKER}\n[profile.dev.package.cc]\n");
        assert!(splice_block(&manifest, "x").is_err());
    }
}
