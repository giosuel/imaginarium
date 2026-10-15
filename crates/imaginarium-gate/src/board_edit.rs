// SPDX-License-Identifier: AGPL-3.0-or-later

//! Editing the shapes board: selection (click, Shift+click, rubber band,
//! Ctrl+A), moving (drag, arrow keys), deleting, undo and redo.
//!
//! A drag lifts the selected shapes out of the tiles: their tiles render
//! once without them, the shapes are drawn live on top while they move,
//! and are put back into the tiles when the drag ends. So a drag costs
//! tile renders at its start and end, not in every frame
//! (`docs/design/rendering.md` §5).

use std::collections::BTreeSet;

use imaginarium_render::Scene;
use imaginarium_render::canvas::ItemId;
use imaginarium_render::kurbo::{Point, Rect, Vec2};

use super::{ACCENT, Board};

/// Shapes taken out of the tiles while they are dragged.
#[derive(Clone, Debug)]
pub(crate) struct Lift {
    pub(crate) ids: Vec<usize>,
    /// The document point the drag is at.
    last: Point,
    /// How far the shapes moved since the drag began.
    total: Vec2,
}

impl Lift {
    /// The document point the drag is at.
    pub(crate) fn at(&self) -> Point {
        self.last
    }
}

/// An undoable change.
#[derive(Clone, Debug)]
pub(crate) enum ShapeEdit {
    Move { ids: Vec<usize>, delta: Vec2 },
    Delete { ids: Vec<usize> },
}

impl Board {
    /// The selected shapes.
    pub(crate) fn selection_len(&self) -> usize {
        self.selection.len()
    }

    pub(super) fn select_only(&mut self, index: usize) {
        self.selection.clear();
        self.selection.insert(index);
    }

    pub(super) fn toggle_selected(&mut self, index: usize) {
        if !self.selection.remove(&index) {
            self.selection.insert(index);
        }
    }

    /// Selects the shapes inside `area` (document units), adding to the
    /// selection if `add`.
    pub(super) fn select_in(&mut self, area: Rect, add: bool) {
        if !add {
            self.selection.clear();
        }
        for (index, shape) in self.shapes.iter().enumerate() {
            if !self.deleted[index] && area.contains_rect(shape.rect) {
                self.selection.insert(index);
            }
        }
    }

    pub(super) fn select_all(&mut self) {
        self.selection = (0..self.shapes.len())
            .filter(|&i| !self.deleted[i])
            .collect();
    }

    /// Starts dragging the selection from document point `at`.
    pub(crate) fn lift(&mut self, at: Point) {
        let ids: Vec<usize> = self.selection.iter().copied().collect();
        for &index in &ids {
            // Their tiles render once without them.
            self.shapes_canvas.invalidate(self.shapes[index].bounds());
        }
        self.lifted = Some(Lift {
            ids,
            last: at,
            total: Vec2::ZERO,
        });
    }

    /// Whether a shape is lifted out of the tiles (drawn live instead).
    #[cfg(test)]
    pub(super) fn is_lifted(&self, index: usize) -> bool {
        self.lifted
            .as_ref()
            .is_some_and(|lift| lift.ids.binary_search(&index).is_ok())
    }

    /// Moves the lifted shapes so the drag is at `to`.
    pub(crate) fn drag_to(&mut self, to: Point) {
        let Some(lift) = &mut self.lifted else {
            return;
        };
        let delta = to - lift.last;
        lift.last = to;
        lift.total += delta;
        for &index in &lift.ids {
            self.shapes[index].translate(delta);
        }
    }

    /// Ends a drag: the shapes go back into the tiles at their new places,
    /// on top.
    pub(crate) fn drop_lift(&mut self) {
        let Some(lift) = self.lifted.take() else {
            return;
        };
        for &index in &lift.ids {
            self.top_z += 1;
            self.shapes[index].z = self.top_z;
            let shape = &self.shapes[index];
            self.shapes_canvas
                .upsert(ItemId(index as u64), shape.z, shape.bounds());
        }
        if lift.total != Vec2::ZERO {
            self.record(ShapeEdit::Move {
                ids: lift.ids,
                delta: lift.total,
            });
        }
    }

    /// Moves the selection by `delta` (arrow keys).
    pub(super) fn nudge(&mut self, delta: Vec2) {
        let ids: Vec<usize> = self.selection.iter().copied().collect();
        if ids.is_empty() {
            return;
        }
        self.apply_move(&ids, delta);
        self.record(ShapeEdit::Move { ids, delta });
    }

    fn apply_move(&mut self, ids: &[usize], delta: Vec2) {
        for &index in ids {
            let shape = &mut self.shapes[index];
            shape.translate(delta);
            let (z, bounds) = (shape.z, shape.bounds());
            self.shapes_canvas.upsert(ItemId(index as u64), z, bounds);
        }
    }

