// SPDX-License-Identifier: AGPL-3.0-or-later

//! The board widget: a document's nodes on the tile canvas, with the
//! board's background, panning and zooming, and what is drawn on top of
//! the tiles (frame names).

use std::collections::HashMap;
use std::rc::Rc;
use std::time::Instant;

use accesskit::{Action, ActionRequest, Node, Role};
use imaginarium_model::{
    Actor, ChangeSet, Document, NodeId, NodeKind, Result, Transaction, UndoScope,
};
use imaginarium_reactive::{Signal, signal};
use imaginarium_render::canvas::{Canvas, CanvasStats, Viewport};
use imaginarium_render::kurbo::{Affine, Point, Rect, Stroke, Vec2};
use imaginarium_render::{Scene, TileWorkers};
use imaginarium_text::{Fonts, TextBlock, TextStyle};
use imaginarium_ui::{
    ACCESS_LOCAL_IDS, Cx, Input, Key, Theme, Token, Widget, access_rect, mix, theme,
};

mod clip;
mod edit;
mod ops;
mod tools;

pub(crate) use ops::{DUPLICATE_OFFSET, Order};
pub use tools::{Preset, Tool};

use edit::{Editing, FIELD_ACCESS};

use crate::commands::{self, box_args, move_args};
use crate::gesture::{self, Handle, SelectionBox, Side, Start};
use crate::hit;
use crate::items::{BoardColors, Items, Look};
use crate::paint::ItemPainter;

/// A press moves this many logical pixels before it drags.
const DRAG_THRESHOLD: f64 = 3.0;

/// What the pointer is doing since its press.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Press {
    /// Pressed at `at` (window space) on a node, which is now selected;
    /// it becomes a move once the pointer leaves the threshold. A click
    /// that doesn't move on one node of several selected (`narrow`)
    /// selects just that one at the release.
    Node { at: Point, narrow: Option<NodeId> },
    /// Pressed in the text field being edited: it has the pointer.
    Field,
    /// Dragging a marquee from `start` (document space); `additive` keeps
    /// the selection it started with.
    Marquee {
        start: Point,
        current: Point,
        additive: bool,
    },
}

/// A gesture in progress: one undo step, its commands applied at most
/// once a frame (`canvas.md` §6), from the latest pointer position.
#[derive(Clone, Debug)]
enum Gesture {
    /// Dragging the selection: `applied` is where the last move left it.
    Move { applied: Point, pointer: Point },
    /// Drawing a shape (`preset`) or a frame (`None`) from `origin`
    /// (document space): inserted once the press travels, its box
    /// following the pointer after.
    Draw {
        preset: Option<tools::Preset>,
        origin: Point,
        pointer: Point,
        node: Option<NodeId>,
    },
    /// The hand tool panning: the pointer's last place (window space).
    Pan { last: Point, pointer: Point },
    Resize {
        handle: (Side, Side),
        selection: SelectionBox,
        starts: Vec<Start>,
        pointer: Point,
        /// The handle's center less where it was pressed: the box follows
        /// the handle, not the pointer's own position.
        grab: Vec2,
        keep_aspect: bool,
    },
    Rotate {
        center: Point,
        from: f64,
        starts: Vec<Start>,
        pointer: Point,
        snap: bool,
    },
    Radius {
        node: NodeId,
        selection: SelectionBox,
        pointer: Point,
        grab: Vec2,
    },
}

/// Rotation snaps to this many degrees with Shift.
const SNAP_DEGREES: f64 = 15.0;

/// The most items listed for screen readers: the visible ones, up to this.
const MAX_ACCESS_ITEMS: usize = 2_000;

/// The zoom's limits.
pub const MIN_ZOOM: f64 = 0.05;
/// The zoom's limits.
pub const MAX_ZOOM: f64 = 8.0;

/// The board's dots, every this many points at 100 %.
const DOT_PITCH: f64 = 24.0;

/// A frame's name sits this far above it, in logical pixels.
const NAME_GAP: f64 = 8.0;

/// What the board window's controls show ([`BoardCanvas::status`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BoardStatus {
    /// Logical pixels per point (1.0 is 100 %).
    pub zoom: f64,
    /// Whether `board.undo` has a step to undo.
    pub can_undo: bool,
    /// Whether `board.redo` has a step to redo.
    pub can_redo: bool,
}

impl Default for BoardStatus {
    fn default() -> Self {
        Self {
            zoom: 1.0,
            can_undo: false,
            can_redo: false,
        }
    }
}

/// A board: the document it shows (ADR-0020: the open document lives on
/// the UI thread, here) and its view.
pub struct BoardCanvas {
    document: Document,
    items: Option<Items>,
    canvas: Canvas,
    origin: Point,
    zoom: f64,
    /// The theme the items were built in; a switch rebuilds them.
    theme: Option<Rc<Theme>>,
    /// Frames drawn so far: the first is drawn without tiles.
    painted: u64,
    /// Tiles are owed: wake to draw them.
    owed_at: Option<Instant>,
    /// Frame names laid out, by frame and text.
    names: HashMap<NodeId, (String, TextBlock)>,
    /// Changes not on the canvas yet (laying out text needs the fonts,
    /// which painting and input bring).
    pending: Vec<ChangeSet>,
    stats: CanvasStats,
    /// The selected nodes, in the order they were selected.
    selection: Signal<Vec<NodeId>>,
    /// The node under the pointer.
    hovered: Option<NodeId>,
    /// The group a double-click went into: its children select one by one.
    entered: Option<NodeId>,
    press: Option<Press>,
    gesture: Option<Gesture>,
    /// A handle's press (window space) that hasn't travelled the drag
    /// threshold yet: until it does, the gesture changes nothing.
    armed: Option<Point>,
    /// The gesture opened an undo group (closed at its end).
    grouped: bool,
    /// The text being edited in place.
    editing: Option<Editing>,
    /// A node `text.edit` asked to edit, started when the fonts are at
    /// hand (the next sync).
    pending_edit: Option<NodeId>,
    /// Shortcuts to the board's commands.
    keymap: imaginarium_commands::Keymap,
    /// The tool in use.
    tool: Signal<Tool>,
    /// What the window's controls show (zoom, undo and redo).
    status: Signal<BoardStatus>,
    /// Counts the document's changes, for panels that read it to know
    /// when to look again.
    revision: u64,
    /// What the clipboard should do at the next hand-over.
    clipboard_work: clip::ClipboardWork,
    /// The last copy pasted (or copied) here, and how often it was pasted.
    pastes: Option<(String, u64)>,
    /// Where frame names were drawn (window space), front last.
    name_rects: Vec<(NodeId, Rect)>,
    /// The board's box in the last layout.
    bounds: Rect,
    /// The dots of the last frame: one path, and the view it was built for
    /// (the box, the first dot, the spacing).
    dots: Option<((Rect, Point, f64), imaginarium_render::kurbo::BezPath)>,
}

impl BoardCanvas {
    /// A board showing `document`, at 100 % with its origin at the top left.
    pub fn new(document: Document) -> Self {
        Self {
            document,
            items: None,
            canvas: Canvas::new(),
            origin: Point::ZERO,
            zoom: 1.0,
            theme: None,
            painted: 0,
            owed_at: None,
            names: HashMap::new(),
            pending: Vec::new(),
            stats: CanvasStats::default(),
            selection: signal(Vec::new()),
            hovered: None,
            entered: None,
            press: None,
            gesture: None,
            armed: None,
            grouped: false,
            editing: None,
            pending_edit: None,
            keymap: crate::keys::default_keymap(imaginarium_commands::Platform::current()),
            tool: signal(Tool::Select),
            status: signal(BoardStatus::default()),
            revision: 0,
            clipboard_work: clip::ClipboardWork::default(),
            pastes: None,
            name_rects: Vec::new(),
            bounds: Rect::ZERO,
            dots: None,
        }
    }

