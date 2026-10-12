// SPDX-License-Identifier: AGPL-3.0-or-later

//! TOML persistence for a settings scope (used for [`Scope::App`]).
//!
//! The file is a convenience for backup and portability. It is **never
//! required** (ADR-0014): the app works without it and the UI edits every
//! value. Rules:
//! - Only values that differ from the default are written.
//! - Dotted keys become nested tables: `ui.theme` → `[ui] theme = "dark"`.
//! - Unknown entries (e.g. from a newer app version) are kept and written
//!   back unchanged, like unknown data in `.imag` files.
//! - Invalid entries are skipped and reported. They never prevent startup.
//! - Secrets appear only as keychain references, written as a small table:
//!
//! ```toml
//! [ai.anthropic_api_key]
//! keychain = "ai.anthropic"
//! ```
//!
//!   An inline table (`api_key = { keychain = "…" }`) is accepted too.

use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use toml::{Table, Value as Toml};

use crate::error::SettingsError;
use crate::registry::Registry;
use crate::setting::Scope;
use crate::value::{ListKind, Rgba, SecretRef, Value, ValueKind};

/// What happened while loading a settings file.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LoadReport {
    /// Keys whose values were applied.
    pub applied: Vec<&'static str>,
    /// Entries that were skipped because they didn't validate: `(key, reason)`.
    pub invalid: Vec<(String, String)>,
    /// Entries for unregistered keys or other scopes. They are preserved.
    pub unknown: Vec<String>,
}

/// First line of every written settings file.
const HEADER: &str = "# Imaginarium settings. Edit them in the app (Settings), not here.\n\
                      # Only changed values are stored. Unknown entries are preserved.\n\n";

impl Registry {
    /// Serializes the modified values of `scope`, plus preserved unknown
    /// entries, as TOML.
    pub fn to_toml(&self, scope: Scope) -> String {
        let mut root = Table::new();
        for (key, value) in self.modified_in(scope) {
            insert_path(&mut root, key, value_to_toml(value));
        }
        if let Some(unknown) = self.unknown.get(&scope) {
            for (key, value) in unknown {
                insert_path(&mut root, key, value.clone());
            }
        }
        format!("{HEADER}{root}")
    }

    /// Replaces the values of `scope` with the contents of a TOML document.
    ///
    /// Keys of `scope` that are missing from the document are reset to
    /// their defaults. Observers are notified for every effective change.
    /// Only a syntactically broken document is an error. Bad entries are
    /// reported in the [`LoadReport`].
    pub fn load_toml(&mut self, scope: Scope, text: &str) -> Result<LoadReport, SettingsError> {
        let root: Table = text
            .parse()
            .map_err(|err: toml::de::Error| SettingsError::Parse(err.to_string()))?;
        let mut leaves = Vec::new();
        flatten(self, "", &root, &mut leaves);

        let mut report = LoadReport::default();
        let mut unknown = BTreeMap::new();
        let mut seen = Vec::new();
        for (key, raw) in leaves {
            let declaration = match self.declaration(&key) {
                Ok(declaration) if declaration.meta().scope == scope => declaration,
                _ => {
                    report.unknown.push(key.clone());
                    unknown.insert(key, raw);
                    continue;
                }
            };
            let applied = toml_to_value(declaration.kind(), &raw)
                .ok_or_else(|| format!("expected {:?}", declaration.kind()))
                .and_then(|value| self.set_value(&key, value).map_err(|err| err.to_string()));
            match applied {
                Ok(()) => {
                    report.applied.push(declaration.key());
                    seen.push(declaration.key());
                }
                Err(reason) => report.invalid.push((key, reason)),
            }
        }

        let stale: Vec<&'static str> = self
            .modified_in(scope)
            .into_keys()
            .filter(|key| !seen.contains(key))
            .collect();
        for key in stale {
            self.reset(key)?;
        }
        self.unknown.insert(scope, unknown);
        Ok(report)
    }

    /// Loads `scope` from a file. A missing file means "all defaults".
    pub fn load_file(&mut self, scope: Scope, path: &Path) -> Result<LoadReport, SettingsError> {
        match fs::read_to_string(path) {
            Ok(text) => self.load_toml(scope, &text),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => self.load_toml(scope, ""),
            Err(err) => Err(SettingsError::Io(format!("{}: {err}", path.display()))),
        }
    }