    /// Deletes the selection.
    pub(super) fn delete_selection(&mut self) {
        let ids: Vec<usize> = std::mem::take(&mut self.selection).into_iter().collect();
        if ids.is_empty() {
            return;
        }
        self.apply_delete(&ids, true);
        self.record(ShapeEdit::Delete { ids });
    }

    fn apply_delete(&mut self, ids: &[usize], delete: bool) {
        for &index in ids {
            self.deleted[index] = delete;
            if delete {
                self.shapes_canvas.remove(ItemId(index as u64));
            } else {
                let shape = &self.shapes[index];
                self.shapes_canvas
                    .upsert(ItemId(index as u64), shape.z, shape.bounds());
            }
        }
    }

    fn record(&mut self, edit: ShapeEdit) {
        self.undo.push(edit);
        self.redo.clear();
    }

    /// Undoes the last change; `false` if there is none.
    pub(super) fn undo(&mut self) -> bool {
        let Some(edit) = self.undo.pop() else {
            return false;
        };
        match &edit {
            ShapeEdit::Move { ids, delta } => self.apply_move(ids, -*delta),
            ShapeEdit::Delete { ids } => {
                self.apply_delete(ids, false);
                self.selection = ids.iter().copied().collect();
            }
        }
        self.redo.push(edit);
        true
    }

    /// Redoes the last undone change; `false` if there is none.
    pub(super) fn redo(&mut self) -> bool {
        let Some(edit) = self.redo.pop() else {
            return false;
        };
        match &edit {
            ShapeEdit::Move { ids, delta } => self.apply_move(ids, *delta),
            ShapeEdit::Delete { ids } => {
                self.apply_delete(ids, true);
                self.selection = BTreeSet::new();
            }
        }
        self.undo.push(edit);
        true
    }

    /// Draws what moves or is marked, on top of the tiles: the lifted
    /// shapes, selection outlines, the rubber band. `view` is the visible
    /// document area.
    pub(super) fn paint_shape_overlay(&self, scene: &mut Scene, view: Rect, level_scale: f64) {
        if let Some(lift) = &self.lifted {
            for &index in &lift.ids {
                let shape = &self.shapes[index];
                if shape.bounds().intersect(view).area() > 0.0 {
                    shape.paint(scene, level_scale);
                }
            }
        }
        let width = 1.5 / self.zoom;
        for &index in &self.selection {
            let shape = &self.shapes[index];
            if shape.bounds().intersect(view).area() > 0.0 {
                scene.stroke(
                    &shape.bounds().inflate(3.0 / self.zoom, 3.0 / self.zoom),
                    width,
                    ACCENT,
                );
            }
        }
        if let Some((start, end)) = self.rubber {
            let area = Rect::from_points(start, end);
            scene.fill(&area, ACCENT.with_alpha(0.08));
            scene.stroke(&area, width, ACCENT.with_alpha(0.8));
        }
    }
}

#[cfg(test)]
mod tests {
    use imaginarium_render::kurbo::{Point, Rect, Vec2};

    use crate::board::Board;

    #[test]
    fn a_drag_moves_the_selection_and_undo_puts_it_back() {
        let mut board = Board::new(10, 100, true, None);
        let before = board.shapes[3].rect;
        board.select_only(3);
        board.select_only(3);
        board.lift(before.center());
        assert!(board.is_lifted(3));
        board.drag_to(before.center() + Vec2::new(40.0, 10.0));
        board.drop_lift();
        assert!(!board.is_lifted(3));
        assert_eq!(board.shapes[3].rect, before + Vec2::new(40.0, 10.0));
        assert!(board.undo());
        assert_eq!(board.shapes[3].rect, before);
        assert!(board.redo());
        assert_eq!(board.shapes[3].rect, before + Vec2::new(40.0, 10.0));
    }

    #[test]
    fn delete_and_undo_restore_shapes_and_their_selection() {
        let mut board = Board::new(10, 100, true, None);
        board.select_in(Rect::new(-100.0, -100.0, 140.0, 140.0), false);
        let selected = board.selection_len();
        assert!(selected >= 2, "{selected}");
        board.delete_selection();
        assert_eq!(board.selection_len(), 0);
        let center = board.shapes[0].rect.center();
        assert_eq!(
            board.shapes_canvas.item_at(center),
            None,
            "removed from the canvas"
        );
        assert!(board.undo());
        assert_eq!(board.selection_len(), selected);
        assert!(board.shapes_canvas.item_at(center).is_some());
        board.nudge(Vec2::new(1.0, 0.0));
        assert!(board.shapes[0].rect.x0 > center.x - board.shapes[0].rect.width() / 2.0);
        let _ = Point::ZERO;
    }
}
