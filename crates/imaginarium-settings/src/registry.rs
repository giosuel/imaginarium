// SPDX-License-Identifier: AGPL-3.0-or-later

//! The settings [`Registry`]: declarations, current values and change
//! notifications.

use std::collections::{BTreeMap, HashMap};

use crate::error::SettingsError;
use crate::setting::{AnySetting, Scope, Setting};
use crate::value::{SettingType, Value, ValueKind};

/// Words that mark a key as holding a secret. Such settings must use
/// [`SecretRef`](crate::SecretRef), never plain text (ADR-0014).
const SECRET_WORDS: &[&str] = &[
    "password",
    "passphrase",
    "secret",
    "token",
    "api_key",
    "apikey",
];

/// A change to a setting's value, passed to observers.
#[derive(Clone, Debug, PartialEq)]
pub struct Change {
    /// The setting's key.
    pub key: &'static str,
    /// The setting's scope.
    pub scope: Scope,
    /// Effective value before the change (the default if it wasn't set).
    pub old: Value,
    /// Effective value after the change.
    pub new: Value,
}

/// Handle returned by [`Registry::subscribe`], used to unsubscribe.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SubscriptionId(u64);

type Observer = Box<dyn FnMut(&Change) + Send>;

/// Gets a scope's settings as TOML after each change ([`Registry::persist_to`]).
type PersistHook = Box<dyn FnMut(String) + Send>;

/// All registered settings and their current values.
///
/// Only values that differ from the default are stored (and persisted), so
/// improved defaults in new releases reach users who never changed them.
///
/// The registry is single-threaded by design (`Send`, not `Sync`). The app
/// owns it on the UI thread. Observers will later be bridged to reactive
/// signals.
#[derive(Default)]
pub struct Registry {
    declarations: BTreeMap<&'static str, &'static dyn AnySetting>,
    values: HashMap<&'static str, Value>,
    observers: Vec<(SubscriptionId, Observer)>,
    next_subscription: u64,
    persist: Vec<(Scope, PersistHook)>,
    /// Loading a file: scopes changed meanwhile are saved once at the end,
    /// not in every half-loaded state.
    pub(crate) persist_paused: bool,
    pub(crate) persist_pending: Vec<Scope>,
    /// Entries from settings files that no registered setting claims, per
    /// scope. They are written back unchanged (forward compatibility).
    pub(crate) unknown: BTreeMap<Scope, BTreeMap<String, toml::Value>>,
}

impl Registry {
    /// Creates an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a setting declaration. Each key may be registered once.
    ///
    /// Rejects keys that are not dotted `snake_case` with at least two
    /// segments, keys that are a prefix of another key (they would clash in
    /// TOML tables), and plain-text settings whose key looks like a secret.
    pub fn register(&mut self, setting: &'static dyn AnySetting) -> Result<(), SettingsError> {
        let key = setting.key();
        validate_key(key)?;
        if self.declarations.contains_key(key) {
            return Err(SettingsError::DuplicateKey(key));
        }
        if let Some(other) = self.declarations.keys().find(|other| {
            other.starts_with(&format!("{key}.")) || key.starts_with(&format!("{other}."))
        }) {
            return Err(SettingsError::InvalidKey {
                key,
                reason: format!("clashes with `{other}`; one key can't be a prefix of another"),
            });
        }
        if matches!(setting.kind(), ValueKind::String | ValueKind::List(_))
            && key
                .split('.')
                .any(|segment| SECRET_WORDS.iter().any(|w| segment.contains(w)))
        {
            return Err(SettingsError::InvalidKey {
                key,
                reason: "looks like a secret; use a SecretRef (OS keychain), never plain text"
                    .to_owned(),
            });
        }
        // A document stores plain values (schema-v1.md §4), and never a
        // secret.
        if setting.meta().per_document
            && matches!(setting.kind(), ValueKind::List(_) | ValueKind::SecretRef)
        {
            return Err(SettingsError::InvalidKey {
                key,
                reason: "a list or a secret can't have a document's own value".to_owned(),
            });
        }
        setting
            .validate(&setting.default_dyn())
            .map_err(|err| SettingsError::Invalid {
                key,
                reason: format!("the default value is invalid: {err}"),
            })?;
        self.declarations.insert(key, setting);
        Ok(())
    }