    /// The selection: the selected nodes, in the order they were selected.
    /// Panels and commands read it; it stays valid as nodes go.
    pub fn selection(&self) -> Signal<Vec<NodeId>> {
        self.selection
    }

    /// The straight box around the selection in window coordinates (a
    /// turned node's box, turned, then boxed), as the last layout placed
    /// the board, for panels that float next to it (the contextual
    /// toolbar). `None` without a selection.
    pub fn selection_on_screen(&self) -> Option<Rect> {
        let selection = self.selection_box()?;
        let shape = Rect::from_origin_size(Point::ZERO, selection.size);
        Some((self.camera(self.bounds) * selection.world).transform_rect_bbox(shape))
    }

    /// Whether the pointer is moving, resizing, turning or drawing
    /// something now (panels next to the selection step aside).
    pub fn is_gesturing(&self) -> bool {
        self.gesture.is_some() && self.armed.is_none()
    }

    /// The zoom and whether undo and redo can run, for the window's
    /// controls. Brought up to date whenever the board paints or takes
    /// input, so a change shows in the next frame.
    pub fn status(&self) -> Signal<BoardStatus> {
        self.status
    }

    /// Writes the current state into [`BoardCanvas::status`] (readers wake
    /// only when it changed).
    fn publish_status(&self) {
        use imaginarium_model::UndoScope;
        self.status.set_if_changed(BoardStatus {
            zoom: self.zoom,
            can_undo: self.document.can_undo(UndoScope::User),
            can_redo: self.document.can_redo(UndoScope::User),
        });
    }

    /// The selected nodes a gesture changes: not locked, and not inside
    /// another selected node (which moves them already), with their boxes
    /// and parents' spaces now.
    fn starts(&self) -> Vec<Start> {
        let view = self.document.view();
        let selection = self.selection.get_untracked();
        let Some(items) = &self.items else {
            return Vec::new();
        };
        selection
            .iter()
            .filter(|node| !hit::locked(view, **node))
            .filter(|node| {
                let mut next = view.node(**node).map(|node| node.parent());
                while let Some(imaginarium_model::Parent::Node(parent)) = next {
                    if selection.contains(&parent) {
                        return false;
                    }
                    next = view.node(parent).map(|node| node.parent());
                }
                true
            })
            .filter_map(|node| {
                let view_node = view.node(*node)?;
                let geometry = view_node.geometry().unwrap_or_default();
                let line = matches!(view_node.kind(), NodeKind::Shape)
                    && view_node
                        .prop("shape")
                        .and_then(imaginarium_model::Value::as_str)
                        == Some("line");
                let flip = line.then(|| {
                    match view_node
                        .prop("flip")
                        .and_then(imaginarium_model::Value::as_str)
                    {
                        Some("h") => "h",
                        Some("v") => "v",
                        Some("hv") => "hv",
                        _ => "",
                    }
                });
                // A group's descendants scale with it.
                let mut children = Vec::new();
                if matches!(view_node.kind(), NodeKind::Group) {
                    let mut stack: Vec<NodeId> = view
                        .children(imaginarium_model::Parent::Node(*node))
                        .collect();
                    while let Some(child) = stack.pop() {
                        if let Some(child_node) = view.node(child) {
                            children.push((child, child_node.geometry().unwrap_or_default()));
                            if matches!(child_node.kind(), NodeKind::Group) {
                                stack.extend(view.children(imaginarium_model::Parent::Node(child)));
                            }
                        }
                    }
                }
                Some(Start {
                    node: *node,
                    geometry,
                    parent: items.parent_world(view, *node)?,
                    flip,
                    children,
                })
            })
            .collect()
    }

    /// Moves the selected nodes by `dx`, `dy` points on the board, in one
    /// transaction. `false` if nothing could move.
    pub(crate) fn move_selection(&mut self, dx: f64, dy: f64) -> Result<bool> {
        let starts = self.starts();
        if starts.is_empty() {
            return Ok(false);
        }
        self.transact(&Actor::User, |txn| {
            for start in &starts {
                // The move in the parent's space (a turned frame or group).
                let inverse = start.parent.inverse();
                let delta = inverse * Point::new(dx, dy) - inverse * Point::ZERO;
                let mut geometry = start.geometry;
                geometry.x += delta.x;
                geometry.y += delta.y;
                txn.set_geometry(start.node, geometry)?;
            }
            Ok(())
        })?;
        Ok(true)
    }

    /// The box around the selection: a single node's own (turned) box, or
    /// the straight box around several.
    fn selection_box(&self) -> Option<SelectionBox> {
        let items = self.items.as_ref()?;
        let view = self.document.view();
        let selection = self.selection.get_untracked();
        if let [node] = selection[..]
            && let Some(item) = items.by_node.get(&node)
        {
            return Some(SelectionBox {
                world: item.world,
                size: item.size,
            });
        }
        let union = selection
            .iter()
            .filter_map(|node| hit::node_box(items, view, *node))
            .reduce(|a, b| a.union(b))?;
        Some(SelectionBox {
            world: Affine::translate(union.origin().to_vec2()),
            size: union.size(),
        })
    }

    /// The corner radius of the selection when it is one rectangle.
    fn selected_radius(&self) -> Option<(NodeId, f64)> {
        let items = self.items.as_ref()?;
        let [node] = self.selection.get_untracked()[..] else {
            return None;
        };
        let view_node = self.document.view().node(node)?;
        let rect = view_node
            .prop("shape")
            .and_then(imaginarium_model::Value::as_str)
            .is_none_or(|preset| preset == "rect");
        match &items.by_node.get(&node)?.look {
            Look::Shape { style, .. } if rect && !view_node.props().contains_key("radii") => {
                Some((node, style.radius))
            }
            _ => None,
        }
    }

