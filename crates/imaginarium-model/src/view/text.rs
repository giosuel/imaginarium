// SPDX-License-Identifier: AGPL-3.0-or-later

//! A text container as paragraphs of marked runs (schema-v1.md §6),
//! updated incrementally from Loro text deltas.
//!
//! Paragraphs are stored in **chunks** of up to [`MAX_CHUNK`] in a
//! persistent vector, so finding a position scans chunk totals, then one
//! chunk, and an edit replaces one chunk. Versions share every chunk and
//! paragraph they didn't change.
//!
//! - **Keystrokes** (an insert without `\n`, a delete inside one paragraph)
//!   edit the affected run of one paragraph directly.
//! - **Everything else** (paragraph splits and joins, formatting) flattens
//!   the paragraphs it touches to characters, edits them and splits the
//!   result at `\n` again.

use std::collections::BTreeMap;
use std::sync::Arc;

use loro::{LoroValue, TextDelta};

use crate::value::Value;

type Attributes<S> = std::collections::HashMap<String, LoroValue, S>;

/// Paragraphs per chunk when chunks are (re)built.
const CHUNK: usize = 64;
/// A chunk that grows beyond this is split.
const MAX_CHUNK: usize = 2 * CHUNK;

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

    fn new(text: String, chars: usize, marks: Marks) -> Self {
        Self {
            text: Arc::from(text),
            marks,
            chars,
        }
    }

    /// The byte offset of character `index`.
    fn byte(&self, index: usize) -> usize {
        self.text
            .char_indices()
            .nth(index)
            .map_or(self.text.len(), |(b, _)| b)
    }
}

/// Merges neighbouring runs with equal marks.
fn normalize(runs: Vec<Run>) -> Vec<Run> {
    let mut out: Vec<Run> = Vec::with_capacity(runs.len());
    for run in runs {
        if run.chars == 0 {
            continue;
        }
        match out.last_mut() {
            Some(last) if last.marks == run.marks => {
                let mut text = String::with_capacity(last.text.len() + run.text.len());
                text.push_str(&last.text);
                text.push_str(&run.text);
                *last = Run::new(text, last.chars + run.chars, run.marks);
            }
            _ => out.push(run),
        }
    }
    out
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

    /// This paragraph with `text` (no `\n`, `chars` long) inserted at
    /// character `offset` (≤ `self.chars`).
    fn with_insert(&self, offset: usize, text: &str, chars: usize, marks: &Marks) -> Paragraph {
        let mut runs = Vec::with_capacity(self.runs.len() + 2);
        let mut start = 0;
        let mut inserted = false;
        for run in &self.runs {
            let end = start + run.chars;
            if !inserted && offset <= end {
                let local = offset - start;
                let at = run.byte(local);
                if &run.marks == marks {
                    let mut joined = String::with_capacity(run.text.len() + text.len());
                    joined.push_str(&run.text[..at]);
                    joined.push_str(text);
                    joined.push_str(&run.text[at..]);
                    runs.push(Run::new(joined, run.chars + chars, run.marks.clone()));
                } else {
                    runs.push(Run::new(
                        run.text[..at].to_owned(),
                        local,
                        run.marks.clone(),
                    ));
                    runs.push(Run::new(text.to_owned(), chars, marks.clone()));
                    runs.push(Run::new(
                        run.text[at..].to_owned(),
                        run.chars - local,
                        run.marks.clone(),
                    ));
                }
                inserted = true;
            } else {
                runs.push(run.clone());
            }
            start = end;
        }
        if !inserted {
            runs.push(Run::new(text.to_owned(), chars, marks.clone()));
        }
        Paragraph {
            runs: normalize(runs),
            end: self.end.clone(),
            chars: self.chars + chars,
        }
    }

    /// This paragraph without characters `offset .. offset + len` (within
    /// the paragraph, not its `\n`).
    fn with_delete(&self, offset: usize, len: usize) -> Paragraph {
        let cut_end = offset + len;
        let mut runs = Vec::with_capacity(self.runs.len());
        let mut start = 0;
        for run in &self.runs {
            let end = start + run.chars;
            if end <= offset || start >= cut_end {
                runs.push(run.clone());
            } else {
                let keep_left = offset.saturating_sub(start);
                let keep_right = cut_end.min(end) - start;
                let mut text = String::new();
                text.push_str(&run.text[..run.byte(keep_left)]);
                text.push_str(&run.text[run.byte(keep_right)..]);
                runs.push(Run::new(
                    text,
                    keep_left + (run.chars - keep_right),
                    run.marks.clone(),
                ));
            }
            start = end;
        }
        Paragraph {
            runs: normalize(runs),
            end: self.end.clone(),
            chars: self.chars - len,
        }
    }
}