    /// Registers several settings, e.g. a crate's `SETTINGS` list.
    pub fn register_all(
        &mut self,
        settings: &[&'static dyn AnySetting],
    ) -> Result<(), SettingsError> {
        settings
            .iter()
            .try_for_each(|setting| self.register(*setting))
    }

    /// The current value of a setting (its default if never set).
    ///
    /// Fails with [`SettingsError::NotRegistered`] if the setting wasn't
    /// registered. Unregistered settings would be invisible in the UI.
    pub fn get<T: SettingType>(&self, setting: &Setting<T>) -> Result<T, SettingsError> {
        let key = setting.key();
        self.declaration(key)?;
        match self.values.get(key) {
            Some(value) => T::from_value(value).ok_or_else(|| SettingsError::Invalid {
                key,
                reason: "stored value has the wrong kind".to_owned(),
            }),
            None => Ok(setting.default_value()),
        }
    }

    /// Validates and sets a typed value, then notifies observers if the
    /// effective value changed.
    pub fn set<T: SettingType>(
        &mut self,
        setting: &Setting<T>,
        value: T,
    ) -> Result<(), SettingsError> {
        self.set_value(setting.key(), value.to_value())
    }

    /// Validates and sets a dynamic value (used by the generated UI and file
    /// loading), then notifies observers if the effective value changed.
    pub fn set_value(&mut self, key: &str, value: Value) -> Result<(), SettingsError> {
        let declaration = self.declaration(key)?;
        declaration.validate(&value)?;
        let value = declaration.normalize(value);
        let key = declaration.key();
        let old = self.effective(declaration);
        let forgot = self.forget_preserved(declaration);
        if value == declaration.default_dyn() {
            self.values.remove(key);
        } else {
            self.values.insert(key, value);
        }
        if !self.notify_if_changed(declaration, old) && forgot {
            // The file still holds the entry this version couldn't read.
            self.persist(declaration.meta().scope);
        }
        Ok(())
    }

    /// Resets a setting to its default.
    pub fn reset(&mut self, key: &str) -> Result<(), SettingsError> {
        let declaration = self.declaration(key)?;
        let old = self.effective(declaration);
        self.values.remove(declaration.key());
        let forgot = self.forget_preserved(declaration);
        if !self.notify_if_changed(declaration, old) && forgot {
            self.persist(declaration.meta().scope);
        }
        Ok(())
    }

    /// Drops a file value preserved for this key because this version
    /// couldn't read it. Called when the user sets or resets the setting, so
    /// their choice wins on the next save.
    fn forget_preserved(&mut self, declaration: &dyn AnySetting) -> bool {
        self.unknown
            .get_mut(&declaration.meta().scope)
            .is_some_and(|unknown| unknown.remove(declaration.key()).is_some())
    }

    /// The effective dynamic value of a registered key.
    pub fn value(&self, key: &str) -> Result<Value, SettingsError> {
        Ok(self.effective(self.declaration(key)?))
    }

    /// Whether the setting currently differs from its default.
    pub fn is_modified(&self, key: &str) -> bool {
        self.values.contains_key(key)
    }

    /// All declarations, sorted by key. The settings UI and search are
    /// generated from these.
    pub fn declarations(&self) -> impl Iterator<Item = &'static dyn AnySetting> + '_ {
        self.declarations.values().copied()
    }

    /// Looks up a declaration by key.
    pub fn declaration(&self, key: &str) -> Result<&'static dyn AnySetting, SettingsError> {
        self.declarations
            .get(key)
            .copied()
            .ok_or_else(|| SettingsError::NotRegistered(key.to_owned()))
    }

