// SPDX-License-Identifier: AGPL-3.0-or-later

//! A text container as paragraphs of marked runs (schema-v1.md §6),
//! updated incrementally from Loro text deltas.
//!
//! An edit touches only the paragraphs it overlaps: they are flattened to
//! characters, edited and split again at `\n`. Paragraphs are shared
//! (`Arc`) between view versions, so a keystroke copies one paragraph,
//! not the text. Very long paragraphs (no `\n` for many thousands of
//! characters) are therefore slower to edit than short ones.

use std::collections::BTreeMap;
use std::sync::Arc;

use loro::{LoroValue, TextDelta};

use crate::value::Value;

type Attributes<S> = std::collections::HashMap<String, LoroValue, S>;

/// The marks on a run: key → value, sorted. Usually empty.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Marks(Option<Arc<BTreeMap<String, Value>>>);

impl Marks {
    /// The value of a mark.
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.0.as_ref()?.get(key)
    }

    /// Whether there are no marks.
    pub fn is_empty(&self) -> bool {
        self.0.is_none()
    }

    /// All marks, sorted by key.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &Value)> {
        self.0
            .iter()
            .flat_map(|map| map.iter().map(|(k, v)| (k.as_str(), v)))
    }

    fn from_map(map: BTreeMap<String, Value>) -> Self {
        if map.is_empty() {
            Self(None)
        } else {
            Self(Some(Arc::new(map)))
        }
    }

    /// The marks of inserted text: the complete set from the delta.
    fn from_attributes<S>(attributes: Option<&Attributes<S>>) -> Self {
        let Some(attributes) = attributes else {
            return Self(None);
        };
        Self::from_map(
            attributes
                .iter()
                .filter(|(_, v)| !matches!(v, LoroValue::Null))
                .map(|(k, v)| (k.clone(), Value::from_loro(v)))
                .collect(),
        )
    }

    /// These marks after a format change (`null` removes a mark).
    fn changed<S>(&self, changes: &Attributes<S>) -> Self {
        let mut map = self.0.as_deref().cloned().unwrap_or_default();
        for (key, value) in changes {
            match value {
                LoroValue::Null => {
                    map.remove(key);
                }
                value => {
                    map.insert(key.clone(), Value::from_loro(value));
                }
            }
        }
        Self::from_map(map)
    }
}

/// A run of text with the same marks.
#[derive(Clone, Debug, PartialEq)]
pub struct Run {
    text: Arc<str>,
    marks: Marks,
    chars: usize,
}

impl Run {
    /// The text.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The marks.
    pub fn marks(&self) -> &Marks {
        &self.marks
    }

    /// Length in characters (Unicode scalar values).
    pub fn chars(&self) -> usize {
        self.chars
    }
}

/// A paragraph: runs, and the marks of its terminating `\n`.
#[derive(Clone, Debug, PartialEq)]
pub struct Paragraph {
    runs: Vec<Run>,
    end: Option<Marks>,
    chars: usize,
}

impl Paragraph {
    /// The runs, without the terminating `\n`.
    pub fn runs(&self) -> &[Run] {
        &self.runs
    }

    /// The plain text, without the terminating `\n`.
    pub fn text(&self) -> String {
        self.runs.iter().map(|r| &*r.text).collect()
    }

    /// Length in characters, without the terminating `\n`.
    pub fn chars(&self) -> usize {
        self.chars
    }

    /// The marks of the terminating `\n`, which hold the paragraph marks
    /// (schema-v1.md §6.2). `None` for a last paragraph without `\n`.
    pub fn end_marks(&self) -> Option<&Marks> {
        self.end.as_ref()
    }

    /// A paragraph mark (`ps`, `h`, `li`, `lvl`, `al`, …).
    pub fn mark(&self, key: &str) -> Option<&Value> {
        self.end.as_ref()?.get(key)
    }

    /// Characters including the terminating `\n`.
    fn span(&self) -> usize {
        self.chars + usize::from(self.end.is_some())
    }

    fn cells(&self, out: &mut Vec<Cell>) {
        for run in &self.runs {
            out.extend(run.text.chars().map(|ch| Cell {
                ch,
                marks: run.marks.clone(),
            }));
        }
        if let Some(marks) = &self.end {
            out.push(Cell {
                ch: '\n',
                marks: marks.clone(),
            });
        }
    }
}

#[derive(Clone)]
struct Cell {
    ch: char,
    marks: Marks,
}

/// The text of a node, as paragraphs.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct TextView {
    paragraphs: imbl::Vector<Arc<Paragraph>>,
    chars: usize,
}

impl TextView {
    /// Builds the view of a complete delta (`LoroText::to_delta`).
    pub(crate) fn from_delta(delta: &[TextDelta]) -> Self {
        let mut builder = Builder::default();
        for op in delta {
            if let TextDelta::Insert { insert, attributes } = op {
                builder.push_str(insert, &Marks::from_attributes(attributes.as_ref()));
            }
        }
        builder.finish()
    }

