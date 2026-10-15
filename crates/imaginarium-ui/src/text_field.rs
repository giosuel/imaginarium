// SPDX-License-Identifier: AGPL-3.0-or-later

//! A text field: an [`Editor`] with focus, keys, the input method, a
//! blinking caret and its accessibility node.

use std::time::{Duration, Instant};

use accesskit::{Action, ActionData, ActionRequest, Node, NodeId, Role, TextPosition};
use imaginarium_render::Scene;
use imaginarium_render::kurbo::{Point, Rect, Size};
use imaginarium_render::peniko::Color;
use imaginarium_text::{Edit, Editor, Fonts, Motion, TextStyle};

use crate::access::{AccessTree, rect};
use crate::clipboard::Clipboard;
use crate::window::{Ime, Input, Key, Modifiers};

/// The caret is on, then off, for this long.
const BLINK: Duration = Duration::from_millis(530);
/// The caret stops blinking (and stays on) this long after the last edit,
/// as GTK does, so an idle window with a focused field draws nothing.
const BLINK_FOR: Duration = Duration::from_secs(10);

/// What a field did with an input.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FieldResponse {
    /// The input was the field's (don't pass it on).
    pub handled: bool,
    /// The field needs a frame.
    pub repaint: bool,
    /// The text changed.
    pub changed: bool,
    /// Enter in a single-line field.
    pub submitted: bool,
    /// The field gave up the focus (Escape, or a press elsewhere).
    pub blurred: bool,
    /// A screen reader acted on the field: it should get the focus (from
    /// its owner, which knows who has it).
    pub wants_focus: bool,
}

/// A single- or multi-line plain text field. It doesn't know where it is:
/// the owner passes its bounds to [`TextField::input`] and
/// [`TextField::paint`] (window coordinates, logical pixels).
pub struct TextField {
    editor: Editor,
    focused: bool,
    /// When the caret last restarted blinking (it is on right after).
    blink_epoch: Instant,
    dragging: bool,
    padding: f64,
}

impl TextField {
    /// A field showing `text`.
    pub fn new(text: &str, style: TextStyle, multiline: bool) -> Self {
        Self {
            editor: Editor::new(text, style, multiline),
            focused: false,
            blink_epoch: Instant::now(),
            dragging: false,
            padding: 0.0,
        }
    }

    /// The same field with `padding` logical pixels between its bounds and
    /// the text.
    pub fn with_padding(mut self, padding: f64) -> Self {
        self.padding = padding;
        self
    }

    /// The text.
    pub fn text(&self) -> String {
        self.editor.text()
    }

    /// The editor inside, for programmatic edits.
    pub fn editor(&mut self) -> &mut Editor {
        &mut self.editor
    }

    /// Whether the field has the keyboard focus.
    pub fn is_focused(&self) -> bool {
        self.focused
    }

    /// Takes the keyboard focus (the caret shows, the input method can
    /// start).
    pub fn focus(&mut self) {
        self.focused = true;
        self.blink_epoch = Instant::now();
    }

    /// Gives up the focus, dropping a composition in progress.
    pub fn blur(&mut self, fonts: &mut Fonts) {
        if self.focused {
            self.editor.cancel_compose(fonts);
        }
        self.focused = false;
        self.dragging = false;
    }

    fn text_origin(&self, bounds: Rect) -> Point {
        Point::new(bounds.x0 + self.padding, bounds.y0 + self.padding)
    }

    /// Lays the text out for `bounds` (multi-line fields wrap at its
    /// width).
    fn fit(&mut self, bounds: Rect) {
        let width = (bounds.width() - 2.0 * self.padding).max(0.0) as f32;
        self.editor.set_width(Some(width));
    }

    /// The text's size, plus padding: how big the field wants to be.
    pub fn preferred_size(&mut self, fonts: &mut Fonts, width: f64) -> Size {
        self.fit(Rect::new(0.0, 0.0, width, 0.0));
        let size = self.editor.size(fonts);
        Size::new(width, size.height + 2.0 * self.padding)
    }

