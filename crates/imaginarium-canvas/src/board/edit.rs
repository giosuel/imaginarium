// SPDX-License-Identifier: AGPL-3.0-or-later

//! Editing text in place (`canvas.md` §7): the toolkit's text field over a
//! text box or a shape's label, at the board's zoom and the node's turn.
//! Every change is a `text.replace` command, so the model is always the
//! truth and undo merges typing.

use imaginarium_model::{NodeId, NodeKind};
use imaginarium_render::Scene;
use imaginarium_render::kurbo::{Affine, Point, Rect};
use imaginarium_text::{Align, Fonts, TextStyle};
use imaginarium_ui::{Cx, Input, TextField, theme};

use super::{BoardCanvas, Press};
use crate::commands::{self, replace_args};
use crate::style;

/// The field's accessibility node: the board's local id this far up, and
/// its lines after it (items keep to the ids below).
pub(super) const FIELD_ACCESS: u64 = 1 << 23;

/// The space between a label's field and the shape's edge, as the label
/// is laid out.
const LABEL_PADDING: f64 = 8.0;

/// A text being edited.
pub(super) struct Editing {
    pub(super) node: NodeId,
    pub(super) field: TextField,
    /// A shape's label (centered), not a text box.
    label: bool,
}

/// The text of a node as the field shows it: without the final newline.
fn shown_text(board: &BoardCanvas, node: NodeId) -> String {
    let text = board
        .document
        .view()
        .node(node)
        .and_then(|node| node.text())
        .map(|text| text.to_plain_string())
        .unwrap_or_default();
    text.strip_suffix('\n').unwrap_or(&text).to_owned()
}

impl BoardCanvas {
    /// Whether `node`'s text is being edited.
    pub fn is_editing(&self, node: NodeId) -> bool {
        self.editing
            .as_ref()
            .is_some_and(|editing| editing.node == node)
    }

    /// Starts editing `node`'s text in place, with the caret at its end:
    /// a text box's text, or a shape's label. `false` for other nodes.
    pub fn edit_text(&mut self, fonts: &mut Fonts, node: NodeId) -> bool {
        self.start_editing(fonts, node, None)
    }

    /// Starts editing; `click` (window space, clicks) places the caret.
    pub(super) fn start_editing(
        &mut self,
        fonts: &mut Fonts,
        node: NodeId,
        click: Option<(Point, u8)>,
    ) -> bool {
        self.sync(fonts);
        self.stop_editing(fonts);
        let view = self.document.view();
        let Some(view_node) = view.node(node) else {
            return false;
        };
        let label = match view_node.kind() {
            NodeKind::Shape => true,
            NodeKind::Text => false,
            _ => return false,
        };
        if crate::hit::locked(view, node)
            || !self
                .items
                .as_ref()
                .is_some_and(|items| items.by_node.contains_key(&node))
        {
            return false;
        }
        let current = theme();
        let colors = crate::items::BoardColors::of(&current);
        let (size, weight, line_height) = if label {
            (style::LABEL_SIZE, 500.0, 1.3)
        } else {
            (style::TEXT_SIZE, 400.0, 1.5)
        };
        let look = style::text_look(view, node, colors.theme, colors.text, size, weight);
        let text_style = TextStyle::new(look.size)
            .weight(look.weight)
            .color(look.color)
            .line_height(line_height)
            .align(if label { Align::Center } else { Align::Start });
        let text = shown_text(self, node);
        let mut field = TextField::new(&text, text_style, true);
        field.focus();
        let end = text.len();
        field.editor().select_bytes(fonts, end, end);
        self.editing = Some(Editing { node, field, label });
        self.select(&[node]);
        // The tiles show the node without its text meanwhile.
        if let Some(item) = self
            .items
            .as_ref()
            .and_then(|items| items.by_node.get(&node))
        {
            self.canvas.invalidate(item.bounds);
        }
        if let Some((at, clicks)) = click
            && let Some((frame, bounds)) = self.field_frame(fonts)
            && let Some(editing) = &mut self.editing
        {
            let local = frame.inverse() * at;
            // Outside the text (a shape's edge), the caret stays at the end.
            if !bounds.contains(local) {
                return true;
            }
            let press = |pressed| Input::PointerButton {
                primary: true,
                pressed,
                position: local,
                clicks,
                modifiers: imaginarium_ui::Modifiers::default(),
            };
            editing.field.input(fonts, &press(true), bounds);
            editing.field.input(fonts, &press(false), bounds);
        }
        true
    }