    /// Length in characters, including every `\n`.
    pub fn len(&self) -> usize {
        self.chars
    }

    /// Whether the text is empty.
    pub fn is_empty(&self) -> bool {
        self.chars == 0
    }

    /// The paragraphs.
    pub fn paragraphs(&self) -> impl Iterator<Item = &Paragraph> {
        self.paragraphs.iter().map(|p| &**p)
    }

    /// Number of paragraphs.
    pub fn paragraph_count(&self) -> usize {
        self.paragraphs.len()
    }

    /// The plain text, with `\n` between paragraphs.
    pub fn to_plain_string(&self) -> String {
        let mut out = String::with_capacity(self.chars);
        for paragraph in &self.paragraphs {
            for run in &paragraph.runs {
                out.push_str(&run.text);
            }
            if paragraph.end.is_some() {
                out.push('\n');
            }
        }
        out
    }

    /// Applies a delta of a text event.
    pub(crate) fn apply_delta(&mut self, delta: &[TextDelta]) {
        let mut pos = 0;
        for op in delta {
            match op {
                TextDelta::Retain { retain, attributes } => {
                    if let Some(changes) = attributes.as_ref().filter(|a| !a.is_empty()) {
                        let len = *retain;
                        self.edit(pos, len, |cells, start| {
                            for cell in &mut cells[start..start + len] {
                                cell.marks = cell.marks.changed(changes);
                            }
                        });
                    }
                    pos += retain;
                }
                TextDelta::Insert { insert, attributes } => {
                    let marks = Marks::from_attributes(attributes.as_ref());
                    self.edit(pos, 0, |cells, start| {
                        cells.splice(
                            start..start,
                            insert.chars().map(|ch| Cell {
                                ch,
                                marks: marks.clone(),
                            }),
                        );
                    });
                    pos += insert.chars().count();
                }
                TextDelta::Delete { delete } => {
                    let len = *delete;
                    self.edit(pos, len, |cells, start| {
                        cells.drain(start..start + len);
                    });
                }
            }
        }
    }

    /// The paragraph containing character `pos`, and where it starts.
    /// `(paragraph count, len)` for the end of the text.
    fn locate(&self, pos: usize) -> (usize, usize) {
        let mut start = 0;
        for (index, paragraph) in self.paragraphs.iter().enumerate() {
            let span = paragraph.span();
            if pos < start + span {
                return (index, start);
            }
            start += span;
        }
        (self.paragraphs.len(), start)
    }

    /// Edits characters `pos .. pos + len` (an empty range for inserts):
    /// flattens the paragraphs it overlaps, lets `edit` change them, and
    /// splits the result into paragraphs again.
    fn edit(&mut self, pos: usize, len: usize, edit: impl FnOnce(&mut Vec<Cell>, usize)) {
        let count = self.paragraphs.len();
        let (mut first, mut first_start) = self.locate(pos);
        if first == count && count > 0 && self.paragraphs[count - 1].end.is_none() {
            // Appending to an unterminated last paragraph.
            first = count - 1;
            first_start -= self.paragraphs[first].chars;
        }
        let mut last = if len == 0 {
            first
        } else {
            self.locate(pos + len - 1).0
        };
        let mut cells = Vec::new();
        for index in first..(last + 1).min(count) {
            self.paragraphs[index].cells(&mut cells);
        }
        edit(&mut cells, pos - first_start);
        // A deleted `\n` merges the edited paragraphs with the next one.
        while last + 1 < count && cells.last().is_some_and(|c| c.ch != '\n') {
            last += 1;
            self.paragraphs[last].cells(&mut cells);
        }
        let mut builder = Builder::default();
        builder.push_cells(&cells);
        let replacement = builder.finish();
        let removed: usize = (first..(last + 1).min(count))
            .map(|i| self.paragraphs[i].span())
            .sum();
        let end = (last + 1).min(count);
        let tail = self.paragraphs.split_off(end);
        self.paragraphs.truncate(first);
        self.paragraphs.append(replacement.paragraphs);
        self.paragraphs.append(tail);
        self.chars = self.chars - removed + replacement.chars;
    }
}

/// Builds paragraphs from text pieces, merging neighbouring equal marks.
#[derive(Default)]
struct Builder {
    paragraphs: imbl::Vector<Arc<Paragraph>>,
    runs: Vec<Run>,
    text: String,
    marks: Marks,
    run_chars: usize,
    paragraph_chars: usize,
    total: usize,
}

