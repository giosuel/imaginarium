// SPDX-License-Identifier: AGPL-3.0-or-later

//! Editable plain text: Parley's `PlainEditor` behind an API of our own,
//! with painting, IME composition and what screen readers need.

use imaginarium_render::Scene;
use imaginarium_render::kurbo::{Point, Rect, Size};
use imaginarium_render::peniko::Color;
use parley::{BoundingBox, PlainEditor, PlainEditorDriver};

use crate::fonts::{Brush, Fonts};
use crate::layout::{TextStyle, paint_layout};

/// Where a caret movement goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Motion {
    /// One character left (visually: bidirectional text moves by what is
    /// on screen).
    Left,
    /// One character right.
    Right,
    /// To the previous word start.
    WordLeft,
    /// To the next word end.
    WordRight,
    /// One line up.
    Up,
    /// One line down.
    Down,
    /// To the start of the visual line.
    LineStart,
    /// To the end of the visual line.
    LineEnd,
    /// To the start of the text.
    TextStart,
    /// To the end of the text.
    TextEnd,
}

/// An editing operation, independent of the keys that trigger it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edit {
    /// Moves the caret, collapsing the selection.
    Move(Motion),
    /// Moves the selection's moving end (Shift + motion).
    Extend(Motion),
    /// Selects everything.
    SelectAll,
    /// Deletes the selection, or the character before the caret.
    Backspace,
    /// Deletes the selection, or the word before the caret.
    BackspaceWord,
    /// Deletes the selection, or the character after the caret.
    Delete,
    /// Deletes the selection, or the word after the caret.
    DeleteWord,
    /// Breaks the line (multi-line editors only).
    Newline,
}

/// One visual line, for the accessibility tree: screen readers read and
/// navigate text by line and character.
#[derive(Clone, Debug, PartialEq)]
pub struct AccessLine {
    /// The line's text (a range of [`Editor::text`]).
    pub text: String,
    /// Where the line starts in the editor's text, in bytes.
    pub start: usize,
    /// The line's box, relative to the editor's origin.
    pub bounds: Rect,
    /// The UTF-8 length of each character, in order.
    pub character_lengths: Vec<u8>,
}

/// Plain text being edited: one style for all of it, a selection, and an
/// input method's composition. Coordinates are logical pixels relative to
/// the editor's top left corner.
pub struct Editor {
    editor: PlainEditor<Brush>,
    style: TextStyle,
    multiline: bool,
}

impl Editor {
    /// An editor showing `text`. A single-line editor never wraps and turns
    /// pasted line breaks into spaces.
    pub fn new(text: &str, style: TextStyle, multiline: bool) -> Self {
        let mut editor = PlainEditor::new(style.size);
        let styles = editor.edit_styles();
        for property in style.properties() {
            styles.insert(property);
        }
        editor.set_alignment(style.alignment());
        editor.set_text(&Self::clean(text, multiline));
        Self {
            editor,
            style,
            multiline,
        }
    }

    fn clean(text: &str, multiline: bool) -> String {
        if multiline {
            text.replace("\r\n", "\n")
        } else {
            text.replace(['\r', '\n'], " ")
        }
    }

    fn driver<'a>(&'a mut self, fonts: &'a mut Fonts) -> PlainEditorDriver<'a, Brush> {
        self.editor.driver(&mut fonts.font_cx, &mut fonts.layout_cx)
    }

    /// The text, without any composition in progress.
    pub fn text(&self) -> String {
        self.editor.text().to_string()
    }

    /// Replaces the whole text (the caret stays where it is if it still
    /// fits).
    pub fn set_text(&mut self, text: &str) {
        self.editor.set_text(&Self::clean(text, self.multiline));
    }

    /// The style.
    pub fn style(&self) -> &TextStyle {
        &self.style
    }

    /// Whether the editor takes line breaks.
    pub fn is_multiline(&self) -> bool {
        self.multiline
    }

    /// Wraps lines at `width` logical pixels (`None`: no wrapping).
    pub fn set_width(&mut self, width: Option<f32>) {
        self.editor
            .set_width(if self.multiline { width } else { None });
    }

    /// The selected text, if the selection isn't empty.
    pub fn selected_text(&self) -> Option<&str> {
        self.editor.selected_text()
    }

