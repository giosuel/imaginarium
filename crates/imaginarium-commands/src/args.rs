// SPDX-License-Identifier: AGPL-3.0-or-later

//! Command arguments: typed schemas, validation, and JSON (Schema) for AI
//! tool use.
//!
//! Every command declares its arguments as an [`ArgsSchema`]. The same
//! schema validates arguments from every source (palette prompts,
//! keybindings, add-ons, the AI assistant), and it exports as JSON Schema,
//! so every command is automatically an AI tool (ADR-0013).

use std::collections::BTreeMap;

use serde_json::{Map, Value as Json, json};

use crate::id::CommandId;

/// The type and limits of one argument.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ArgKind {
    /// `true` / `false`.
    Bool,
    /// Integer, optionally bounded (inclusive).
    Int {
        /// Smallest allowed value.
        min: Option<i64>,
        /// Largest allowed value.
        max: Option<i64>,
    },
    /// Float, optionally bounded (inclusive). Must be finite.
    Float {
        /// Smallest allowed value.
        min: Option<f64>,
        /// Largest allowed value.
        max: Option<f64>,
    },
    /// Text, optionally limited in characters.
    String {
        /// Maximum length in characters.
        max_len: Option<usize>,
    },
    /// One of a fixed set of string ids.
    Enum {
        /// Allowed ids.
        variants: &'static [&'static str],
    },
    /// A color as `#rrggbb` or `#rrggbbaa`.
    Color,
    /// An opaque reference to a document object (node, page, style, …),
    /// passed as its id string. `target` names what it refers to.
    Ref {
        /// What the id refers to, e.g. `"node"`.
        target: &'static str,
    },
}

/// One declared argument.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ArgSpec {
    /// Name (`snake_case`), the JSON property name in tool calls.
    pub name: &'static str,
    /// What the argument means. Shown in prompts and given to AI models.
    pub description: &'static str,
    /// Type and limits.
    pub kind: ArgKind,
    /// Whether the argument must be present.
    pub required: bool,
}

/// The arguments a command accepts. `const`-constructible, so commands can
/// be `static`s.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ArgsSchema {
    /// The declared arguments, in display order.
    pub args: &'static [ArgSpec],
}

impl ArgsSchema {
    /// A command without arguments.
    pub const NONE: Self = Self { args: &[] };

    /// Checks `args` against the schema: no unknown names, all required
    /// arguments present, kinds and limits respected.
    pub fn validate(&self, id: CommandId, args: &Args) -> Result<(), crate::CommandError> {
        let invalid = |reason: String| crate::CommandError::InvalidArgs {
            id: id.as_str(),
            reason,
        };
        for name in args.values.keys() {
            if !self.args.iter().any(|spec| spec.name == name) {
                return Err(invalid(format!("unknown argument `{name}`")));
            }
        }
        for spec in self.args {
            match args.values.get(spec.name) {
                None if spec.required => {
                    return Err(invalid(format!(
                        "missing required argument `{}`",
                        spec.name
                    )));
                }
                None => {}
                Some(value) => check_value(spec, value).map_err(invalid)?,
            }
        }
        Ok(())
    }

    /// JSON Schema (draft 2020-12 subset) of the arguments object, as used
    /// for AI tool definitions.
    pub fn to_json_schema(&self) -> Json {
        let mut properties = Map::new();
        for spec in self.args {
            let mut property = match spec.kind {
                ArgKind::Bool => json!({ "type": "boolean" }),
                ArgKind::Int { min, max } => {
                    let mut p = json!({ "type": "integer" });
                    if let Some(min) = min {
                        p["minimum"] = json!(min);
                    }
                    if let Some(max) = max {
                        p["maximum"] = json!(max);
                    }
                    p
                }
                ArgKind::Float { min, max } => {
                    let mut p = json!({ "type": "number" });
                    if let Some(min) = min {
                        p["minimum"] = json!(min);
                    }
                    if let Some(max) = max {
                        p["maximum"] = json!(max);
                    }
                    p
                }
                ArgKind::String { max_len } => {
                    let mut p = json!({ "type": "string" });
                    if let Some(max_len) = max_len {
                        p["maxLength"] = json!(max_len);
                    }
                    p
                }
                ArgKind::Enum { variants } => json!({ "type": "string", "enum": variants }),
                ArgKind::Color => json!({
                    "type": "string",
                    "pattern": "^#([0-9a-fA-F]{6}|[0-9a-fA-F]{8})$",
                }),
                ArgKind::Ref { target } => json!({
                    "type": "string",
                    "x-imaginarium-ref": target,
                }),
            };
            let description = match spec.kind {
                ArgKind::Ref { target } => format!("{} (id of a {target})", spec.description),
                _ => spec.description.to_owned(),
            };
            property["description"] = json!(description);
            properties.insert(spec.name.to_owned(), property);
        }
        let required: Vec<&str> = self
            .args
            .iter()
            .filter(|spec| spec.required)
            .map(|spec| spec.name)
            .collect();
        json!({
            "type": "object",
            "properties": properties,
            "required": required,
            "additionalProperties": false,
        })
    }

