// SPDX-License-Identifier: AGPL-3.0-or-later

//! The typed settings registry (ADR-0014).
//!
//! Every setting in Imaginarium is declared here with enough metadata to
//! generate its UI. The settings screen, the command palette's "Settings: …"
//! entries and the Hub admin panel are all built from these declarations.
//! No setting may exist only in a config file.
//!
//! # How it fits together
//! - A **declaration** is a `static` [`Setting<T>`] next to the code that
//!   uses it. Its constructor requires a title and a description, so a
//!   setting without UI metadata can't be declared at all.
//! - Each crate exposes its declarations (e.g. a `SETTINGS: &[&dyn
//!   AnySetting]` list), and the app registers them all at startup
//!   ([`Registry::register_all`]).
//! - Code reads values with [`Registry::get`]. Reading a setting that was
//!   never registered is an error ([`SettingsError::NotRegistered`]), which
//!   surfaces forgotten registrations in tests.
//! - The UI edits values through the dynamic API ([`Registry::set_value`],
//!   [`AnySetting`]), and observers ([`Registry::subscribe`]) react to
//!   changes. Observers become reactive signals with the UI toolkit (M2).
//!
//! ```
//! use imaginarium_settings::{setting_enum, Registry, Scope, Setting};
//!
//! #[derive(Clone, Copy, Debug, PartialEq, Eq)]
//! pub enum Theme { System, Light, Dark }
//! setting_enum!(Theme {
//!     System => "system", "Follow system",
//!     Light => "light", "Light",
//!     Dark => "dark", "Dark",
//! });
//!
//! pub static THEME: Setting<Theme> = Setting::new(
//!     "ui.theme",
//!     || Theme::System,
//!     Scope::App,
//!     "Theme",
//!     "Light, dark, or follow the operating system.",
//! )
//! .synonyms(&["dark mode", "appearance"]);
//!
//! let mut registry = Registry::new();
//! registry.register(&THEME)?;
//! registry.set(&THEME, Theme::Dark)?;
//! assert!(registry.to_toml(Scope::App).contains("theme = \"dark\""));
//! # Ok::<(), imaginarium_settings::SettingsError>(())
//! ```
//!
//! # Rules
//! - Keys are dotted `snake_case` (`group.name`), stable forever. Renaming
//!   one loses users' values. Add a new key and migrate instead.
//! - Secrets (API keys, tokens, passwords) are [`SecretRef`]s pointing into
//!   the OS keychain. Registration rejects plain-text settings whose keys
//!   look like secrets.
//! - Titles and descriptions are English for now. They become Fluent
//!   message ids derived from the key when i18n arrives (M13).
//!
//! # Responsibility
//! - Declarations with scope, default, validation and UI metadata.
//! - Reading and writing values, with validation and change notification.
//! - TOML persistence for the app scope ([`Registry::save_file`],
//!   [`Registry::load_file`]). Other scopes are persisted by their owners:
//!   user and workspace via encrypted sync, document scope in the CRDT.
//!
//! # Non-goals
//! - No UI. The shell generates it from [`Registry::declarations`].
//! - No keychain access. A higher layer resolves [`SecretRef`]s.

mod error;
mod persist;
mod registry;
mod setting;
mod value;

pub use error::SettingsError;
pub use persist::{FileOpened, FileWriter, LoadReport, SaveStatus, write_atomically};
pub use registry::{Change, Registry, SubscriptionId};
pub use setting::{AnySetting, Constraint, Scope, Setting, SettingMeta, Validator};
pub use value::{
    EnumVariant, ListElement, ListKind, Rgba, SecretRef, SettingEnum, SettingType, Value,
    ValueKind, VariantInfo,
};
