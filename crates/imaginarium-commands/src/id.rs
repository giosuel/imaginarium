// SPDX-License-Identifier: AGPL-3.0-or-later

//! Command identifiers.

use std::borrow::Borrow;
use std::fmt;

use crate::error::CommandError;

/// Stable identifier of a command, e.g. `"text.bold"` or `"board.insert_shape"`.
///
/// Ids are dotted `snake_case` with at least two segments, the first
/// naming the area (`text`, `board`, `slides`, `file`, `ai`, …). They are
/// referenced by keymaps, usage statistics, add-ons and AI tool calls, so
/// **never rename one**: add a new command and deprecate the old one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CommandId(&'static str);

impl CommandId {
    /// Wraps a string. Validity is checked when the command is registered
    /// ([`CommandId::validate`]).
    pub const fn new(id: &'static str) -> Self {
        Self(id)
    }

    /// The id as a string.
    pub const fn as_str(self) -> &'static str {
        self.0
    }

    /// Name for AI tool definitions. Tool names may only contain
    /// `[a-zA-Z0-9_-]`, so dots become double underscores:
    /// `text.bold` → `text__bold`. This is reversible, because valid ids
    /// never contain `__`.
    pub fn tool_name(self) -> String {
        self.0.replace('.', "__")
    }

    /// Checks the id format: at least two `.`-separated segments, each
    /// starting with a lowercase ASCII letter and continuing with lowercase
    /// letters, digits or single underscores (no `__`, no trailing `_`).
    pub fn validate(self) -> Result<(), CommandError> {
        let invalid = |reason: &str| CommandError::InvalidId {
            id: self.0,
            reason: reason.to_owned(),
        };
        let segments: Vec<&str> = self.0.split('.').collect();
        if segments.len() < 2 {
            return Err(invalid("needs at least two segments, like `text.bold`"));
        }
        for segment in segments {
            let mut chars = segment.chars();
            let starts_with_letter = chars.next().is_some_and(|c| c.is_ascii_lowercase());
            let rest_ok = chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
            if !starts_with_letter || !rest_ok {
                return Err(invalid(
                    "segments must be snake_case and start with a letter",
                ));
            }
            if segment.contains("__") || segment.ends_with('_') {
                return Err(invalid("no double or trailing underscores"));
            }
        }
        Ok(())
    }
}

/// Lets maps keyed by `CommandId` be queried with a plain `&str`. Consistent
/// with the derived `Eq`/`Ord`/`Hash`, which all compare the string.
impl Borrow<str> for CommandId {
    fn borrow(&self) -> &str {
        self.0
    }
}

impl fmt::Display for CommandId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::CommandId;

    #[test]
    fn valid_ids() {
        for id in [
            "text.bold",
            "board.insert_shape",
            "slides.go_to_2",
            "ai.image.remove_background",
        ] {
            assert!(CommandId::new(id).validate().is_ok(), "{id}");
        }
    }

    #[test]
    fn invalid_ids() {
        for id in [
            "bold",
            "Text.bold",
            "text.Bold",
            "text..bold",
            "text.2x",
            "text.a__b",
            "text.bold_",
            "",
        ] {
            assert!(
                CommandId::new(id).validate().is_err(),
                "{id:?} should be invalid"
            );
        }
    }

    #[test]
    fn tool_names_are_reversible() {
        let id = CommandId::new("ai.image.remove_background");
        assert_eq!(id.tool_name(), "ai__image__remove_background");
        assert_eq!(id.tool_name().replace("__", "."), id.as_str());
    }
}