    /// Parses a JSON object (e.g. an AI tool call's input) into [`Args`]
    /// and validates it.
    pub fn args_from_json(&self, id: CommandId, input: &Json) -> Result<Args, crate::CommandError> {
        let invalid = |reason: String| crate::CommandError::InvalidArgs {
            id: id.as_str(),
            reason,
        };
        let object = input
            .as_object()
            .ok_or_else(|| invalid("arguments must be a JSON object".to_owned()))?;
        let mut args = Args::new();
        for (name, raw) in object {
            let spec = self
                .args
                .iter()
                .find(|spec| spec.name == name)
                .ok_or_else(|| invalid(format!("unknown argument `{name}`")))?;
            let value = match (spec.kind, raw) {
                (ArgKind::Bool, Json::Bool(b)) => ArgValue::Bool(*b),
                (ArgKind::Int { .. }, Json::Number(n)) => ArgValue::Int(
                    n.as_i64()
                        .ok_or_else(|| invalid(format!("`{name}` must be an integer")))?,
                ),
                (ArgKind::Float { .. }, Json::Number(n)) => ArgValue::Float(
                    n.as_f64()
                        .ok_or_else(|| invalid(format!("`{name}` must be a number")))?,
                ),
                (
                    ArgKind::String { .. }
                    | ArgKind::Enum { .. }
                    | ArgKind::Color
                    | ArgKind::Ref { .. },
                    Json::String(s),
                ) => ArgValue::String(s.clone()),
                _ => return Err(invalid(format!("`{name}` has the wrong type"))),
            };
            args.values.insert(name.clone(), value);
        }
        self.validate(id, &args)?;
        Ok(args)
    }
}

/// An argument value. Enum ids, colors and references are strings.
#[derive(Clone, Debug, PartialEq)]
pub enum ArgValue {
    /// Boolean.
    Bool(bool),
    /// Integer.
    Int(i64),
    /// Float.
    Float(f64),
    /// String, enum id, color or reference id.
    String(String),
}

/// Arguments for one invocation, by name.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Args {
    values: BTreeMap<String, ArgValue>,
}

impl Args {
    /// No arguments.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds or replaces an argument (builder style).
    pub fn with(mut self, name: &str, value: ArgValue) -> Self {
        self.values.insert(name.to_owned(), value);
        self
    }

    /// Raw access.
    pub fn get(&self, name: &str) -> Option<&ArgValue> {
        self.values.get(name)
    }

    /// A boolean argument, if present with that type.
    pub fn bool(&self, name: &str) -> Option<bool> {
        match self.get(name)? {
            ArgValue::Bool(b) => Some(*b),
            _ => None,
        }
    }

    /// An integer argument, if present with that type.
    pub fn int(&self, name: &str) -> Option<i64> {
        match self.get(name)? {
            ArgValue::Int(i) => Some(*i),
            _ => None,
        }
    }

    /// A float argument (integers are accepted), if present.
    pub fn float(&self, name: &str) -> Option<f64> {
        match self.get(name)? {
            ArgValue::Float(f) => Some(*f),
            ArgValue::Int(i) => Some(*i as f64),
            _ => None,
        }
    }

    /// A string-like argument (string, enum id, color, reference), if present.
    pub fn str(&self, name: &str) -> Option<&str> {
        match self.get(name)? {
            ArgValue::String(s) => Some(s),
            _ => None,
        }
    }

    /// Whether there are no arguments.
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }
}

