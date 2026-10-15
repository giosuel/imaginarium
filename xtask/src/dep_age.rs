// SPDX-License-Identifier: AGPL-3.0-or-later

//! The two-week rule (MAINTAINERS.md, "Dependency versions"): every crate
//! version in every committed `Cargo.lock` must have been public for at
//! least 14 days.
//!
//! Publish times come from the crates.io **sparse index** (the `pubtime`
//! field), which a CDN serves without the API's rate limit: one request per
//! crate covers all its versions. Index entries without `pubtime` predate the
//! field and are therefore old enough. The index is fetched with `curl`, so
//! the xtask needs no HTTP library.
//!
//! Exceptions (security fixes, and explicit owner exceptions) are listed in
//! `xtask/dep-age-exceptions.txt`, one `name version  # reason` per line.
//! Exceptions that are no longer needed (the version is old enough now, or
//! in no lockfile) are reported, to be removed.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;

/// A locked registry package.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Package {
    pub name: String,
    pub version: String,
}

/// One version of a crate in the sparse index.
#[derive(Clone, Debug)]
pub(crate) struct Published {
    pub version: String,
    /// Unix seconds, `None` if the entry predates the `pubtime` field.
    pub time: Option<i64>,
    pub yanked: bool,
}

/// A package that is too new, with the newest compatible version that isn't.
#[derive(Debug, PartialEq)]
pub(crate) struct Finding {
    pub package: Package,
    pub age_days: i64,
    pub published: String,
    pub suggestion: Option<String>,
}

/// The registry packages of a `Cargo.lock` (line-based: the format is
/// stable and simple, so this needs no TOML parser).
pub(crate) fn parse_lock(text: &str) -> Vec<Package> {
    let mut packages = Vec::new();
    let (mut name, mut version, mut registry) = (None, None, false);
    let mut flush =
        |name: &mut Option<String>, version: &mut Option<String>, registry: &mut bool| {
            if let (Some(n), Some(v), true) = (name.take(), version.take(), *registry) {
                packages.push(Package {
                    name: n,
                    version: v,
                });
            }
            *registry = false;
        };
    for line in text.lines() {
        let line = line.trim();
        if line == "[[package]]" {
            flush(&mut name, &mut version, &mut registry);
        } else if let Some(value) = string_field(line, "name") {
            name = Some(value);
        } else if let Some(value) = string_field(line, "version") {
            version = Some(value);
        } else if let Some(value) = string_field(line, "source") {
            registry = value.starts_with("registry+");
        }
    }
    flush(&mut name, &mut version, &mut registry);
    packages
}

fn string_field(line: &str, key: &str) -> Option<String> {
    let rest = line
        .strip_prefix(key)?
        .trim_start()
        .strip_prefix('=')?
        .trim();
    Some(rest.strip_prefix('"')?.strip_suffix('"')?.to_owned())
}

/// Parses the sparse index file of one crate (one JSON object per line).
pub(crate) fn parse_index(text: &str) -> Vec<Published> {
    text.lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter_map(|entry| {
            Some(Published {
                version: entry.get("vers")?.as_str()?.to_owned(),
                time: entry
                    .get("pubtime")
                    .and_then(|t| t.as_str())
                    .and_then(parse_rfc3339),
                yanked: entry
                    .get("yanked")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false),
            })
        })
        .collect()
}

/// Unix seconds of a `YYYY-MM-DDTHH:MM:SSZ` timestamp.
pub(crate) fn parse_rfc3339(text: &str) -> Option<i64> {
    let bytes = text.as_bytes();
    if bytes.len() < 20 || bytes[4] != b'-' || bytes[7] != b'-' || bytes[10] != b'T' {
        return None;
    }
    let number = |range: std::ops::Range<usize>| text.get(range)?.parse::<i64>().ok();
    let (year, month, day) = (number(0..4)?, number(5..7)?, number(8..10)?);
    let (hour, minute, second) = (number(11..13)?, number(14..16)?, number(17..19)?);
    Some(days_from_civil(year, month, day) * 86_400 + hour * 3600 + minute * 60 + second)
}