    /// Applies the gesture's latest state: one command per node, at most
    /// once a frame.
    fn flush_gesture(&mut self) {
        let Some(gesture) = self.gesture.clone() else {
            return;
        };
        // A press on a handle that hasn't moved yet changes nothing.
        if self.armed.is_some() {
            return;
        }
        let run = |board: &mut Self, placed: Vec<gesture::Placed>| {
            for placed in placed {
                // A node gone meanwhile leaves the rest as they are.
                let _ = commands::execute(
                    board,
                    "board.set_box",
                    &box_args(placed.node, placed.geometry, placed.flip),
                );
            }
        };
        match gesture {
            Gesture::Draw {
                preset,
                origin,
                pointer,
                node,
            } => {
                let area = Rect::from_points(origin, pointer);
                match node {
                    None => {
                        let args = commands::insert_args(preset, area);
                        let id = if preset.is_some() {
                            "board.insert_shape"
                        } else {
                            "board.insert_frame"
                        };
                        if commands::execute(self, id, &args).is_ok() {
                            let created = self.selection.get_untracked().first().copied();
                            if let Some(Gesture::Draw { node, .. }) = &mut self.gesture {
                                *node = created;
                            }
                        }
                    }
                    Some(node) => {
                        let view = self.document.view();
                        let parent = self
                            .items
                            .as_ref()
                            .and_then(|items| items.parent_world(view, node))
                            .unwrap_or_default();
                        let line = preset == Some(tools::Preset::Line);
                        // A line runs between the drag's ends in the parent's
                        // space (a turned frame turns it with it).
                        let (start, end) = (parent.inverse() * origin, parent.inverse() * pointer);
                        let local = if line {
                            Rect::from_points(start, end)
                        } else {
                            parent.inverse().transform_rect_bbox(area)
                        };
                        let geometry = imaginarium_model::Geometry::new(
                            local.x0,
                            local.y0,
                            local.width(),
                            local.height(),
                        );
                        let flip = line.then_some(match (end.x < start.x, end.y < start.y) {
                            (false, false) => "",
                            (true, false) => "h",
                            (false, true) => "v",
                            (true, true) => "hv",
                        });
                        let _ = commands::execute(
                            self,
                            "board.set_box",
                            &box_args(node, geometry, flip),
                        );
                    }
                }
            }
            Gesture::Pan { last, pointer } => {
                self.origin -= (pointer - last) / self.zoom;
                self.gesture = Some(Gesture::Pan {
                    last: pointer,
                    pointer,
                });
            }
            Gesture::Move { applied, pointer } => {
                let delta = pointer - applied;
                if delta.hypot() == 0.0 {
                    return;
                }
                self.gesture = Some(Gesture::Move {
                    applied: pointer,
                    pointer,
                });
                let _ = commands::execute(self, "board.move", &move_args(delta.x, delta.y));
            }
            Gesture::Resize {
                handle,
                selection,
                starts,
                pointer,
                grab,
                keep_aspect,
            } => {
                let to = gesture::resized(&selection, handle, pointer + grab, keep_aspect);
                run(self, gesture::resize_all(&selection, &starts, to));
            }
            Gesture::Rotate {
                center,
                from,
                starts,
                pointer,
                snap,
            } => {
                let mut degrees = gesture::angle(center, pointer) - from;
                if snap && let [ref start] = starts[..] {
                    let target =
                        ((start.geometry.r + degrees) / SNAP_DEGREES).round() * SNAP_DEGREES;
                    degrees = target - start.geometry.r;
                }
                run(self, gesture::rotate_all(&starts, center, degrees));
            }
            Gesture::Radius {
                node,
                selection,
                pointer,
                grab,
            } => {
                let radius = (gesture::radius_at(&selection, pointer + grab) * 2.0).round() / 2.0;
                let _ = commands::execute(
                    self,
                    "shape.set_radius",
                    &commands::radius_args(node, radius),
                );
            }
        }
    }

    /// Starts a gesture: one undo step until the release.
    fn begin_gesture(&mut self, gesture: Gesture) {
        // An open group would otherwise swallow the gesture's step.
        if self.gesture.is_some() {
            self.end_gesture();
        }
        // Only a group this gesture opened is closed at its end; panning
        // changes no document.
        self.grouped = !matches!(gesture, Gesture::Pan { .. })
            && self.document.begin_undo_group(UndoScope::User).is_ok();
        self.gesture = Some(gesture);
    }

    /// Applies the gesture's last state and closes its undo step.
    fn end_gesture(&mut self) {
        if self.gesture.is_some() {
            // A shape tool's click without a drag places the tool's size;
            // a drag released before any paint inserts what was dragged.
            if self.armed.is_none() {
                self.flush_gesture();
            }
            if let Some(Gesture::Draw {
                preset,
                origin,
                node: None,
                ..
            }) = self.gesture.clone()
                && self.armed.is_some()
            {
                let tool = preset.map_or(Tool::Frame, Tool::Shape);
                let area = Self::click_area(tool, origin);
                let id = if preset.is_some() {
                    "board.insert_shape"
                } else {
                    "board.insert_frame"
                };
                let _ = commands::execute(self, id, &commands::insert_args(preset, area));
                // Placed: nothing more to draw.
                self.gesture = Some(Gesture::Draw {
                    preset,
                    origin,
                    pointer: origin,
                    node: self.selection.get_untracked().first().copied(),
                });
                self.armed = Some(origin);
            }
            self.flush_gesture();
            if let Some(Gesture::Draw { preset, node, .. }) = self.gesture.clone() {
                // A shape dragged back to nothing goes again.
                if let Some(node) = node
                    && self
                        .document
                        .view()
                        .node(node)
                        .and_then(|node| node.geometry())
                        .is_some_and(|g| g.w < 1.0 && g.h < 1.0)
                {
                    let _ = self.transact(&Actor::User, |txn| txn.delete_node(node));
                    self.selection.set_if_changed(Vec::new());
                }
                // One shape, then Select returns (unless another tool was
                // picked meanwhile).
                let drawing = preset.map_or(Tool::Frame, Tool::Shape);
                if self.tool.get_untracked() == drawing {
                    self.set_tool(Tool::Select);
                }
            }
            self.gesture = None;
            self.armed = None;
            if std::mem::take(&mut self.grouped) {
                self.document.end_undo_group(UndoScope::User);
            }
        }
    }

    /// The handle under `screen` (window space), if the selection has one
    /// there.
    fn handle_at(&self, screen: Point) -> Option<(Handle, SelectionBox)> {
        let selection = self.selection_box()?;
        self.shown_handles(&selection)
            .into_iter()
            .filter(|(_, at)| (*at - screen).hypot() <= gesture::HANDLE_REACH)
            .min_by(|a, b| (a.1 - screen).hypot().total_cmp(&(b.1 - screen).hypot()))
            .map(|(handle, _)| (handle, selection))
    }

    /// The handles the selection shows (and takes presses on): a single
    /// line's two ends, fewer on small boxes so they stay movable.
    fn shown_handles(&self, selection: &SelectionBox) -> Vec<(Handle, Point)> {
        let radius = self.selected_radius().map(|(_, radius)| radius);
        let line = match self.starts()[..] {
            [ref start] if self.selection.with_untracked(Vec::len) == 1 => start.flip,
            _ => None,
        };
        gesture::handles_shown(selection, self.camera(self.bounds), radius, line)
    }

    /// Starts the gesture of a press on `handle`.
    fn press_handle(
        &mut self,
        handle: Handle,
        selection: SelectionBox,
        screen: Point,
        shift: bool,
    ) {
        let starts = self.starts();
        if starts.is_empty() {
            return;
        }
        let pointer = self.to_document(self.bounds, screen);
        // Where the handle itself is, so the box follows it.
        let at = self
            .shown_handles(&selection)
            .into_iter()
            .find(|(shown, _)| *shown == handle)
            .map_or(pointer, |(_, at)| self.to_document(self.bounds, at));
        let grab = at - pointer;
        let gesture = match handle {
            Handle::Resize { x, y } => Gesture::Resize {
                handle: (x, y),
                selection,
                starts,
                pointer,
                grab,
                keep_aspect: shift,
            },
            Handle::Rotate => {
                let center = selection.center();
                Gesture::Rotate {
                    center,
                    from: gesture::angle(center, pointer),
                    starts,
                    pointer,
                    snap: shift,
                }
            }
            Handle::Radius => match self.selected_radius() {
                Some((node, _)) => Gesture::Radius {
                    node,
                    selection,
                    pointer,
                    grab,
                },
                None => return,
            },
        };
        self.begin_gesture(gesture);
        self.armed = Some(screen);
    }

    /// The board's shortcuts (tooltips and menus show them).
    pub fn keymap(&self) -> &imaginarium_commands::Keymap {
        &self.keymap
    }

    /// The board's shortcuts, to change them.
    pub fn keymap_mut(&mut self) -> &mut imaginarium_commands::Keymap {
        &mut self.keymap
    }

    /// Asks to edit the one selected node's text (`text.edit`); editing
    /// starts at the next paint or input. `false` without one such node.
    pub(crate) fn request_edit(&mut self) -> bool {
        let [node] = self.selection.get_untracked()[..] else {
            return false;
        };
        self.pending_edit = Some(node);
        true
    }