    /// Like [`TextField::input`], with copy, cut and paste (Ctrl/⌘ + C, X,
    /// V; Shift+Delete and Shift+Insert as well).
    pub fn input_with_clipboard(
        &mut self,
        fonts: &mut Fonts,
        input: &Input,
        bounds: Rect,
        clipboard: &mut Clipboard,
    ) -> FieldResponse {
        if let Input::Key {
            key,
            pressed: true,
            modifiers,
            ..
        } = input
            && self.focused
        {
            let command = |c: &str| {
                modifiers.command()
                    && matches!(key, Key::Character(typed) if typed.eq_ignore_ascii_case(c))
            };
            let copy = command("c") || (modifiers.ctrl && *key == Key::Other("Insert".into()));
            let cut = command("x") || (modifiers.shift && *key == Key::Delete);
            let paste = command("v") || (modifiers.shift && *key == Key::Other("Insert".into()));
            if copy || cut {
                let selected = self.editor.selected_text().map(str::to_owned);
                if let Some(text) = &selected {
                    clipboard.set_text(text);
                }
                let before = self.editor.text();
                // Without a selection, Delete would take the next
                // character: there is nothing to cut.
                if cut && selected.is_some() {
                    self.editor.edit(fonts, Edit::Delete);
                }
                self.blink_epoch = Instant::now();
                return FieldResponse {
                    handled: true,
                    repaint: cut,
                    changed: self.editor.text() != before,
                    ..FieldResponse::default()
                };
            }
            if paste {
                let before = self.editor.text();
                if let Some(text) = clipboard.text() {
                    self.editor.insert(fonts, &text);
                }
                self.blink_epoch = Instant::now();
                return FieldResponse {
                    handled: true,
                    repaint: true,
                    changed: self.editor.text() != before,
                    ..FieldResponse::default()
                };
            }
        }
        self.input(fonts, input, bounds)
    }

    /// Handles an input for a field at `bounds`.
    pub fn input(&mut self, fonts: &mut Fonts, input: &Input, bounds: Rect) -> FieldResponse {
        self.fit(bounds);
        let origin = self.text_origin(bounds);
        let before = self.editor.text();
        let mut response = FieldResponse::default();
        match input {
            Input::PointerButton {
                primary: true,
                pressed: true,
                position,
                clicks,
                modifiers,
            } => {
                if bounds.contains(*position) {
                    if !self.focused {
                        self.focus();
                    }
                    self.editor.click(
                        fonts,
                        *position - origin.to_vec2(),
                        *clicks,
                        modifiers.shift,
                    );
                    self.dragging = true;
                    response.handled = true;
                    response.repaint = true;
                } else if self.focused {
                    self.blur(fonts);
                    response.blurred = true;
                    response.repaint = true;
                }
            }
            Input::PointerButton {
                primary: true,
                pressed: false,
                ..
            } if self.dragging => {
                self.dragging = false;
                response.handled = true;
            }
            Input::PointerMoved(position) if self.dragging => {
                self.editor.drag(fonts, *position - origin.to_vec2());
                response.handled = true;
                response.repaint = true;
            }
            Input::Key {
                key,
                text,
                pressed: true,
                modifiers,
                ..
            } if self.focused => {
                response.handled = true;
                response.repaint = true;
                match key_edit(key, *modifiers, self.editor.is_multiline()) {
                    Some(KeyAction::Edit(edit)) => self.editor.edit(fonts, edit),
                    Some(KeyAction::Submit) => response.submitted = true,
                    Some(KeyAction::Blur) => {
                        self.blur(fonts);
                        response.blurred = true;
                    }
                    None => match text {
                        Some(text) if !self.editor.is_composing() => {
                            self.editor.insert(fonts, text);
                        }
                        _ => {
                            response.handled = false;
                            response.repaint = false;
                        }
                    },
                }
            }
            Input::Ime(ime) if self.focused => {
                match ime {
                    Ime::Preedit { text, cursor } => self.editor.compose(fonts, text, *cursor),
                    Ime::Commit(text) => {
                        self.editor.compose(fonts, "", None);
                        self.editor.insert(fonts, text);
                    }
                    Ime::Disabled => self.editor.cancel_compose(fonts),
                    Ime::Enabled => {}
                }
                response.handled = true;
                response.repaint = true;
            }
            Input::WindowFocus(false) if self.focused => {
                self.editor.cancel_compose(fonts);
                self.dragging = false;
                response.repaint = true;
            }
            _ => {}
        }
        if response.repaint {
            self.blink_epoch = Instant::now();
        }
        response.changed = self.editor.text() != before;
        response
    }