/// Days since 1970-01-01 (Howard Hinnant's `days_from_civil`).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let day_of_year = (153 * ((month + 9) % 12) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// `(major, minor, patch)` of a release version; `None` for pre-releases.
fn release(version: &str) -> Option<(u64, u64, u64)> {
    let core = version.split('+').next()?;
    if core.contains('-') {
        return None;
    }
    let mut parts = core.split('.').map(|p| p.parse::<u64>().ok());
    Some((parts.next()??, parts.next()??, parts.next()??))
}

/// Whether `candidate` is semver-compatible with `current` (cargo's rules).
fn compatible(current: (u64, u64, u64), candidate: (u64, u64, u64)) -> bool {
    match current {
        (0, 0, patch) => candidate == (0, 0, patch),
        (0, minor, _) => candidate.0 == 0 && candidate.1 == minor,
        (major, _, _) => candidate.0 == major,
    }
}

/// The packages that are younger than `min_days` at `now`.
pub(crate) fn evaluate(
    packages: &BTreeSet<Package>,
    index: &BTreeMap<String, Vec<Published>>,
    exceptions: &BTreeSet<Package>,
    now: i64,
    min_days: i64,
) -> Result<Vec<Finding>, String> {
    let cutoff = now - min_days * 86_400;
    let mut findings = Vec::new();
    for package in packages {
        if exceptions.contains(package) {
            continue;
        }
        let versions = index
            .get(&package.name)
            .ok_or_else(|| format!("{} isn't in the index", package.name))?;
        let entry = versions
            .iter()
            .find(|v| v.version == package.version)
            .ok_or_else(|| format!("{} {} isn't in the index", package.name, package.version))?;
        let Some(time) = entry.time else { continue };
        if time <= cutoff {
            continue;
        }
        let suggestion = release(&package.version).and_then(|current| {
            versions
                .iter()
                .filter(|v| !v.yanked && v.time.is_none_or(|t| t <= cutoff))
                .filter_map(|v| Some((release(&v.version)?, v)))
                .filter(|(r, _)| compatible(current, *r) && *r < current)
                .max_by_key(|(r, _)| *r)
                .map(|(_, v)| v.version.clone())
        });
        findings.push(Finding {
            package: package.clone(),
            age_days: (now - time) / 86_400,
            published: format_date(time),
            suggestion,
        });
    }
    Ok(findings)
}

/// Exceptions that no longer do anything: the version is in no lockfile,
/// or old enough by now. With why.
pub(crate) fn unneeded_exceptions(
    packages: &BTreeSet<Package>,
    index: &BTreeMap<String, Vec<Published>>,
    exceptions: &BTreeSet<Package>,
    now: i64,
    min_days: i64,
) -> Vec<(Package, &'static str)> {
    let cutoff = now - min_days * 86_400;
    exceptions
        .iter()
        .filter_map(|exception| {
            if !packages.contains(exception) {
                return Some((exception.clone(), "in no lockfile"));
            }
            let time = index
                .get(&exception.name)?
                .iter()
                .find(|v| v.version == exception.version)?
                .time;
            time.is_none_or(|time| time <= cutoff)
                .then(|| (exception.clone(), "old enough now"))
        })
        .collect()
}

fn format_date(unix: i64) -> String {
    // civil_from_days (inverse of days_from_civil).
    let z = unix.div_euclid(86_400) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

/// Exceptions: `name version  # reason` per line.
pub(crate) fn parse_exceptions(text: &str) -> Vec<Package> {
    text.lines()
        .map(|line| line.split('#').next().unwrap_or("").trim())
        .filter(|line| !line.is_empty())
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            Some(Package {
                name: parts.next()?.to_owned(),
                version: parts.next()?.to_owned(),
            })
        })
        .collect()
}

