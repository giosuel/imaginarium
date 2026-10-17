// SPDX-License-Identifier: AGPL-3.0-or-later

//! Setting declarations: [`Setting`], its metadata and the dynamic
//! [`AnySetting`] view used by the registry and the generated settings UI.

use std::marker::PhantomData;

use crate::error::SettingsError;
use crate::value::{SettingType, Value, ValueKind, VariantInfo};

/// Where a setting's value lives, and therefore who it applies to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Scope {
    /// This installation on this device (window behavior, GPU backend, …).
    /// Persisted in the local app settings file.
    App,
    /// The user, on all their devices. Synced end-to-end encrypted via the Hub.
    User,
    /// A team workspace on a Hub (e.g. its AI policy).
    Workspace,
    /// One document. Stored inside the document (CRDT).
    Document,
    /// A Hub server. Hub admins edit these from the desktop client.
    Hub,
}

/// Bounds and limits shown in, and enforced by, the settings UI.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Constraint {
    /// No constraint beyond the value's kind.
    None,
    /// Inclusive numeric range, with an optional UI step (slider/spinner).
    Range {
        /// Smallest allowed value.
        min: f64,
        /// Largest allowed value.
        max: f64,
        /// Increment for UI controls, if any.
        step: Option<f64>,
    },
    /// Maximum string length in characters.
    MaxLen(usize),
}

/// Everything the settings UI and search need to present a setting.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SettingMeta {
    /// Where the value lives.
    pub scope: Scope,
    /// Short UI title, e.g. "Theme". Sentence case, no trailing period.
    pub title: &'static str,
    /// One or two sentences explaining the effect. Shown below the control.
    pub description: &'static str,
    /// UI section. Defaults to the key's first segment (`"ui"` for `"ui.theme"`).
    pub group: &'static str,
    /// Extra search terms, e.g. `&["dark mode", "appearance"]`.
    pub synonyms: &'static [&'static str],
    /// Hidden behind "Show advanced" in the UI.
    pub advanced: bool,
    /// A document can have its own value ("This board"), stored in the
    /// document; the setting's own value is the default for documents
    /// without one.
    pub per_document: bool,
}

/// Custom validation hook: returns a user-facing message on failure.
pub type Validator<T> = fn(&T) -> Result<(), &'static str>;

/// A typed setting declaration. Declare each setting once as a `static`
/// next to the code that uses it, and register it at startup.
///
/// The constructor *requires* a title and description, so a setting
/// without UI metadata can't be declared at all (ADR-0014).
///
/// ```
/// use imaginarium_settings::{Registry, Scope, Setting};
///
/// pub static AUTOSAVE_SECONDS: Setting<i64> = Setting::new(
///     "files.autosave_seconds",
///     || 30,
///     Scope::App,
///     "Autosave interval",
///     "How often open documents are saved in the background, in seconds.",
/// )
/// .range(5, 600)
/// .synonyms(&["save", "backup"]);
///
/// let mut registry = Registry::new();
/// registry.register(&AUTOSAVE_SECONDS)?;
/// assert_eq!(registry.get(&AUTOSAVE_SECONDS)?, 30);
/// registry.set(&AUTOSAVE_SECONDS, 60)?;
/// assert!(registry.set(&AUTOSAVE_SECONDS, 1).is_err()); // below the range
/// # Ok::<(), imaginarium_settings::SettingsError>(())
/// ```
pub struct Setting<T> {
    key: &'static str,
    default: fn() -> T,
    meta: SettingMeta,
    constraint: Constraint,
    validator: Option<Validator<T>>,
    _type: PhantomData<fn() -> T>,
}

impl<T> Setting<T> {
    /// Declares a setting.
    ///
    /// `key` is dotted `snake_case` with at least two segments
    /// (`group.name`, e.g. `"ui.theme"`). It is checked when registered.
    /// `default` is a function, so non-`const` defaults (`String`, `Vec`)
    /// work in `static`s.
    pub const fn new(
        key: &'static str,
        default: fn() -> T,
        scope: Scope,
        title: &'static str,
        description: &'static str,
    ) -> Self {
        Self {
            key,
            default,
            meta: SettingMeta {
                scope,
                title,
                description,
                group: "",
                synonyms: &[],
                advanced: false,
                per_document: false,
            },
            constraint: Constraint::None,
            validator: None,
            _type: PhantomData,
        }
    }

    /// Sets the UI group (default: the key's first segment).
    pub const fn group(mut self, group: &'static str) -> Self {
        self.meta.group = group;
        self
    }