#[derive(Clone)]
struct Cell {
    ch: char,
    marks: Marks,
}

#[derive(Clone, Debug)]
struct Chunk {
    paragraphs: Vec<Arc<Paragraph>>,
    chars: usize,
}

impl Chunk {
    fn new(paragraphs: Vec<Arc<Paragraph>>) -> Self {
        let chars = paragraphs.iter().map(|p| p.span()).sum();
        Self { paragraphs, chars }
    }
}

/// Splits paragraphs into chunks.
fn chunk_up(paragraphs: Vec<Arc<Paragraph>>) -> Vec<Arc<Chunk>> {
    if paragraphs.is_empty() {
        return Vec::new();
    }
    if paragraphs.len() <= MAX_CHUNK {
        return vec![Arc::new(Chunk::new(paragraphs))];
    }
    paragraphs
        .chunks(CHUNK)
        .map(|c| Arc::new(Chunk::new(c.to_vec())))
        .collect()
}

/// Where a paragraph is.
#[derive(Clone, Copy)]
struct Location {
    chunk: usize,
    in_chunk: usize,
    /// Index among all paragraphs.
    index: usize,
    /// Character offset of the paragraph's start.
    start: usize,
}

/// The text of a node, as paragraphs.
#[derive(Clone, Debug, Default)]
pub struct TextView {
    chunks: imbl::Vector<Arc<Chunk>>,
    chars: usize,
    count: usize,
}