    /// The selection as byte offsets: (anchor, focus). Equal for a caret.
    pub fn selection(&self) -> (usize, usize) {
        let selection = self.editor.raw_selection();
        (selection.anchor().index(), selection.focus().index())
    }

    /// Selects from byte `anchor` to byte `focus` (a screen reader moving
    /// the caret). Offsets that aren't character boundaries are ignored.
    pub fn select_bytes(&mut self, fonts: &mut Fonts, anchor: usize, focus: usize) {
        self.driver(fonts).select_byte_range(anchor, focus);
    }

    /// Whether an input method is composing.
    pub fn is_composing(&self) -> bool {
        self.editor.is_composing()
    }

    /// Applies an editing operation.
    pub fn edit(&mut self, fonts: &mut Fonts, edit: Edit) {
        let multiline = self.multiline;
        let mut driver = self.driver(fonts);
        match edit {
            Edit::Move(motion) => match motion {
                Motion::Left => driver.move_left(),
                Motion::Right => driver.move_right(),
                Motion::WordLeft => driver.move_word_left(),
                Motion::WordRight => driver.move_word_right(),
                Motion::Up if multiline => driver.move_up(),
                Motion::Down if multiline => driver.move_down(),
                Motion::Up | Motion::LineStart => driver.move_to_line_start(),
                Motion::Down | Motion::LineEnd => driver.move_to_line_end(),
                Motion::TextStart => driver.move_to_text_start(),
                Motion::TextEnd => driver.move_to_text_end(),
            },
            Edit::Extend(motion) => match motion {
                Motion::Left => driver.select_left(),
                Motion::Right => driver.select_right(),
                Motion::WordLeft => driver.select_word_left(),
                Motion::WordRight => driver.select_word_right(),
                Motion::Up if multiline => driver.select_up(),
                Motion::Down if multiline => driver.select_down(),
                Motion::Up | Motion::LineStart => driver.select_to_line_start(),
                Motion::Down | Motion::LineEnd => driver.select_to_line_end(),
                Motion::TextStart => driver.select_to_text_start(),
                Motion::TextEnd => driver.select_to_text_end(),
            },
            Edit::SelectAll => driver.select_all(),
            Edit::Backspace => driver.backdelete(),
            Edit::BackspaceWord => driver.backdelete_word(),
            Edit::Delete => driver.delete(),
            Edit::DeleteWord => driver.delete_word(),
            Edit::Newline if multiline => driver.insert_or_replace_selection("\n"),
            Edit::Newline => {}
        }
    }

    /// Types or pastes `text` over the selection.
    pub fn insert(&mut self, fonts: &mut Fonts, text: &str) {
        let text = Self::clean(text, self.multiline);
        if !text.is_empty() {
            self.driver(fonts).insert_or_replace_selection(&text);
        }
    }

    /// Shows an input method's composition (`text` is not part of the
    /// document yet). `cursor` is the IME's caret or highlight, a byte
    /// range in `text`. An empty `text` ends the composition.
    pub fn compose(&mut self, fonts: &mut Fonts, text: &str, cursor: Option<(usize, usize)>) {
        let mut driver = self.driver(fonts);
        if text.is_empty() {
            driver.clear_compose();
        } else {
            let cursor = cursor.filter(|(start, end)| start <= end && *end <= text.len());
            driver.set_compose(text, cursor);
        }
    }

    /// Drops a composition in progress (the input method was turned off).
    pub fn cancel_compose(&mut self, fonts: &mut Fonts) {
        self.driver(fonts).clear_compose();
    }

    /// A press at `point`: 1 click places the caret, 2 select a word, 3 a
    /// line. `extend` (Shift) moves the selection's end instead.
    pub fn click(&mut self, fonts: &mut Fonts, point: Point, clicks: u8, extend: bool) {
        let (x, y) = (point.x as f32, point.y as f32);
        let mut driver = self.driver(fonts);
        match (clicks, extend) {
            (_, true) => driver.shift_click_extension(x, y),
            (2, _) => driver.select_word_at_point(x, y),
            (3.., _) => driver.select_line_at_point(x, y),
            _ => driver.move_to_point(x, y),
        }
    }

    /// The pointer moved to `point` with the button held: extends the
    /// selection (by words or lines after a double or triple click).
    pub fn drag(&mut self, fonts: &mut Fonts, point: Point) {
        self.driver(fonts)
            .extend_selection_to_point(point.x as f32, point.y as f32);
    }