/// The sparse index path of a crate name.
pub(crate) fn index_path(name: &str) -> String {
    let n = name.to_lowercase();
    match n.len() {
        1 | 2 => format!("{}/{n}", n.len()),
        3 => format!("3/{}/{n}", &n[..1]),
        _ => format!("{}/{}/{n}", &n[..2], &n[2..4]),
    }
}

/// Fetches the index files of `names`, eight at a time.
fn fetch_index(names: &BTreeSet<String>) -> Result<BTreeMap<String, Vec<Published>>, String> {
    let names: Vec<&String> = names.iter().collect();
    let mut index = BTreeMap::new();
    for batch in names.chunks(8) {
        let results: Vec<(String, Result<String, String>)> = std::thread::scope(|scope| {
            let handles: Vec<_> = batch
                .iter()
                .map(|name| {
                    let url = format!("https://index.crates.io/{}", index_path(name));
                    (name.to_string(), scope.spawn(move || curl(&url)))
                })
                .collect();
            handles
                .into_iter()
                .map(|(name, handle)| {
                    (
                        name,
                        handle
                            .join()
                            .unwrap_or_else(|_| Err("fetch thread panicked".into())),
                    )
                })
                .collect()
        });
        for (name, result) in results {
            index.insert(name, parse_index(&result?));
        }
    }
    Ok(index)
}