/// Texts are equal if their paragraphs are, however they are chunked.
impl PartialEq for TextView {
    fn eq(&self, other: &Self) -> bool {
        self.chars == other.chars
            && self.count == other.count
            && self.paragraphs().eq(other.paragraphs())
    }
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
        Self::from_paragraphs(builder.finish())
    }

    fn from_paragraphs(paragraphs: Vec<Arc<Paragraph>>) -> Self {
        let chars = paragraphs.iter().map(|p| p.span()).sum();
        let count = paragraphs.len();
        Self {
            chunks: chunk_up(paragraphs).into_iter().collect(),
            chars,
            count,
        }
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
        self.chunks
            .iter()
            .flat_map(|c| c.paragraphs.iter().map(|p| &**p))
    }

    /// Number of paragraphs.
    pub fn paragraph_count(&self) -> usize {
        self.count
    }

    /// The plain text, with `\n` between paragraphs.
    pub fn to_plain_string(&self) -> String {
        let mut out = String::with_capacity(self.chars);
        for paragraph in self.paragraphs() {
            for run in &paragraph.runs {
                out.push_str(&run.text);
            }
            if paragraph.end.is_some() {
                out.push('\n');
            }
        }
        out
    }

    /// Applies a delta of a text event. `false` if the delta doesn't fit
    /// this text (the view and the CRDT disagree): the caller then re-reads
    /// the text instead of trusting what was applied.
    #[must_use]
    pub(crate) fn apply_delta(&mut self, delta: &[TextDelta]) -> bool {
        let mut pos: usize = 0;
        for op in delta {
            let fits = match op {
                TextDelta::Retain { retain: len, .. } | TextDelta::Delete { delete: len } => {
                    pos.checked_add(*len).is_some_and(|end| end <= self.len())
                }
                TextDelta::Insert { .. } => pos <= self.len(),
            };
            if !fits {
                return false;
            }
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
                    let chars = insert.chars().count();
                    if !self.insert_fast(pos, insert, chars, &marks) {
                        self.edit(pos, 0, |cells, start| {
                            cells.splice(
                                start..start,
                                insert.chars().map(|ch| Cell {
                                    ch,
                                    marks: marks.clone(),
                                }),
                            );
                        });
                    }
                    pos += chars;
                }
                TextDelta::Delete { delete } => {
                    let len = *delete;
                    if !self.delete_fast(pos, len) {
                        self.edit(pos, len, |cells, start| {
                            cells.drain(start..start + len);
                        });
                    }
                }
            }
        }
        true
    }

    /// The paragraph containing character `pos`, or `None` at the end.
    fn locate(&self, pos: usize) -> Option<Location> {
        let (mut start, mut index) = (0, 0);
        for (chunk_index, chunk) in self.chunks.iter().enumerate() {
            if pos < start + chunk.chars {
                for (in_chunk, paragraph) in chunk.paragraphs.iter().enumerate() {
                    let span = paragraph.span();
                    if pos < start + span {
                        return Some(Location {
                            chunk: chunk_index,
                            in_chunk,
                            index,
                            start,
                        });
                    }
                    start += span;
                    index += 1;
                }
                return None; // unreachable while chunk totals are right
            }
            start += chunk.chars;
            index += chunk.paragraphs.len();
        }
        None
    }

    /// The last paragraph, if it has no `\n` (text typed at the very end
    /// belongs to it).
    fn open_last(&self) -> Option<Location> {
        let chunk = self.chunks.len().checked_sub(1)?;
        let paragraphs = &self.chunks[chunk].paragraphs;
        let last = paragraphs.last()?;
        last.end.is_none().then(|| Location {
            chunk,
            in_chunk: paragraphs.len() - 1,
            index: self.count - 1,
            start: self.chars - last.chars,
        })
    }

    /// Replaces one paragraph in place.
    fn replace_one(&mut self, at: Location, paragraph: Paragraph) {
        if let Some(slot) = self.chunks.get_mut(at.chunk) {
            let chunk = Arc::make_mut(slot);
            let old = chunk.paragraphs[at.in_chunk].span();
            let new = paragraph.span();
            chunk.paragraphs[at.in_chunk] = Arc::new(paragraph);
            chunk.chars = chunk.chars - old + new;
            self.chars = self.chars - old + new;
        }
    }

    /// An insert without `\n` into one paragraph.
    fn insert_fast(&mut self, pos: usize, text: &str, chars: usize, marks: &Marks) -> bool {
        if text.is_empty() || text.contains('\n') {
            return false;
        }
        let Some(at) = self.locate(pos).or_else(|| self.open_last()) else {
            return false;
        };
        let paragraph = &self.chunks[at.chunk].paragraphs[at.in_chunk];
        let offset = pos - at.start;
        if offset > paragraph.chars {
            return false;
        }
        let edited = paragraph.with_insert(offset, text, chars, marks);
        self.replace_one(at, edited);
        true
    }

    /// A delete inside one paragraph (not its `\n`).
    fn delete_fast(&mut self, pos: usize, len: usize) -> bool {
        let Some(at) = self.locate(pos) else {
            return false;
        };
        let paragraph = &self.chunks[at.chunk].paragraphs[at.in_chunk];
        let offset = pos - at.start;
        // Emptying an unterminated last paragraph removes it: general path.
        if len == 0
            || offset + len > paragraph.chars
            || (paragraph.end.is_none() && len == paragraph.chars)
        {
            return false;
        }
        let edited = paragraph.with_delete(offset, len);
        self.replace_one(at, edited);
        true
    }

    /// The general edit of characters `pos .. pos + len` (an empty range
    /// for inserts): flattens the paragraphs it overlaps, lets `edit`
    /// change them, and splits the result into paragraphs again.
    fn edit(&mut self, pos: usize, len: usize, edit: impl FnOnce(&mut Vec<Cell>, usize)) {
        let (mut first, mut first_start) = match self.locate(pos) {
            Some(at) => (at.index, at.start),
            None => (self.count, self.chars),
        };
        if let (true, Some(open)) = (first == self.count, self.open_last()) {
            first = open.index;
            first_start = open.start;
        }
        let mut end = if len == 0 {
            (first + 1).min(self.count)
        } else {
            self.locate(pos + len - 1)
                .map_or(self.count, |at| at.index + 1)
        };
        let mut cells = Vec::new();
        for paragraph in self.range(first, end) {
            paragraph.cells(&mut cells);
        }
        edit(&mut cells, pos - first_start);
        // A deleted `\n` joins the edited paragraphs with the next one.
        while end < self.count && cells.last().is_some_and(|c| c.ch != '\n') {
            if let Some(next) = self.range(end, end + 1).next() {
                next.cells(&mut cells);
            }
            end += 1;
        }
        let mut builder = Builder::default();
        for cell in &cells {
            builder.push_char(cell.ch, &cell.marks);
        }
        self.splice(first, end, builder.finish());
    }

    /// Paragraphs `first .. end`.
    fn range(&self, first: usize, end: usize) -> impl Iterator<Item = &Arc<Paragraph>> {
        self.chunks
            .iter()
            .flat_map(|c| c.paragraphs.iter())
            .skip(first)
            .take(end.saturating_sub(first))
    }

    /// The chunk holding paragraph `index`, and the index of its first
    /// paragraph. The last chunk for `index == count`.
    fn chunk_of(&self, index: usize) -> (usize, usize) {
        let mut first = 0;
        for (chunk_index, chunk) in self.chunks.iter().enumerate() {
            let len = chunk.paragraphs.len();
            if index < first + len || chunk_index + 1 == self.chunks.len() {
                return (chunk_index, first);
            }
            first += len;
        }
        (0, 0)
    }

    /// Replaces paragraphs `first .. end` with `new`.
    fn splice(&mut self, first: usize, end: usize, new: Vec<Arc<Paragraph>>) {
        if self.chunks.is_empty() {
            *self = Self::from_paragraphs(new);
            return;
        }
        let (first_chunk, first_base) = self.chunk_of(first);
        let (last_chunk, last_base) = if end > first {
            self.chunk_of(end - 1)
        } else {
            (first_chunk, first_base)
        };
        let suffix_from = if end > first {
            end - last_base
        } else {
            first - first_base
        };
        let new_count = new.len();
        let mut combined: Vec<Arc<Paragraph>> =
            self.chunks[first_chunk].paragraphs[..first - first_base].to_vec();
        combined.extend(new);
        combined.extend_from_slice(&self.chunks[last_chunk].paragraphs[suffix_from..]);
        let removed_chars: usize = (first_chunk..=last_chunk)
            .map(|c| self.chunks[c].chars)
            .sum();
        let rebuilt = chunk_up(combined);
        let added_chars: usize = rebuilt.iter().map(|c| c.chars).sum();
        let tail = self.chunks.split_off(last_chunk + 1);
        self.chunks.truncate(first_chunk);
        self.chunks.extend(rebuilt);
        self.chunks.append(tail);
        self.chars = self.chars - removed_chars + added_chars;
        self.count = self.count - (end - first) + new_count;
    }
}

