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

    check_age(LUCIDE_VERSION)?;
    let work = root.join("target/xtask-icons");
    std::fs::create_dir_all(&work)
        .map_err(|err| format!("can't create {}: {err}", work.display()))?;
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
    run_command(
        Command::new("tar")
            .arg("xzf")
            .arg(&tarball)
            .arg("-C")
            .arg(&work),
        "tar",
    )?;
    let nodes = std::fs::read_to_string(work.join("package/icon-nodes.json"))
        .map_err(|err| format!("the release has no icon-nodes.json: {err}"))?;
    let nodes: Value =
        serde_json::from_str(&nodes).map_err(|err| format!("icon-nodes.json: {err}"))?;
    let license = std::fs::read_to_string(work.join("package/LICENSE"))
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

/// Refuses a release younger than 14 days.
fn check_age(version: &str) -> Result<(), String> {
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
    let age = days_since(published).ok_or_else(|| format!("can't read the date `{published}`"))?;
    if age < MIN_AGE_DAYS {
        return Err(format!(
            "lucide-static {version} is {age} days old; the rule is {MIN_AGE_DAYS} (MAINTAINERS.md)"
        ));
    }
    Ok(())
}

/// Whole days from an RFC 3339 date (`2026-10-02T11:32:31.773Z`) to now.
fn days_since(date: &str) -> Option<i64> {
    let day = |text: &str| -> Option<i64> {
        let mut parts = text.get(..10)?.split('-');
        let (y, m, d) = (
            parts.next()?.parse::<i64>().ok()?,
            parts.next()?.parse::<i64>().ok()?,
            parts.next()?.parse::<i64>().ok()?,
        );
        // Days since 1970-01-01 (civil calendar, Howard Hinnant's method).
        let y = if m <= 2 { y - 1 } else { y };
        let era = y.div_euclid(400);
        let yoe = y - era * 400;
        let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        Some(era * 146_097 + doe - 719_468)
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs() as i64
        / 86_400;
    Some(now - day(date)?)
}

/// `lucide.rs`: each listed icon as SVG path strings, sorted by name.
fn generate(names: &[String], nodes: &Value) -> Result<String, String> {
    let mut out = String::new();
    let _ = writeln!(out, "// SPDX-License-Identifier: AGPL-3.0-or-later");
    let _ = writeln!(
        out,
        "// The icon data is Lucide's, under the ISC license in LICENSE-lucide."
    );
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "//! Generated by `cargo xtask icons` from lucide-static {LUCIDE_VERSION} (ISC, see\n//! `LICENSE-lucide`). Don't edit: change `icons.txt` and run the task again."
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
            let rx = attr("rx")?;
            let ry = if element[1]["ry"].is_null() {
                rx
            } else {
                attr("ry")?
            };
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
    fn days_are_counted_from_the_epoch() {
        assert!(days_since("1970-01-02T00:00:00Z").unwrap() > 20_000);
        assert!(days_since("not a date").is_none());
    }
}