    /// The laid-out size. A single-line editor is one line high even when
    /// empty.
    pub fn size(&mut self, fonts: &mut Fonts) -> Size {
        let layout = self.editor.layout(&mut fonts.font_cx, &mut fonts.layout_cx);
        let line = f64::from(self.style.size * self.style.line_height);
        Size::new(
            f64::from(layout.width()),
            f64::from(layout.height()).max(line),
        )
    }

    /// Where the caret or the composition is, for the input method's
    /// candidate window.
    pub fn ime_area(&mut self, fonts: &mut Fonts) -> Rect {
        self.editor
            .refresh_layout(&mut fonts.font_cx, &mut fonts.layout_cx);
        rect(self.editor.ime_cursor_area())
    }

    /// The caret's box, if it is shown (an input method can hide it).
    pub fn caret(&mut self, fonts: &mut Fonts) -> Option<Rect> {
        self.editor
            .refresh_layout(&mut fonts.font_cx, &mut fonts.layout_cx);
        self.editor.cursor_geometry(1.5).map(rect)
    }

    /// Draws the selection (when `focused`), the text, the composition's
    /// underline and the caret (when `focused && caret_on`) at `origin`.
    pub fn paint(
        &mut self,
        fonts: &mut Fonts,
        scene: &mut Scene,
        origin: Point,
        focused: bool,
        caret_on: bool,
        accent: Color,
    ) {
        self.editor
            .refresh_layout(&mut fonts.font_cx, &mut fonts.layout_cx);
        let offset = origin.to_vec2();
        if focused {
            let selection = accent.with_alpha(0.25);
            self.editor.selection_geometry_with(|bounds, _line| {
                scene.fill(&(rect(bounds) + offset), selection);
            });
        }
        if let Some(layout) = self.editor.try_layout() {
            paint_layout(layout, scene, origin, true);
        }
        // The composition is underlined, as platforms do.
        if let Some(range) = self.editor.raw_compose().clone()
            && let Some(layout) = self.editor.try_layout()
        {
            let selection = parley::Selection::new(
                parley::Cursor::from_byte_index(layout, range.start, parley::Affinity::Downstream),
                parley::Cursor::from_byte_index(layout, range.end, parley::Affinity::Upstream),
            );
            selection.geometry_with(layout, |bounds, _| {
                let bounds = rect(bounds) + offset;
                scene.fill(
                    &Rect::new(bounds.x0, bounds.y1 - 2.0, bounds.x1, bounds.y1 - 0.5),
                    self.style.color,
                );
            });
        }
        if focused
            && caret_on
            && let Some(caret) = self.editor.cursor_geometry(1.5)
        {
            scene.fill(&(rect(caret) + offset), accent);
        }
    }

    /// The visual lines, for screen readers.
    pub fn access_lines(&mut self, fonts: &mut Fonts) -> Vec<AccessLine> {
        self.editor
            .refresh_layout(&mut fonts.font_cx, &mut fonts.layout_cx);
        let Some(layout) = self.editor.try_layout() else {
            return Vec::new();
        };
        let text = self.editor.raw_text();
        let width = f64::from(layout.full_width().max(layout.width()));
        let mut lines = Vec::new();
        for line in layout.lines() {
            let range = line.text_range();
            let metrics = line.metrics();
            let line_text = text.get(range.clone()).unwrap_or_default();
            lines.push(AccessLine {
                text: line_text.to_owned(),
                start: range.start,
                bounds: Rect::new(
                    0.0,
                    f64::from(metrics.block_min_coord),
                    width,
                    f64::from(metrics.block_max_coord),
                ),
                character_lengths: line_text.chars().map(|c| c.len_utf8() as u8).collect(),
            });
        }
        if lines.is_empty() {
            lines.push(AccessLine {
                text: String::new(),
                start: 0,
                bounds: Rect::new(
                    0.0,
                    0.0,
                    0.0,
                    f64::from(self.style.size * self.style.line_height),
                ),
                character_lengths: Vec::new(),
            });
        }
        lines
    }
}

fn rect(bounds: BoundingBox) -> Rect {
    Rect::new(bounds.x0, bounds.y0, bounds.x1, bounds.y1)
}