    /// Forgets an edit asked for (a text inserted with its text).
    pub(crate) fn cancel_pending_edit(&mut self) {
        self.pending_edit = None;
    }

    /// Brings the board up to date with the document now (painting and
    /// input do it themselves): pending changes go onto the canvas.
    pub fn update(&mut self, fonts: &mut Fonts) {
        self.sync(fonts);
    }

    /// Runs one of the board's commands (`commands::board_commands`) on
    /// it, with the registry's availability and argument checks.
    pub fn execute(
        &mut self,
        id: &str,
        args: &imaginarium_commands::Args,
    ) -> std::result::Result<imaginarium_commands::Outcome, imaginarium_commands::CommandError>
    {
        commands::execute(self, id, args)
    }

    /// The node a click at `screen` (window space, in the board's box from
    /// the last layout or paint) would select.
    pub fn node_at(&self, screen: Point) -> Option<NodeId> {
        self.hit_at(screen)
    }

    /// Selects `nodes` (those that exist and are visible).
    pub fn select(&mut self, nodes: &[NodeId]) {
        let view = self.document.view();
        let mut chosen: Vec<NodeId> = Vec::new();
        for node in nodes {
            let visible = self
                .items
                .as_ref()
                .is_none_or(|items| items.is_visible(*node));
            if view.node(*node).is_some() && visible && !chosen.contains(node) {
                chosen.push(*node);
            }
        }
        self.selection.set_if_changed(chosen);
    }

