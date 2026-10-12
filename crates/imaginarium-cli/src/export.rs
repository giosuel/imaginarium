// SPDX-License-Identifier: AGPL-3.0-or-later

//! `imag export-json`: the decrypted document as JSON, for debugging.

use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result};
use imaginarium_model::Value;
use serde_json::json;

use crate::keys::KeyArgs;

pub(crate) fn run(path: &Path, keys: &KeyArgs, compact: bool, out: &mut dyn Write) -> Result<()> {
    let opened = imaginarium_io_native::open_path(path, keys.credentials()?)
        .with_context(|| format!("opening {}", path.display()))?;
    if keys.owner.is_none()
        && let Some(acl) = &opened.acl
    {
        let owner: String = acl
            .genesis_owner()
            .0
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        eprintln!(
            "warning: the document's owner wasn't checked; its ACL chain was started by \
             {owner} (pass --owner {owner} if you trust it)"
        );
    }
    let value = json!({
        "doc_id": opened.header.doc_id.to_string(),
        "encrypted": opened.header.is_encrypted(),
        "created_with": opened.meta.as_ref().map(|m| m.created_with.clone()),
        "document": to_json(&opened.document.to_value()),
    });
    if compact {
        serde_json::to_writer(&mut *out, &value)?;
    } else {
        serde_json::to_writer_pretty(&mut *out, &value)?;
    }
    writeln!(out)?;
    Ok(())
}

/// A model value as JSON. Bytes become `{"$binary": "<hex>"}`; numbers that
/// JSON can't hold (NaN, infinities) become `null`.
pub(crate) fn to_json(value: &Value) -> serde_json::Value {
    match value {
        Value::Null => serde_json::Value::Null,
        Value::Bool(b) => json!(b),
        Value::Int(v) => json!(v),
        Value::Number(v) => serde_json::Number::from_f64(*v)
            .map_or(serde_json::Value::Null, serde_json::Value::Number),
        Value::String(s) => json!(&**s),
        Value::Binary(bytes) => {
            json!({ "$binary": bytes.iter().map(|b| format!("{b:02x}")).collect::<String>() })
        }
        Value::List(items) => serde_json::Value::Array(items.iter().map(to_json).collect()),
        Value::Map(map) => {
            serde_json::Value::Object(map.iter().map(|(k, v)| (k.clone(), to_json(v))).collect())
        }
    }
}
