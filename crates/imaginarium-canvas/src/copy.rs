// SPDX-License-Identifier: AGPL-3.0-or-later

//! Nodes as plain data (`canvas.md` §8): a subtree read from the view and
//! written back as new nodes, for duplicating and the clipboard.

use imaginarium_model::{
    DocumentView, Geometry, NodeId, NodeKind, Parent, Result, StyleId, Transaction, Value,
};

/// A character run of a node's text with its marks.
#[derive(Clone, Debug, PartialEq)]
pub struct RunCopy {
    /// The run's text (no line break).
    pub text: String,
    /// Its character marks (schema-v1.md §6.1).
    pub marks: Vec<(String, Value)>,
}

/// A paragraph: its runs and its paragraph marks (§6.2).
#[derive(Clone, Debug, PartialEq)]
pub struct ParagraphCopy {
    /// The runs, in order.
    pub runs: Vec<RunCopy>,
    /// The marks on its line break: paragraph marks, and character or
    /// add-on marks running over it.
    pub marks: Vec<(String, Value)>,
}

/// A node and its subtree, without identities: what a copy or a paste
/// creates anew.
#[derive(Clone, Debug, PartialEq)]
pub struct NodeCopy {
    /// The kind (`k`).
    pub kind: String,
    /// The box in the parent's space.
    pub geometry: Option<Geometry>,
    /// The named style, if any.
    pub style: Option<String>,
    /// Local style overrides.
    pub overrides: Vec<(String, Value)>,
    /// The text, by paragraph.
    pub text: Vec<ParagraphCopy>,
    /// Other keys (`shape`, `name`, `radii`, …).
    pub props: Vec<(String, Value)>,
    /// Asset references: role → blob id text.
    pub assets: Vec<(String, String)>,
    /// Add-on data.
    pub ext: Vec<(String, Value)>,
    /// AI provenance.
    pub generation: Vec<(String, Value)>,
    /// The children, back to front.
    pub children: Vec<NodeCopy>,
}

fn pairs(props: &imaginarium_model::Props) -> Vec<(String, Value)> {
    props
        .iter()
        .map(|(key, value)| (key.to_owned(), value.clone()))
        .collect()
}

impl NodeCopy {
    /// Reads `node` and its subtree from `view`.
    pub fn read(view: &DocumentView, node: NodeId) -> Option<Self> {
        let source = view.node(node)?;
        let text = source
            .text()
            .map(|text| {
                text.paragraphs()
                    .map(|paragraph| ParagraphCopy {
                        runs: paragraph
                            .runs()
                            .iter()
                            .map(|run| RunCopy {
                                text: run.text().trim_end_matches('\n').to_owned(),
                                marks: run
                                    .marks()
                                    .iter()
                                    .map(|(key, value)| (key.to_owned(), value.clone()))
                                    .collect(),
                            })
                            .collect(),
                        marks: paragraph
                            .end_marks()
                            .map(|marks| {
                                marks
                                    .iter()
                                    .map(|(key, value)| (key.to_owned(), value.clone()))
                                    .collect()
                            })
                            .unwrap_or_default(),
                    })
                    .collect()
            })
            .unwrap_or_default();
        Some(Self {
            kind: source.kind().as_str().to_owned(),
            geometry: source.geometry(),
            style: source.style().map(|style| style.as_str().to_owned()),
            overrides: pairs(source.style_overrides()),
            text,
            props: pairs(source.props()),
            assets: source
                .assets()
                .iter()
                .map(|(role, blob)| (role.clone(), blob.to_string()))
                .collect(),
            ext: pairs(source.ext()),
            generation: pairs(source.generation()),
            children: view
                .children(Parent::Node(node))
                .filter_map(|child| Self::read(view, child))
                .collect(),
        })
    }

    /// The text as one string, with its line breaks.
    pub fn plain_text(&self) -> String {
        let mut out = String::new();
        for paragraph in &self.text {
            for run in &paragraph.runs {
                out.push_str(&run.text);
            }
            out.push('\n');
        }
        out
    }

    /// Writes it as new nodes under `parent` at `index` (`None`: in
    /// front), `offset` added to its box. Returns the new node's id.
    pub fn write(
        &self,
        txn: &mut Transaction<'_>,
        parent: Parent,
        index: Option<usize>,
        offset: (f64, f64),
    ) -> Result<NodeId> {
        let id = txn.create_node(parent, index, &NodeKind::parse(&self.kind))?;
        if let Some(mut geometry) = self.geometry {
            geometry.x += offset.0;
            geometry.y += offset.1;
            txn.set_geometry(id, geometry)?;
        }
        if let Some(style) = &self.style
            && let Ok(style) = StyleId::new(style.as_str())
        {
            txn.set_style(id, Some(&style))?;
        }
        // A key the model refuses (`id`, a container's name, an add-on id
        // that isn't one; a copy can come from anywhere) is left out, not
        // the whole node.
        for (key, value) in &self.overrides {
            let _ = txn.set_style_override(id, key, value.clone());
        }
        for (key, value) in &self.props {
            let _ = txn.set_prop(id, key, value.clone());
        }
        for (role, blob) in &self.assets {
            if let Some(blob) = imaginarium_model::BlobId::parse(blob) {
                let _ = txn.set_asset(id, role, Some(blob));
            }
        }
        for (addon, value) in &self.ext {
            let _ = txn.set_node_ext(id, addon, value.clone());
        }
        if !self.generation.is_empty() {
            let generation: Vec<(&str, Value)> = self
                .generation
                .iter()
                .map(|(key, value)| (key.as_str(), value.clone()))
                .collect();
            txn.set_generation(id, &generation)?;
        }
        let text = self.plain_text();
        // Any paragraph, even one empty line, is text.
        if !self.text.is_empty() {
            txn.insert_text(id, 0, &text)?;
            let mut at = 0;
            for paragraph in &self.text {
                let start = at;
                for run in &paragraph.runs {
                    let len = run.text.chars().count();
                    for (key, value) in &run.marks {
                        // Marks of an unknown or add-on key may be refused:
                        // the text stays.
                        let _ = txn.mark_text(id, at..at + len, key, value.clone());
                    }
                    at += len;
                }
                // The line break's marks: paragraph marks as such, and the
                // others (a bold running across lines, an add-on's) on it.
                for (key, value) in &paragraph.marks {
                    if imaginarium_model::mark(key).is_some_and(|mark| mark.paragraph) {
                        let _ = txn.set_paragraph_mark(id, start, key, value.clone());
                    } else {
                        let _ = txn.mark_text(id, at..at + 1, key, value.clone());
                    }
                }
                at += 1;
            }
        }
        // Children back to front, so they keep their order.
        for child in &self.children {
            child.write(txn, Parent::Node(id), None, (0.0, 0.0))?;
        }
        Ok(id)
    }
}