    /// Runs `edit` as one transaction of `actor` and puts its changes on
    /// the board (`Document::transact`). Every edit of a shown document
    /// goes through here, or its changes through [`BoardCanvas::apply`].
    pub fn transact<R>(
        &mut self,
        actor: &Actor,
        edit: impl FnOnce(&mut Transaction<'_>) -> Result<R>,
    ) -> Result<R> {
        let (result, changes) = self.document.transact(actor, edit)?;
        self.apply(changes);
        Ok(result)
    }

    /// Undoes the last step of `scope`'s stack. `false` if there was none.
    pub fn undo(&mut self, scope: UndoScope, by: &Actor) -> Result<bool> {
        let changes = self.document.undo(scope, by)?;
        let undone = changes.is_some();
        if let Some(changes) = changes {
            self.apply(changes);
        }
        Ok(undone)
    }

    /// Redoes the last undone step of `scope`'s stack. `false` if there was
    /// none.
    pub fn redo(&mut self, scope: UndoScope, by: &Actor) -> Result<bool> {
        let changes = self.document.redo(scope, by)?;
        let redone = changes.is_some();
        if let Some(changes) = changes {
            self.apply(changes);
        }
        Ok(redone)
    }

    /// Applies updates from other peers (`Document::apply_remote`).
    pub fn apply_remote(&mut self, updates: &[&[u8]]) -> Result<()> {
        let changes = self.document.apply_remote(updates)?;
        self.apply(changes);
        Ok(())
    }

    /// Puts changes the document already contains on the board: only the
    /// tiles they touch are drawn again. They take effect at the next
    /// paint or input.
    pub fn apply(&mut self, changes: ChangeSet) {
        if !changes.is_empty() {
            self.pending.push(changes);
            self.revision += 1;
        }
    }

    /// A number that changes whenever the document does (local edits,
    /// undo, remote changes): panels compare it to skip work when nothing
    /// changed.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    #[cfg(test)]
    pub(crate) fn sync_for_tests(&mut self, fonts: &mut Fonts) {
        self.sync(fonts);
    }

    #[cfg(test)]
    pub(crate) fn items_for_tests(&self) -> &Items {
        self.items.as_ref().expect("synced")
    }

    #[cfg(test)]
    pub(crate) fn items_mut_for_tests(&mut self) -> &mut Items {
        self.items.as_mut().expect("synced")
    }

    #[cfg(test)]
    pub(crate) fn clipboard_work_for_tests(&self) -> &clip::ClipboardWork {
        &self.clipboard_work
    }

    #[cfg(test)]
    pub(crate) fn canvas_len_for_tests(&self) -> usize {
        self.canvas.len()
    }

    #[cfg(test)]
    pub(crate) fn canvas_item_for_tests(
        &self,
        id: imaginarium_render::canvas::ItemId,
    ) -> Option<(u64, Rect)> {
        Some((self.canvas.z(id)?, self.canvas.bounds(id)?))
    }

    #[cfg(test)]
    pub(crate) fn document_mut_for_tests(&mut self) -> &mut Document {
        &mut self.document
    }

    /// Brings the items up to date with the document: built in the
    /// current theme, with every pending change applied.
    fn sync(&mut self, fonts: &mut Fonts) {
        self.ensure_items(fonts);
        self.publish_status();
        if let Some(node) = self.pending_edit.take() {
            self.start_editing(fonts, node, None);
        }
        if self.pending.is_empty() {
            return;
        }
        let colors = BoardColors::of(&theme());
        let Some(items) = &mut self.items else {
            return;
        };
        // All at once: each set names what changed, the view is after all.
        let changes = crate::items::merge(std::mem::take(&mut self.pending));
        let update = items.apply(self.document.view(), &changes, fonts, &colors);
        for id in &update.removed {
            self.canvas.remove(*id);
        }
        for node in &update.upserted {
            if let Some(item) = items.by_node.get(node) {
                self.canvas.upsert(item.id, item.z, item.bounds);
            }
        }
        for node in &update.rekeyed {
            if let Some(item) = items.by_node.get(node) {
                self.canvas.rekey(item.id, item.z);
            }
        }
        for node in &changes.deleted {
            self.names.remove(node);
        }
        // Gone or hidden nodes leave the selection and the hover.
        let still =
            |node: &NodeId| {
                items.is_visible(*node)
                    || self.document.view().node(*node).is_some_and(|_| {
                        hit::node_box(items, self.document.view(), *node).is_some()
                    })
            };
        let selection = self.selection.get_untracked();
        if !selection.iter().all(still) {
            self.selection
                .set(selection.into_iter().filter(still).collect());
        }
        if self.hovered.is_some_and(|node| !still(&node)) {
            self.hovered = None;
        }
        // A group gone, hidden or under a hidden parent is entered no more.
        if self.entered.is_some_and(|node| !items.is_visible(node)) {
            self.entered = None;
        }
        self.sync_editing(fonts);
    }

    /// Renders tiles on `workers` instead of in the frame (ADR-0019).
    pub fn with_workers(mut self, workers: TileWorkers) -> Self {
        self.canvas = self.canvas.with_workers(workers);
        self
    }

    /// The document shown.
    pub fn document(&self) -> &Document {
        &self.document
    }

    /// The document point at the board's top left corner.
    pub fn origin(&self) -> Point {
        self.origin
    }

    /// Logical pixels per point.
    pub fn zoom(&self) -> f64 {
        self.zoom
    }

    /// Shows the board from `origin` at `zoom` (clamped to the limits).
    pub fn set_view(&mut self, origin: Point, zoom: f64) {
        self.origin = origin;
        self.zoom = zoom.clamp(MIN_ZOOM, MAX_ZOOM);
    }

    /// What the last frame's tiles cost.
    pub fn stats(&self) -> &CanvasStats {
        &self.stats
    }

    /// The document point under `screen` (logical pixels in the window),
    /// for a board whose box is `bounds`.
    pub fn to_document(&self, bounds: Rect, screen: Point) -> Point {
        self.origin + (screen - bounds.origin()) / self.zoom
    }

    /// Zooms by `factor` keeping the document point under `anchor` in place.
    pub fn zoom_about(&mut self, bounds: Rect, factor: f64, anchor: Point) {
        let before = self.to_document(bounds, anchor);
        self.zoom = (self.zoom * factor).clamp(MIN_ZOOM, MAX_ZOOM);
        let after = self.to_document(bounds, anchor);
        self.origin += before - after;
    }

    fn viewport(&self, bounds: Rect) -> Viewport {
        Viewport {
            origin: self.origin,
            zoom: self.zoom,
            screen: bounds,
            scale_factor: imaginarium_ui::scale_factor(),
        }
    }

    /// The camera: document points to the window's logical pixels.
    fn camera(&self, bounds: Rect) -> Affine {
        Affine::translate(bounds.origin().to_vec2())
            * Affine::scale(self.zoom)
            * Affine::translate(-self.origin.to_vec2())
    }

    /// Builds the items when there are none, or in a new theme.
    fn ensure_items(&mut self, fonts: &mut Fonts) {
        let current = theme();
        let stale = self
            .theme
            .as_ref()
            .is_none_or(|built| !Rc::ptr_eq(built, &current));
        if !stale && self.items.is_some() {
            return;
        }
        let colors = BoardColors::of(&current);
        // Built again, items keep their ids (and text layouts that fit).
        let mut items = self.items.take().unwrap_or_default();
        let before: Vec<imaginarium_render::canvas::ItemId> =
            items.by_node.values().map(|item| item.id).collect();
        items.rebuild(self.document.view(), fonts, &colors);
        for id in before {
            if !items
                .by_item
                .get(&id)
                .is_some_and(|node| items.by_node.contains_key(node))
            {
                self.canvas.remove(id);
            }
        }
        for item in items.by_node.values() {
            self.canvas.upsert(item.id, item.z, item.bounds);
        }
        self.names.clear();
        // A build reads the whole view, pending changes included.
        self.pending.clear();
        self.items = Some(items);
        self.theme = Some(current);
    }

    /// Paints the board into `bounds` (window coordinates) as its widget
    /// does: the background, the tiles (directly in the first frame) and
    /// the frame names. For the widget, exports and tests.
    pub fn paint_into(&mut self, fonts: &mut Fonts, scene: &mut Scene, bounds: Rect, now: Instant) {
        // In a new theme every tile is wrong: drop them rather than show
        // them as stand-ins, and draw the first frame directly.
        let current = theme();
        if self.items.is_some()
            && self
                .theme
                .as_ref()
                .is_some_and(|built| !Rc::ptr_eq(built, &current))
        {
            self.canvas.release(scene);
            self.painted = 0;
        }
        // A gesture's pointer moves since the last frame become one command.
        self.sync(fonts);
        self.flush_gesture();
        self.sync(fonts);
        scene.push_clip(&bounds);
        scene.fill(&bounds, theme().color(Token::BoardBg));
        self.paint_dots(scene, bounds);
        let viewport = self.viewport(bounds);
        let mut stats = CanvasStats::default();
        if let Some(items) = &self.items {
            let mut painter = ItemPainter::new(items, &[]);
            painter.editing = self.editing.as_ref().map(|editing| editing.node);
            // Rendering every visible tile would delay the first frame:
            // draw it directly, tiles from the next one.
            stats = if self.painted == 0 {
                self.canvas.draw_direct(scene, &viewport, &mut painter)
            } else {
                self.canvas.frame(scene, &viewport, now, &mut painter)
            };
        }
        let owed = stats.owed > 0 || self.painted == 0;
        self.painted += 1;
        self.stats = stats;
        self.owed_at = owed.then_some(now);
        self.paint_names(fonts, scene, bounds);
        self.paint_selection(fonts, scene, bounds);
        self.paint_editing(fonts, scene);
        scene.pop();
        self.bounds = bounds;
    }

    /// The color of selection outlines and handles: the accent, lighter
    /// in the dark theme (the mockups).
    fn selection_color() -> imaginarium_render::peniko::Color {
        let theme = theme();
        let accent = theme.color(Token::Accent);
        match theme.kind() {
            imaginarium_ui::ThemeKind::Dark => {
                mix(accent, imaginarium_render::peniko::Color::WHITE, 0.35)
            }
            imaginarium_ui::ThemeKind::Light => accent,
        }
    }

    /// The outline of a node's box in window space: its item's turned box,
    /// a group's bounding box.
    fn node_outline(
        &self,
        node: NodeId,
        bounds: Rect,
    ) -> Option<imaginarium_render::kurbo::BezPath> {
        let items = self.items.as_ref()?;
        let camera = self.camera(bounds);
        if let Some(item) = items.by_node.get(&node) {
            let rect = Rect::from_origin_size(Point::ZERO, item.size);
            return Some(
                camera * item.world * imaginarium_render::kurbo::Shape::to_path(&rect, 0.1),
            );
        }
        let rect = hit::node_box(items, self.document.view(), node)?;
        Some(camera * imaginarium_render::kurbo::Shape::to_path(&rect, 0.1))
    }

    /// The hover outline, the selection's outlines and the marquee, on top
    /// of the tiles.
    fn paint_selection(&self, fonts: &mut Fonts, scene: &mut Scene, bounds: Rect) {
        let color = Self::selection_color();
        let selection = self.selection.get_untracked();
        self.paint_handles(fonts, scene, bounds, color);
        if let Some(hovered) = self.hovered
            && !selection.contains(&hovered)
            && self.press.is_none()
            && let Some(outline) = self.node_outline(hovered, bounds)
        {
            scene.stroke_with(&outline, &Stroke::new(1.5), color.with_alpha(0.6));
        }
        for node in &selection {
            if let Some(outline) = self.node_outline(*node, bounds) {
                scene.stroke_with(&outline, &Stroke::new(1.5), color);
            }
        }
        if let Some(Press::Marquee { start, current, .. }) = self.press {
            let camera = self.camera(bounds);
            let area = Rect::from_points(camera * start, camera * current);
            scene.fill(&area, color.with_alpha(0.08));
            scene.stroke_with(&area, &Stroke::new(1.0), color.with_alpha(0.8));
        }
    }

    /// The selection's box and handles, and its size while resizing.
    fn paint_handles(
        &self,
        fonts: &mut Fonts,
        scene: &mut Scene,
        bounds: Rect,
        color: imaginarium_render::peniko::Color,
    ) {
        use imaginarium_render::kurbo::{Circle, RoundedRect, Shape};
        let Some(selection) = self.selection_box() else {
            return;
        };
        let camera = self.camera(bounds);
        let knob = theme().color(Token::BoardBg);
        if self
            .selection
            .with_untracked(|selection| selection.len() > 1)
        {
            let rect = Rect::from_origin_size(Point::ZERO, selection.size);
            let outline = camera * selection.world * rect.to_path(0.1);
            scene.stroke_with(
                &outline,
                &Stroke::new(1.0).with_dashes(0.0, [4.0, 3.0]),
                color,
            );
        }
        // While the selection moves, only its outlines show.
        if matches!(self.gesture, Some(Gesture::Move { .. })) {
            return;
        }
        let line = Stroke::new(1.5);
        for (handle, at) in self.shown_handles(&selection) {
            match handle {
                Handle::Resize { .. } => {
                    let half = gesture::HANDLE_SIZE / 2.0 - 0.75;
                    let square = RoundedRect::from_rect(
                        Rect::from_center_size(at, (2.0 * half, 2.0 * half)),
                        2.0,
                    );
                    scene.fill(&square, knob);
                    scene.stroke_with(&square, &line, color);
                }
                Handle::Rotate | Handle::Radius => {
                    let size = if handle == Handle::Rotate {
                        gesture::ROTATE_SIZE
                    } else {
                        gesture::RADIUS_SIZE
                    };
                    let circle = Circle::new(at, size / 2.0 - 0.75);
                    scene.fill(&circle, knob);
                    scene.stroke_with(&circle, &line, color);
                }
            }
        }
        // The size, under the box while resizing.
        if matches!(self.gesture, Some(Gesture::Resize { .. })) {
            let size = selection.size;
            let label = format!("{} × {}", size.width.round(), size.height.round());
            let style = TextStyle::new(11.0)
                .weight(600.0)
                .color(theme().color(Token::OnAccent));
            let block = TextBlock::new(fonts, &label, &style, None);
            let text = block.size();
            let screen = camera * selection.world;
            let corners = [
                Point::ZERO,
                Point::new(size.width, 0.0),
                Point::new(0.0, size.height),
                Point::new(size.width, size.height),
            ]
            .map(|corner| screen * corner);
            let bottom = corners
                .iter()
                .map(|corner| corner.y)
                .fold(f64::MIN, f64::max);
            let left = corners
                .iter()
                .map(|corner| corner.x)
                .fold(f64::MAX, f64::min);
            let right = corners
                .iter()
                .map(|corner| corner.x)
                .fold(f64::MIN, f64::max);
            let pill = Rect::from_center_size(
                Point::new((left + right) / 2.0, bottom + 20.0),
                (text.width + 14.0, 20.0),
            );
            scene.fill(
                &RoundedRect::from_rect(pill, 5.0),
                theme().color(Token::Accent),
            );
            block.paint(
                scene,
                Point::new(pill.x0 + 7.0, pill.center().y - text.height / 2.0),
                true,
            );
        }
    }

    /// What the pointer at `screen` (window space) would select.
    fn hit_at(&self, screen: Point) -> Option<NodeId> {
        let items = self.items.as_ref()?;
        let view = self.document.view();
        // A frame's name, drawn on top, selects the frame.
        // (Only frames still on the board: the names are from the last
        // paint, and a change since may have removed one.)
        if let Some((frame, _)) = self
            .name_rects
            .iter()
            .rev()
            .find(|(frame, rect)| rect.contains(screen) && items.frames.contains(frame))
        {
            return (!hit::locked(view, *frame))
                .then(|| hit::selectable(view, *frame, self.entered));
        }
        let point = self.to_document(self.bounds, screen);
        let tolerance = hit::TOLERANCE_PIXELS / self.zoom;
        // Items within the tolerance count: a thin line's bounds are
        // narrower than it.
        let near = Rect::from_center_size(point, (2.0 * tolerance, 2.0 * tolerance));
        let mut candidates = self.canvas.items_overlapping(near);
        candidates.reverse();
        hit::hit(items, view, &candidates, point, tolerance, self.entered)
    }

    /// The nodes a marquee from `start` to `end` (document space) selects.
    fn marquee_hits(&self, start: Point, end: Point) -> Vec<NodeId> {
        let Some(items) = &self.items else {
            return Vec::new();
        };
        let area = Rect::from_points(start, end);
        let candidates = self.canvas.items_overlapping(area);
        hit::hit_rect(items, self.document.view(), &candidates, area, self.entered)
    }

    /// The nodes Tab steps through: what a click on each item would
    /// select, in z order.
    fn tab_order(&self) -> Vec<NodeId> {
        let Some(items) = &self.items else {
            return Vec::new();
        };
        let view = self.document.view();
        let open = hit::open_groups(view, self.entered);
        let mut seen = std::collections::HashSet::new();
        let mut order: Vec<NodeId> = Vec::new();
        for node in view.walk() {
            if items.by_node.contains_key(&node) && !hit::locked(view, node) {
                let unit = hit::selectable_in(view, node, &open);
                if seen.insert(unit) {
                    order.push(unit);
                }
            }
        }
        order
    }

    fn pointer(&mut self, cx: &mut Cx<'_>, input: &Input) -> bool {
        match input {
            Input::PointerButton {
                primary: true,
                pressed: true,
                position,
                clicks,
                modifiers,
            } => {
                cx.request_focus();
                cx.capture_pointer();
                let at = self.to_document(cx.bounds(), *position);
                match self.tool.get_untracked() {
                    Tool::Select => {}
                    Tool::Hand => {
                        self.begin_gesture(Gesture::Pan {
                            last: *position,
                            pointer: *position,
                        });
                        return true;
                    }
                    Tool::Text => {
                        // On a text box or a shape, the text tool edits it.
                        if let Some(node) = self.hit_at(*position)
                            && self.document.view().node(node).is_some_and(|node| {
                                matches!(node.kind(), NodeKind::Text | NodeKind::Shape)
                            })
                        {
                            self.set_tool(Tool::Select);
                            cx.release_pointer();
                            self.start_editing(cx.fonts(), node, Some((*position, 1)));
                            cx.request_paint();
                            return true;
                        }
                        let args = commands::text_args(at);
                        let _ = commands::execute(self, "board.insert_text", &args);
                        self.set_tool(Tool::Select);
                        self.sync(cx.fonts());
                        cx.release_pointer();
                        cx.request_paint();
                        return true;
                    }
                    tool @ (Tool::Shape(_) | Tool::Frame) => {
                        let preset = match tool {
                            Tool::Shape(preset) => Some(preset),
                            _ => None,
                        };
                        self.selection.set_if_changed(Vec::new());
                        self.begin_gesture(Gesture::Draw {
                            preset,
                            origin: at,
                            pointer: at,
                            node: None,
                        });
                        self.armed = Some(*position);
                        cx.request_paint();
                        return true;
                    }
                }
                if *clicks == 1
                    && let Some((handle, selection)) = self.handle_at(*position)
                {
                    self.press_handle(handle, selection, *position, modifiers.shift);
                    cx.request_paint();
                    return true;
                }
                let hit = self.hit_at(*position);
                let mut selection = self.selection.get_untracked();
                match hit {
                    // A double-click on a group goes into it.
                    Some(node)
                        if *clicks >= 2
                            && self
                                .document
                                .view()
                                .node(node)
                                .is_some_and(|node| matches!(node.kind(), NodeKind::Group)) =>
                    {
                        self.entered = Some(node);
                        let inner = self.hit_at(*position).unwrap_or(node);
                        self.select(&[inner]);
                        self.press = Some(Press::Node {
                            at: *position,
                            narrow: None,
                        });
                    }
                    // A double-click on a text box or shape edits its text.
                    Some(node)
                        if *clicks >= 2
                            && self.document.view().node(node).is_some_and(|node| {
                                matches!(node.kind(), NodeKind::Text | NodeKind::Shape)
                            }) =>
                    {
                        cx.release_pointer();
                        self.start_editing(cx.fonts(), node, Some((*position, 1)));
                    }
                    Some(node) => {
                        // A click outside the entered group leaves it.
                        if let Some(group) = self.entered
                            && node != group
                            && !hit::is_inside(self.document.view(), node, group)
                        {
                            self.entered = None;
                        }
                        let narrow =
                            (!modifiers.shift && selection.len() > 1 && selection.contains(&node))
                                .then_some(node);
                        if modifiers.shift {
                            if let Some(at) =
                                selection.iter().position(|selected| *selected == node)
                            {
                                selection.remove(at);
                            } else {
                                selection.push(node);
                            }
                            self.selection.set(selection);
                        } else if !selection.contains(&node) {
                            self.selection.set(vec![node]);
                        }
                        self.press = Some(Press::Node {
                            at: *position,
                            narrow,
                        });
                    }
                    None => {
                        // Empty board: out of an entered group, and a marquee.
                        if self.entered.take().is_some() || !modifiers.shift {
                            self.selection.set_if_changed(Vec::new());
                        }
                        let start = self.to_document(cx.bounds(), *position);
                        self.press = Some(Press::Marquee {
                            start,
                            current: start,
                            additive: modifiers.shift,
                        });
                    }
                }
                cx.request_paint();
                true
            }
            Input::PointerMoved(position) => {
                let pointer = self.to_document(cx.bounds(), *position);
                if let Some(gesture) = &mut self.gesture {
                    match gesture {
                        Gesture::Move { pointer: at, .. }
                        | Gesture::Draw { pointer: at, .. }
                        | Gesture::Resize { pointer: at, .. }
                        | Gesture::Rotate { pointer: at, .. }
                        | Gesture::Radius { pointer: at, .. } => *at = pointer,
                        // The hand pans in window space.
                        Gesture::Pan { pointer: at, .. } => *at = *position,
                    }
                    if self
                        .armed
                        .is_some_and(|pressed| (*position - pressed).hypot() >= DRAG_THRESHOLD)
                    {
                        self.armed = None;
                    }
                    cx.request_paint();
                    return true;
                }
                if let Some(Press::Node { at, .. }) = self.press
                    && (*position - at).hypot() >= DRAG_THRESHOLD
                    && !self.starts().is_empty()
                {
                    let from = self.to_document(cx.bounds(), at);
                    self.press = None;
                    self.hovered = None;
                    self.begin_gesture(Gesture::Move {
                        applied: from,
                        pointer,
                    });
                    cx.request_paint();
                    return true;
                }
                if let Some(Press::Marquee {
                    start, additive, ..
                }) = self.press
                {
                    let current = self.to_document(cx.bounds(), *position);
                    self.press = Some(Press::Marquee {
                        start,
                        current,
                        additive,
                    });
                    cx.request_paint();
                    return true;
                }
                let hovered = if self.tool.get_untracked() == Tool::Select {
                    self.hit_at(*position)
                } else {
                    None
                };
                if hovered != self.hovered {
                    self.hovered = hovered;
                    cx.request_paint();
                }
                false
            }
            Input::PointerButton {
                primary: true,
                pressed: false,
                ..
            } => {
                cx.release_pointer();
                self.end_gesture();
                if let Some(Press::Node {
                    narrow: Some(node), ..
                }) = self.press
                {
                    self.select(&[node]);
                }
                if let Some(Press::Marquee {
                    start,
                    current,
                    additive,
                }) = self.press.take()
                {
                    let found = self.marquee_hits(start, current);
                    let mut selection = if additive {
                        self.selection.get_untracked()
                    } else {
                        Vec::new()
                    };
                    for node in found {
                        if !selection.contains(&node) {
                            selection.push(node);
                        }
                    }
                    self.selection.set_if_changed(selection);
                }
                self.press = None;
                cx.request_paint();
                true
            }
            Input::PointerLeft => {
                if self.hovered.take().is_some() {
                    cx.request_paint();
                }
                false
            }
            _ => false,
        }
    }

    fn key(&mut self, cx: &mut Cx<'_>, input: &Input) -> bool {
        let Input::Key {
            key,
            pressed: true,
            modifiers,
            ..
        } = input
        else {
            return false;
        };
        // Shortcuts run the board's commands (the keymap); a command that
        // can't run now leaves the key to the rest.
        if let Some(press) = crate::keys::key_press(key, *modifiers)
            && let Some(command) = self.keymap.command_for_press(press)
            && commands::execute(self, command.as_str(), &imaginarium_commands::Args::new()).is_ok()
        {
            self.hand_over(cx.clipboard());
            self.sync(cx.fonts());
            cx.request_paint();
            return true;
        }
        match key {
            Key::Escape if self.tool.get_untracked() != Tool::Select => {
                self.set_tool(Tool::Select);
                cx.request_paint();
                true
            }
            Key::Escape => {
                let had = !self.selection.get_untracked().is_empty() || self.entered.is_some();
                // Out of an entered group, with the group selected.
                match self.entered.take() {
                    Some(group) => self.select(&[group]),
                    None => {
                        self.selection.set_if_changed(Vec::new());
                    }
                }
                cx.request_paint();
                had
            }
            Key::Tab if !modifiers.command() => {
                let order = self.tab_order();
                if order.is_empty() {
                    return false;
                }
                let current = self.selection.get_untracked().last().copied();
                let at = current.and_then(|node| order.iter().position(|unit| *unit == node));
                let next = match (at, modifiers.shift) {
                    (Some(at), false) => Some(at + 1).filter(|next| *next < order.len()),
                    (Some(at), true) => at.checked_sub(1),
                    (None, false) => Some(0),
                    (None, true) => Some(order.len() - 1),
                };
                cx.request_paint();
                match next {
                    Some(next) => {
                        self.selection.set(vec![order[next]]);
                        true
                    }
                    // Past the last node (or before the first) the focus
                    // moves on out of the board, as Tab does everywhere.
                    None => {
                        self.selection.set_if_changed(Vec::new());
                        false
                    }
                }
            }
            _ => false,
        }
    }

    fn paint_dots(&mut self, scene: &mut Scene, bounds: Rect) {
        // At least this far apart on screen, so a screenful stays a few
        // thousand dots.
        let mut pitch = DOT_PITCH;
        while pitch * self.zoom < 16.0 {
            pitch *= 2.0;
        }
        let step = pitch * self.zoom;
        let start = self.origin;
        let first = Point::new(
            bounds.x0 + ((start.x / pitch).ceil() * pitch - start.x) * self.zoom,
            bounds.y0 + ((start.y / pitch).ceil() * pitch - start.y) * self.zoom,
        );
        // One path of every dot, built again only when the view moves.
        let key = (bounds, first, step);
        if self.dots.as_ref().is_none_or(|(built, _)| *built != key) {
            let mut path = imaginarium_render::kurbo::BezPath::new();
            let mut y = first.y;
            while y < bounds.y1 {
                let mut x = first.x;
                while x < bounds.x1 {
                    path.extend(imaginarium_render::kurbo::Shape::path_elements(
                        &imaginarium_render::kurbo::Circle::new((x, y), 1.2),
                        0.1,
                    ));
                    x += step;
                }
                y += step;
            }
            self.dots = Some((key, path));
        }
        if let Some((_, path)) = &self.dots {
            scene.fill(path, theme().color(Token::BoardDot));
        }
    }

    /// Frame names, above their frames at the interface's size.
    fn paint_names(&mut self, fonts: &mut Fonts, scene: &mut Scene, bounds: Rect) {
        let Some(items) = &self.items else {
            return;
        };
        let camera = self.camera(bounds);
        let color = theme().color(Token::TextMuted);
        let style = TextStyle::new(13.0).weight(500.0).color(color);
        self.name_rects.clear();
        let mut frames: Vec<(u64, NodeId)> = items
            .frames
            .iter()
            .filter_map(|node| Some((items.by_node.get(node)?.z, *node)))
            .collect();
        frames.sort_unstable();
        for (_, node) in &frames {
            let Some(item) = items.by_node.get(node) else {
                continue;
            };
            let Some(name) = &item.name else {
                continue;
            };
            // A frame its parent frame clips away entirely has no name.
            if item.bounds.width() <= 0.0 || item.bounds.height() <= 0.0 {
                continue;
            }
            let corner = camera * (item.world * Point::ZERO);
            if !bounds.inflate(0.0, 40.0).contains(corner)
                && !item.bounds.overlaps(self.viewport(bounds).visible())
            {
                continue;
            }
            let fresh = self.names.get(node).is_none_or(|(text, _)| text != name);
            if fresh {
                let block = TextBlock::new(fonts, name, &style, None);
                self.names.insert(*node, (name.clone(), block));
            }
            if let Some((_, block)) = self.names.get(node) {
                let origin = corner - Vec2::new(0.0, block.size().height + NAME_GAP);
                block.paint(scene, origin, true);
                self.name_rects
                    .push((*node, Rect::from_origin_size(origin, block.size())));
            }
        }
    }
}

impl Widget for BoardCanvas {
    fn style(&self) -> taffy::Style {
        taffy::Style {
            flex_grow: 1.0,
            size: taffy::Size {
                width: taffy::prelude::percent(1.0),
                height: taffy::prelude::percent(1.0),
            },
            ..taffy::Style::default()
        }
    }