    /// Handles a screen reader's request for this field (node `id`). It
    /// doesn't take the focus itself: [`FieldResponse::wants_focus`] asks
    /// the owner, so focus stays in one place.
    pub fn access_action(
        &mut self,
        fonts: &mut Fonts,
        request: &ActionRequest,
        id: u64,
    ) -> FieldResponse {
        let before = self.editor.text();
        let handled = match (&request.action, &request.data) {
            (Action::Focus | Action::Click, _) => true,
            (Action::SetTextSelection, Some(ActionData::SetTextSelection(selection))) => {
                let lines = self.editor.access_lines(fonts);
                let offset = |position: &TextPosition| {
                    let line = usize::try_from(position.node.0.checked_sub(id + 1)?).ok()?;
                    let line = lines.get(line)?;
                    let bytes: usize = line
                        .character_lengths
                        .iter()
                        .take(position.character_index)
                        .map(|&n| usize::from(n))
                        .sum();
                    Some(line.start + bytes)
                };
                if let (Some(anchor), Some(focus)) =
                    (offset(&selection.anchor), offset(&selection.focus))
                {
                    self.editor.select_bytes(fonts, anchor, focus);
                    true
                } else {
                    false
                }
            }
            (Action::ReplaceSelectedText, Some(ActionData::Value(text))) => {
                self.editor.insert(fonts, text);
                true
            }
            _ => false,
        };
        FieldResponse {
            handled,
            repaint: handled,
            changed: self.editor.text() != before,
            wants_focus: handled,
            ..FieldResponse::default()
        }
    }

    /// When the caret next turns on or off, while it blinks.
    pub fn wake_at(&self) -> Option<Instant> {
        if !self.focused {
            return None;
        }
        let elapsed = self.blink_epoch.elapsed();
        if elapsed >= BLINK_FOR {
            return None;
        }
        let phases = elapsed.as_nanos() / BLINK.as_nanos() + 1;
        let next = BLINK.saturating_mul(u32::try_from(phases).unwrap_or(u32::MAX));
        Some(self.blink_epoch + next.min(BLINK_FOR))
    }

    fn caret_on(&self) -> bool {
        let elapsed = self.blink_epoch.elapsed();
        elapsed >= BLINK_FOR || (elapsed.as_nanos() / BLINK.as_nanos()).is_multiple_of(2)
    }

    /// Where the input method's candidate window should go, while focused.
    pub fn ime_area(&mut self, fonts: &mut Fonts, bounds: Rect) -> Option<Rect> {
        if !self.focused {
            return None;
        }
        self.fit(bounds);
        Some(self.editor.ime_area(fonts) + self.text_origin(bounds).to_vec2())
    }

    /// Draws the text, the selection and the caret (no background: that is
    /// the owner's). `accent` colors the caret and selection.
    pub fn paint(&mut self, fonts: &mut Fonts, scene: &mut Scene, bounds: Rect, accent: Color) {
        self.fit(bounds);
        let origin = self.text_origin(bounds);
        scene.push_clip(&bounds);
        let caret_on = self.caret_on();
        self.editor
            .paint(fonts, scene, origin, self.focused, caret_on, accent);
        scene.pop();
    }

    /// Adds the field to the accessibility tree as node `id`, with one text
    /// run per line as nodes `id + 1`, `id + 2`, …
    pub fn access(
        &mut self,
        fonts: &mut Fonts,
        tree: &mut AccessTree,
        id: u64,
        bounds: Rect,
        label: &str,
    ) -> NodeId {
        self.fit(bounds);
        let origin = self.text_origin(bounds).to_vec2();
        let role = if self.editor.is_multiline() {
            Role::MultilineTextInput
        } else {
            Role::TextInput
        };
        let mut node = Node::new(role);
        node.set_label(label);
        node.set_bounds(rect(bounds));
        // What is shown, as the text runs are: with a composition in it.
        node.set_value(self.editor.shown_text());
        node.add_action(Action::Focus);
        node.add_action(Action::SetTextSelection);
        node.add_action(Action::ReplaceSelectedText);
        let lines = self.editor.access_lines(fonts);
        let (anchor, focus) = self.editor.selection();
        let position = |byte: usize| {
            // The line holding `byte` (the last one for the end of text).
            let index = lines
                .iter()
                .rposition(|line| line.start <= byte)
                .unwrap_or(0);
            let mut chars = 0;
            if let Some(line) = lines.get(index) {
                let mut at = line.start;
                for &len in &line.character_lengths {
                    if at >= byte {
                        break;
                    }
                    at += usize::from(len);
                    chars += 1;
                }
            }
            TextPosition {
                node: NodeId(id + 1 + index as u64),
                character_index: chars,
            }
        };
        node.set_text_selection(accesskit::TextSelection {
            anchor: position(anchor),
            focus: position(focus),
        });
        let mut children = Vec::with_capacity(lines.len());
        for (index, line) in lines.iter().enumerate() {
            let run_id = NodeId(id + 1 + index as u64);
            let mut run = Node::new(Role::TextRun);
            run.set_value(line.text.clone());
            run.set_bounds(rect(line.bounds + origin));
            run.set_character_lengths(line.character_lengths.clone());
            tree.push(run_id, run);
            children.push(run_id);
        }
        node.set_children(children);
        let node_id = NodeId(id);
        tree.push(node_id, node);
        if self.focused {
            tree.set_focus(node_id);
        }
        node_id
    }
}

