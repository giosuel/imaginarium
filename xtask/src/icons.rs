// SPDX-License-Identifier: AGPL-3.0-or-later

//! `cargo xtask icons`: vendors the Lucide icons the app uses.
//!
//! Reads the names from `crates/imaginarium-ui/src/icons/icons.txt`,
//! downloads the pinned `lucide-static` release from npm (with `curl` and
//! `tar`, like `dep-age`), refuses it if it is younger than 14 days (the
//! dependency rule in MAINTAINERS.md), converts every icon's shapes
//! (circles, rectangles, lines, polygons) to SVG path data, and writes
//! `lucide.rs` and Lucide's license next to the list. Only the listed
//! icons are kept.

use std::fmt::Write as _;
use std::path::Path;
use std::process::Command;

use serde_json::Value;

/// The pinned release (2026-10-02). Change it to update the icons; the
/// task checks its age.
pub(crate) const LUCIDE_VERSION: &str = "1.50.0";

const MIN_AGE_DAYS: i64 = 14;

/// Runs the task in the workspace at `root`.
pub(crate) fn run(root: &Path) -> Result<usize, String> {
    let dir = root.join("crates/imaginarium-ui/src/icons");
    let list = std::fs::read_to_string(dir.join("icons.txt"))
        .map_err(|err| format!("can't read icons.txt: {err}"))?;
    let names = parse_list(&list)?;

    let integrity = check_release(LUCIDE_VERSION)?;
    // A fresh folder per run, so a file missing from this release can't be
    // read from an earlier one.
    let work = root.join("target/xtask-icons");
    let unpacked = work.join(format!("lucide-static-{LUCIDE_VERSION}"));
    if unpacked.exists() {
        std::fs::remove_dir_all(&unpacked)
            .map_err(|err| format!("can't remove {}: {err}", unpacked.display()))?;
    }
    std::fs::create_dir_all(&unpacked)
        .map_err(|err| format!("can't create {}: {err}", unpacked.display()))?;
    let tarball = work.join(format!("lucide-static-{LUCIDE_VERSION}.tgz"));
    let url =
        format!("https://registry.npmjs.org/lucide-static/-/lucide-static-{LUCIDE_VERSION}.tgz");
    run_command(
        Command::new("curl")
            .args(["-fsSL", "--retry", "2", "-o"])
            .arg(&tarball)
            .arg(&url),
        "curl",
    )?;
    check_integrity(&tarball, &integrity)?;
    run_command(
        Command::new("tar")
            .arg("xzf")
            .arg(&tarball)
            .arg("-C")
            .arg(&unpacked),
        "tar",
    )?;
    let package = unpacked.join("package");
    let nodes = std::fs::read_to_string(package.join("icon-nodes.json"))
        .map_err(|err| format!("the release has no icon-nodes.json: {err}"))?;
    let nodes: Value =
        serde_json::from_str(&nodes).map_err(|err| format!("icon-nodes.json: {err}"))?;
    let license = std::fs::read_to_string(package.join("LICENSE"))
        .map_err(|err| format!("the release has no LICENSE: {err}"))?;

    let source = generate(&names, &nodes)?;
    std::fs::write(dir.join("lucide.rs"), source)
        .map_err(|err| format!("can't write lucide.rs: {err}"))?;
    std::fs::write(dir.join("LICENSE-lucide"), license)
        .map_err(|err| format!("can't write LICENSE-lucide: {err}"))?;
    Ok(names.len())
}

/// The names in `icons.txt`: one per line, `#` comments, no duplicates.
fn parse_list(list: &str) -> Result<Vec<String>, String> {
    let mut names: Vec<String> = list
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(str::to_owned)
        .collect();
    names.sort();
    if let Some(pair) = names.windows(2).find(|pair| pair[0] == pair[1]) {
        return Err(format!("icons.txt lists `{}` twice", pair[0]));
    }
    Ok(names)
}

