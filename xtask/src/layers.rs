// SPDX-License-Identifier: AGPL-3.0-or-later

//! Crate layering and dependency rules (`cargo xtask check-layers`).
//!
//! The architecture (docs/PLAN.md §4.1) arranges crates in layers.
//! Dependencies may only point **downwards or sideways** (to the same
//! layer). This module holds the authoritative layer table, mirroring the
//! PLAN, and checks every workspace member against these rules:
//!
//! 1. Every member is listed in [`LAYERS`].
//! 2. Normal and build dependencies on other members respect the layer
//!    order. Dev-dependencies are exempt: tests may use anything.
//! 3. `imaginarium-test-utils` is only ever a dev-dependency.
//! 4. Nothing depends on tooling crates (`xtask`).
//! 5. The lowest layers (support, core) don't pull in UI, GPU, networking or
//!    async-runtime crates ([`FORBIDDEN_IN_LOW_LAYERS`]).
//! 6. Every member is licensed `AGPL-3.0-or-later` (ADR-0008).
//!
//! To add a crate, add it to [`LAYERS`] *and* to the crate map in
//! docs/PLAN.md §4.1 in the same commit.

use std::collections::HashMap;
use std::fmt;

use crate::metadata::{Metadata, Package};

/// Architectural layers, from lowest to highest.
///
/// The derived `Ord` follows declaration order, and comparisons rely on it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Layer {
    /// Cross-cutting helpers every crate may use.
    Support,
    /// Document model, format, crypto, commands, protocol. No UI, network or async.
    Core,
    /// Our UI toolkit, renderer and text stack.
    Toolkit,
    /// Settings, sync, AI, add-on host, MCP.
    Services,
    /// Diagramming, layout, paint, import/export, packages.
    Features,
    /// The application surface: shell, canvas, AI and admin UI.
    Product,
    /// Binaries: desktop app, Hub server, CLI.
    Apps,
    /// Repository tooling. May depend on anything; nothing may depend on it.
    Tooling,
}

impl fmt::Display for Layer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Layer::Support => "support",
            Layer::Core => "core",
            Layer::Toolkit => "toolkit",
            Layer::Services => "services",
            Layer::Features => "features",
            Layer::Product => "product",
            Layer::Apps => "apps",
            Layer::Tooling => "tooling",
        };
        f.write_str(name)
    }
}

/// The layer of every crate, existing and planned (docs/PLAN.md §4.1).
///
/// Planned crates are listed ahead of time, so creating them later needs no
/// table change unless the plan changes.
pub(crate) const LAYERS: &[(&str, Layer)] = &[
    // support
    ("imaginarium-profiling", Layer::Support),
    ("imaginarium-test-utils", Layer::Support),
    ("imaginarium-addon-api", Layer::Support),
    // core
    ("imaginarium-model", Layer::Core),
    ("imaginarium-commands", Layer::Core),
    ("imaginarium-format", Layer::Core),
    ("imaginarium-crypto", Layer::Core),
    ("imaginarium-protocol", Layer::Core),
    // toolkit
    ("imaginarium-reactive", Layer::Toolkit),
    ("imaginarium-ui", Layer::Toolkit),
    ("imaginarium-render", Layer::Toolkit),
    ("imaginarium-text", Layer::Toolkit),
    // services
    ("imaginarium-settings", Layer::Services),
    ("imaginarium-sync", Layer::Services),
    ("imaginarium-ai", Layer::Services),
    ("imaginarium-ai-providers", Layer::Services),
    ("imaginarium-ai-local", Layer::Services),
    ("imaginarium-mcp", Layer::Services),
    ("imaginarium-addon-host", Layer::Services),
    // features
    ("imaginarium-diagram", Layer::Features),
    ("imaginarium-layout", Layer::Features),
    ("imaginarium-paint", Layer::Features),
    ("imaginarium-packages", Layer::Features),
    ("imaginarium-io", Layer::Features),
    ("imaginarium-io-native", Layer::Features),
    ("imaginarium-io-ooxml", Layer::Features),
    ("imaginarium-io-odf", Layer::Features),
    ("imaginarium-io-export", Layer::Features),
    // product
    ("imaginarium-shell", Layer::Product),
    ("imaginarium-canvas", Layer::Product),
    ("imaginarium-ai-ui", Layer::Product),
    ("imaginarium-admin-ui", Layer::Product),
    // apps
    ("imaginarium-app", Layer::Apps),
    ("imaginarium-hub", Layer::Apps),
    ("imaginarium-cli", Layer::Apps),
    ("imaginarium-gate", Layer::Apps),
    // tooling
    ("xtask", Layer::Tooling),
];

