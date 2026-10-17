// SPDX-License-Identifier: AGPL-3.0-or-later

//! Settings a board can have its own value of ("This board", `shell.md`
//! §5): stored in the document (`meta.settings`, schema-v1.md §4), set
//! with `board.set_setting`, and otherwise following the user's.

use imaginarium_commands::{
    ArgKind, ArgSpec, ArgsSchema, CommandError, CommandMeta, FnCommand, Outcome,
};
use imaginarium_model::{Actor, Value as DocValue};
use imaginarium_settings::{AnySetting, SettingType, Value, ValueKind};

use crate::background::{BACKGROUND, Background};
use crate::board::BoardCanvas;

/// The settings a board can have its own value of.
pub static BOARD_SETTINGS: &[&dyn AnySetting] = &[&BACKGROUND];

/// A setting's value as a document stores it: plain values, an enum's id,
/// a color's hex. `None` for kinds a document never stores (lists,
/// secrets: the registry refuses them for `.per_document()`).
pub fn to_document(value: &Value) -> Option<DocValue> {
    Some(match value {
        Value::Bool(on) => DocValue::Bool(*on),
        Value::Int(n) => DocValue::Int(*n),
        Value::Float(x) => DocValue::Number(*x),
        Value::String(text) | Value::Enum(text) => DocValue::String(text.as_str().into()),
        Value::Color(color) => DocValue::String(color.to_string().into()),
        Value::Path(path) => DocValue::String(path.display().to_string().into()),
        Value::SecretRef(_) | Value::List(_) => return None,
    })
}

/// A document's stored value of `setting`, if it is one that setting takes
/// (a value a newer version wrote, or nonsense, is ignored).
pub fn from_document(setting: &dyn AnySetting, value: &DocValue) -> Option<Value> {
    let one = |kind: ValueKind, value: &DocValue| -> Option<Value> {
        Some(match (kind, value) {
            (ValueKind::Bool, DocValue::Bool(on)) => Value::Bool(*on),
            (ValueKind::Int, DocValue::Int(n)) => Value::Int(*n),
            // Not NaN: it never equals itself, so it would look changed on
            // every frame.
            (ValueKind::Float, DocValue::Number(x)) if x.is_finite() => Value::Float(*x),
            (ValueKind::Float, DocValue::Int(n)) => Value::Float(*n as f64),
            (ValueKind::String, DocValue::String(text)) => Value::String(text.to_string()),
            (ValueKind::Enum, DocValue::String(text)) => Value::Enum(text.to_string()),
            (ValueKind::Color, DocValue::String(text)) => Value::Color(text.parse().ok()?),
            (ValueKind::Path, DocValue::String(text)) => Value::Path(text.to_string().into()),
            _ => return None,
        })
    };
    let value = match (setting.kind(), value) {
        (ValueKind::List(_), DocValue::List(_)) | (ValueKind::SecretRef, _) => return None,
        (kind, value) => one(kind, value)?,
    };
    setting.validate(&value).ok()?;
    Some(setting.normalize(value))
}

impl BoardCanvas {
    /// The board's own value of the setting `key`, if it has a valid one.
    pub fn own_setting(&self, key: &str) -> Option<Value> {
        let setting = BOARD_SETTINGS.iter().find(|setting| setting.key() == key)?;
        let view = self.document().view();
        from_document(*setting, view.meta().settings().get(key)?)
    }

    /// The background drawn now: the board's own, else the user's. The
    /// board's is worked out again only when the document's settings
    /// change (comparing a map of a few entries), not every frame.
    pub(crate) fn effective_background(&self) -> Background {
        let settings = self.document().view().meta().settings().clone();
        let cached = self
            .own_background
            .borrow()
            .as_ref()
            .and_then(|(seen, own)| (*seen == settings).then_some(*own));
        let own = cached.unwrap_or_else(|| {
            let own = self
                .own_setting(BACKGROUND.key())
                .and_then(|value| Background::from_value(&value));
            *self.own_background.borrow_mut() = Some((settings, own));
            own
        });
        own.unwrap_or(self.background())
    }
}