    fn paint(&mut self, cx: &mut Cx<'_>, scene: &mut Scene) {
        // A command run from elsewhere (the palette, the AI) may have left
        // clipboard work.
        self.hand_over(cx.clipboard());
        let bounds = cx.bounds();
        self.paint_into(cx.fonts(), scene, bounds, Instant::now());
    }

    fn focusable(&self) -> bool {
        true
    }

    fn event(&mut self, cx: &mut Cx<'_>, input: &Input) -> bool {
        // Input reads the items (hit testing): bring them up to date.
        self.sync(cx.fonts());
        self.bounds = cx.bounds();
        self.hand_over(cx.clipboard());
        if let Some(handled) = self.edit_input(cx, input) {
            return handled;
        }
        // What the clipboard gave for a paste the board asked for.
        if let Input::Paste(pasted) = input {
            let _ = self.paste(pasted);
            self.sync(cx.fonts());
            cx.request_paint();
            return true;
        }
        if self.pointer(cx, input) || self.key(cx, input) {
            return true;
        }
        match input {
            Input::Scroll {
                delta,
                position,
                modifiers,
            } => {
                // The names move with the view: none can be clicked until
                // the next paint places them.
                self.name_rects.clear();
                if modifiers.command() {
                    // Ctrl + wheel zooms around the pointer.
                    let factor = (delta.y / 240.0).exp();
                    self.zoom_about(cx.bounds(), factor, *position);
                } else {
                    self.origin -= *delta / self.zoom;
                }
                cx.request_paint();
                true
            }
            _ => false,
        }
    }