/// Crates that may only appear as dev-dependencies.
const DEV_ONLY: &[&str] = &["imaginarium-test-utils"];

/// External crates the support and core layers must not depend on. These
/// layers stay free of UI, GPU, networking and async runtimes
/// (MAINTAINERS.md, invariant 1).
pub(crate) const FORBIDDEN_IN_LOW_LAYERS: &[&str] = &[
    "async-std",
    "hyper",
    "reqwest",
    "smol",
    "tokio",
    "vello",
    "wgpu",
    "winit",
];

/// The license every workspace member must declare (ADR-0008).
const REQUIRED_LICENSE: &str = "AGPL-3.0-or-later";

/// A rule violation, with enough context to fix it.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Violation {
    /// A workspace member has no entry in [`LAYERS`].
    UnknownCrate { krate: String },
    /// A dependency points to a higher layer.
    Upwards {
        krate: String,
        layer: Layer,
        dep: String,
        dep_layer: Layer,
    },
    /// A dev-only crate is used as a normal or build dependency.
    DevOnlyAsNormal { krate: String, dep: String },
    /// Something depends on a tooling crate.
    DependsOnTooling { krate: String, dep: String },
    /// A support/core crate pulls in a forbidden external crate.
    ForbiddenExternal {
        krate: String,
        layer: Layer,
        dep: String,
    },
    /// A member has a missing or wrong license.
    WrongLicense {
        krate: String,
        found: Option<String>,
    },
}

impl fmt::Display for Violation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Violation::UnknownCrate { krate } => write!(
                f,
                "`{krate}` has no layer. Add it to LAYERS in xtask/src/layers.rs \
                 and to the crate map in docs/PLAN.md §4.1"
            ),
            Violation::Upwards {
                krate,
                layer,
                dep,
                dep_layer,
            } => write!(
                f,
                "`{krate}` ({layer}) depends on `{dep}` ({dep_layer}), which is a higher \
                 layer. Move the shared code down, or invert the dependency"
            ),
            Violation::DevOnlyAsNormal { krate, dep } => write!(
                f,
                "`{krate}` uses `{dep}` as a normal/build dependency. It may only be \
                 a [dev-dependency]"
            ),
            Violation::DependsOnTooling { krate, dep } => {
                write!(
                    f,
                    "`{krate}` depends on tooling crate `{dep}`, which is not allowed"
                )
            }
            Violation::ForbiddenExternal { krate, layer, dep } => write!(
                f,
                "`{krate}` ({layer}) depends on `{dep}`. The support and core layers \
                 must stay free of UI, GPU, networking and async runtimes"
            ),
            Violation::WrongLicense { krate, found } => write!(
                f,
                "`{krate}` has license {found:?}, expected `{REQUIRED_LICENSE}`. \
                 Use `license.workspace = true`"
            ),
        }
    }
}