/// `board.set_setting`.
pub static SET_SETTING: FnCommand<BoardCanvas> = FnCommand::<BoardCanvas>::new(
    "board.set_setting",
    CommandMeta::new(
        "Set this board's setting",
        "Give this board its own value of a setting that can differ per board (`board.background`: dots, grid or plain), stored in the board; without a value, the board follows your setting again.",
    )
    .category("Board")
    .synonyms(&["this board", "board setting", "per board"]),
    |board, args| {
        let id = "board.set_setting";
        let invalid = |reason: String| CommandError::InvalidArgs { id, reason };
        let key = args.str("key").unwrap_or("");
        let Some(setting) = BOARD_SETTINGS.iter().find(|setting| setting.key() == key) else {
            return Err(invalid(format!("`{key}` isn't a setting a board has")));
        };
        // Text is kept as typed; other kinds may have spaces around them.
        // An empty value lets the board follow your setting again.
        let text = args.str("value").map(|text| match setting.kind() {
            ValueKind::String | ValueKind::Path => text,
            _ => text.trim(),
        });
        let value = match text.filter(|text| !text.is_empty()) {
            None => DocValue::Null,
            Some(text) => {
                let value = typed(*setting, text).ok_or_else(|| {
                    invalid(format!("`{text}` isn't a value of `{key}`"))
                })?;
                setting
                    .validate(&value)
                    .map_err(|err| invalid(err.to_string()))?;
                to_document(&setting.normalize(value))
                    .ok_or_else(|| invalid(format!("`{key}` can't be stored in a board")))?
            }
        };
        let current = board.document().view().meta().settings().get(key).cloned();
        if current.unwrap_or(DocValue::Null) == value {
            return Ok(Outcome::Unchanged);
        }
        board
            .transact(&Actor::User, |txn| txn.set_document_setting(key, value))
            .map_err(|err| CommandError::Failed {
                id,
                message: err.to_string(),
            })?;
        Ok(Outcome::Done)
    },
)
.args(ArgsSchema {
    args: &[
        ArgSpec {
            name: "key",
            description: "The setting's key, e.g. `board.background`.",
            kind: ArgKind::String { max_len: Some(128) },
            required: true,
        },
        ArgSpec {
            name: "value",
            description: "The value as text (an enum's id like `grid`, a number, `true`); empty or absent to follow your setting again.",
            kind: ArgKind::String { max_len: Some(256) },
            required: false,
        },
    ],
});

