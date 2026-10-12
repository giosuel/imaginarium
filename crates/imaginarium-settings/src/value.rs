// SPDX-License-Identifier: AGPL-3.0-or-later

//! Setting values: the dynamic [`Value`] and the typed [`SettingType`] bridge.

use std::fmt;
use std::path::PathBuf;
use std::str::FromStr;

/// The kind of value a setting holds. The settings UI picks its editor
/// from this (toggle, number field, dropdown, color picker, …).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ValueKind {
    /// `true` / `false`.
    Bool,
    /// 64-bit signed integer.
    Int,
    /// 64-bit float.
    Float,
    /// UTF-8 text.
    String,
    /// One of a fixed set of variants (see [`SettingEnum`]).
    Enum,
    /// sRGB color with alpha.
    Color,
    /// File-system path.
    Path,
    /// Reference to an entry in the OS keychain. Never the secret itself.
    SecretRef,
    /// List of values of one kind.
    List(ListKind),
}

/// Element kind of a [`ValueKind::List`]. Lists don't nest.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ListKind {
    /// List of booleans.
    Bool,
    /// List of integers.
    Int,
    /// List of floats.
    Float,
    /// List of strings.
    String,
    /// List of enum variant ids.
    Enum,
    /// List of colors.
    Color,
    /// List of paths.
    Path,
}

/// A setting value in dynamic form, as stored in the registry and in files.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    /// See [`ValueKind::Bool`].
    Bool(bool),
    /// See [`ValueKind::Int`].
    Int(i64),
    /// See [`ValueKind::Float`].
    Float(f64),
    /// See [`ValueKind::String`].
    String(String),
    /// Variant id of an enum setting, e.g. `"dark"`.
    Enum(String),
    /// See [`ValueKind::Color`].
    Color(Rgba),
    /// See [`ValueKind::Path`].
    Path(PathBuf),
    /// See [`ValueKind::SecretRef`].
    SecretRef(SecretRef),
    /// See [`ValueKind::List`]. All elements have the same kind.
    List(Vec<Value>),
}

impl Value {
    /// Short human-readable name of the value's kind, for error messages.
    pub fn kind_name(&self) -> &'static str {
        match self {
            Value::Bool(_) => "bool",
            Value::Int(_) => "int",
            Value::Float(_) => "float",
            Value::String(_) => "string",
            Value::Enum(_) => "enum",
            Value::Color(_) => "color",
            Value::Path(_) => "path",
            Value::SecretRef(_) => "secret reference",
            Value::List(_) => "list",
        }
    }
}

/// An sRGB color with straight (non-premultiplied) alpha.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Rgba {
    /// Red, 0–255.
    pub r: u8,
    /// Green, 0–255.
    pub g: u8,
    /// Blue, 0–255.
    pub b: u8,
    /// Alpha, 0 (transparent) – 255 (opaque).
    pub a: u8,
}

impl Rgba {
    /// An opaque color.
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }
}

/// Formats as `#rrggbb` when opaque, else `#rrggbbaa` (lowercase).
impl fmt::Display for Rgba {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{:02x}{:02x}{:02x}", self.r, self.g, self.b)?;
        if self.a != 255 {
            write!(f, "{:02x}", self.a)?;
        }
        Ok(())
    }
}

/// Parses `#rrggbb` or `#rrggbbaa` (case-insensitive).
impl FromStr for Rgba {
    type Err = &'static str;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        const ERROR: &str = "expected a color like #1e90ff or #1e90ff80";
        let hex = s.strip_prefix('#').ok_or(ERROR)?;
        if !(hex.len() == 6 || hex.len() == 8) || !hex.is_ascii() {
            return Err(ERROR);
        }
        let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).map_err(|_| ERROR);
        Ok(Rgba {
            r: byte(0)?,
            g: byte(2)?,
            b: byte(4)?,
            a: if hex.len() == 8 { byte(6)? } else { 255 },
        })
    }
}

/// A reference to a secret stored in the OS keychain (Keychain, Credential
/// Manager, Secret Service), e.g. an AI provider's API key.
///
/// It holds only the entry's **id**, never the secret. Higher layers
/// resolve it through the keychain. This is the only way to "store" a
/// secret in settings: plaintext secrets never reach settings files
/// (ADR-0014).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SecretRef {
    /// Keychain entry id, e.g. `"ai.provider.anthropic"`.
    pub id: String,
}