/// Checks all workspace members against the layering rules, using `table`
/// (normally [`LAYERS`]). Returns every violation found, in package order.
pub(crate) fn check(metadata: &Metadata, table: &[(&str, Layer)]) -> Vec<Violation> {
    let layers: HashMap<&str, Layer> = table.iter().copied().collect();
    let members: Vec<&str> = metadata.packages.iter().map(|p| p.name.as_str()).collect();
    let mut violations = Vec::new();

    for package in &metadata.packages {
        if package.license.as_deref() != Some(REQUIRED_LICENSE) {
            violations.push(Violation::WrongLicense {
                krate: package.name.clone(),
                found: package.license.clone(),
            });
        }
        let Some(&layer) = layers.get(package.name.as_str()) else {
            violations.push(Violation::UnknownCrate {
                krate: package.name.clone(),
            });
            continue;
        };
        check_dependencies(package, layer, &layers, &members, &mut violations);
    }
    violations
}

/// Applies the per-dependency rules to one package.
fn check_dependencies(
    package: &Package,
    layer: Layer,
    layers: &HashMap<&str, Layer>,
    members: &[&str],
    violations: &mut Vec<Violation>,
) {
    for dep in &package.dependencies {
        let krate = || package.name.clone();
        let dep_name = || dep.name.clone();

        if !members.contains(&dep.name.as_str()) {
            // External crate: only the low-layer denylist applies.
            let low_layer = matches!(layer, Layer::Support | Layer::Core);
            if low_layer && !dep.is_dev() && FORBIDDEN_IN_LOW_LAYERS.contains(&dep.name.as_str()) {
                violations.push(Violation::ForbiddenExternal {
                    krate: krate(),
                    layer,
                    dep: dep_name(),
                });
            }
            continue;
        }

        // Internal dependency. Members without a layer are already reported.
        let Some(&dep_layer) = layers.get(dep.name.as_str()) else {
            continue;
        };
        if dep_layer == Layer::Tooling {
            violations.push(Violation::DependsOnTooling {
                krate: krate(),
                dep: dep_name(),
            });
            continue;
        }
        if dep.is_dev() {
            continue;
        }
        if DEV_ONLY.contains(&dep.name.as_str()) {
            violations.push(Violation::DevOnlyAsNormal {
                krate: krate(),
                dep: dep_name(),
            });
        }
        if layer != Layer::Tooling && dep_layer > layer {
            violations.push(Violation::Upwards {
                krate: krate(),
                layer,
                dep: dep_name(),
                dep_layer,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::metadata::Dependency;

    const TABLE: &[(&str, Layer)] = &[
        ("low", Layer::Core),
        ("high", Layer::Product),
        ("peer", Layer::Product),
        ("utils", Layer::Support),
        ("xtask", Layer::Tooling),
    ];

    fn dep(name: &str, kind: Option<&str>) -> Dependency {
        Dependency {
            name: name.to_owned(),
            kind: kind.map(str::to_owned),
        }
    }

    fn package(name: &str, deps: Vec<Dependency>) -> Package {
        Package {
            name: name.to_owned(),
            license: Some(REQUIRED_LICENSE.to_owned()),
            dependencies: deps,
        }
    }

    /// Builds metadata where `packages` are the members, plus leaf members
    /// for every table entry not given explicitly.
    fn metadata(packages: Vec<Package>) -> Metadata {
        let mut all = packages;
        for (name, _) in TABLE {
            if !all.iter().any(|p| p.name == *name) {
                all.push(package(name, vec![]));
            }
        }
        Metadata {
            packages: all,
            workspace_root: PathBuf::from("/ws"),
        }
    }

    #[test]
    fn downward_and_sideways_dependencies_pass() {
        let md = metadata(vec![
            package("high", vec![dep("low", None), dep("peer", None)]),
            package("low", vec![dep("utils", Some("build"))]),
        ]);
        assert_eq!(check(&md, TABLE), vec![]);
    }

    #[test]
    fn upward_dependency_fails() {
        let md = metadata(vec![package("low", vec![dep("high", None)])]);
        assert_eq!(
            check(&md, TABLE),
            vec![Violation::Upwards {
                krate: "low".into(),
                layer: Layer::Core,
                dep: "high".into(),
                dep_layer: Layer::Product,
            }]
        );
    }

    #[test]
    fn upward_build_dependency_fails() {
        let md = metadata(vec![package("low", vec![dep("high", Some("build"))])]);
        assert_eq!(check(&md, TABLE).len(), 1);
    }

    #[test]
    fn dev_dependencies_may_point_anywhere() {
        let md = metadata(vec![package("low", vec![dep("high", Some("dev"))])]);
        assert_eq!(check(&md, TABLE), vec![]);
    }

    #[test]
    fn unknown_crate_fails() {
        let md = metadata(vec![package("mystery", vec![])]);
        assert_eq!(
            check(&md, TABLE),
            vec![Violation::UnknownCrate {
                krate: "mystery".into()
            }]
        );
    }

    #[test]
    fn test_utils_only_as_dev_dependency() {
        let table = &[
            ("imaginarium-test-utils", Layer::Support),
            ("high", Layer::Product),
        ];
        let md = Metadata {
            packages: vec![
                package("high", vec![dep("imaginarium-test-utils", None)]),
                package("imaginarium-test-utils", vec![]),
            ],
            workspace_root: PathBuf::from("/ws"),
        };
        assert_eq!(
            check(&md, table),
            vec![Violation::DevOnlyAsNormal {
                krate: "high".into(),
                dep: "imaginarium-test-utils".into()
            }]
        );

        let md_dev = Metadata {
            packages: vec![
                package("high", vec![dep("imaginarium-test-utils", Some("dev"))]),
                package("imaginarium-test-utils", vec![]),
            ],
            workspace_root: PathBuf::from("/ws"),
        };
        assert_eq!(check(&md_dev, table), vec![]);
    }

    #[test]
    fn nothing_may_depend_on_tooling_even_as_dev() {
        let md = metadata(vec![package("high", vec![dep("xtask", Some("dev"))])]);
        assert_eq!(
            check(&md, TABLE),
            vec![Violation::DependsOnTooling {
                krate: "high".into(),
                dep: "xtask".into()
            }]
        );
    }

    #[test]
    fn tooling_may_depend_on_anything() {
        let md = metadata(vec![package("xtask", vec![dep("high", None)])]);
        assert_eq!(check(&md, TABLE), vec![]);
    }

    #[test]
    fn low_layers_must_not_use_forbidden_externals() {
        let md = metadata(vec![
            package("low", vec![dep("tokio", None)]),
            package("high", vec![dep("tokio", None)]),
        ]);
        assert_eq!(
            check(&md, TABLE),
            vec![Violation::ForbiddenExternal {
                krate: "low".into(),
                layer: Layer::Core,
                dep: "tokio".into()
            }]
        );
    }

    #[test]
    fn forbidden_externals_are_fine_as_dev_dependencies() {
        let md = metadata(vec![package("low", vec![dep("tokio", Some("dev"))])]);
        assert_eq!(check(&md, TABLE), vec![]);
    }

    #[test]
    fn wrong_or_missing_license_fails() {
        let mut gpl = package("low", vec![]);
        gpl.license = Some("GPL-3.0-only".into());
        let mut missing = package("high", vec![]);
        missing.license = None;
        let md = metadata(vec![gpl, missing]);
        assert_eq!(check(&md, TABLE).len(), 2);
    }

    #[test]
    fn real_layer_table_has_no_duplicates() {
        let mut names: Vec<&str> = LAYERS.iter().map(|(name, _)| *name).collect();
        names.sort_unstable();
        let before = names.len();
        names.dedup();
        assert_eq!(before, names.len(), "duplicate crate in LAYERS");
    }

    #[test]
    fn layer_order_matches_the_plan() {
        assert!(Layer::Support < Layer::Core);
        assert!(Layer::Core < Layer::Toolkit);
        assert!(Layer::Toolkit < Layer::Services);
        assert!(Layer::Services < Layer::Features);
        assert!(Layer::Features < Layer::Product);
        assert!(Layer::Product < Layer::Apps);
    }
}