    /// Values that differ from the default, for one scope (used by persistence).
    pub(crate) fn modified_in(&self, scope: Scope) -> BTreeMap<&'static str, &Value> {
        self.values
            .iter()
            .filter(|(key, _)| {
                self.declarations
                    .get(*key)
                    .is_some_and(|decl| decl.meta().scope == scope)
            })
            .map(|(key, value)| (*key, value))
            .collect()
    }

    /// Calls `observer` after every effective change of any setting.
    pub fn subscribe(&mut self, observer: impl FnMut(&Change) + Send + 'static) -> SubscriptionId {
        let id = SubscriptionId(self.next_subscription);
        self.next_subscription += 1;
        self.observers.push((id, Box::new(observer)));
        id
    }

    /// Calls `hook` with `scope`'s settings as TOML ([`Registry::to_toml`])
    /// after every effective change in that scope, so they can be saved.
    /// Install it after loading the file: loading changes values too.
    ///
    /// ```
    /// use std::sync::{Arc, Mutex};
    /// use imaginarium_settings::{Registry, Scope, Setting};
    ///
    /// static SNAP: Setting<bool> = Setting::new(
    ///     "board.snap", || true, Scope::App, "Snap", "Snap to the grid.",
    /// );
    /// let mut registry = Registry::new();
    /// registry.register(&SNAP)?;
    /// let saved = Arc::new(Mutex::new(String::new()));
    /// let into = saved.clone();
    /// registry.persist_to(Scope::App, move |toml| *into.lock().unwrap() = toml);
    /// registry.set(&SNAP, false)?;
    /// assert!(saved.lock().unwrap().contains("snap = false"));
    /// # Ok::<(), imaginarium_settings::SettingsError>(())
    /// ```
    pub fn persist_to(&mut self, scope: Scope, hook: impl FnMut(String) + Send + 'static) {
        self.persist.push((scope, Box::new(hook)));
    }

    /// Removes an observer. Returns whether it was subscribed.
    pub fn unsubscribe(&mut self, id: SubscriptionId) -> bool {
        let before = self.observers.len();
        self.observers.retain(|(other, _)| *other != id);
        self.observers.len() != before
    }

    fn effective(&self, declaration: &dyn AnySetting) -> Value {
        self.values
            .get(declaration.key())
            .cloned()
            .unwrap_or_else(|| declaration.default_dyn())
    }

    /// Notifies observers and saves, if the effective value changed;
    /// returns whether it did.
    fn notify_if_changed(&mut self, declaration: &'static dyn AnySetting, old: Value) -> bool {
        let new = self.effective(declaration);
        if new == old {
            return false;
        }
        let change = Change {
            key: declaration.key(),
            scope: declaration.meta().scope,
            old,
            new,
        };
        for (_, observer) in &mut self.observers {
            observer(&change);
        }
        self.persist(change.scope);
        true
    }

    /// Hands `scope`'s TOML to its hooks (after loading, if it is loading).
    pub(crate) fn persist(&mut self, scope: Scope) {
        if self.persist_paused {
            if !self.persist_pending.contains(&scope) {
                self.persist_pending.push(scope);
            }
            return;
        }
        if self.persist.iter().any(|(of, _)| *of == scope) {
            let toml = self.to_toml(scope);
            for (_, hook) in self.persist.iter_mut().filter(|(of, _)| *of == scope) {
                hook(toml.clone());
            }
        }
    }
}