/// Builds paragraphs from text pieces, merging neighbouring equal marks.
#[derive(Default)]
struct Builder {
    paragraphs: Vec<Arc<Paragraph>>,
    runs: Vec<Run>,
    text: String,
    marks: Marks,
    run_chars: usize,
    paragraph_chars: usize,
}

impl Builder {
    fn push_char(&mut self, ch: char, marks: &Marks) {
        if ch == '\n' {
            self.end_paragraph(Some(marks.clone()));
            return;
        }
        self.start_run(marks);
        self.text.push(ch);
        self.run_chars += 1;
    }

    /// Whole segments between newlines at a time: much faster than
    /// characters for large texts.
    fn push_str(&mut self, text: &str, marks: &Marks) {
        let mut rest = text;
        while !rest.is_empty() {
            match rest.find('\n') {
                Some(at) => {
                    self.push_segment(&rest[..at], marks);
                    self.end_paragraph(Some(marks.clone()));
                    rest = &rest[at + 1..];
                }
                None => {
                    self.push_segment(rest, marks);
                    rest = "";
                }
            }
        }
    }

    fn push_segment(&mut self, segment: &str, marks: &Marks) {
        if segment.is_empty() {
            return;
        }
        self.start_run(marks);
        self.text.push_str(segment);
        self.run_chars += segment.chars().count();
    }