impl Builder {
    fn push_char(&mut self, ch: char, marks: &Marks) {
        if ch == '\n' {
            self.end_paragraph(Some(marks.clone()));
            return;
        }
        if self.run_chars > 0 && &self.marks != marks {
            self.end_run();
        }
        if self.run_chars == 0 {
            self.marks = marks.clone();
        }
        self.text.push(ch);
        self.run_chars += 1;
    }

    fn push_str(&mut self, text: &str, marks: &Marks) {
        for ch in text.chars() {
            self.push_char(ch, marks);
        }
    }

    fn push_cells(&mut self, cells: &[Cell]) {
        for cell in cells {
            self.push_char(cell.ch, &cell.marks);
        }
    }

    fn end_run(&mut self) {
        if self.run_chars == 0 {
            return;
        }
        self.runs.push(Run {
            text: Arc::from(std::mem::take(&mut self.text)),
            marks: std::mem::take(&mut self.marks),
            chars: self.run_chars,
        });
        self.paragraph_chars += self.run_chars;
        self.run_chars = 0;
    }

    fn end_paragraph(&mut self, end: Option<Marks>) {
        self.end_run();
        if end.is_none() && self.runs.is_empty() {
            return;
        }
        let paragraph = Paragraph {
            runs: std::mem::take(&mut self.runs),
            chars: self.paragraph_chars,
            end,
        };
        self.total += paragraph.span();
        self.paragraph_chars = 0;
        self.paragraphs.push_back(Arc::new(paragraph));
    }

    fn finish(mut self) -> TextView {
        self.end_paragraph(None);
        TextView {
            paragraphs: self.paragraphs,
            chars: self.total,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use loro::{ExpandType, LoroDoc, StyleConfig, StyleConfigMap};

    /// Applies `edit` to a Loro text and checks that the incrementally
    /// updated view equals a view built from scratch.
    fn check(edits: impl FnOnce(&loro::LoroText)) {
        let doc = LoroDoc::new();
        let mut styles = StyleConfigMap::new();
        styles.insert(
            "b".into(),
            StyleConfig {
                expand: ExpandType::After,
            },
        );
        styles.insert(
            "h".into(),
            StyleConfig {
                expand: ExpandType::None,
            },
        );
        doc.config_text_style(styles);
        let deltas = Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = deltas.clone();
        let _sub = doc.subscribe_root(Arc::new(move |event| {
            for diff in &event.events {
                if let loro::event::Diff::Text(delta) = &diff.diff {
                    sink.lock().unwrap().push(delta.clone());
                }
            }
        }));
        let text = doc.get_text("t");
        text.insert(0, "first line\nsecond\nthird").unwrap();
        doc.commit();
        let mut view = TextView::from_delta(&text.to_delta());
        deltas.lock().unwrap().clear();
        edits(&text);
        doc.commit();
        for delta in deltas.lock().unwrap().iter() {
            view.apply_delta(delta);
        }
        assert_eq!(view, TextView::from_delta(&text.to_delta()));
        assert_eq!(view.to_plain_string(), text.to_string());
        assert_eq!(view.len(), text.len_unicode());
    }

    #[test]
    fn builds_paragraphs_with_end_marks() {
        let view = TextView::from_delta(&[
            TextDelta::Insert {
                insert: "Title".into(),
                attributes: None,
            },
            TextDelta::Insert {
                insert: "\n".into(),
                attributes: Some([("h".to_string(), LoroValue::I64(1))].into_iter().collect()),
            },
            TextDelta::Insert {
                insert: "Body".into(),
                attributes: None,
            },
        ]);
        let paragraphs: Vec<_> = view.paragraphs().collect();
        assert_eq!(paragraphs.len(), 2);
        assert_eq!(paragraphs[0].text(), "Title");
        assert_eq!(paragraphs[0].mark("h"), Some(&Value::Int(1)));
        assert_eq!(paragraphs[1].end_marks(), None, "unterminated");
        assert_eq!(view.len(), 10);
    }

    #[test]
    fn inserts_deletes_and_formats_match_a_rebuild() {
        check(|t| t.insert(3, "XY").unwrap());
        check(|t| t.insert(0, "start ").unwrap());
        check(|t| t.insert(t.len_unicode(), " end\n").unwrap());
        check(|t| t.insert(5, "a\nb\nc").unwrap());
        check(|t| t.delete(10, 1).unwrap()); // the first \n: paragraphs merge
        check(|t| t.delete(2, 15).unwrap());
        check(|t| t.delete(0, t.len_unicode()).unwrap());
        check(|t| t.mark(2..14, "b", true).unwrap());
        check(|t| {
            t.mark(0..5, "b", true).unwrap();
            t.unmark(1..3, "b").unwrap();
        });
        check(|t| t.mark(10..11, "h", 2).unwrap()); // a paragraph mark
        check(|t| {
            t.insert(t.len_unicode(), "\n").unwrap();
            t.delete(t.len_unicode() - 1, 1).unwrap();
        });
    }
}