    /// Ends editing (the text is in the model already).
    pub(super) fn stop_editing(&mut self, fonts: &mut Fonts) {
        if let Some(mut editing) = self.editing.take() {
            editing.field.blur(fonts);
            if let Some(item) = self
                .items
                .as_ref()
                .and_then(|items| items.by_node.get(&editing.node))
            {
                self.canvas.invalidate(item.bounds);
            }
            if self.press == Some(Press::Field) {
                self.press = None;
            }
        }
    }

    /// Where the field goes: the node's box space to the window, and the
    /// field's box in it (a label's in the middle, a text box's at the top,
    /// growing with its text).
    pub(super) fn field_frame(&mut self, fonts: &mut Fonts) -> Option<(Affine, Rect)> {
        let camera = self.camera(self.bounds);
        let editing = self.editing.as_mut()?;
        let item = self.items.as_ref()?.by_node.get(&editing.node)?;
        let size = item.size;
        let frame = camera * item.world;
        let bounds = if editing.label {
            let width = (size.width - 2.0 * LABEL_PADDING).max(8.0);
            let height = editing.field.preferred_size(fonts, width).height;
            let triangle = self
                .document
                .view()
                .node(editing.node)
                .and_then(|node| node.prop("shape"))
                .and_then(imaginarium_model::Value::as_str)
                == Some("triangle");
            let middle = if triangle {
                size.height * 2.0 / 3.0
            } else {
                size.height / 2.0
            };
            let top = (middle - height / 2.0).max(0.0);
            Rect::new(LABEL_PADDING, top, LABEL_PADDING + width, top + height)
        } else {
            let width = size.width.max(8.0);
            let height = editing
                .field
                .preferred_size(fonts, width)
                .height
                .max(size.height);
            Rect::new(0.0, 0.0, width, height)
        };
        Some((frame, bounds))
    }

    /// Puts the field's text into the model: the changed run, as one
    /// `text.replace`.
    fn commit_field(&mut self) {
        let Some(editing) = &self.editing else {
            return;
        };
        let node = editing.node;
        let new: Vec<char> = editing.field.text().chars().collect();
        let old: Vec<char> = shown_text(self, node).chars().collect();
        let prefix = old.iter().zip(&new).take_while(|(a, b)| a == b).count();
        let suffix = old[prefix..]
            .iter()
            .rev()
            .zip(new[prefix..].iter().rev())
            .take_while(|(a, b)| a == b)
            .count();
        let delete = old.len() - prefix - suffix;
        let insert: String = new[prefix..new.len() - suffix].iter().collect();
        if delete == 0 && insert.is_empty() {
            return;
        }
        // Refused (the node went meanwhile): the field keeps its text.
        let _ = commands::execute(
            self,
            "text.replace",
            &replace_args(node, prefix, delete, &insert),
        );
    }

    /// Input while editing: the field's, or `None` for the board's (a
    /// press outside the field ends editing first).
    pub(super) fn edit_input(&mut self, cx: &mut Cx<'_>, input: &Input) -> Option<bool> {
        self.editing.as_ref()?;
        let (frame, bounds) = self.field_frame(cx.fonts())?;
        let inverse = frame.inverse();
        let local = |input: &Input| match input {
            Input::PointerButton {
                primary,
                pressed,
                position,
                clicks,
                modifiers,
            } => Input::PointerButton {
                primary: *primary,
                pressed: *pressed,
                position: inverse * *position,
                clicks: *clicks,
                modifiers: *modifiers,
            },
            Input::PointerMoved(position) => Input::PointerMoved(inverse * *position),
            other => other.clone(),
        };
        let response = match input {
            Input::PointerButton {
                primary: true,
                pressed: true,
                position,
                ..
            } => {
                if !bounds.contains(inverse * *position) {
                    self.stop_editing(cx.fonts());
                    cx.request_paint();
                    return None;
                }
                cx.request_focus();
                cx.capture_pointer();
                self.press = Some(Press::Field);
                let editing = self.editing.as_mut()?;
                editing.field.input(cx.fonts(), &local(input), bounds)
            }
            Input::PointerMoved(_) | Input::PointerButton { .. }
                if self.press == Some(Press::Field) =>
            {
                if matches!(input, Input::PointerButton { pressed: false, .. }) {
                    self.press = None;
                    cx.release_pointer();
                }
                let editing = self.editing.as_mut()?;
                editing.field.input(cx.fonts(), &local(input), bounds)
            }
            Input::Key { .. } | Input::Ime(_) | Input::Paste(_) => {
                let editing = self.editing.as_mut()?;
                let (fonts, clipboard) = cx.fonts_and_clipboard();
                editing
                    .field
                    .input_with_clipboard(fonts, input, bounds, clipboard)
            }
            _ => return None,
        };
        if response.changed {
            self.commit_field();
        }
        if response.blurred {
            self.stop_editing(cx.fonts());
        }
        if response.repaint || response.changed || response.blurred {
            cx.request_paint();
        }
        // Keys stay with the field while it edits (arrows move the caret,
        // not the shape).
        Some(response.handled || matches!(input, Input::Key { .. }))
    }

