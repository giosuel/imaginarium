// SPDX-License-Identifier: AGPL-3.0-or-later

//! Copy, cut and paste (`canvas.md` §8, M3.3). The commands only note
//! what the clipboard should do; the board hands it over where the
//! clipboard is at hand (its input), since reading the system clipboard
//! never blocks the UI thread.

use std::collections::HashSet;

use imaginarium_model::{Actor, Geometry, NodeId, Parent, Result, StyleId, Value};
use imaginarium_render::kurbo::Point;
use imaginarium_ui::Clipboard;

use super::BoardCanvas;
use crate::clipboard::{self, Copied};
use crate::copy::NodeCopy;

/// Pasting the same copy again moves it this much further each time.
const PASTE_OFFSET: f64 = 16.0;

/// What the clipboard should do next.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct ClipboardWork {
    /// HTML and text to put on it.
    pub(crate) copy: Option<(String, String)>,
    /// A paste was asked for.
    pub(crate) paste: bool,
}

impl BoardCanvas {
    /// The selected nodes as a copy: their boxes in board coordinates, and
    /// the named styles they use.
    pub(crate) fn copy_selection(&self) -> Option<Copied> {
        let units = self.selected_units();
        let view = self.document.view();
        let items = self.items.as_ref()?;
        let mut nodes = Vec::new();
        for node in &units {
            let Some(mut copy) = NodeCopy::read(view, *node) else {
                continue;
            };
            // On the board, whatever frame or group it was in.
            if let (Some(g), Some(world)) = (copy.geometry, items.world_of(*node)) {
                let center = world * Point::new(g.w / 2.0, g.h / 2.0);
                let axis = world * Point::new(1.0, 0.0) - world * Point::ZERO;
                copy.geometry = Some(Geometry {
                    x: center.x - g.w / 2.0,
                    y: center.y - g.h / 2.0,
                    w: g.w,
                    h: g.h,
                    r: crate::gesture::normalize_degrees(axis.y.atan2(axis.x).to_degrees()),
                });
            }
            nodes.push(copy);
        }
        if nodes.is_empty() {
            return None;
        }
        let mut used = HashSet::new();
        fn styles_of(node: &NodeCopy, used: &mut HashSet<String>) {
            if let Some(style) = &node.style {
                used.insert(style.clone());
            }
            for child in &node.children {
                styles_of(child, used);
            }
        }
        for node in &nodes {
            styles_of(node, &mut used);
        }
        let styles = used
            .into_iter()
            .filter_map(|id| {
                let style_id = StyleId::new(id.as_str()).ok()?;
                // Its effective properties: its own, then its ancestors'.
                let mut props: Vec<(String, Value)> = Vec::new();
                let mut seen = HashSet::new();
                let mut next = Some(style_id);
                while let Some(current) = next {
                    if !seen.insert(current.clone()) {
                        break;
                    }
                    let Some(style) = view.style(&current) else {
                        break;
                    };
                    for (key, value) in style.props().iter() {
                        if !props.iter().any(|(known, _)| known == key) {
                            props.push((key.to_owned(), value.clone()));
                        }
                    }
                    next = style.parent().cloned();
                }
                Some((id, props))
            })
            .collect();
        Some(Copied { nodes, styles })
    }

    /// `board.copy`: puts the selection on the clipboard (at the next
    /// hand-over). `false` without a selection.
    pub(crate) fn copy_to_clipboard(&mut self) -> bool {
        let Some(copied) = self.copy_selection() else {
            return false;
        };
        let html = clipboard::to_html(&copied);
        let text = clipboard::plain_text(&copied);
        // A paste of this copy here goes a little down and right.
        self.pastes = clipboard::payload(&html).map(|payload| (payload.to_owned(), 0));
        self.clipboard_work.copy = Some((html, text));
        true
    }

    /// After a cut, the first paste lands where the nodes were.
    pub(crate) fn forget_pastes(&mut self) {
        self.pastes = None;
    }

    /// `board.paste`: asks the clipboard for what to paste.
    pub(crate) fn request_paste(&mut self) {
        self.clipboard_work.paste = true;
    }

    /// Hands the clipboard what the commands asked for.
    pub(super) fn hand_over(&mut self, clipboard: &mut Clipboard) {
        let work = std::mem::take(&mut self.clipboard_work);
        if let Some((html, text)) = work.copy {
            clipboard.set_html(&html, &text);
        }
        if work.paste {
            clipboard.request_marked(clipboard::MARKER);
        }
    }

    /// Pastes what the clipboard gave: the board's nodes, or text as a
    /// text box in the middle of the view. One transaction; the pasted
    /// nodes are selected.
    pub(crate) fn paste(&mut self, pasted: &str) -> Result<Vec<NodeId>> {
        // The board's copy that doesn't read (damaged, or nested too deep
        // for the parser) pastes nothing rather than its raw HTML.
        if pasted.contains(clipboard::MARKER) && clipboard::from_html(pasted).is_none() {
            return Ok(Vec::new());
        }
        let Some(copied) = clipboard::from_html(pasted) else {
            let text = pasted.replace("\r\n", "\n").replace('\r', "\n");
            let text = text.trim_end_matches('\n');
            if text.trim().is_empty() {
                return Ok(Vec::new());
            }
            let id = self.insert_text_at(self.view_middle())?;
            self.cancel_pending_edit();
            self.transact(&Actor::User, |txn| txn.insert_text(id, 0, text))?;
            return Ok(vec![id]);
        };
        // The same copy pasted again here lands further down and right.
        // By the payload, not the HTML: a clipboard may wrap it (macOS).
        let payload = clipboard::payload(pasted).unwrap_or_default().to_owned();
        let count = match &mut self.pastes {
            Some((known, count)) if *known == payload => {
                *count += 1;
                *count
            }
            _ => {
                self.pastes = Some((payload, 0));
                0
            }
        };
        let offset = PASTE_OFFSET * count as f64;
        let view = self.document.view();
        // Styles this board doesn't have come by value.
        let missing: Vec<(String, Vec<(String, Value)>)> = copied
            .styles
            .iter()
            .filter(|(id, _)| {
                StyleId::new(id.as_str())
                    .ok()
                    .and_then(|id| view.style(&id))
                    .is_none()
            })
            .cloned()
            .collect();
        fn inline(node: &mut NodeCopy, missing: &[(String, Vec<(String, Value)>)]) {
            if let Some(style) = &node.style
                && let Some((_, props)) = missing.iter().find(|(id, _)| id == style)
            {
                for (key, value) in props {
                    if !node.overrides.iter().any(|(known, _)| known == key) {
                        node.overrides.push((key.clone(), value.clone()));
                    }
                }
                node.style = None;
            }
            for child in &mut node.children {
                inline(child, missing);
            }
        }
        let mut nodes = copied.nodes;
        for node in &mut nodes {
            inline(node, &missing);
        }
        let created = self.transact(&Actor::User, |txn| {
            nodes
                .iter()
                .map(|node| node.write(txn, Parent::Root, None, (offset, offset)))
                .collect::<Result<Vec<_>>>()
        })?;
        self.selection.set(created.clone());
        Ok(created)
    }
}
