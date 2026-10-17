// SPDX-License-Identifier: AGPL-3.0-or-later

//! Copied nodes on the clipboard (`canvas.md` §8): HTML with the nodes as
//! JSON in a marked comment (base64, so nothing in it ends the comment),
//! and their text for other apps.

use std::collections::BTreeMap;
use std::sync::Arc;

use imaginarium_model::{Geometry, Value};
use serde_json::{Map, Value as Json, json};

use crate::copy::{NodeCopy, ParagraphCopy, RunCopy};

/// What marks the board's HTML: the comment's start.
pub const MARKER: &str = "<!--imaginarium:nodes:";

/// What a copy holds: the nodes (their boxes in board coordinates), and
/// the effective properties of the named styles they use, so a paste
/// into a board without those styles keeps their look.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Copied {
    /// The copied nodes, back to front.
    pub nodes: Vec<NodeCopy>,
    /// Style id → its effective properties.
    pub styles: Vec<(String, Vec<(String, Value)>)>,
}

fn value_to_json(value: &Value) -> Json {
    match value {
        Value::Null => Json::Null,
        Value::Bool(value) => Json::Bool(*value),
        Value::Int(value) => json!(value),
        // A float always has a fraction or an exponent in JSON, so it reads
        // back as a float.
        Value::Number(value) => {
            serde_json::Number::from_f64(*value).map_or(Json::Null, Json::Number)
        }
        Value::String(value) => Json::String(value.to_string()),
        Value::Binary(bytes) => json!({ "$binary": base64(bytes) }),
        Value::List(items) => Json::Array(items.iter().map(value_to_json).collect()),
        Value::Map(map) => Json::Object(
            map.iter()
                .map(|(key, value)| (key.clone(), value_to_json(value)))
                .collect(),
        ),
    }
}

fn json_to_value(json: &Json) -> Value {
    match json {
        Json::Null => Value::Null,
        Json::Bool(value) => Value::Bool(*value),
        Json::Number(number) => match number.as_i64() {
            Some(int) if !number.is_f64() => Value::Int(int),
            _ => Value::Number(number.as_f64().unwrap_or(0.0)),
        },
        Json::String(text) => Value::String(text.as_str().into()),
        Json::Array(items) => {
            Value::List(items.iter().map(json_to_value).collect::<Vec<_>>().into())
        }
        Json::Object(map) => {
            if let (1, Some(Json::String(bytes))) = (map.len(), map.get("$binary")) {
                return Value::Binary(unbase64(bytes).unwrap_or_default().into());
            }
            Value::Map(Arc::new(
                map.iter()
                    .map(|(key, value)| (key.clone(), json_to_value(value)))
                    .collect::<BTreeMap<_, _>>(),
            ))
        }
    }
}

fn pairs_to_json(pairs: &[(String, Value)]) -> Json {
    Json::Object(
        pairs
            .iter()
            .map(|(key, value)| (key.clone(), value_to_json(value)))
            .collect(),
    )
}

fn json_to_pairs(json: Option<&Json>) -> Vec<(String, Value)> {
    json.and_then(Json::as_object)
        .map(|map| {
            map.iter()
                .map(|(key, value)| (key.clone(), json_to_value(value)))
                .collect()
        })
        .unwrap_or_default()
}

fn node_to_json(node: &NodeCopy) -> Json {
    let text: Vec<Json> = node
        .text
        .iter()
        .map(|paragraph| {
            json!({
                "runs": paragraph.runs.iter().map(|run| json!({ "text": run.text, "marks": pairs_to_json(&run.marks) })).collect::<Vec<_>>(),
                "marks": pairs_to_json(&paragraph.marks),
            })
        })
        .collect();
    let mut out = Map::new();
    out.insert("kind".into(), json!(node.kind));
    if let Some(g) = node.geometry {
        out.insert(
            "g".into(),
            json!({ "x": g.x, "y": g.y, "w": g.w, "h": g.h, "r": g.r }),
        );
    }
    if let Some(style) = &node.style {
        out.insert("style".into(), json!(style));
    }
    out.insert("overrides".into(), pairs_to_json(&node.overrides));
    out.insert("text".into(), Json::Array(text));
    out.insert("props".into(), pairs_to_json(&node.props));
    out.insert(
        "assets".into(),
        Json::Object(
            node.assets
                .iter()
                .map(|(role, blob)| (role.clone(), json!(blob)))
                .collect(),
        ),
    );
    out.insert("ext".into(), pairs_to_json(&node.ext));
    out.insert("gen".into(), pairs_to_json(&node.generation));
    out.insert(
        "children".into(),
        Json::Array(node.children.iter().map(node_to_json).collect()),
    );
    Json::Object(out)
}