fn run_command(command: &mut Command, what: &str) -> Result<(), String> {
    let output = command
        .output()
        .map_err(|err| format!("can't run {what}: {err}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "{what} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

/// Refuses a release younger than 14 days, and returns the tarball's
/// `sha512-…` integrity from npm.
fn check_release(version: &str) -> Result<String, String> {
    let output = Command::new("curl")
        .args([
            "-fsSL",
            "--retry",
            "2",
            "https://registry.npmjs.org/lucide-static",
        ])
        .output()
        .map_err(|err| format!("can't run curl: {err}"))?;
    if !output.status.success() {
        return Err("can't read lucide-static's release dates from npm".into());
    }
    let meta: Value =
        serde_json::from_slice(&output.stdout).map_err(|err| format!("npm metadata: {err}"))?;
    let published = meta["time"][version]
        .as_str()
        .ok_or_else(|| format!("npm doesn't know lucide-static {version}"))?;
    let published = crate::dep_age::parse_rfc3339(published)
        .ok_or_else(|| format!("can't read the date `{published}`"))?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|err| format!("the clock is before 1970: {err}"))?
        .as_secs() as i64;
    if !old_enough(published, now) {
        let days = (now - published) / 86_400;
        return Err(format!(
            "lucide-static {version} is {days} days old; the rule is {MIN_AGE_DAYS} (MAINTAINERS.md)"
        ));
    }
    meta["versions"][version]["dist"]["integrity"]
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| format!("npm has no integrity for lucide-static {version}"))
}

/// Whether a release published at `published` (Unix seconds) is at least
/// 14 full days old at `now`.
fn old_enough(published: i64, now: i64) -> bool {
    now - published >= MIN_AGE_DAYS * 86_400
}

/// Checks the downloaded tarball against npm's `sha512-<base64>`.
fn check_integrity(tarball: &Path, integrity: &str) -> Result<(), String> {
    let expected = integrity
        .strip_prefix("sha512-")
        .ok_or_else(|| format!("unexpected integrity `{integrity}`"))?;
    let output = Command::new("sha512sum")
        .arg(tarball)
        .output()
        .map_err(|err| format!("can't run sha512sum: {err}"))?;
    let text = String::from_utf8_lossy(&output.stdout);
    let hex = text.split_whitespace().next().unwrap_or_default();
    let digest = (0..hex.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(hex.get(at..at + 2)?, 16).ok())
        .collect::<Option<Vec<u8>>>()
        .filter(|digest| output.status.success() && digest.len() == 64)
        .ok_or("sha512sum failed")?;
    if base64(&digest) == expected {
        Ok(())
    } else {
        Err("the downloaded tarball doesn't match npm's sha512".into())
    }
}

/// Standard base64 with padding.
fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let value = chunk.iter().enumerate().fold(0u32, |value, (index, byte)| {
            value | u32::from(*byte) << (16 - 8 * index)
        });
        for index in 0..4 {
            if index <= chunk.len() {
                out.push(char::from(
                    ALPHABET[(value >> (18 - 6 * index) & 63) as usize],
                ));
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// `lucide.rs`: each listed icon as SVG path strings, sorted by name.
fn generate(names: &[String], nodes: &Value) -> Result<String, String> {
    let mut out = String::new();
    let _ = writeln!(out, "{}", crate::headers::VENDORED_HEADER);
    let _ = writeln!(
        out,
        "// The icon data is Lucide's (ISC) and, for the icons from Feather, MIT;\n// both notices are in LICENSE-lucide."
    );
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "//! Generated by `cargo xtask icons` from lucide-static {LUCIDE_VERSION} (ISC and MIT,\n//! see `LICENSE-lucide`). Don't edit: change `icons.txt` and run the task again."
    );
    let _ = writeln!(out);
    let _ = writeln!(out, "/// The Lucide release these come from.");
    let _ = writeln!(
        out,
        "pub const LUCIDE_VERSION: &str = \"{LUCIDE_VERSION}\";"
    );
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "/// Every vendored icon by name, sorted, as path data on a 24-unit grid."
    );
    let _ = writeln!(out, "#[rustfmt::skip]");
    let _ = writeln!(out, "pub(super) static ICONS: &[(&str, &[&str])] = &[");
    for name in names {
        let elements = nodes[name.as_str()]
            .as_array()
            .ok_or_else(|| format!("lucide-static {LUCIDE_VERSION} has no icon `{name}`"))?;
        let mut paths = Vec::new();
        for element in elements {
            paths.push(element_to_path(element).map_err(|err| format!("{name}: {err}"))?);
        }
        let quoted: Vec<String> = paths.iter().map(|path| format!("{path:?}")).collect();
        let _ = writeln!(out, "    ({name:?}, &[{}]),", quoted.join(", "));
    }
    let _ = writeln!(out, "];");
    Ok(out)
}

/// One SVG element (`["circle", {"cx": "12", …}]`) as path data.
fn element_to_path(element: &Value) -> Result<String, String> {
    let kind = element[0].as_str().ok_or("an element without a kind")?;
    let attr = |key: &str| -> Result<f64, String> {
        element[1][key]
            .as_str()
            .unwrap_or("0")
            .parse::<f64>()
            .map_err(|_| format!("{kind}: `{key}` isn't a number"))
    };
    let n = |value: f64| {
        let text = format!("{value:.3}");
        let text = text.trim_end_matches('0').trim_end_matches('.');
        if text == "-0" {
            "0".to_owned()
        } else {
            text.to_owned()
        }
    };
    Ok(match kind {
        "path" => element[1]["d"]
            .as_str()
            .ok_or("a path without `d`")?
            .to_owned(),
        "circle" | "ellipse" => {
            let (cx, cy) = (attr("cx")?, attr("cy")?);
            let (rx, ry) = if kind == "circle" {
                (attr("r")?, attr("r")?)
            } else {
                (attr("rx")?, attr("ry")?)
            };
            // Two half arcs around the center.
            format!(
                "M{} {}A{} {} 0 1 0 {} {}A{} {} 0 1 0 {} {}Z",
                n(cx - rx),
                n(cy),
                n(rx),
                n(ry),
                n(cx + rx),
                n(cy),
                n(rx),
                n(ry),
                n(cx - rx),
                n(cy)
            )
        }
        "rect" => {
            let (x, y, w, h) = (attr("x")?, attr("y")?, attr("width")?, attr("height")?);
            // A missing radius takes the other's value (SVG).
            let radius = |key: &str, other: &str| {
                if element[1][key].is_null() {
                    attr(other)
                } else {
                    attr(key)
                }
            };
            let (rx, ry) = (radius("rx", "ry")?, radius("ry", "rx")?);
            let (rx, ry) = (rx.min(w / 2.0), ry.min(h / 2.0));
            if rx <= 0.0 && ry <= 0.0 {
                format!("M{} {}H{}V{}H{}Z", n(x), n(y), n(x + w), n(y + h), n(x))
            } else {
                let arc =
                    |ex: f64, ey: f64| format!("A{} {} 0 0 1 {} {}", n(rx), n(ry), n(ex), n(ey));
                format!(
                    "M{} {}H{}{}V{}{}H{}{}V{}{}Z",
                    n(x + rx),
                    n(y),
                    n(x + w - rx),
                    arc(x + w, y + ry),
                    n(y + h - ry),
                    arc(x + w - rx, y + h),
                    n(x + rx),
                    arc(x, y + h - ry),
                    n(y + ry),
                    arc(x + rx, y)
                )
            }
        }
        "line" => format!(
            "M{} {}L{} {}",
            n(attr("x1")?),
            n(attr("y1")?),
            n(attr("x2")?),
            n(attr("y2")?)
        ),
        "polyline" | "polygon" => {
            let points = element[1]["points"].as_str().ok_or("no `points`")?;
            let numbers: Vec<f64> = points
                .split(|c: char| c == ',' || c.is_whitespace())
                .filter(|part| !part.is_empty())
                .map(|part| {
                    part.parse::<f64>()
                        .map_err(|_| format!("bad point `{part}`"))
                })
                .collect::<Result<_, _>>()?;
            let mut path = String::new();
            for (index, pair) in numbers.chunks(2).enumerate() {
                let [px, py] = pair else {
                    return Err("an odd number of coordinates".into());
                };
                let _ = write!(
                    path,
                    "{}{} {}",
                    if index == 0 { "M" } else { "L" },
                    n(*px),
                    n(*py)
                );
            }
            if kind == "polygon" {
                path.push('Z');
            }
            path
        }
        other => return Err(format!("unknown element `{other}`")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shapes_become_paths() {
        let circle = serde_json::json!(["circle", {"cx": "12", "cy": "12", "r": "10"}]);
        assert_eq!(
            element_to_path(&circle).unwrap(),
            "M2 12A10 10 0 1 0 22 12A10 10 0 1 0 2 12Z"
        );
        let rect = serde_json::json!(["rect", {"x": "3", "y": "3", "width": "18", "height": "18", "rx": "2"}]);
        assert_eq!(
            element_to_path(&rect).unwrap(),
            "M5 3H19A2 2 0 0 1 21 5V19A2 2 0 0 1 19 21H5A2 2 0 0 1 3 19V5A2 2 0 0 1 5 3Z"
        );
        // Only `ry` given: `rx` takes its value.
        let rect = serde_json::json!(["rect", {"x": "3", "y": "3", "width": "18", "height": "18", "ry": "2"}]);
        assert_eq!(
            element_to_path(&rect).unwrap(),
            "M5 3H19A2 2 0 0 1 21 5V19A2 2 0 0 1 19 21H5A2 2 0 0 1 3 19V5A2 2 0 0 1 5 3Z"
        );
        let line = serde_json::json!(["line", {"x1": "5", "y1": "12", "x2": "19", "y2": "12"}]);
        assert_eq!(element_to_path(&line).unwrap(), "M5 12L19 12");
        let polygon = serde_json::json!(["polygon", {"points": "12 2 22 22 2 22"}]);
        assert_eq!(element_to_path(&polygon).unwrap(), "M12 2L22 22L2 22Z");
    }

    #[test]
    fn the_list_is_sorted_and_without_duplicates() {
        assert_eq!(
            parse_list("# tools\nx\n\nhand\n").unwrap(),
            vec!["hand", "x"]
        );
        assert!(parse_list("x\nx\n").is_err());
    }

    #[test]
    fn a_release_must_be_fourteen_full_days_old() {
        let published = crate::dep_age::parse_rfc3339("2026-10-02T23:59:00Z").unwrap();
        let almost = crate::dep_age::parse_rfc3339("2026-10-16T00:01:00Z").unwrap();
        assert!(!old_enough(published, almost), "13 days and 2 minutes");
        assert!(old_enough(published, published + 14 * 86_400));
    }

    #[test]
    fn the_vendored_icons_come_from_the_pinned_release() {
        let generated = include_str!("../../crates/imaginarium-ui/src/icons/lucide.rs");
        let pinned = format!("pub const LUCIDE_VERSION: &str = \"{LUCIDE_VERSION}\";");
        assert!(generated.contains(&pinned), "run `cargo xtask icons`");
    }

    #[test]
    fn base64_matches_the_standard() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }
}
