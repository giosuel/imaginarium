// SPDX-License-Identifier: AGPL-3.0-or-later

//! The names and rules of `docs/format/schema-v1.md`, in one place.

use std::sync::Arc;

use loro::ExpandType;

/// The schema major version this crate writes and understands.
pub const SCHEMA_VERSION: i64 = 1;

/// The codec id of `SNAP` and `UPDT` records with this model (spec §3.1).
pub const CODEC: &str = "loro-1";

/// Features this version understands (schema-v1.md §10.2). Version 1
/// defines none.
pub const KNOWN_FEATURES: &[&str] = &[];

/// Root container names (schema-v1.md §3).
pub(crate) mod root {
    pub(crate) const META: &str = "meta";
    pub(crate) const NODES: &str = "nodes";
    pub(crate) const STYLES: &str = "styles";
    pub(crate) const ASSETS: &str = "assets";
    pub(crate) const PAGES: &str = "pages";
    pub(crate) const EXT: &str = "ext";
}

/// Keys of `meta` (schema-v1.md §4).
pub(crate) mod meta {
    pub(crate) const SCHEMA_VERSION: &str = "schema_version";
    pub(crate) const KIND: &str = "kind";
    pub(crate) const TITLE: &str = "title";
    pub(crate) const LANGUAGE: &str = "language";
    pub(crate) const CREATED: &str = "created";
    pub(crate) const REQUIRED_FEATURES: &str = "required_features";
}

/// Keys of a node's meta map (schema-v1.md §5).
pub(crate) mod node {
    /// The node's stable identity (16 hex characters).
    pub(crate) const ID: &str = "id";
    pub(crate) const KIND: &str = "k";
    pub(crate) const GEOMETRY: &str = "g";
    pub(crate) const STYLE: &str = "s";
    pub(crate) const TEXT: &str = "t";
    pub(crate) const ASSETS: &str = "a";
    pub(crate) const EXT: &str = "x";
    pub(crate) const GEN: &str = "gen";
    /// Keys whose values are containers the model manages itself.
    pub(crate) const CONTAINERS: &[&str] = &[GEOMETRY, STYLE, TEXT, ASSETS, EXT, GEN];
    /// The style id inside `s`.
    pub(crate) const STYLE_ID: &str = "id";
}

/// The kind of a node (schema-v1.md §5.1). Unknown kinds are kept as text.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum NodeKind {
    /// Groups its children.
    Group,
    /// A board frame or artboard.
    Frame,
    /// A page of a document or layout.
    Page,
    /// A slide of a deck.
    Slide,
    /// A vector shape (`shape` names the preset).
    Shape,
    /// A text box.
    Text,
    /// An image.
    Image,
    /// A connector between nodes.
    Connector,
    /// A paint layer of tiles.
    Raster,
    /// A kind from a newer version. Rendered as a box (schema-v1.md §1).
    Unknown(Arc<str>),
}

impl NodeKind {
    /// The `k` value.
    pub fn as_str(&self) -> &str {
        match self {
            NodeKind::Group => "group",
            NodeKind::Frame => "frame",
            NodeKind::Page => "page",
            NodeKind::Slide => "slide",
            NodeKind::Shape => "shape",
            NodeKind::Text => "text",
            NodeKind::Image => "image",
            NodeKind::Connector => "connector",
            NodeKind::Raster => "raster",
            NodeKind::Unknown(kind) => kind,
        }
    }

    /// Parses a `k` value.
    pub fn parse(kind: &str) -> Self {
        match kind {
            "group" => NodeKind::Group,
            "frame" => NodeKind::Frame,
            "page" => NodeKind::Page,
            "slide" => NodeKind::Slide,
            "shape" => NodeKind::Shape,
            "text" => NodeKind::Text,
            "image" => NodeKind::Image,
            "connector" => NodeKind::Connector,
            "raster" => NodeKind::Raster,
            other => NodeKind::Unknown(other.into()),
        }
    }
}