    /// Writes `scope` to a file atomically: write a temporary file next to
    /// it, flush it to disk, then rename it over the target. A crash never
    /// leaves a half-written settings file.
    pub fn save_file(&self, scope: Scope, path: &Path) -> Result<(), SettingsError> {
        let io = |err: std::io::Error| SettingsError::Io(format!("{}: {err}", path.display()));
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            fs::create_dir_all(parent).map_err(io)?;
        }
        let mut temp = path.as_os_str().to_owned();
        temp.push(".tmp");
        let temp = PathBuf::from(temp);
        let mut file = fs::File::create(&temp).map_err(io)?;
        file.write_all(self.to_toml(scope).as_bytes()).map_err(io)?;
        file.sync_all().map_err(io)?;
        drop(file);
        fs::rename(&temp, path).map_err(io)
    }
}

/// Collects `(dotted key, value)` leaves. A table is a leaf if its path is
/// a registered key (e.g. a `SecretRef` inline table), otherwise it is
/// descended into.
fn flatten(registry: &Registry, prefix: &str, table: &Table, out: &mut Vec<(String, Toml)>) {
    for (name, value) in table {
        let key = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}.{name}")
        };
        match value {
            Toml::Table(inner) if registry.declaration(&key).is_err() => {
                flatten(registry, &key, inner, out);
            }
            _ => out.push((key, value.clone())),
        }
    }
}

/// Inserts `value` at a dotted path, creating intermediate tables.
fn insert_path(root: &mut Table, key: &str, value: Toml) {
    let mut segments = key.split('.').peekable();
    let mut table = root;
    while let Some(segment) = segments.next() {
        if segments.peek().is_none() {
            table.insert(segment.to_owned(), value);
            return;
        }
        let entry = table
            .entry(segment.to_owned())
            .or_insert_with(|| Toml::Table(Table::new()));
        if !entry.is_table() {
            // Can't happen for registered keys (prefix clashes are rejected),
            // but a malformed unknown entry must not panic.
            *entry = Toml::Table(Table::new());
        }
        let Toml::Table(next) = entry else {
            return;
        };
        table = next;
    }
}

fn value_to_toml(value: &Value) -> Toml {
    match value {
        Value::Bool(b) => Toml::Boolean(*b),
        Value::Int(i) => Toml::Integer(*i),
        Value::Float(f) => Toml::Float(*f),
        Value::String(s) | Value::Enum(s) => Toml::String(s.clone()),
        Value::Color(c) => Toml::String(c.to_string()),
        // Non-UTF-8 paths are written lossily. Settings paths come from the
        // UI and are practically always UTF-8.
        Value::Path(p) => Toml::String(p.to_string_lossy().into_owned()),
        Value::SecretRef(secret) => {
            let mut table = Table::new();
            table.insert("keychain".to_owned(), Toml::String(secret.id.clone()));
            Toml::Table(table)
        }
        Value::List(items) => Toml::Array(items.iter().map(value_to_toml).collect()),
    }
}

fn toml_to_value(kind: ValueKind, raw: &Toml) -> Option<Value> {
    Some(match (kind, raw) {
        (ValueKind::Bool, Toml::Boolean(b)) => Value::Bool(*b),
        (ValueKind::Int, Toml::Integer(i)) => Value::Int(*i),
        (ValueKind::Float, Toml::Float(f)) => Value::Float(*f),
        (ValueKind::Float, Toml::Integer(i)) => Value::Float(*i as f64),
        (ValueKind::String, Toml::String(s)) => Value::String(s.clone()),
        (ValueKind::Enum, Toml::String(s)) => Value::Enum(s.clone()),
        (ValueKind::Color, Toml::String(s)) => Value::Color(s.parse::<Rgba>().ok()?),
        (ValueKind::Path, Toml::String(s)) => Value::Path(PathBuf::from(s)),
        (ValueKind::SecretRef, Toml::Table(table)) => Value::SecretRef(SecretRef {
            id: table.get("keychain")?.as_str()?.to_owned(),
        }),
        (ValueKind::List(element), Toml::Array(items)) => Value::List(
            items
                .iter()
                .map(|item| toml_to_value(list_element_kind(element), item))
                .collect::<Option<Vec<_>>>()?,
        ),
        _ => return None,
    })
}