/// A fixed set of named variants, usable as a setting type.
///
/// Implement it with [`setting_enum!`](crate::setting_enum), which also
/// implements [`SettingType`].
pub trait SettingEnum: Sized + Copy + 'static {
    /// All variants with their stable ids and UI labels, in display order.
    const VARIANTS: &'static [EnumVariant<Self>];
}

/// One variant of a [`SettingEnum`].
#[derive(Debug)]
pub struct EnumVariant<T: 'static> {
    /// The variant.
    pub value: T,
    /// Stable id stored in files (`snake_case`). Never change it.
    pub id: &'static str,
    /// UI label.
    pub label: &'static str,
}

/// Id and label of an enum variant, without the typed value (for the UI).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VariantInfo {
    /// Stable id stored in files.
    pub id: &'static str,
    /// UI label.
    pub label: &'static str,
}

/// A Rust type that can be a setting value.
pub trait SettingType: Sized + Clone + Send + Sync + 'static {
    /// The value kind, which decides the UI editor and the file encoding.
    const KIND: ValueKind;

    /// Converts to the dynamic form.
    fn to_value(&self) -> Value;

    /// Converts from the dynamic form. Returns `None` if the kind or the
    /// content doesn't fit (e.g. an unknown enum id).
    fn from_value(value: &Value) -> Option<Self>;

    /// Variant ids and labels, for enum types. Empty for all other kinds.
    fn variants() -> Vec<VariantInfo> {
        Vec::new()
    }
}

/// Implements [`SettingType`] for a scalar with a one-to-one [`Value`]
/// variant.
macro_rules! scalar_setting_type {
    ($ty:ty, $kind:ident, $variant:ident) => {
        impl SettingType for $ty {
            const KIND: ValueKind = ValueKind::$kind;

            fn to_value(&self) -> Value {
                Value::$variant(self.clone())
            }

            fn from_value(value: &Value) -> Option<Self> {
                match value {
                    Value::$variant(inner) => Some(inner.clone()),
                    _ => None,
                }
            }
        }
    };
}

scalar_setting_type!(bool, Bool, Bool);
scalar_setting_type!(i64, Int, Int);
scalar_setting_type!(String, String, String);
scalar_setting_type!(Rgba, Color, Color);
scalar_setting_type!(PathBuf, Path, Path);
scalar_setting_type!(SecretRef, SecretRef, SecretRef);

impl SettingType for f64 {
    const KIND: ValueKind = ValueKind::Float;

    fn to_value(&self) -> Value {
        Value::Float(*self)
    }

    /// Integers are accepted too (`1` in a file means `1.0`).
    fn from_value(value: &Value) -> Option<Self> {
        match value {
            Value::Float(f) => Some(*f),
            Value::Int(i) => Some(*i as f64),
            _ => None,
        }
    }
}

/// Element types allowed in list settings.
pub trait ListElement: SettingType {
    /// The list's element kind.
    const LIST_KIND: ListKind;
}

impl ListElement for bool {
    const LIST_KIND: ListKind = ListKind::Bool;
}
impl ListElement for i64 {
    const LIST_KIND: ListKind = ListKind::Int;
}
impl ListElement for f64 {
    const LIST_KIND: ListKind = ListKind::Float;
}
impl ListElement for String {
    const LIST_KIND: ListKind = ListKind::String;
}
impl ListElement for Rgba {
    const LIST_KIND: ListKind = ListKind::Color;
}
impl ListElement for PathBuf {
    const LIST_KIND: ListKind = ListKind::Path;
}

impl<T: ListElement> SettingType for Vec<T> {
    const KIND: ValueKind = ValueKind::List(T::LIST_KIND);

    fn to_value(&self) -> Value {
        Value::List(self.iter().map(SettingType::to_value).collect())
    }

    fn from_value(value: &Value) -> Option<Self> {
        match value {
            Value::List(items) => items.iter().map(T::from_value).collect(),
            _ => None,
        }
    }
}