/// Checks one value against its spec. Returns a reason on failure.
fn check_value(spec: &ArgSpec, value: &ArgValue) -> Result<(), String> {
    let name = spec.name;
    match (spec.kind, value) {
        (ArgKind::Bool, ArgValue::Bool(_)) => Ok(()),
        (ArgKind::Int { min, max }, ArgValue::Int(n)) => {
            if min.is_some_and(|min| *n < min) || max.is_some_and(|max| *n > max) {
                Err(format!("`{name}` = {n} is out of range"))
            } else {
                Ok(())
            }
        }
        (ArgKind::Float { min, max }, ArgValue::Float(f)) => {
            if !f.is_finite() {
                Err(format!("`{name}` must be a finite number"))
            } else if min.is_some_and(|min| *f < min) || max.is_some_and(|max| *f > max) {
                Err(format!("`{name}` = {f} is out of range"))
            } else {
                Ok(())
            }
        }
        (ArgKind::Float { .. }, ArgValue::Int(n)) => check_value(spec, &ArgValue::Float(*n as f64)),
        (ArgKind::String { max_len }, ArgValue::String(s)) => {
            if max_len.is_some_and(|max| s.chars().count() > max) {
                Err(format!("`{name}` is too long"))
            } else {
                Ok(())
            }
        }
        (ArgKind::Enum { variants }, ArgValue::String(s)) => {
            if variants.contains(&s.as_str()) {
                Ok(())
            } else {
                Err(format!("`{name}` must be one of: {}", variants.join(", ")))
            }
        }
        (ArgKind::Color, ArgValue::String(s)) => {
            let hex = s.strip_prefix('#').unwrap_or("");
            if (hex.len() == 6 || hex.len() == 8) && hex.chars().all(|c| c.is_ascii_hexdigit()) {
                Ok(())
            } else {
                Err(format!("`{name}` must be a color like #1e90ff"))
            }
        }
        (ArgKind::Ref { .. }, ArgValue::String(s)) if !s.is_empty() => Ok(()),
        _ => Err(format!("`{name}` has the wrong type")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CommandError;

    const ID: CommandId = CommandId::new("board.insert_shape");
    const SCHEMA: ArgsSchema = ArgsSchema {
        args: &[
            ArgSpec {
                name: "kind",
                description: "Shape to insert",
                kind: ArgKind::Enum {
                    variants: &["rectangle", "ellipse"],
                },
                required: true,
            },
            ArgSpec {
                name: "width",
                description: "Width in points",
                kind: ArgKind::Float {
                    min: Some(1.0),
                    max: Some(10_000.0),
                },
                required: false,
            },
            ArgSpec {
                name: "count",
                description: "How many",
                kind: ArgKind::Int {
                    min: Some(1),
                    max: Some(100),
                },
                required: false,
            },
            ArgSpec {
                name: "fill",
                description: "Fill color",
                kind: ArgKind::Color,
                required: false,
            },
            ArgSpec {
                name: "parent",
                description: "Container to insert into",
                kind: ArgKind::Ref { target: "node" },
                required: false,
            },
            ArgSpec {
                name: "label",
                description: "Text inside",
                kind: ArgKind::String { max_len: Some(5) },
                required: false,
            },
        ],
    };

    fn ok(args: Args) -> bool {
        SCHEMA.validate(ID, &args).is_ok()
    }

    #[test]
    fn valid_arguments_pass() {
        let args = Args::new()
            .with("kind", ArgValue::String("ellipse".into()))
            .with("width", ArgValue::Float(20.0))
            .with("count", ArgValue::Int(3))
            .with("fill", ArgValue::String("#FF0080".into()))
            .with("parent", ArgValue::String("n42".into()))
            .with("label", ArgValue::String("héllo".into()));
        assert!(ok(args));
        assert!(ok(
            Args::new().with("kind", ArgValue::String("rectangle".into()))
        ));
    }

    #[test]
    fn invalid_arguments_fail() {
        let base = || Args::new().with("kind", ArgValue::String("ellipse".into()));
        assert!(!ok(Args::new()), "missing required");
        assert!(
            !ok(base().with("bogus", ArgValue::Bool(true))),
            "unknown name"
        );
        assert!(
            !ok(Args::new().with("kind", ArgValue::String("star".into()))),
            "bad enum"
        );
        assert!(!ok(base().with("width", ArgValue::Float(0.5))), "below min");
        assert!(
            !ok(base().with("width", ArgValue::Float(f64::INFINITY))),
            "not finite"
        );
        assert!(!ok(base().with("count", ArgValue::Int(101))), "above max");
        assert!(
            !ok(base().with("count", ArgValue::Float(2.0))),
            "float for int"
        );
        assert!(
            !ok(base().with("fill", ArgValue::String("red".into()))),
            "bad color"
        );
        assert!(
            !ok(base().with("parent", ArgValue::String(String::new()))),
            "empty ref"
        );
        assert!(
            !ok(base().with("label", ArgValue::String("toolong".into()))),
            "too long"
        );
        assert!(
            ok(base().with("width", ArgValue::Int(20))),
            "int accepted for float"
        );
    }

    #[test]
    fn json_schema_describes_every_argument() {
        let schema = SCHEMA.to_json_schema();
        assert_eq!(schema["type"], "object");
        assert_eq!(schema["required"], json!(["kind"]));
        assert_eq!(schema["additionalProperties"], json!(false));
        assert_eq!(
            schema["properties"]["kind"]["enum"],
            json!(["rectangle", "ellipse"])
        );
        assert_eq!(schema["properties"]["count"]["maximum"], json!(100));
        assert_eq!(schema["properties"]["width"]["type"], "number");
        assert_eq!(schema["properties"]["label"]["maxLength"], json!(5));
        assert!(
            schema["properties"]["parent"]["description"]
                .as_str()
                .unwrap()
                .contains("id of a node")
        );
        assert_eq!(ArgsSchema::NONE.to_json_schema()["properties"], json!({}));
    }

    #[test]
    fn json_tool_input_is_parsed_and_validated() {
        let args = SCHEMA
            .args_from_json(
                ID,
                &json!({ "kind": "rectangle", "width": 12.5, "count": 2 }),
            )
            .unwrap();
        assert_eq!(args.str("kind"), Some("rectangle"));
        assert_eq!(args.float("width"), Some(12.5));
        assert_eq!(args.int("count"), Some(2));

        for bad in [
            json!("not an object"),
            json!({ "kind": "rectangle", "extra": 1 }),
            json!({ "kind": 3 }),
            json!({ "kind": "rectangle", "count": 1.5 }),
            json!({ "width": 10 }),
        ] {
            assert!(
                matches!(
                    SCHEMA.args_from_json(ID, &bad),
                    Err(CommandError::InvalidArgs { .. })
                ),
                "{bad} should fail"
            );
        }
    }
}