/// Which view a document opens in by default (schema-v1.md §4).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum DocumentKind {
    /// A whiteboard or diagram.
    #[default]
    Board,
    /// A text document.
    Document,
    /// A slide deck.
    Deck,
    /// A page layout.
    Layout,
}

impl DocumentKind {
    /// The `meta.kind` value.
    pub fn as_str(self) -> &'static str {
        match self {
            DocumentKind::Board => "board",
            DocumentKind::Document => "document",
            DocumentKind::Deck => "deck",
            DocumentKind::Layout => "layout",
        }
    }

    /// Parses `meta.kind`. Unknown values mean a board.
    pub fn parse(kind: &str) -> Self {
        match kind {
            "document" => DocumentKind::Document,
            "deck" => DocumentKind::Deck,
            "layout" => DocumentKind::Layout,
            _ => DocumentKind::Board,
        }
    }
}

/// A text mark of schema-v1.md §6.1 and §6.2.
#[derive(Clone, Copy, Debug)]
pub struct MarkSpec {
    /// The mark key.
    pub key: &'static str,
    /// Whether text typed at the mark's edge gets the mark.
    pub expand: ExpandType,
    /// A paragraph mark, set only on a paragraph's terminating `\n`.
    pub paragraph: bool,
}

const fn character(key: &'static str, expand: ExpandType) -> MarkSpec {
    MarkSpec {
        key,
        expand,
        paragraph: false,
    }
}

const fn paragraph(key: &'static str) -> MarkSpec {
    MarkSpec {
        key,
        expand: ExpandType::None,
        paragraph: true,
    }
}

/// Every mark the schema defines.
pub const MARKS: &[MarkSpec] = &[
    character("b", ExpandType::After),
    character("i", ExpandType::After),
    character("u", ExpandType::After),
    character("s", ExpandType::After),
    character("va", ExpandType::After),
    character("font", ExpandType::After),
    character("size", ExpandType::After),
    character("color", ExpandType::After),
    character("bg", ExpandType::After),
    character("link", ExpandType::None),
    character("code", ExpandType::None),
    paragraph("ps"),
    paragraph("h"),
    paragraph("li"),
    paragraph("lvl"),
    paragraph("al"),
];

/// The schema's spec for a mark key, if it defines one.
pub fn mark(key: &str) -> Option<&'static MarkSpec> {
    MARKS.iter().find(|m| m.key == key)
}

/// Whether `key` is an add-on's namespaced mark (`<add-on id>:<key>`).
pub fn is_addon_mark(key: &str) -> bool {
    key.split_once(':')
        .is_some_and(|(id, name)| id.contains('.') && !name.is_empty())
}

/// The text style configuration every Loro document of this schema uses.
pub(crate) fn text_styles() -> loro::StyleConfigMap {
    let mut styles = loro::StyleConfigMap::new();
    for spec in MARKS {
        styles.insert(
            spec.key.into(),
            loro::StyleConfig {
                expand: spec.expand,
            },
        );
    }
    styles
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_round_trip() {
        for kind in [
            "group",
            "frame",
            "page",
            "slide",
            "shape",
            "text",
            "image",
            "connector",
            "raster",
            "table",
        ] {
            assert_eq!(NodeKind::parse(kind).as_str(), kind);
        }
        assert_eq!(NodeKind::parse("table"), NodeKind::Unknown("table".into()));
        for kind in ["board", "document", "deck", "layout"] {
            assert_eq!(DocumentKind::parse(kind).as_str(), kind);
        }
        assert_eq!(DocumentKind::parse("spreadsheet"), DocumentKind::Board);
    }

    #[test]
    fn marks_follow_the_schema() {
        assert!(mark("b").is_some_and(|m| !m.paragraph));
        assert!(mark("h").is_some_and(|m| m.paragraph && m.expand == ExpandType::None));
        assert!(mark("unknown").is_none());
        assert!(is_addon_mark("org.example.charts:series"));
        assert!(!is_addon_mark("b"));
        assert!(!is_addon_mark("nodot:key"));
    }
}