fn json_to_node(json: &Json) -> Option<NodeCopy> {
    let map = json.as_object()?;
    let number = |g: &Map<String, Json>, key: &str| {
        g.get(key)
            .and_then(Json::as_f64)
            .filter(|value| value.is_finite())
            .unwrap_or(0.0)
    };
    let geometry = map.get("g").and_then(Json::as_object).map(|g| Geometry {
        x: number(g, "x"),
        y: number(g, "y"),
        w: number(g, "w").max(0.0),
        h: number(g, "h").max(0.0),
        r: number(g, "r"),
    });
    let text = map
        .get("text")
        .and_then(Json::as_array)
        .map(|paragraphs| {
            paragraphs
                .iter()
                .map(|paragraph| ParagraphCopy {
                    runs: paragraph
                        .get("runs")
                        .and_then(Json::as_array)
                        .map(|runs| {
                            runs.iter()
                                .map(|run| RunCopy {
                                    // No line breaks inside a run.
                                    text: run
                                        .get("text")
                                        .and_then(Json::as_str)
                                        .unwrap_or("")
                                        .replace(['\n', '\r'], " "),
                                    marks: json_to_pairs(run.get("marks")),
                                })
                                .collect()
                        })
                        .unwrap_or_default(),
                    marks: json_to_pairs(paragraph.get("marks")),
                })
                .collect()
        })
        .unwrap_or_default();
    Some(NodeCopy {
        kind: map
            .get("kind")
            .and_then(Json::as_str)
            .unwrap_or("shape")
            .to_owned(),
        geometry,
        style: map.get("style").and_then(Json::as_str).map(str::to_owned),
        overrides: json_to_pairs(map.get("overrides")),
        text,
        props: json_to_pairs(map.get("props")),
        assets: map
            .get("assets")
            .and_then(Json::as_object)
            .map(|assets| {
                assets
                    .iter()
                    .filter_map(|(role, blob)| Some((role.clone(), blob.as_str()?.to_owned())))
                    .collect()
            })
            .unwrap_or_default(),
        ext: json_to_pairs(map.get("ext")),
        generation: json_to_pairs(map.get("gen")),
        children: map
            .get("children")
            .and_then(Json::as_array)
            .map(|children| children.iter().filter_map(json_to_node).collect())
            .unwrap_or_default(),
    })
}

/// The copy as clipboard HTML: the nodes' text, readable, and the nodes in
/// the marked comment.
pub fn to_html(copied: &Copied) -> String {
    let styles: Map<String, Json> = copied
        .styles
        .iter()
        .map(|(id, props)| (id.clone(), pairs_to_json(props)))
        .collect();
    let payload = json!({
        "v": 1,
        "nodes": copied.nodes.iter().map(node_to_json).collect::<Vec<_>>(),
        "styles": styles,
    });
    let mut html = String::from("<meta charset=\"utf-8\">");
    for paragraph in plain_text(copied)
        .lines()
        .filter(|line| !line.trim().is_empty())
    {
        html.push_str("<p>");
        for character in paragraph.chars() {
            match character {
                '<' => html.push_str("&lt;"),
                '>' => html.push_str("&gt;"),
                '&' => html.push_str("&amp;"),
                other => html.push(other),
            }
        }
        html.push_str("</p>");
    }
    html.push_str(MARKER);
    html.push_str(&base64(payload.to_string().as_bytes()));
    html.push_str("-->");
    html
}

/// The nodes in clipboard HTML, if it holds the board's comment.
pub fn from_html(html: &str) -> Option<Copied> {
    let start = html.find(MARKER)? + MARKER.len();
    let end = html[start..].find("-->")? + start;
    let bytes = unbase64(html[start..end].trim())?;
    let payload: Json = serde_json::from_slice(&bytes).ok()?;
    let nodes = payload
        .get("nodes")?
        .as_array()?
        .iter()
        .filter_map(json_to_node)
        .collect();
    let styles = payload
        .get("styles")
        .and_then(Json::as_object)
        .map(|styles| {
            styles
                .iter()
                .map(|(id, props)| (id.clone(), json_to_pairs(Some(props))))
                .collect()
        })
        .unwrap_or_default();
    Some(Copied { nodes, styles })
}

/// The copy's text for other apps: each node's text, a blank line between
/// nodes.
pub fn plain_text(copied: &Copied) -> String {
    fn collect(node: &NodeCopy, out: &mut Vec<String>) {
        let text = node.plain_text();
        if !text.trim().is_empty() {
            out.push(text.trim_end().to_owned());
        }
        for child in &node.children {
            collect(child, out);
        }
    }
    let mut texts = Vec::new();
    for node in &copied.nodes {
        collect(node, &mut texts);
    }
    texts.join("\n\n")
}

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Standard base64 with padding.
fn base64(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let value = chunk.iter().enumerate().fold(0u32, |value, (index, byte)| {
            value | u32::from(*byte) << (16 - 8 * index)
        });
        for index in 0..4 {
            if index <= chunk.len() {
                out.push(char::from(
                    ALPHABET[(value >> (18 - 6 * index) & 63) as usize],
                ));
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Decodes standard base64 (padding optional); `None` on other text.
fn unbase64(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len() / 4 * 3);
    let mut buffer = 0u32;
    let mut bits = 0;
    for byte in text.bytes() {
        if byte == b'=' {
            break;
        }
        let digit = ALPHABET.iter().position(|letter| *letter == byte)? as u32;
        buffer = buffer << 6 | digit;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
            buffer &= (1 << bits) - 1;
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_round_trips() {
        for bytes in [&b""[..], b"f", b"fo", b"foo", b"foob", b"\x00\xff\x10"] {
            assert_eq!(unbase64(&base64(bytes)).unwrap(), bytes);
        }
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
        assert!(unbase64("not base64!").is_none());
    }

    #[test]
    fn values_round_trip_through_json() {
        let values = [
            Value::Null,
            Value::Bool(true),
            Value::Int(42),
            Value::Number(1.0),
            Value::Number(1.5),
            Value::String("x".into()),
            Value::Binary(vec![1u8, 2, 3].into()),
            Value::List(vec![Value::Int(1), Value::Number(2.5)].into()),
            Value::Map(Arc::new(BTreeMap::from([(
                "k".to_owned(),
                Value::Bool(false),
            )]))),
        ];
        for value in values {
            assert_eq!(json_to_value(&value_to_json(&value)), value, "{value:?}");
        }
    }
}