/// Keys are dotted `snake_case`: at least two segments, each starting with a
/// lowercase letter, followed by lowercase letters, digits or `_`.
fn validate_key(key: &'static str) -> Result<(), SettingsError> {
    let invalid = |reason: &str| SettingsError::InvalidKey {
        key,
        reason: reason.to_owned(),
    };
    let segments: Vec<&str> = key.split('.').collect();
    if segments.len() < 2 {
        return Err(invalid("needs at least two segments, like `group.name`"));
    }
    for segment in segments {
        let mut chars = segment.chars();
        let valid_start = chars.next().is_some_and(|c| c.is_ascii_lowercase());
        let valid_rest = chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
        if !valid_start || !valid_rest {
            return Err(invalid(
                "segments must be snake_case and start with a letter",
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::value::SecretRef;

    static THEME_NAME: Setting<String> = Setting::new(
        "ui.theme_name",
        || "system".into(),
        Scope::User,
        "Theme",
        "Look and feel.",
    )
    .max_len(16);
    static ZOOM: Setting<f64> =
        Setting::new("canvas.zoom", || 1.0, Scope::App, "Zoom", "Default zoom.")
            .range(0.1, 8.0, 0.1);
    static EVEN: Setting<i64> =
        Setting::new("test.even", || 2, Scope::App, "Even", "An even number.").validator(|n| {
            if n % 2 == 0 {
                Ok(())
            } else {
                Err("must be even")
            }
        });
    static API_KEY: Setting<SecretRef> = Setting::new(
        "ai.anthropic_api_key",
        || SecretRef {
            id: "ai.anthropic".into(),
        },
        Scope::User,
        "Anthropic API key",
        "Stored in the OS keychain.",
    );

    fn registry() -> Registry {
        let mut registry = Registry::new();
        registry.register_all(&[&THEME_NAME, &ZOOM, &EVEN]).unwrap();
        registry
    }

    #[test]
    fn defaults_then_set_then_reset() {
        let mut r = registry();
        assert_eq!(r.get(&ZOOM).unwrap(), 1.0);
        r.set(&ZOOM, 2.5).unwrap();
        assert_eq!(r.get(&ZOOM).unwrap(), 2.5);
        assert!(r.is_modified("canvas.zoom"));
        r.reset("canvas.zoom").unwrap();
        assert_eq!(r.get(&ZOOM).unwrap(), 1.0);
        assert!(!r.is_modified("canvas.zoom"));
    }

    #[test]
    fn setting_the_default_value_clears_the_override() {
        let mut r = registry();
        r.set(&ZOOM, 3.0).unwrap();
        r.set(&ZOOM, 1.0).unwrap();
        assert!(!r.is_modified("canvas.zoom"));
    }

    #[test]
    fn reading_an_unregistered_setting_is_an_error() {
        let r = Registry::new();
        assert_eq!(
            r.get(&ZOOM),
            Err(SettingsError::NotRegistered("canvas.zoom".into()))
        );
        assert!(matches!(
            r.value("nope.nothing"),
            Err(SettingsError::NotRegistered(_))
        ));
    }

    #[test]
    fn duplicate_and_malformed_keys_are_rejected() {
        let mut r = registry();
        assert_eq!(
            r.register(&ZOOM),
            Err(SettingsError::DuplicateKey("canvas.zoom"))
        );
        static NO_GROUP: Setting<bool> = Setting::new("theme", || true, Scope::App, "T", "D");
        static UPPER: Setting<bool> = Setting::new("ui.Theme", || true, Scope::App, "T", "D");
        static DIGIT: Setting<bool> = Setting::new("ui.2d", || true, Scope::App, "T", "D");
        static EMPTY: Setting<bool> = Setting::new("ui..x", || true, Scope::App, "T", "D");
        for bad in [&NO_GROUP, &UPPER, &DIGIT, &EMPTY] {
            assert!(matches!(
                r.register(bad),
                Err(SettingsError::InvalidKey { .. })
            ));
        }
    }

    #[test]
    fn prefix_keys_clash_in_both_directions() {
        let mut r = registry();
        // New key below an existing one ("canvas.zoom" is registered).
        static BELOW: Setting<bool> =
            Setting::new("canvas.zoom.max", || true, Scope::App, "T", "D");
        assert!(matches!(
            r.register(&BELOW),
            Err(SettingsError::InvalidKey { .. })
        ));
        // New key above an existing one.
        static DEEP: Setting<bool> = Setting::new("view.grid.size", || true, Scope::App, "T", "D");
        static ABOVE: Setting<bool> = Setting::new("view.grid", || true, Scope::App, "T", "D");
        r.register(&DEEP).unwrap();
        assert!(matches!(
            r.register(&ABOVE),
            Err(SettingsError::InvalidKey { .. })
        ));
        // Sharing a prefix without a dot boundary is fine.
        static SIBLING: Setting<bool> =
            Setting::new("canvas.zoom_step", || true, Scope::App, "T", "D");
        assert_eq!(r.register(&SIBLING), Ok(()));
    }

    #[test]
    fn plain_text_secrets_are_rejected_but_secret_refs_are_fine() {
        let mut r = Registry::new();
        static PLAIN: Setting<String> = Setting::new(
            "ai.openai_api_key",
            String::new,
            Scope::User,
            "Key",
            "Plain text.",
        );
        static TOKEN: Setting<String> = Setting::new(
            "hub.session_token",
            String::new,
            Scope::User,
            "Token",
            "Plain text.",
        );
        assert!(matches!(
            r.register(&PLAIN),
            Err(SettingsError::InvalidKey { .. })
        ));
        assert!(matches!(
            r.register(&TOKEN),
            Err(SettingsError::InvalidKey { .. })
        ));
        assert_eq!(r.register(&API_KEY), Ok(()));
    }

    #[test]
    fn constraints_and_validators_are_enforced() {
        let mut r = registry();
        assert!(r.set(&ZOOM, 9.0).is_err());
        assert!(r.set(&ZOOM, f64::NAN).is_err());
        assert!(r.set(&THEME_NAME, "x".repeat(17)).is_err());
        assert!(
            r.set(&THEME_NAME, "é".repeat(16)).is_ok(),
            "length counts characters"
        );
        assert_eq!(
            r.set(&EVEN, 3),
            Err(SettingsError::Invalid {
                key: "test.even",
                reason: "must be even".into()
            })
        );
        assert!(
            r.set_value("canvas.zoom", Value::Bool(true)).is_err(),
            "kind mismatch"
        );
        assert_eq!(r.get(&ZOOM).unwrap(), 1.0, "failed sets change nothing");
    }

    #[test]
    fn dynamic_values_are_normalized_to_the_setting_type() {
        let mut r = registry();
        let changes = Arc::new(Mutex::new(0));
        let sink = Arc::clone(&changes);
        r.subscribe(move |_| *sink.lock().unwrap() += 1);
        r.set_value("canvas.zoom", Value::Int(1)).unwrap(); // equals the 1.0 default
        assert!(!r.is_modified("canvas.zoom"));
        assert_eq!(*changes.lock().unwrap(), 0, "no spurious change");
        r.set_value("canvas.zoom", Value::Int(2)).unwrap();
        assert_eq!(r.value("canvas.zoom").unwrap(), Value::Float(2.0));
    }

    #[test]
    fn invalid_defaults_are_rejected_at_registration() {
        static BAD_DEFAULT: Setting<i64> =
            Setting::new("test.bad_default", || 0, Scope::App, "T", "D").range(1, 10);
        assert!(matches!(
            Registry::new().register(&BAD_DEFAULT),
            Err(SettingsError::Invalid { .. })
        ));
    }

    #[test]
    fn observers_see_effective_changes_only() {
        let mut r = registry();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&seen);
        let id = r.subscribe(move |change| sink.lock().unwrap().push(change.clone()));

        r.set(&ZOOM, 2.0).unwrap();
        r.set(&ZOOM, 2.0).unwrap(); // no change
        r.reset("canvas.zoom").unwrap();
        r.reset("canvas.zoom").unwrap(); // no change
        {
            let seen = seen.lock().unwrap();
            assert_eq!(seen.len(), 2);
            assert_eq!(seen[0].old, Value::Float(1.0));
            assert_eq!(seen[0].new, Value::Float(2.0));
            assert_eq!(seen[0].scope, Scope::App);
            assert_eq!(seen[1].new, Value::Float(1.0));
        }

        assert!(r.unsubscribe(id));
        assert!(!r.unsubscribe(id));
        r.set(&ZOOM, 4.0).unwrap();
        assert_eq!(seen.lock().unwrap().len(), 2);
    }

    #[test]
    fn group_defaults_to_the_first_key_segment() {
        assert_eq!(ZOOM.meta().group, "canvas");
        static GROUPED: Setting<bool> =
            Setting::new("canvas.grid", || true, Scope::App, "Grid", "Show grid.").group("view");
        assert_eq!(GROUPED.meta().group, "view");
    }

    #[test]
    fn a_list_cant_have_a_documents_own_value() {
        static TAGS: Setting<Vec<String>> =
            Setting::new("board.tags", Vec::new, Scope::App, "Tags", "Tags.").per_document();
        static SNAP: Setting<bool> =
            Setting::new("board.snap", || true, Scope::App, "Snap", "Snap.").per_document();
        let mut registry = Registry::new();
        assert!(registry.register(&TAGS).is_err());
        assert!(registry.register(&SNAP).is_ok());
    }
}