fn curl(url: &str) -> Result<String, String> {
    let output = Command::new("curl")
        .args([
            "--silent",
            "--show-error",
            "--fail",
            "--location",
            "--max-time",
            "60",
            url,
        ])
        .output()
        .map_err(|err| format!("can't run curl: {err}"))?;
    if !output.status.success() {
        return Err(format!(
            "{url}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    String::from_utf8(output.stdout).map_err(|_| format!("{url}: not UTF-8"))
}

/// The committed lockfiles (`git ls-files`), relative to `root`.
fn lockfiles(root: &Path) -> Result<Vec<PathBuf>, String> {
    let output = Command::new("git")
        .args(["ls-files", "--", "Cargo.lock", "*/Cargo.lock"])
        .current_dir(root)
        .output()
        .map_err(|err| format!("can't run git: {err}"))?;
    if !output.status.success() {
        return Err("`git ls-files` failed".into());
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|l| root.join(l))
        .collect())
}

/// What a check found.
pub(crate) struct Report {
    pub(crate) files: usize,
    pub(crate) packages: usize,
    /// Versions that are too new.
    pub(crate) findings: Vec<Finding>,
    /// Exceptions to remove, with why.
    pub(crate) unneeded: Vec<(Package, &'static str)>,
}

/// Runs the check, or `Err` if it couldn't run.
pub(crate) fn check(root: &Path, min_days: i64) -> Result<Report, String> {
    let files = lockfiles(root)?;
    let mut packages = BTreeSet::new();
    for file in &files {
        let text =
            std::fs::read_to_string(file).map_err(|err| format!("{}: {err}", file.display()))?;
        packages.extend(parse_lock(&text));
    }
    let exceptions_path = root.join("xtask/dep-age-exceptions.txt");
    let exceptions: BTreeSet<Package> = std::fs::read_to_string(&exceptions_path)
        .map(|text| parse_exceptions(&text).into_iter().collect())
        .unwrap_or_default();
    let names: BTreeSet<String> = packages.iter().map(|p| p.name.clone()).collect();
    let index = fetch_index(&names)?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|err| err.to_string())?
        .as_secs();
    let now = i64::try_from(now).map_err(|err| err.to_string())?;
    let findings = evaluate(&packages, &index, &exceptions, now, min_days)?;
    let unneeded = unneeded_exceptions(&packages, &index, &exceptions, now, min_days);
    Ok(Report {
        files: files.len(),
        packages: packages.len(),
        findings,
        unneeded,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOCK: &str = r#"
version = 4

[[package]]
name = "serde"
version = "1.0.229"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "abc"

[[package]]
name = "imaginarium-model"
version = "0.0.1"
dependencies = ["serde"]

[[package]]
name = "zerocopy"
version = "0.8.59"
source = "registry+https://github.com/rust-lang/crates.io-index"
"#;

    fn package(name: &str, version: &str) -> Package {
        Package {
            name: name.into(),
            version: version.into(),
        }
    }

    #[test]
    fn lockfiles_yield_registry_packages_only() {
        assert_eq!(
            parse_lock(LOCK),
            vec![package("serde", "1.0.229"), package("zerocopy", "0.8.59")]
        );
    }

    #[test]
    fn timestamps_and_dates() {
        assert_eq!(parse_rfc3339("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_rfc3339("2026-10-11T15:31:46Z"), Some(1_791_732_706));
        assert_eq!(format_date(1_791_732_706), "2026-10-11");
        assert_eq!(parse_rfc3339("yesterday"), None);
    }

    #[test]
    fn index_paths_follow_the_sparse_layout() {
        assert_eq!(index_path("a"), "1/a");
        assert_eq!(index_path("cc"), "2/cc");
        assert_eq!(index_path("syn"), "3/s/syn");
        assert_eq!(index_path("Serde_JSON"), "se/rd/serde_json");
    }

    #[test]
    fn too_new_versions_are_found_with_a_suggestion() {
        let day = 86_400;
        let now = 100 * day;
        let index: BTreeMap<String, Vec<Published>> = [
            (
                "zerocopy".to_owned(),
                vec![
                    Published {
                        version: "0.8.57".into(),
                        time: Some(70 * day),
                        yanked: false,
                    },
                    Published {
                        version: "0.8.58".into(),
                        time: Some(80 * day),
                        yanked: true,
                    },
                    Published {
                        version: "0.8.59".into(),
                        time: Some(95 * day),
                        yanked: false,
                    },
                    Published {
                        version: "0.9.0".into(),
                        time: Some(10 * day),
                        yanked: false,
                    },
                ],
            ),
            (
                "serde".to_owned(),
                vec![Published {
                    version: "1.0.229".into(),
                    time: None,
                    yanked: false,
                }],
            ),
        ]
        .into_iter()
        .collect();
        let packages: BTreeSet<Package> = parse_lock(LOCK).into_iter().collect();
        let findings = evaluate(&packages, &index, &BTreeSet::new(), now, 14).unwrap();
        assert_eq!(findings.len(), 1, "serde has no pubtime, so it is old");
        assert_eq!(findings[0].package, package("zerocopy", "0.8.59"));
        assert_eq!(findings[0].age_days, 5);
        assert_eq!(
            findings[0].suggestion.as_deref(),
            Some("0.8.57"),
            "not yanked, compatible, old enough"
        );

        let exceptions: BTreeSet<Package> =
            parse_exceptions("zerocopy 0.8.59  # RUSTSEC fix\n# comment\n")
                .into_iter()
                .collect();
        assert!(
            evaluate(&packages, &index, &exceptions, now, 14)
                .unwrap()
                .is_empty()
        );
        assert!(
            evaluate(&packages, &BTreeMap::new(), &BTreeSet::new(), now, 14).is_err(),
            "missing crates are errors"
        );
        assert!(unneeded_exceptions(&packages, &index, &exceptions, now, 14).is_empty());
        // Ten days later it is old enough, and the exception can go; so
        // can one for a version no lockfile has.
        let more: BTreeSet<Package> =
            parse_exceptions("zerocopy 0.8.59  # RUSTSEC fix\nserde 1.0.300  # gone\n")
                .into_iter()
                .collect();
        let unneeded = unneeded_exceptions(&packages, &index, &more, now + 10 * day, 14);
        assert_eq!(
            unneeded,
            [
                (package("serde", "1.0.300"), "in no lockfile"),
                (package("zerocopy", "0.8.59"), "old enough now"),
            ]
        );
    }
}