fn list_element_kind(element: ListKind) -> ValueKind {
    match element {
        ListKind::Bool => ValueKind::Bool,
        ListKind::Int => ValueKind::Int,
        ListKind::Float => ValueKind::Float,
        ListKind::String => ValueKind::String,
        ListKind::Enum => ValueKind::Enum,
        ListKind::Color => ValueKind::Color,
        ListKind::Path => ValueKind::Path,
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::setting::Setting;
    use crate::setting_enum;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Theme {
        System,
        Dark,
    }
    setting_enum!(Theme { System => "system", "Follow system", Dark => "dark", "Dark" });

    static THEME: Setting<Theme> = Setting::new(
        "ui.theme",
        || Theme::System,
        Scope::App,
        "Theme",
        "Look and feel.",
    );
    static GRID: Setting<bool> = Setting::new(
        "canvas.grid.visible",
        || true,
        Scope::App,
        "Grid",
        "Show the grid.",
    );
    static ZOOM: Setting<f64> =
        Setting::new("canvas.zoom", || 1.0, Scope::App, "Zoom", "Zoom.").range(0.1, 8.0, 0.1);
    static RECENT: Setting<i64> = Setting::new(
        "files.recent_count",
        || 10,
        Scope::App,
        "Recent",
        "Files shown.",
    )
    .range(0, 50);
    static ACCENT: Setting<Rgba> = Setting::new(
        "ui.accent",
        || Rgba::rgb(0, 0, 255),
        Scope::App,
        "Accent",
        "Accent color.",
    );
    static FOLDER: Setting<PathBuf> = Setting::new(
        "files.folder",
        PathBuf::new,
        Scope::App,
        "Folder",
        "Default folder.",
    );
    static TAGS: Setting<Vec<String>> =
        Setting::new("files.tags", Vec::new, Scope::App, "Tags", "Default tags.");
    static KEY: Setting<SecretRef> = Setting::new(
        "ai.anthropic_api_key",
        || SecretRef { id: String::new() },
        Scope::App,
        "API key",
        "Keychain entry.",
    );
    static USER_NAME: Setting<String> = Setting::new(
        "profile.display_name",
        String::new,
        Scope::User,
        "Name",
        "Shown to others.",
    );

    fn registry() -> Registry {
        let mut r = Registry::new();
        r.register_all(&[
            &THEME, &GRID, &ZOOM, &RECENT, &ACCENT, &FOLDER, &TAGS, &KEY, &USER_NAME,
        ])
        .unwrap();
        r
    }

    #[test]
    fn round_trip_of_every_kind() {
        let mut a = registry();
        a.set(&THEME, Theme::Dark).unwrap();
        a.set(&GRID, false).unwrap();
        a.set(&ZOOM, 2.5).unwrap();
        a.set(&RECENT, 25).unwrap();
        a.set(
            &ACCENT,
            Rgba {
                r: 1,
                g: 2,
                b: 3,
                a: 4,
            },
        )
        .unwrap();
        a.set(&FOLDER, PathBuf::from("/home/me/Docs")).unwrap();
        a.set(&TAGS, vec!["work".into(), "draft".into()]).unwrap();
        a.set(
            &KEY,
            SecretRef {
                id: "ai.anthropic".into(),
            },
        )
        .unwrap();
        let text = a.to_toml(Scope::App);

        let mut b = registry();
        let report = b.load_toml(Scope::App, &text).unwrap();
        assert_eq!(report.applied.len(), 8, "{report:?}\n{text}");
        assert!(
            report.invalid.is_empty() && report.unknown.is_empty(),
            "{report:?}"
        );
        assert_eq!(b.get(&THEME).unwrap(), Theme::Dark);
        assert!(!b.get(&GRID).unwrap());
        assert_eq!(b.get(&ZOOM).unwrap(), 2.5);
        assert_eq!(b.get(&RECENT).unwrap(), 25);
        assert_eq!(
            b.get(&ACCENT).unwrap(),
            Rgba {
                r: 1,
                g: 2,
                b: 3,
                a: 4
            }
        );
        assert_eq!(b.get(&FOLDER).unwrap(), PathBuf::from("/home/me/Docs"));
        assert_eq!(
            b.get(&TAGS).unwrap(),
            vec!["work".to_owned(), "draft".to_owned()]
        );
        assert_eq!(b.get(&KEY).unwrap().id, "ai.anthropic");
        assert_eq!(b.to_toml(Scope::App), text, "stable output");
    }

    #[test]
    fn file_layout_is_nested_and_only_contains_changes() {
        let mut r = registry();
        r.set(&THEME, Theme::Dark).unwrap();
        r.set(&GRID, false).unwrap();
        let text = r.to_toml(Scope::App);
        assert!(text.contains("[ui]\ntheme = \"dark\""), "{text}");
        assert!(text.contains("[canvas.grid]\nvisible = false"), "{text}");
        assert!(!text.contains("zoom"), "defaults are not written: {text}");
    }

    #[test]
    fn secrets_are_only_keychain_references() {
        let mut r = registry();
        r.set(
            &KEY,
            SecretRef {
                id: "ai.anthropic".into(),
            },
        )
        .unwrap();
        let text = r.to_toml(Scope::App);
        // Inline (`key = { keychain = … }`) or as a section; both load back.
        assert!(text.contains(r#"keychain = "ai.anthropic""#), "{text}");
        let mut fresh = registry();
        fresh.load_toml(Scope::App, &text).unwrap();
        assert_eq!(fresh.get(&KEY).unwrap().id, "ai.anthropic");

        let inline = "[ai]\nanthropic_api_key = { keychain = \"ai.other\" }\n";
        fresh.load_toml(Scope::App, inline).unwrap();
        assert_eq!(fresh.get(&KEY).unwrap().id, "ai.other");
    }

    #[test]
    fn unknown_entries_are_preserved() {
        let mut r = registry();
        let text =
            "[ui]\ntheme = \"dark\"\nfuture_option = 42\n\n[newer_feature]\nenabled = true\n";
        let report = r.load_toml(Scope::App, text).unwrap();
        assert_eq!(report.applied, vec!["ui.theme"]);
        assert_eq!(
            report.unknown,
            vec!["newer_feature.enabled", "ui.future_option"]
        );
        let out = r.to_toml(Scope::App);
        assert!(out.contains("future_option = 42"), "{out}");
        assert!(out.contains("[newer_feature]\nenabled = true"), "{out}");
    }

    #[test]
    fn invalid_entries_are_reported_and_skipped() {
        let mut r = registry();
        let text = "[canvas]\nzoom = 99.0\n\n[files]\nrecent_count = \"many\"\n\n[ui]\naccent = \"blue\"\ntheme = \"neon\"\n";
        let report = r.load_toml(Scope::App, text).unwrap();
        assert!(report.applied.is_empty(), "{report:?}");
        let keys: Vec<&str> = report.invalid.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(
            keys,
            ["canvas.zoom", "files.recent_count", "ui.accent", "ui.theme"]
        );
        assert_eq!(r.get(&ZOOM).unwrap(), 1.0);
    }

    #[test]
    fn floats_accept_integers_in_files() {
        let mut r = registry();
        r.load_toml(Scope::App, "[canvas]\nzoom = 2\n").unwrap();
        assert_eq!(r.get(&ZOOM).unwrap(), 2.0);
    }

    #[test]
    fn other_scopes_are_neither_written_nor_loaded() {
        let mut r = registry();
        r.set(&USER_NAME, "Giosuel".into()).unwrap();
        assert!(!r.to_toml(Scope::App).contains("Giosuel"));
        let report = r
            .load_toml(Scope::App, "[profile]\ndisplay_name = \"Mallory\"\n")
            .unwrap();
        assert_eq!(report.unknown, vec!["profile.display_name"]);
        assert_eq!(r.get(&USER_NAME).unwrap(), "Giosuel");
    }

    #[test]
    fn reloading_resets_missing_keys_and_notifies() {
        let mut r = registry();
        r.set(&GRID, false).unwrap();
        r.set(&RECENT, 5).unwrap();
        let changes = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&changes);
        r.subscribe(move |c| sink.lock().unwrap().push(c.key));
        r.load_toml(Scope::App, "[files]\nrecent_count = 5\n")
            .unwrap();
        assert!(r.get(&GRID).unwrap(), "missing key reset to default");
        assert_eq!(r.get(&RECENT).unwrap(), 5);
        assert_eq!(*changes.lock().unwrap(), vec!["canvas.grid.visible"]);
    }

    #[test]
    fn broken_toml_is_an_error() {
        assert!(matches!(
            registry().load_toml(Scope::App, "[ui\ntheme ="),
            Err(SettingsError::Parse(_))
        ));
    }

    #[test]
    fn save_and_load_files_atomically() {
        let dir = std::env::temp_dir().join(format!("imaginarium-settings-{}", std::process::id()));
        let path = dir.join("nested").join("settings.toml");
        let mut r = registry();
        assert_eq!(
            r.load_file(Scope::App, &path).unwrap(),
            LoadReport::default()
        );
        r.set(&THEME, Theme::Dark).unwrap();
        r.save_file(Scope::App, &path).unwrap();
        assert!(!dir.join("nested").join("settings.toml.tmp").exists());

        let mut fresh = registry();
        fresh.load_file(Scope::App, &path).unwrap();
        assert_eq!(fresh.get(&THEME).unwrap(), Theme::Dark);
        let _ = fs::remove_dir_all(&dir);
    }
}