    fn start_run(&mut self, marks: &Marks) {
        if self.run_chars > 0 && &self.marks != marks {
            self.end_run();
        }
        if self.run_chars == 0 {
            self.marks = marks.clone();
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
        self.paragraphs.push(Arc::new(Paragraph {
            runs: std::mem::take(&mut self.runs),
            chars: self.paragraph_chars,
            end,
        }));
        self.paragraph_chars = 0;
    }

    fn finish(mut self) -> Vec<Arc<Paragraph>> {
        self.end_paragraph(None);
        self.paragraphs
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use loro::{ExpandType, LoroDoc, StyleConfig, StyleConfigMap};

    #[test]
    fn deltas_that_do_not_fit_are_refused_not_panicked_on() {
        let doc = LoroDoc::new();
        let text = doc.get_text("t");
        text.insert(0, "abc\n").unwrap();
        let mut view = TextView::from_delta(&text.to_delta());
        for delta in [
            vec![TextDelta::Delete { delete: 10 }],
            vec![
                TextDelta::Retain {
                    retain: 9,
                    attributes: None,
                },
                TextDelta::Insert {
                    insert: "x".into(),
                    attributes: None,
                },
            ],
            vec![TextDelta::Retain {
                retain: usize::MAX,
                attributes: None,
            }],
        ] {
            assert!(!view.apply_delta(&delta));
        }
    }

    /// Applies `edits` to a Loro text holding `initial` and checks that the
    /// incrementally updated view equals a view built from scratch.
    fn check_with(initial: &str, edits: impl FnOnce(&loro::LoroText)) {
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
        text.insert(0, initial).unwrap();
        doc.commit();
        let mut view = TextView::from_delta(&text.to_delta());
        deltas.lock().unwrap().clear();
        edits(&text);
        doc.commit();
        for delta in deltas.lock().unwrap().iter() {
            assert!(view.apply_delta(delta));
        }
        assert_eq!(view, TextView::from_delta(&text.to_delta()));
        assert_eq!(view.to_plain_string(), text.to_string());
        assert_eq!(view.len(), text.len_unicode());
        assert_eq!(view.paragraph_count(), view.paragraphs().count());
    }

    fn check(edits: impl FnOnce(&loro::LoroText)) {
        check_with("first line\nsecond\nthird", edits);
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
        check(|t| t.insert(t.len_unicode(), " end").unwrap());
        check(|t| t.insert(5, "a\nb\nc").unwrap());
        check(|t| t.insert(10, "é😀").unwrap()); // just before the first \n
        check(|t| t.delete(10, 1).unwrap()); // the first \n: paragraphs merge
        check(|t| t.delete(2, 15).unwrap());
        check(|t| t.delete(2, 3).unwrap());
        check(|t| t.delete(0, t.len_unicode()).unwrap());
        check(|t| t.mark(2..14, "b", true).unwrap());
        check(|t| {
            t.mark(0..5, "b", true).unwrap();
            t.unmark(1..3, "b").unwrap();
        });
        check(|t| {
            t.mark(0..5, "b", true).unwrap();
            t.insert(5, "!").unwrap(); // expands the bold run
            t.insert(2, "-").unwrap();
            t.delete(1, 3).unwrap();
        });
        check(|t| t.mark(10..11, "h", 2).unwrap()); // a paragraph mark
        check(|t| {
            t.insert(t.len_unicode(), "\n").unwrap();
            t.delete(t.len_unicode() - 1, 1).unwrap();
        });
        check_with("", |t| t.insert(0, "x").unwrap());
        check_with("x", |t| t.delete(0, 1).unwrap());
    }

    #[test]
    fn many_paragraphs_are_chunked_and_still_match() {
        let many: String = (0..300).map(|i| format!("paragraph {i}\n")).collect();
        let view = TextView::from_delta(&[TextDelta::Insert {
            insert: many.clone(),
            attributes: None,
        }]);
        assert!(view.chunks.len() > 1, "300 paragraphs need several chunks");
        check_with(&many, |t| t.insert(2000, "typed").unwrap());
        check_with(&many, |t| t.insert(2000, "split\nhere").unwrap());
        check_with(&many, |t| t.delete(100, 3000).unwrap()); // across chunks
        check_with(&many, |t| t.mark(50..2500, "b", true).unwrap());
        check_with(&many, |t| {
            for i in 0..200 {
                t.insert(10 + i * 3, "\n").unwrap(); // grows one chunk past MAX_CHUNK
            }
        });
    }
}
