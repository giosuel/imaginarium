// SPDX-License-Identifier: AGPL-3.0-or-later

//! Owned values of the view model, independent of the CRDT library.

use std::collections::BTreeMap;
use std::sync::Arc;

use loro::LoroValue;

/// A scalar or nested value (schema-v1.md §2).
#[derive(Clone, Debug, PartialEq, Default)]
pub enum Value {
    /// No value. Writing it deletes the key.
    #[default]
    Null,
    /// A boolean.
    Bool(bool),
    /// An integer.
    Int(i64),
    /// A floating-point number (lengths, angles, opacities).
    Number(f64),
    /// A string.
    String(Arc<str>),
    /// Bytes.
    Binary(Arc<[u8]>),
    /// A list.
    List(Arc<[Value]>),
    /// A map with sorted keys.
    Map(Arc<BTreeMap<String, Value>>),
}

impl Value {
    /// The value as a number, accepting integers too (schema-v1.md §2).
    pub fn as_number(&self) -> Option<f64> {
        match self {
            Value::Number(v) => Some(*v),
            Value::Int(v) => Some(*v as f64),
            _ => None,
        }
    }

    /// The value as a string.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::String(s) => Some(s),
            _ => None,
        }
    }

    /// The value as a boolean.
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }

    /// The value as an integer.
    pub fn as_int(&self) -> Option<i64> {
        match self {
            Value::Int(v) => Some(*v),
            _ => None,
        }
    }

    pub(crate) fn from_loro(value: &LoroValue) -> Self {
        match value {
            LoroValue::Null => Value::Null,
            LoroValue::Bool(b) => Value::Bool(*b),
            LoroValue::I64(v) => Value::Int(*v),
            LoroValue::Double(v) => Value::Number(*v),
            LoroValue::String(s) => Value::String(Arc::from(s.as_str())),
            LoroValue::Binary(b) => Value::Binary(Arc::from(b.as_slice())),
            LoroValue::List(list) => Value::List(list.iter().map(Value::from_loro).collect()),
            LoroValue::Map(map) => Value::Map(Arc::new(
                map.iter()
                    .map(|(k, v)| (k.clone(), Value::from_loro(v)))
                    .collect(),
            )),
            // Containers only appear in shallow reads. The view reads nested
            // containers explicitly.
            LoroValue::Container(_) => Value::Null,
        }
    }

    pub(crate) fn to_loro(&self) -> LoroValue {
        match self {
            Value::Null => LoroValue::Null,
            Value::Bool(b) => LoroValue::Bool(*b),
            Value::Int(v) => LoroValue::I64(*v),
            Value::Number(v) => LoroValue::Double(*v),
            Value::String(s) => LoroValue::from(&**s),
            Value::Binary(b) => LoroValue::from(b.to_vec()),
            Value::List(list) => {
                LoroValue::from(list.iter().map(Value::to_loro).collect::<Vec<_>>())
            }
            Value::Map(map) => LoroValue::from(
                map.iter()
                    .map(|(k, v)| (k.clone(), v.to_loro()))
                    .collect::<std::collections::HashMap<_, _>>(),
            ),
        }
    }
}

impl From<bool> for Value {
    fn from(v: bool) -> Self {
        Value::Bool(v)
    }
}

impl From<i64> for Value {
    fn from(v: i64) -> Self {
        Value::Int(v)
    }
}

impl From<f64> for Value {
    fn from(v: f64) -> Self {
        Value::Number(v)
    }
}

impl From<&str> for Value {
    fn from(v: &str) -> Self {
        Value::String(v.into())
    }
}

impl From<String> for Value {
    fn from(v: String) -> Self {
        Value::String(v.into())
    }
}

/// A small, sorted, shared map of properties.
///
/// Stored as one exact-size slice, so a node with a single style override
/// costs tens of bytes instead of a B-tree leaf (about 550 bytes); on a
/// 10,000-node board that is the difference of ~10 MiB. Empty maps share
/// one allocation.
#[derive(Clone, PartialEq)]
pub struct Props(Arc<[(Arc<str>, Value)]>);

static EMPTY_PROPS: std::sync::OnceLock<Props> = std::sync::OnceLock::new();

impl Props {
    /// The value of `key`.
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.0
            .binary_search_by(|(k, _)| (**k).cmp(key))
            .ok()
            .map(|index| &self.0[index].1)
    }

    /// Whether `key` is present.
    pub fn contains_key(&self, key: &str) -> bool {
        self.get(key).is_some()
    }

    /// All entries, sorted by key.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &Value)> {
        self.0.iter().map(|(k, v)| (&**k, v))
    }

    /// Number of entries.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether there are no entries.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub(crate) fn from_map(map: BTreeMap<String, Value>) -> Self {
        if map.is_empty() {
            return Self::default();
        }
        Self(map.into_iter().map(|(k, v)| (Arc::from(k), v)).collect())
    }
}

impl Default for Props {
    fn default() -> Self {
        EMPTY_PROPS.get_or_init(|| Props(Arc::from([]))).clone()
    }
}

impl std::fmt::Debug for Props {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_map().entries(self.iter()).finish()
    }
}

/// A [`Props`] value from a map.
pub(crate) fn props(map: BTreeMap<String, Value>) -> Props {
    Props::from_map(map)
}

/// A shared map, sharing the allocation when it's empty.
pub(crate) fn shared<V: Send + Sync + 'static>(
    map: BTreeMap<String, V>,
    empty: &'static std::sync::OnceLock<Arc<BTreeMap<String, V>>>,
) -> Arc<BTreeMap<String, V>> {
    if map.is_empty() {
        empty.get_or_init(|| Arc::new(BTreeMap::new())).clone()
    } else {
        Arc::new(map)
    }
}

/// A node's box (schema-v1.md §5.2), in points.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Geometry {
    /// Left edge.
    pub x: f64,
    /// Top edge.
    pub y: f64,
    /// Width.
    pub w: f64,
    /// Height.
    pub h: f64,
    /// Rotation in degrees, clockwise around the center.
    pub r: f64,
}

impl Geometry {
    /// A box without rotation.
    pub fn new(x: f64, y: f64, w: f64, h: f64) -> Self {
        Self { x, y, w, h, r: 0.0 }
    }
}