/// Declares a setting enum: implements [`SettingEnum`], [`SettingType`] and
/// [`ListElement`] (so `Vec<YourEnum>` settings work too).
///
/// ```
/// use imaginarium_settings::setting_enum;
///
/// #[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// pub enum Theme { System, Light, Dark }
///
/// setting_enum!(Theme {
///     System => "system", "Follow system",
///     Light => "light", "Light",
///     Dark => "dark", "Dark",
/// });
/// ```
#[macro_export]
macro_rules! setting_enum {
    ($ty:ident { $($variant:ident => $id:literal, $label:literal),+ $(,)? }) => {
        impl $crate::SettingEnum for $ty {
            const VARIANTS: &'static [$crate::EnumVariant<Self>] = &[
                $($crate::EnumVariant { value: $ty::$variant, id: $id, label: $label }),+
            ];
        }

        impl $crate::SettingType for $ty {
            const KIND: $crate::ValueKind = $crate::ValueKind::Enum;

            fn to_value(&self) -> $crate::Value {
                let id = match self {
                    $($ty::$variant => $id),+
                };
                $crate::Value::Enum(id.to_owned())
            }

            fn from_value(value: &$crate::Value) -> Option<Self> {
                match value {
                    $crate::Value::Enum(id) => <$ty as $crate::SettingEnum>::VARIANTS
                        .iter()
                        .find(|variant| variant.id == id)
                        .map(|variant| variant.value),
                    _ => None,
                }
            }

            fn variants() -> Vec<$crate::VariantInfo> {
                <$ty as $crate::SettingEnum>::VARIANTS
                    .iter()
                    .map(|variant| $crate::VariantInfo { id: variant.id, label: variant.label })
                    .collect()
            }
        }

        impl $crate::ListElement for $ty {
            const LIST_KIND: $crate::ListKind = $crate::ListKind::Enum;
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Mode {
        Fast,
        Pretty,
    }

    setting_enum!(Mode {
        Fast => "fast", "Fast",
        Pretty => "pretty", "Pretty",
    });

    #[test]
    fn scalars_round_trip() {
        assert_eq!(bool::from_value(&true.to_value()), Some(true));
        assert_eq!(i64::from_value(&(-7i64).to_value()), Some(-7));
        assert_eq!(f64::from_value(&1.5f64.to_value()), Some(1.5));
        let s = "hello".to_owned();
        assert_eq!(String::from_value(&s.to_value()), Some(s));
        let c = Rgba::rgb(1, 2, 3);
        assert_eq!(Rgba::from_value(&c.to_value()), Some(c));
        let p = PathBuf::from("/tmp/x");
        assert_eq!(PathBuf::from_value(&p.to_value()), Some(p));
    }

    #[test]
    fn floats_accept_ints_but_not_the_reverse() {
        assert_eq!(f64::from_value(&Value::Int(2)), Some(2.0));
        assert_eq!(i64::from_value(&Value::Float(2.0)), None);
        assert_eq!(bool::from_value(&Value::Int(1)), None);
    }

    #[test]
    fn enums_use_stable_ids() {
        assert_eq!(Mode::Pretty.to_value(), Value::Enum("pretty".into()));
        assert_eq!(
            Mode::from_value(&Value::Enum("fast".into())),
            Some(Mode::Fast)
        );
        assert_eq!(Mode::from_value(&Value::Enum("turbo".into())), None);
        assert_eq!(Mode::variants().len(), 2);
        assert_eq!(Mode::KIND, ValueKind::Enum);
    }

    #[test]
    fn lists_round_trip_and_reject_mixed_kinds() {
        let modes = vec![Mode::Fast, Mode::Pretty];
        assert_eq!(Vec::<Mode>::from_value(&modes.to_value()), Some(modes));
        let mixed = Value::List(vec![Value::Int(1), Value::Bool(true)]);
        assert_eq!(Vec::<i64>::from_value(&mixed), None);
        assert_eq!(Vec::<Mode>::KIND, ValueKind::List(ListKind::Enum));
    }

    #[test]
    fn colors_parse_and_format() {
        let c: Rgba = "#1E90FF".parse().unwrap();
        assert_eq!(c, Rgba::rgb(0x1e, 0x90, 0xff));
        assert_eq!(c.to_string(), "#1e90ff");
        let translucent: Rgba = "#1e90ff80".parse().unwrap();
        assert_eq!(translucent.a, 0x80);
        assert_eq!(translucent.to_string(), "#1e90ff80");
        for bad in ["1e90ff", "#1e90f", "#gggggg", "#1e90ff8", "#ééé"] {
            assert!(bad.parse::<Rgba>().is_err(), "{bad} should fail");
        }
    }
}
