// SPDX-License-Identifier: AGPL-3.0-or-later

//! Forbidden tracked files check (`cargo xtask check-tracked`).
//!
//! Some files must never be committed (MAINTAINERS.md, "Reporting to the
//! owner", rule 4):
//! - **agent and tool files**: `CLAUDE.md`, `AGENTS.md`, `.claude/`,
//!   `.cursor/`, … The shared rules live in `MAINTAINERS.md` instead.
//! - **generated artifacts**: screenshots and reports in `artifacts/`,
//!   profiling outputs (tracy captures, perf data, flamegraphs, cargo timing
//!   reports, …).
//!
//! `.gitignore` keeps these out by default. This check catches the cases
//! where they slipped in anyway, e.g. through `git add -f`.

use std::path::Path;
use std::process::Command;

/// Agent/tool file names that are forbidden in any directory.
const FORBIDDEN_FILE_NAMES: &[&str] = &[
    "CLAUDE.md",
    "CLAUDE.local.md",
    "AGENTS.md",
    "AGENTS.local.md",
    ".cursorrules",
    ".clinerules",
];

/// Directory names that are forbidden at any depth (agent/tool state).
const FORBIDDEN_DIRS: &[&str] = &[
    ".claude",
    ".cursor",
    ".codex",
    ".gemini",
    ".windsurf",
    ".continue",
    ".roo",
    ".cline",
    ".agent",
    ".agents",
];

/// Top-level directories for generated output.
const FORBIDDEN_TOP_LEVEL_DIRS: &[&str] = &["artifacts", "profiling", "target"];

/// Returns why `path` (a `/`-separated, repo-relative path as printed by
/// `git ls-files`) must not be tracked, or `None` if it is fine.
pub(crate) fn forbidden_reason(path: &str) -> Option<&'static str> {
    let components: Vec<&str> = path.split('/').collect();
    let file_name = components.last().copied().unwrap_or(path);
    let dirs = &components[..components.len().saturating_sub(1)];

    if FORBIDDEN_FILE_NAMES.contains(&file_name) {
        return Some("agent/tool file; shared rules belong in MAINTAINERS.md");
    }
    if dirs
        .iter()
        .any(|dir| FORBIDDEN_DIRS.contains(dir) || dir.starts_with(".aider"))
        || file_name.starts_with(".aider")
    {
        return Some("agent/tool directory");
    }
    if let Some(top) = dirs.first()
        && FORBIDDEN_TOP_LEVEL_DIRS.contains(top)
    {
        return Some("generated output directory");
    }
    let generated = file_name.ends_with(".tracy")
        || file_name.starts_with("perf.data")
        || (file_name.starts_with("flamegraph") && file_name.ends_with(".svg"))
        || file_name.ends_with(".profraw")
        || file_name.ends_with(".profdata")
        || file_name.starts_with("heaptrack.")
        || file_name == "dhat-heap.json"
        || (file_name.starts_with("cargo-timing") && file_name.ends_with(".html"))
        || (file_name.starts_with("gate-report") && file_name.ends_with(".json"));
    if generated {
        return Some("profiling output");
    }
    None
}

/// Lists tracked files via `git ls-files` and returns the forbidden ones,
/// each with the reason.
pub(crate) fn check(root: &Path) -> Result<Vec<(String, &'static str)>, String> {
    let output = Command::new("git")
        .args(["ls-files", "-z"])
        .current_dir(root)
        .output()
        .map_err(|err| format!("failed to run `git ls-files`: {err}"))?;
    if !output.status.success() {
        return Err(format!(
            "`git ls-files` failed:\n{}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let listing = String::from_utf8_lossy(&output.stdout);
    Ok(listing
        .split('\0')
        .filter(|path| !path.is_empty())
        .filter_map(|path| forbidden_reason(path).map(|reason| (path.to_owned(), reason)))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::forbidden_reason;

    #[test]
    fn agent_files_are_forbidden_anywhere() {
        for path in [
            "CLAUDE.md",
            "crates/imaginarium-ui/CLAUDE.md",
            "AGENTS.md",
            "CLAUDE.local.md",
            ".claude/settings.local.json",
            "crates/x/.cursor/rules.md",
            ".aider.chat.history.md",
            ".aider/cache",
            ".cursorrules",
        ] {
            assert!(
                forbidden_reason(path).is_some(),
                "{path} should be forbidden"
            );
        }
    }

    #[test]
    fn generated_artifacts_are_forbidden() {
        for path in [
            "artifacts/screenshot.png",
            "gate-report.json",
            "profiling/run1.json",
            "target/debug/x",
            "capture.tracy",
            "perf.data.old",
            "flamegraph.svg",
            "default.profraw",
            "dhat-heap.json",
            "cargo-timing-20261012.html",
        ] {
            assert!(
                forbidden_reason(path).is_some(),
                "{path} should be forbidden"
            );
        }
    }

    #[test]
    fn normal_project_files_are_allowed() {
        for path in [
            "MAINTAINERS.md",
            "README.md",
            "crates/imaginarium-render/tests/golden/shapes.png",
            "docs/design/file-format.md",
            "crates/imaginarium-profiling/src/lib.rs",
            "website/src/content/docs/profiling.md",
            "docs/artifacts-policy.md",
            ".github/workflows/ci.yml",
            ".cargo/config.toml",
        ] {
            assert_eq!(forbidden_reason(path), None, "{path} should be allowed");
        }
    }
}