    /// Adds search synonyms.
    pub const fn synonyms(mut self, synonyms: &'static [&'static str]) -> Self {
        self.meta.synonyms = synonyms;
        self
    }

    /// Hides the setting behind "Show advanced".
    pub const fn advanced(mut self) -> Self {
        self.meta.advanced = true;
        self
    }

    /// Lets a document have its own value ([`SettingMeta::per_document`]).
    /// Not for lists or secrets: registering one is an error.
    pub const fn per_document(mut self) -> Self {
        self.meta.per_document = true;
        self
    }

    /// Adds a custom validation rule on top of the constraint.
    pub const fn validator(mut self, validator: Validator<T>) -> Self {
        self.validator = Some(validator);
        self
    }

    /// The setting's key.
    pub const fn key(&self) -> &'static str {
        self.key
    }

    /// Its default value.
    pub fn default_value(&self) -> T {
        (self.default)()
    }
}

impl Setting<i64> {
    /// Restricts the value to `min..=max`.
    pub const fn range(mut self, min: i64, max: i64) -> Self {
        self.constraint = Constraint::Range {
            min: min as f64,
            max: max as f64,
            step: Some(1.0),
        };
        self
    }
}

impl Setting<f64> {
    /// Restricts the value to `min..=max`, with a UI step.
    pub const fn range(mut self, min: f64, max: f64, step: f64) -> Self {
        self.constraint = Constraint::Range {
            min,
            max,
            step: Some(step),
        };
        self
    }
}

impl Setting<String> {
    /// Limits the length in characters.
    pub const fn max_len(mut self, max_len: usize) -> Self {
        self.constraint = Constraint::MaxLen(max_len);
        self
    }
}

/// Type-erased view of a [`Setting`], used by the registry, persistence and
/// the generated settings UI.
pub trait AnySetting: Send + Sync {
    /// Dotted key, e.g. `"ui.theme"`.
    fn key(&self) -> &'static str;
    /// UI metadata (the group is already resolved).
    fn meta(&self) -> SettingMeta;
    /// Value kind.
    fn kind(&self) -> ValueKind;
    /// UI and validation constraint.
    fn constraint(&self) -> Constraint;
    /// Enum variants (empty unless the kind is `Enum` or a list of enums).
    fn variants(&self) -> Vec<VariantInfo>;
    /// The default value, in dynamic form.
    fn default_dyn(&self) -> Value;
    /// Checks kind, constraint and custom validator.
    fn validate(&self, value: &Value) -> Result<(), SettingsError>;
    /// Converts a valid value to the setting's canonical form (e.g. `Int(2)`
    /// → `Float(2.0)` for a float setting), so comparisons and storage are
    /// exact. Invalid values are returned unchanged.
    fn normalize(&self, value: Value) -> Value;
}

impl<T: SettingType> AnySetting for Setting<T> {
    fn key(&self) -> &'static str {
        self.key
    }

    fn meta(&self) -> SettingMeta {
        let mut meta = self.meta;
        if meta.group.is_empty() {
            meta.group = self.key.split('.').next().unwrap_or(self.key);
        }
        meta
    }

    fn kind(&self) -> ValueKind {
        T::KIND
    }

    fn constraint(&self) -> Constraint {
        self.constraint
    }

    fn variants(&self) -> Vec<VariantInfo> {
        T::variants()
    }

    fn default_dyn(&self) -> Value {
        self.default_value().to_value()
    }

    fn normalize(&self, value: Value) -> Value {
        T::from_value(&value).map_or(value, |typed| typed.to_value())
    }

    fn validate(&self, value: &Value) -> Result<(), SettingsError> {
        let invalid = |reason: String| SettingsError::Invalid {
            key: self.key,
            reason,
        };
        let typed = T::from_value(value)
            .ok_or_else(|| invalid(format!("expected {:?}, got {}", T::KIND, value.kind_name())))?;
        match (self.constraint, value) {
            (Constraint::Range { min, max, .. }, Value::Int(n))
                if !(min..=max).contains(&(*n as f64)) =>
            {
                return Err(invalid(format!("{n} is outside {min}..={max}")));
            }
            (Constraint::Range { min, max, .. }, Value::Float(f)) if !(min..=max).contains(f) => {
                return Err(invalid(format!("{f} is outside {min}..={max}")));
            }
            (Constraint::Range { .. }, Value::Float(f)) if !f.is_finite() => {
                return Err(invalid("must be a finite number".to_owned()));
            }
            (Constraint::MaxLen(max), Value::String(s)) if s.chars().count() > max => {
                return Err(invalid(format!("longer than {max} characters")));
            }
            _ => {}
        }
        if let Some(validator) = self.validator {
            validator(&typed).map_err(|reason| invalid(reason.to_owned()))?;
        }
        Ok(())
    }
}