    fn wake_at(&self) -> Option<Instant> {
        let canvas = self.canvas.wake_at(&Viewport {
            origin: self.origin,
            zoom: self.zoom,
            screen: Rect::ZERO,
            scale_factor: imaginarium_ui::scale_factor(),
        });
        [self.owed_at, canvas, self.editing_wake_at()]
            .into_iter()
            .flatten()
            .min()
    }

    fn ime_area(&mut self, cx: &mut Cx<'_>) -> Option<Rect> {
        self.editing_ime_area(cx.fonts())
    }

    fn wake(&mut self, cx: &mut Cx<'_>, _now: Instant) {
        self.owed_at = None;
        cx.request_paint();
    }

    fn focus_changed(&mut self, cx: &mut Cx<'_>, focused: bool) {
        // Clicking elsewhere (a panel, a search box) ends editing.
        if !focused && self.editing.is_some() {
            self.stop_editing(cx.fonts());
            cx.request_paint();
        }
    }

    fn capture_lost(&mut self, cx: &mut Cx<'_>) {
        let gesture = self.gesture.is_some();
        self.end_gesture();
        if self.press.take().is_some() || gesture {
            cx.request_paint();
        }
    }

    fn accessibility(&mut self, cx: &mut Cx<'_>) -> Option<Node> {
        let bounds = cx.bounds();
        let mut board = Node::new(Role::Canvas);
        board.set_label("Board");
        board.set_bounds(access_rect(bounds));
        board.set_clips_children();
        let Some(items) = &self.items else {
            return Some(board);
        };
        let visible = self.viewport(bounds).visible();
        let mut shown: Vec<(u64, NodeId)> = self
            .canvas
            .items_overlapping(visible)
            .into_iter()
            .filter_map(|id| {
                let node = *items.by_item.get(&id)?;
                Some((items.by_node.get(&node)?.z, node))
            })
            .collect();
        shown.sort_unstable();
        shown.truncate(MAX_ACCESS_ITEMS);
        let base = cx.id().access_base();
        let camera = self.camera(bounds);
        let selection = self.selection.get_untracked();
        let focused = cx.is_focused();
        let view = self.document.view();
        for (_, node_id) in shown {
            let (Some(item), Some(node)) = (items.by_node.get(&node_id), view.node(node_id)) else {
                continue;
            };
            // The item's id, which it keeps for its life on the board: a
            // screen reader's node stays the same as others come and go.
            if item.id.0 == 0 || item.id.0 >= FIELD_ACCESS.min(ACCESS_LOCAL_IDS) {
                continue;
            }
            let id = accesskit::NodeId(base + item.id.0);
            let role = match node.kind() {
                NodeKind::Text => Role::Paragraph,
                NodeKind::Frame => Role::Group,
                _ => Role::GraphicsObject,
            };
            let mut access = Node::new(role);
            let text = node
                .text()
                .map(|text| text.to_plain_string())
                .unwrap_or_default();
            let label = node
                .name()
                .map(str::to_owned)
                .or_else(|| (!text.trim().is_empty()).then(|| text.trim().to_owned()))
                .unwrap_or_else(|| describe(node));
            access.set_label(label);
            let rect = Rect::from_origin_size(Point::ZERO, item.size);
            access.set_bounds(access_rect(
                camera.transform_rect_bbox(item.world.transform_rect_bbox(rect)),
            ));
            access.add_action(Action::Focus);
            if selection.contains(&node_id) {
                access.set_selected(true);
            }
            if let Some(tree) = cx.access_tree() {
                tree.push(id, access);
                if focused && selection.last() == Some(&node_id) {
                    tree.set_focus(id);
                }
            }
            cx.add_access_child(id);
        }
        self.editing_access(cx, base, focused);
        Some(board)
    }