/// `text` as a value of `setting`'s kind.
fn typed(setting: &dyn AnySetting, text: &str) -> Option<Value> {
    Some(match setting.kind() {
        ValueKind::Bool => Value::Bool(text.parse().ok()?),
        ValueKind::Int => Value::Int(text.parse().ok()?),
        ValueKind::Float => Value::Float(text.parse().ok()?),
        ValueKind::String => Value::String(text.to_owned()),
        ValueKind::Enum => Value::Enum(text.to_owned()),
        ValueKind::Color => Value::Color(text.parse().ok()?),
        ValueKind::Path => Value::Path(text.into()),
        ValueKind::SecretRef | ValueKind::List(_) => return None,
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use imaginarium_commands::{ArgValue, Args};
    use imaginarium_model::{Document, DocumentKind, UndoScope};

    use super::*;

    fn set(board: &mut BoardCanvas, value: Option<&str>) -> Result<Outcome, CommandError> {
        let mut args = Args::new().with("key", ArgValue::String("board.background".into()));
        if let Some(value) = value {
            args = args.with("value", ArgValue::String(value.into()));
        }
        board.execute("board.set_setting", &args)
    }

    #[test]
    fn a_board_has_its_own_background_or_follows_the_users() {
        // Each change its own undo step (not merged by time).
        let undo = imaginarium_model::UndoConfig {
            merge_interval_ms: 0,
            max_steps: 100,
        };
        let mut board = BoardCanvas::new(Document::new_with(DocumentKind::Board, undo).unwrap());
        board.set_background(Background::Plain);
        assert_eq!(board.effective_background(), Background::Plain);
        assert_eq!(set(&mut board, Some("grid")).unwrap(), Outcome::Done);
        assert_eq!(board.effective_background(), Background::Grid);
        assert_eq!(
            board.own_setting("board.background"),
            Some(Value::Enum("grid".into()))
        );
        assert_eq!(set(&mut board, Some("grid")).unwrap(), Outcome::Unchanged);
        // One undo step takes it back.
        set(&mut board, Some("plain")).unwrap();
        board.execute("edit.undo", &Args::new()).unwrap();
        assert_eq!(board.effective_background(), Background::Grid);
        assert!(board.document().can_undo(UndoScope::User));
        // Without a value: the user's again.
        set(&mut board, None).unwrap();
        assert_eq!(board.effective_background(), Background::Plain);
        assert_eq!(board.own_setting("board.background"), None);
    }

    #[test]
    fn only_board_settings_and_their_values_are_taken() {
        let mut board = BoardCanvas::new(Document::new(DocumentKind::Board).unwrap());
        assert!(set(&mut board, Some("sepia")).is_err());
        let other = Args::new()
            .with("key", ArgValue::String("ui.theme".into()))
            .with("value", ArgValue::String("dark".into()));
        assert!(board.execute("board.set_setting", &other).is_err());
    }

    #[test]
    fn a_value_this_version_cant_use_is_ignored() {
        // A newer version's background: the user's is drawn.
        let mut document = Document::new(DocumentKind::Board).unwrap();
        document
            .transact(&Actor::User, |txn| {
                txn.set_document_setting("board.background", DocValue::String("hexagons".into()))
            })
            .unwrap();
        let board = BoardCanvas::new(document);
        assert_eq!(board.own_setting("board.background"), None);
        assert_eq!(board.effective_background(), Background::Dots);
    }

    #[test]
    fn values_go_to_the_document_and_back() {
        use imaginarium_settings::{Rgba, Scope, Setting};
        static SNAP: Setting<bool> =
            Setting::new("board.snap", || true, Scope::App, "Snap", "Snap.");
        static GAP: Setting<i64> = Setting::new("board.gap", || 4, Scope::App, "Gap", "Gap.");
        static ZOOM: Setting<f64> = Setting::new("board.zoom", || 1.0, Scope::App, "Zoom", "Zoom.");
        static INK: Setting<Rgba> = Setting::new(
            "board.ink",
            || Rgba::rgb(0, 0, 0),
            Scope::App,
            "Ink",
            "Ink.",
        );
        let round_trip = |setting: &dyn AnySetting, value: Value| {
            from_document(setting, &to_document(&value).unwrap())
        };
        assert_eq!(
            round_trip(&BACKGROUND, Value::Enum("plain".into())),
            Some(Value::Enum("plain".into()))
        );
        assert_eq!(
            round_trip(&SNAP, Value::Bool(false)),
            Some(Value::Bool(false))
        );
        assert_eq!(round_trip(&GAP, Value::Int(-3)), Some(Value::Int(-3)));
        assert_eq!(
            round_trip(&ZOOM, Value::Float(0.1 + 0.2)),
            Some(Value::Float(0.1 + 0.2))
        );
        let ink = Value::Color(Rgba {
            r: 1,
            g: 2,
            b: 3,
            a: 4,
        });
        assert_eq!(round_trip(&INK, ink.clone()), Some(ink));
        // Not a number is never a value; lists and secrets aren't stored.
        assert_eq!(from_document(&ZOOM, &DocValue::Number(f64::NAN)), None);
        assert_eq!(to_document(&Value::List(vec![Value::Bool(true)])), None);
        assert_eq!(
            to_document(&Value::SecretRef(imaginarium_settings::SecretRef {
                id: "x".into()
            })),
            None
        );
    }
}