enum KeyAction {
    Edit(Edit),
    Submit,
    Blur,
}

/// The platform conventions for editing keys.
fn key_edit(key: &Key, modifiers: Modifiers, multiline: bool) -> Option<KeyAction> {
    let word = modifiers.word();
    let horizontal = |plain, by_word| if word { by_word } else { plain };
    let motion = match key {
        Key::ArrowLeft if cfg!(target_os = "macos") && modifiers.meta => Some(Motion::LineStart),
        Key::ArrowRight if cfg!(target_os = "macos") && modifiers.meta => Some(Motion::LineEnd),
        Key::ArrowLeft => Some(horizontal(Motion::Left, Motion::WordLeft)),
        Key::ArrowRight => Some(horizontal(Motion::Right, Motion::WordRight)),
        Key::ArrowUp if cfg!(target_os = "macos") && modifiers.meta => Some(Motion::TextStart),
        Key::ArrowDown if cfg!(target_os = "macos") && modifiers.meta => Some(Motion::TextEnd),
        Key::ArrowUp => Some(Motion::Up),
        Key::ArrowDown => Some(Motion::Down),
        Key::Home if modifiers.ctrl => Some(Motion::TextStart),
        Key::End if modifiers.ctrl => Some(Motion::TextEnd),
        Key::Home => Some(Motion::LineStart),
        Key::End => Some(Motion::LineEnd),
        _ => None,
    };
    if let Some(motion) = motion {
        return Some(KeyAction::Edit(if modifiers.shift {
            Edit::Extend(motion)
        } else {
            Edit::Move(motion)
        }));
    }
    let edit = match key {
        Key::Backspace if word => Edit::BackspaceWord,
        Key::Backspace => Edit::Backspace,
        Key::Delete if word => Edit::DeleteWord,
        Key::Delete => Edit::Delete,
        Key::Enter if multiline => Edit::Newline,
        Key::Enter => return Some(KeyAction::Submit),
        Key::Escape => return Some(KeyAction::Blur),
        Key::Character(c) if modifiers.command() && c.eq_ignore_ascii_case("a") => Edit::SelectAll,
        _ => return None,
    };
    Some(KeyAction::Edit(edit))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(key: Key, text: Option<&str>, modifiers: Modifiers) -> Input {
        Input::Key {
            key,
            text: text.map(str::to_owned),
            pressed: true,
            repeat: false,
            modifiers,
        }
    }

    fn typed(c: &str) -> Input {
        key(Key::Character(c.into()), Some(c), Modifiers::default())
    }

    const BOUNDS: Rect = Rect::new(10.0, 10.0, 210.0, 40.0);

    fn press(at: Point) -> Input {
        Input::PointerButton {
            primary: true,
            pressed: true,
            position: at,
            clicks: 1,
            modifiers: Modifiers::default(),
        }
    }

    #[test]
    fn a_focused_field_takes_typing_and_editing_keys() {
        let mut fonts = Fonts::bundled_only();
        let mut field = TextField::new("", TextStyle::new(16.0), false).with_padding(4.0);
        assert!(
            !field.input(&mut fonts, &typed("x"), BOUNDS).handled,
            "not focused"
        );
        assert!(
            field
                .input(&mut fonts, &press(Point::new(20.0, 20.0)), BOUNDS)
                .handled
        );
        for c in ["h", "e", "y"] {
            field.input(&mut fonts, &typed(c), BOUNDS);
        }
        let word = Modifiers {
            ctrl: !cfg!(target_os = "macos"),
            alt: cfg!(target_os = "macos"),
            ..Modifiers::default()
        };
        field.input(&mut fonts, &typed(" "), BOUNDS);
        field.input(&mut fonts, &typed("you"), BOUNDS);
        let response = field.input(&mut fonts, &key(Key::Backspace, None, word), BOUNDS);
        assert!(response.changed);
        assert_eq!(field.text(), "hey ");
        let enter = field.input(
            &mut fonts,
            &key(Key::Enter, None, Modifiers::default()),
            BOUNDS,
        );
        assert!(enter.submitted);
        let escape = field.input(
            &mut fonts,
            &key(Key::Escape, None, Modifiers::default()),
            BOUNDS,
        );
        assert!(escape.blurred && !field.is_focused());
    }

    #[test]
    fn the_input_method_composes_in_the_field() {
        let mut fonts = Fonts::bundled_only();
        let mut field = TextField::new("", TextStyle::new(16.0), false);
        field.focus();
        let preedit = Input::Ime(Ime::Preedit {
            text: "かな".into(),
            cursor: Some((6, 6)),
        });
        field.input(&mut fonts, &preedit, BOUNDS);
        assert_eq!(field.text(), "");
        assert!(field.ime_area(&mut fonts, BOUNDS).is_some());
        // Keys typed while composing belong to the input method.
        assert!(!field.input(&mut fonts, &typed("k"), BOUNDS).changed);
        field.input(&mut fonts, &Input::Ime(Ime::Commit("仮名".into())), BOUNDS);
        assert_eq!(field.text(), "仮名");
    }

    #[test]
    fn cutting_without_a_selection_deletes_nothing() {
        let mut fonts = Fonts::bundled_only();
        let mut field = TextField::new("ab", TextStyle::new(16.0), false);
        field.focus();
        field
            .editor()
            .edit(&mut fonts, Edit::Move(Motion::TextStart));
        let command = Modifiers {
            ctrl: !cfg!(target_os = "macos"),
            meta: cfg!(target_os = "macos"),
            ..Modifiers::default()
        };
        let cut = key(Key::Character("x".into()), None, command);
        let mut clipboard = Clipboard::default();
        let response = field.input_with_clipboard(&mut fonts, &cut, BOUNDS, &mut clipboard);
        assert!(response.handled && !response.changed);
        assert_eq!(field.text(), "ab");
    }

    #[test]
    fn the_access_value_shows_the_composition_like_the_runs() {
        let mut fonts = Fonts::bundled_only();
        let mut field = TextField::new("ab", TextStyle::new(16.0), false);
        field.focus();
        field.editor().edit(&mut fonts, Edit::Move(Motion::TextEnd));
        let preedit = Input::Ime(Ime::Preedit {
            text: "にほん".into(),
            cursor: Some((9, 9)),
        });
        field.input(&mut fonts, &preedit, BOUNDS);
        let mut tree = AccessTree::default();
        field.access(&mut fonts, &mut tree, 10, BOUNDS, "Name");
        let value = |id| {
            tree.nodes()
                .iter()
                .find(|(node, _)| *node == NodeId(id))
                .and_then(|(_, node)| node.value().map(str::to_owned))
                .unwrap()
        };
        assert_eq!(value(10), value(11));
        assert_eq!(value(10), "abにほん");
    }

    #[test]
    fn the_caret_blinks_for_a_while_then_rests() {
        let mut field = TextField::new("", TextStyle::new(16.0), false);
        assert_eq!(field.wake_at(), None, "no caret without focus");
        field.focus();
        let next = field.wake_at().expect("blinking");
        assert!(next - field.blink_epoch == BLINK);
        field.blink_epoch -= BLINK_FOR;
        assert_eq!(field.wake_at(), None, "resting: no more frames");
        assert!(field.caret_on());
    }

    #[test]
    fn the_access_node_reports_value_lines_and_selection() {
        let mut fonts = Fonts::bundled_only();
        let mut field = TextField::new("hello", TextStyle::new(16.0), false);
        field.focus();
        field.editor().edit(&mut fonts, Edit::SelectAll);
        let mut tree = AccessTree::default();
        let id = field.access(&mut fonts, &mut tree, 10, BOUNDS, "Search");
        assert_eq!(id, NodeId(10));
        assert_eq!(tree.focus(), Some(NodeId(10)));
        let (_, node) = tree
            .nodes()
            .iter()
            .find(|(id, _)| *id == NodeId(10))
            .unwrap();
        assert_eq!(node.value(), Some("hello"));
        let selection = node.text_selection().unwrap();
        assert_eq!(selection.anchor.character_index, 0);
        assert_eq!(selection.focus.character_index, 5);
        assert_eq!(selection.focus.node, NodeId(11));
    }
}