    fn access_action(&mut self, cx: &mut Cx<'_>, request: &ActionRequest) -> bool {
        // Focusing an item's node selects it.
        let local = request.target_node.0.wrapping_sub(cx.id().access_base());
        if let Some(handled) = self.editing_access_action(cx, request, local) {
            return handled;
        }
        if request.action != Action::Focus || local == 0 {
            return false;
        }
        let Some(node) = self.items.as_ref().and_then(|items| {
            let node = *items
                .by_item
                .get(&imaginarium_render::canvas::ItemId(local))?;
            items.by_node.contains_key(&node).then_some(node)
        }) else {
            return false;
        };
        self.select(&[node]);
        cx.request_focus();
        cx.request_paint();
        true
    }
}

/// What a node is, for screen readers when it has no name or text.
fn describe(node: &imaginarium_model::view::NodeView) -> String {
    match node.kind() {
        NodeKind::Shape => match node
            .prop("shape")
            .and_then(imaginarium_model::Value::as_str)
        {
            Some("ellipse") => "Ellipse",
            Some("line") => "Line",
            Some("triangle") => "Triangle",
            Some("diamond") => "Diamond",
            _ => "Rectangle",
        }
        .to_owned(),
        NodeKind::Text => "Text".to_owned(),
        NodeKind::Frame => "Frame".to_owned(),
        other => other.as_str().to_owned(),
    }
}