    /// The field, on top of the tiles at the node's place.
    pub(super) fn paint_editing(&mut self, fonts: &mut Fonts, scene: &mut Scene) {
        let Some((frame, bounds)) = self.field_frame(fonts) else {
            return;
        };
        let accent = theme().color(imaginarium_ui::Token::Accent);
        if let Some(editing) = &mut self.editing {
            scene.paint_with(frame, |scene| {
                editing.field.paint(fonts, scene, bounds, accent)
            });
        }
    }

    /// The input method's place for the field, in window space.
    pub(super) fn editing_ime_area(&mut self, fonts: &mut Fonts) -> Option<Rect> {
        let (frame, bounds) = self.field_frame(fonts)?;
        let area = self.editing.as_mut()?.field.ime_area(fonts, bounds)?;
        Some(frame.transform_rect_bbox(area))
    }

    /// After changes: the edited node gone or hidden ends editing; its
    /// text changed elsewhere (an undo, a collaborator) shows in the field.
    pub(super) fn sync_editing(&mut self, fonts: &mut Fonts) {
        let Some(node) = self.editing.as_ref().map(|editing| editing.node) else {
            return;
        };
        let visible = self
            .items
            .as_ref()
            .is_some_and(|items| items.by_node.contains_key(&node));
        if !visible {
            self.stop_editing(fonts);
            return;
        }
        let text = shown_text(self, node);
        if let Some(editing) = &mut self.editing
            && editing.field.text() != text
            && !editing.field.editor().is_composing()
        {
            editing.field.editor().set_text(fonts, &text);
        }
    }

    /// The field's accessibility node, under the board (`base` is the
    /// board's access id); `focused`: the board has the focus.
    pub(super) fn editing_access(&mut self, cx: &mut Cx<'_>, base: u64, focused: bool) {
        let Some((frame, bounds)) = self.field_frame(cx.fonts()) else {
            return;
        };
        let window = frame.transform_rect_bbox(bounds);
        let Some(editing) = &mut self.editing else {
            return;
        };
        let label = if editing.label { "Label" } else { "Text" };
        let (fonts, tree) = cx.fonts_and_access();
        let Some(tree) = tree else {
            return;
        };
        let id = editing
            .field
            .access(fonts, tree, base + FIELD_ACCESS, window, label);
        if focused {
            tree.set_focus(id);
        }
        cx.add_access_child(id);
    }

    /// A screen reader's request for the field (`local`: its id from the
    /// board's base). `None` when it isn't the field's.
    pub(super) fn editing_access_action(
        &mut self,
        cx: &mut Cx<'_>,
        request: &accesskit::ActionRequest,
        local: u64,
    ) -> Option<bool> {
        if local < FIELD_ACCESS {
            return None;
        }
        let base = cx.id().access_base();
        let editing = self.editing.as_mut()?;
        let response = editing
            .field
            .access_action(cx.fonts(), request, base + FIELD_ACCESS);
        if response.changed {
            self.commit_field();
        }
        cx.request_paint();
        Some(response.handled)
    }

    /// The field's caret blink.
    pub(super) fn editing_wake_at(&self) -> Option<std::time::Instant> {
        self.editing.as_ref()?.field.wake_at()
    }
}
