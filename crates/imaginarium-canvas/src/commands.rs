// SPDX-License-Identifier: AGPL-3.0-or-later

//! The board's commands (`canvas.md` §6): gestures, keys, the palette and
//! the AI call the same ones. Their context is the board.

use std::cell::RefCell;

use imaginarium_commands::{
    ArgKind, ArgSpec, ArgValue, Args, ArgsSchema, Availability, CommandError, CommandMeta,
    FnCommand, Outcome, Registry,
};
use imaginarium_model::{Actor, Geometry, ModelError, NodeId, Value};

use crate::BoardCanvas;

/// The longest move or largest box a command accepts, in points: far
/// beyond any board, and well inside what `f64` keeps exact.
const LIMIT: f64 = 1.0e7;

const fn float(
    name: &'static str,
    description: &'static str,
    min: f64,
    max: f64,
    required: bool,
) -> ArgSpec {
    ArgSpec {
        name,
        description,
        kind: ArgKind::Float {
            min: Some(min),
            max: Some(max),
        },
        required,
    }
}

fn failed(id: &'static str) -> impl Fn(ModelError) -> CommandError {
    move |err| CommandError::Failed {
        id,
        message: err.to_string(),
    }
}

fn locked(id: &'static str) -> CommandError {
    CommandError::InvalidArgs {
        id,
        reason: "the node is locked".into(),
    }
}

fn has_selection(board: &BoardCanvas) -> Availability {
    if board.selection().with_untracked(Vec::is_empty) {
        Availability::Disabled("Select something first")
    } else {
        Availability::Enabled
    }
}

/// `board.move`: moves the selection.
pub static MOVE: FnCommand<BoardCanvas> = FnCommand::<BoardCanvas>::new(
    "board.move",
    CommandMeta::new(
        "Move",
        "Move the selected nodes by dx and dy points on the board (positive dx is right, positive dy is down).",
    )
    .category("Arrange")
    .synonyms(&["nudge", "shift", "translate", "verschieben"])
    .icon("move"),
    |board, args| {
        let dx = args.float("dx").unwrap_or(0.0);
        let dy = args.float("dy").unwrap_or(0.0);
        if dx == 0.0 && dy == 0.0 {
            return Ok(Outcome::Unchanged);
        }
        let moved = board.move_selection(dx, dy).map_err(failed("board.move"))?;
        Ok(if moved { Outcome::Done } else { Outcome::Unchanged })
    },
)
.args(ArgsSchema {
    args: &[
        float("dx", "Points to move right (negative: left).", -LIMIT, LIMIT, false),
        float("dy", "Points to move down (negative: up).", -LIMIT, LIMIT, false),
    ],
})
.availability(has_selection);

/// `board.set_box`: places one node's box.
pub static SET_BOX: FnCommand<BoardCanvas> = FnCommand::<BoardCanvas>::new(
    "board.set_box",
    CommandMeta::new(
        "Set position and size",
        "Set a node's box in its parent's space: its left x, top y, width w and height h in points, and its clockwise rotation in degrees around the box's center.",
    )
    .category("Arrange")
    .synonyms(&["resize", "rotate", "position", "size", "transform"])
    .icon("frame"),
    |board, args| {
        let node = args
            .str("node")
            .and_then(NodeId::parse)
            .ok_or_else(|| CommandError::InvalidArgs {
                id: "board.set_box",
                reason: "`node` is not a node id".into(),
            })?;
        let current = board
            .document()
            .view()
            .node(node)
            .ok_or_else(|| CommandError::InvalidArgs {
                id: "board.set_box",
                reason: "no such node".into(),
            })?
            .geometry()
            .unwrap_or_default();
        let geometry = Geometry {
            x: args.float("x").unwrap_or(current.x),
            y: args.float("y").unwrap_or(current.y),
            w: args.float("w").unwrap_or(current.w),
            h: args.float("h").unwrap_or(current.h),
            r: args.float("rotation").unwrap_or(current.r),
        };
        if crate::hit::locked(board.document().view(), node) {
            return Err(locked("board.set_box"));
        }
        let flip = args.str("flip").map(|flip| match flip {
            "none" => Value::Null,
            other => Value::String(other.into()),
        });
        let flip_changes = flip.as_ref().is_some_and(|flip| {
            board.document().view().node(node).and_then(|node| node.prop("flip")).unwrap_or(&Value::Null) != flip
        });
        if geometry == current && !flip_changes {
            return Ok(Outcome::Unchanged);
        }
        board
            .transact(&Actor::User, |txn| {
                txn.set_geometry(node, geometry)?;
                if let Some(flip) = flip.filter(|_| flip_changes) {
                    txn.set_prop(node, "flip", flip)?;
                }
                Ok(())
            })
            .map_err(failed("board.set_box"))?;
        Ok(Outcome::Done)
    },
)
.args(ArgsSchema {
    args: &[
        ArgSpec {
            name: "node",
            description: "The node to place.",
            kind: ArgKind::Ref { target: "node" },
            required: true,
        },
        float("x", "Left edge in points, in the parent's space.", -LIMIT, LIMIT, false),
        float("y", "Top edge in points, in the parent's space.", -LIMIT, LIMIT, false),
        float("w", "Width in points.", 0.0, LIMIT, false),
        float("h", "Height in points.", 0.0, LIMIT, false),
        float("rotation", "Clockwise rotation in degrees around the box's center.", -3600.0, 3600.0, false),
        ArgSpec {
            name: "flip",
            description: "For a line: which corners it joins (none: top left to bottom right; h: mirrored horizontally; v: vertically; hv: both).",
            kind: ArgKind::Enum {
                variants: &["none", "h", "v", "hv"],
            },
            required: false,
        },
    ],
});

/// `shape.set_radius`: rounds a rectangle's corners.
pub static SET_RADIUS: FnCommand<BoardCanvas> = FnCommand::<BoardCanvas>::new(
    "shape.set_radius",
    CommandMeta::new(
        "Corner radius",
        "Set the corner radius of a rectangle in points: 0 for sharp corners, up to half its shorter side for round ends.",
    )
    .category("Style")
    .synonyms(&["round corners", "rounded", "radius", "abrunden"])
    .icon("square"),
    |board, args| {
        let node = args
            .str("node")
            .and_then(NodeId::parse)
            .ok_or_else(|| CommandError::InvalidArgs {
                id: "shape.set_radius",
                reason: "`node` is not a node id".into(),
            })?;
        let view = board.document().view();
        let shape = view.node(node).filter(|shape| {
            matches!(shape.kind(), imaginarium_model::NodeKind::Shape)
                && shape.prop("shape").and_then(Value::as_str).is_none_or(|preset| preset == "rect")
        });
        let Some(shape) = shape else {
            return Err(CommandError::InvalidArgs {
                id: "shape.set_radius",
                reason: "only a rectangle has a corner radius".into(),
            });
        };
        if crate::hit::locked(view, node) {
            return Err(locked("shape.set_radius"));
        }
        // Up to half the shorter side, as the description says.
        let size = shape.geometry().unwrap_or_default();
        let limit = (size.w.min(size.h) / 2.0).max(0.0);
        let radius = args.float("radius").unwrap_or(0.0).min(limit);
        let current = board
            .document()
            .view()
            .style_prop(node, "radius")
            .and_then(Value::as_number)
            .unwrap_or(0.0);
        if current == radius {
            return Ok(Outcome::Unchanged);
        }
        board
            .transact(&Actor::User, |txn| txn.set_style_override(node, "radius", Value::Number(radius)))
            .map_err(failed("shape.set_radius"))?;
        Ok(Outcome::Done)
    },
)
.args(ArgsSchema {
    args: &[
        ArgSpec {
            name: "node",
            description: "The rectangle.",
            kind: ArgKind::Ref { target: "node" },
            required: true,
        },
        float("radius", "The corner radius in points.", 0.0, LIMIT, true),
    ],
});

/// `text.replace`: replaces characters of a node's text.
pub static REPLACE_TEXT: FnCommand<BoardCanvas> = FnCommand::<BoardCanvas>::new(
    "text.replace",
    CommandMeta::new(
        "Replace text",
        "Replace `delete` characters at position `at` of a text box's or shape's text with `insert` (plain text; positions count Unicode characters from 0).",
    )
    .category("Text")
    .synonyms(&["type", "insert text", "delete text", "edit text"])
    .icon("type"),
    |board, args| {
        let node = args
            .str("node")
            .and_then(NodeId::parse)
            .ok_or_else(|| CommandError::InvalidArgs {
                id: "text.replace",
                reason: "`node` is not a node id".into(),
            })?;
        let at = usize::try_from(args.int("at").unwrap_or(0)).unwrap_or(0);
        let delete = usize::try_from(args.int("delete").unwrap_or(0)).unwrap_or(0);
        // Line breaks are \n, as the editor writes them.
        let insert = args.str("insert").unwrap_or("").replace("\r\n", "\n").replace('\r', "\n");
        if delete == 0 && insert.is_empty() {
            return Ok(Outcome::Unchanged);
        }
        board
            .transact(&Actor::User, |txn| {
                let len = txn.text_len(node)?;
                // The text without its final newline (schema §6).
                let shown = len.saturating_sub(1);
                if at + delete > shown {
                    return Err(ModelError::OutOfRange {
                        what: "text range end",
                        value: at + delete,
                        len: shown,
                    });
                }
                if delete > 0 {
                    // All of it: the final newline goes too.
                    let whole = at == 0 && delete == shown && insert.is_empty();
                    txn.delete_text(node, at, if whole { len } else { delete })?;
                }
                if !insert.is_empty() {
                    // Into an empty text, a final line break is the text's
                    // own and a typed one needs another (schema §6): the
                    // model adds the final one only when it is missing.
                    let len = txn.text_len(node)?;
                    if len == 0 && insert.ends_with('\n') {
                        txn.insert_text(node, at, &format!("{insert}\n"))?;
                    } else {
                        txn.insert_text(node, at, &insert)?;
                    }
                }
                Ok(())
            })
            .map_err(failed("text.replace"))?;
        Ok(Outcome::Done)
    },
)
.args(ArgsSchema {
    args: &[
        ArgSpec {
            name: "node",
            description: "The text box or shape.",
            kind: ArgKind::Ref { target: "node" },
            required: true,
        },
        ArgSpec {
            name: "at",
            description: "Where the replaced characters start, counted from 0.",
            kind: ArgKind::Int {
                min: Some(0),
                max: Some(1 << 40),
            },
            required: true,
        },
        ArgSpec {
            name: "delete",
            description: "How many characters to remove there.",
            kind: ArgKind::Int {
                min: Some(0),
                max: Some(1 << 40),
            },
            required: false,
        },
        ArgSpec {
            name: "insert",
            description: "The text to put there.",
            kind: ArgKind::String {
                max_len: Some(1 << 20),
            },
            required: false,
        },
    ],
});

/// A command without arguments that acts on the board (M3.3).
macro_rules! board_command {
    ($(#[$doc:meta])* $name:ident, $id:literal, $title:literal, $description:literal,
     $category:literal, [$($synonym:literal),*], $icon:literal, $availability:expr, $run:expr) => {
        $(#[$doc])*
        pub static $name: FnCommand<BoardCanvas> = FnCommand::<BoardCanvas>::new(
            $id,
            CommandMeta::new($title, $description)
                .category($category)
                .synonyms(&[$($synonym),*])
                .icon($icon),
            $run,
        )
        .availability($availability);
    };
}

fn done(changed: bool) -> Outcome {
    if changed {
        Outcome::Done
    } else {
        Outcome::Unchanged
    }
}

fn always(_board: &BoardCanvas) -> Availability {
    Availability::Enabled
}

fn can_undo(board: &BoardCanvas) -> Availability {
    if board
        .document()
        .can_undo(imaginarium_model::UndoScope::User)
    {
        Availability::Enabled
    } else {
        Availability::Disabled("Nothing to undo")
    }
}

fn can_redo(board: &BoardCanvas) -> Availability {
    if board
        .document()
        .can_redo(imaginarium_model::UndoScope::User)
    {
        Availability::Enabled
    } else {
        Availability::Disabled("Nothing to redo")
    }
}

fn can_group(board: &BoardCanvas) -> Availability {
    if board.can_group() {
        Availability::Enabled
    } else {
        Availability::Disabled("Select two or more nodes side by side")
    }
}

fn has_group(board: &BoardCanvas) -> Availability {
    let view = board.document().view();
    let any = board.selection().with_untracked(|selection| {
        selection.iter().any(|node| {
            !crate::hit::locked(view, *node)
                && view
                    .node(*node)
                    .is_some_and(|node| matches!(node.kind(), imaginarium_model::NodeKind::Group))
        })
    });
    if any {
        Availability::Enabled
    } else {
        Availability::Disabled("Select a group first")
    }
}

fn has_text_node(board: &BoardCanvas) -> Availability {
    let view = board.document().view();
    let one = board
        .selection()
        .with_untracked(|selection| match selection[..] {
            [node] => {
                !crate::hit::locked(view, node)
                    && board.is_on_board(node)
                    && view.node(node).is_some_and(|node| {
                        matches!(
                            node.kind(),
                            imaginarium_model::NodeKind::Text | imaginarium_model::NodeKind::Shape
                        )
                    })
            }
            _ => false,
        });
    if one {
        Availability::Enabled
    } else {
        Availability::Disabled("Select one text box or shape")
    }
}

board_command!(
    /// `edit.undo`.
    UNDO, "edit.undo", "Undo", "Undo the last change you made on this board.", "Edit",
    ["back", "revert", "rückgängig"], "undo-2", can_undo,
    |board, _| {
        let undone = board
            .undo(imaginarium_model::UndoScope::User, &Actor::User)
            .map_err(failed("edit.undo"))?;
        Ok(done(undone))
    }
);

board_command!(
    /// `edit.redo`.
    REDO, "edit.redo", "Redo", "Redo the last change you undid on this board.", "Edit",
    ["again", "wiederholen"], "redo-2", can_redo,
    |board, _| {
        let redone = board
            .redo(imaginarium_model::UndoScope::User, &Actor::User)
            .map_err(failed("edit.redo"))?;
        Ok(done(redone))
    }
);

board_command!(
    /// `board.delete`.
    DELETE, "board.delete", "Delete", "Delete the selected nodes and everything in them.", "Edit",
    ["remove", "erase", "löschen"], "trash", has_selection,
    |board, _| Ok(done(board.delete_selection().map_err(failed("board.delete"))?))
);

board_command!(
    /// `board.duplicate`.
    DUPLICATE, "board.duplicate", "Duplicate",
    "Copy the selected nodes in place, slightly down and to the right, and select the copies.",
    "Edit", ["clone", "copy in place", "duplizieren"], "copy-plus", has_selection,
    |board, _| {
        let offset = crate::board::DUPLICATE_OFFSET;
        let created = board
            .duplicate_selection((offset, offset))
            .map_err(failed("board.duplicate"))?;
        Ok(done(!created.is_empty()))
    }
);

board_command!(
    /// `board.select_all`.
    SELECT_ALL, "board.select_all", "Select all",
    "Select every node on the board (inside an entered group: every node of the group).",
    "Select", ["everything", "alles auswählen"], "mouse-pointer-2", always,
    |board, _| Ok(done(board.select_all()))
);

board_command!(
    /// `board.bring_to_front`.
    BRING_TO_FRONT, "board.bring_to_front", "Bring to front",
    "Put the selected nodes in front of everything else in their frame or group.",
    "Arrange", ["top", "raise", "nach vorne"], "arrow-up-to-line", has_selection,
    |board, _| Ok(done(board.reorder_selection(crate::board::Order::Front).map_err(failed("board.bring_to_front"))?))
);

board_command!(
    /// `board.send_to_back`.
    SEND_TO_BACK, "board.send_to_back", "Send to back",
    "Put the selected nodes behind everything else in their frame or group.",
    "Arrange", ["bottom", "lower", "nach hinten"], "arrow-down-to-line", has_selection,
    |board, _| Ok(done(board.reorder_selection(crate::board::Order::Back).map_err(failed("board.send_to_back"))?))
);

board_command!(
    /// `board.bring_forward`.
    BRING_FORWARD, "board.bring_forward", "Bring forward",
    "Move the selected nodes one step to the front in their frame or group.",
    "Arrange", ["raise", "up one"], "arrow-up", has_selection,
    |board, _| Ok(done(board.reorder_selection(crate::board::Order::Forward).map_err(failed("board.bring_forward"))?))
);

board_command!(
    /// `board.send_backward`.
    SEND_BACKWARD, "board.send_backward", "Send backward",
    "Move the selected nodes one step to the back in their frame or group.",
    "Arrange", ["lower", "down one"], "arrow-down", has_selection,
    |board, _| Ok(done(board.reorder_selection(crate::board::Order::Backward).map_err(failed("board.send_backward"))?))
);

board_command!(
    /// `board.group`.
    GROUP, "board.group", "Group",
    "Put the selected nodes into a new group, which then moves and selects as one.",
    "Arrange", ["combine", "gruppieren"], "group", can_group,
    |board, _| Ok(done(board.group_selection().map_err(failed("board.group"))?.is_some()))
);

board_command!(
    /// `board.ungroup`.
    UNGROUP, "board.ungroup", "Ungroup",
    "Take the selected groups apart: their nodes stay where they are.",
    "Arrange", ["split", "break apart", "gruppierung aufheben"], "group", has_group,
    |board, _| Ok(done(!board.ungroup_selection().map_err(failed("board.ungroup"))?.is_empty()))
);

board_command!(
    /// `board.lock`.
    LOCK, "board.lock", "Lock",
    "Lock the selected nodes, so they can't be selected or moved by a click.",
    "Arrange", ["pin", "freeze", "sperren"], "lock", has_selection,
    |board, _| Ok(done(board.set_locked(true).map_err(failed("board.lock"))?))
);

board_command!(
    /// `board.unlock`.
    UNLOCK, "board.unlock", "Unlock",
    "Unlock the selected nodes; with no locked node selected, every locked node on the board.",
    "Arrange", ["unpin", "entsperren"], "lock", always,
    |board, _| Ok(done(board.set_locked(false).map_err(failed("board.unlock"))?))
);

board_command!(
    /// `view.zoom_in`.
    ZOOM_IN, "view.zoom_in", "Zoom in", "Zoom in one step, around the middle of the board.", "View",
    ["bigger", "magnify", "vergrößern"], "zoom-in", always,
    |board, _| {
        board.zoom_in();
        Ok(Outcome::Done)
    }
);

board_command!(
    /// `view.zoom_out`.
    ZOOM_OUT, "view.zoom_out", "Zoom out", "Zoom out one step, around the middle of the board.", "View",
    ["smaller", "verkleinern"], "zoom-in", always,
    |board, _| {
        board.zoom_out();
        Ok(Outcome::Done)
    }
);

board_command!(
    /// `view.zoom_100`.
    ZOOM_100, "view.zoom_100", "Zoom to 100 %", "Show the board at its actual size.", "View",
    ["actual size", "reset zoom", "100%"], "scan", always,
    |board, _| {
        board.zoom_actual();
        Ok(Outcome::Done)
    }
);

board_command!(
    /// `view.zoom_to_fit`.
    ZOOM_TO_FIT, "view.zoom_to_fit", "Zoom to fit", "Show everything on the board.", "View",
    ["fit", "show all", "overview"], "scan", always,
    |board, _| Ok(done(board.zoom_to_fit()))
);

board_command!(
    /// `view.zoom_to_selection`.
    ZOOM_TO_SELECTION, "view.zoom_to_selection", "Zoom to selection",
    "Show the selected nodes as large as they fit, up to 100 %.", "View", ["focus", "fit selection"], "scan",
    has_selection,
    |board, _| Ok(done(board.zoom_to_selection()))
);

board_command!(
    /// `text.edit`.
    EDIT_TEXT, "text.edit", "Edit text",
    "Start editing the text of the selected text box or the label of the selected shape.",
    "Text", ["type", "rename", "label"], "type", has_text_node,
    |board, _| Ok(done(board.request_edit()))
);

const PRESETS: &[&str] = &["rect", "ellipse", "diamond", "triangle", "line"];

/// The box arguments of an insert, in board coordinates: given, or the
/// tool's click size in the middle of the view.
fn insert_area(
    board: &BoardCanvas,
    args: &Args,
    preset: Option<crate::board::Preset>,
) -> imaginarium_render::kurbo::Rect {
    let tool = preset.map_or(crate::board::Tool::Frame, crate::board::Tool::Shape);
    let default = BoardCanvas::click_area(tool, board.view_middle());
    let x = args.float("x").unwrap_or(default.x0);
    let y = args.float("y").unwrap_or(default.y0);
    let w = args.float("w").unwrap_or(default.width());
    let h = args.float("h").unwrap_or(default.height());
    imaginarium_render::kurbo::Rect::new(x, y, x + w, y + h)
}

/// `board.insert_shape`.
pub static INSERT_SHAPE: FnCommand<BoardCanvas> = FnCommand::<BoardCanvas>::new(
    "board.insert_shape",
    CommandMeta::new(
        "Insert shape",
        "Insert a shape (rect, ellipse, diamond, triangle or line) with its box at x, y (board points, top left) and w by h; without a box, in the middle of the view. It goes into the frame at that place, and is selected.",
    )
    .category("Insert")
    .synonyms(&["add shape", "rectangle", "circle", "ellipse", "diamond", "triangle", "line", "form"])
    .icon("square-plus"),
    |board, args| {
        let preset = args
            .str("preset")
            .and_then(crate::board::Preset::parse)
            .ok_or_else(|| CommandError::InvalidArgs {
                id: "board.insert_shape",
                reason: "`preset` names no shape".into(),
            })?;
        let area = insert_area(board, args, Some(preset));
        board
            .insert_box(Some(preset), area)
            .map_err(failed("board.insert_shape"))?;
        Ok(Outcome::Done)
    },
)
.args(ArgsSchema {
    args: &[
        ArgSpec {
            name: "preset",
            description: "Which shape.",
            kind: ArgKind::Enum { variants: PRESETS },
            required: true,
        },
        float("x", "Left edge in board points.", -LIMIT, LIMIT, false),
        float("y", "Top edge in board points.", -LIMIT, LIMIT, false),
        float("w", "Width in points.", 0.0, LIMIT, false),
        float("h", "Height in points.", 0.0, LIMIT, false),
    ],
});

/// `board.insert_frame`.
pub static INSERT_FRAME: FnCommand<BoardCanvas> = FnCommand::<BoardCanvas>::new(
    "board.insert_frame",
    CommandMeta::new(
        "Insert frame",
        "Insert a frame (an area that clips what is in it, with a name above) with its box at x, y (board points) and w by h; without a box, in the middle of the view.",
    )
    .category("Insert")
    .synonyms(&["artboard", "area", "section", "rahmen"])
    .icon("frame"),
    |board, args| {
        let area = insert_area(board, args, None);
        board.insert_box(None, area).map_err(failed("board.insert_frame"))?;
        Ok(Outcome::Done)
    },
)
.args(ArgsSchema {
    args: &[
        float("x", "Left edge in board points.", -LIMIT, LIMIT, false),
        float("y", "Top edge in board points.", -LIMIT, LIMIT, false),
        float("w", "Width in points.", 0.0, LIMIT, false),
        float("h", "Height in points.", 0.0, LIMIT, false),
    ],
});

/// `board.insert_text`.
pub static INSERT_TEXT: FnCommand<BoardCanvas> = FnCommand::<BoardCanvas>::new(
    "board.insert_text",
    CommandMeta::new(
        "Insert text",
        "Insert a text box with its top left at x, y (board points; without them, the middle of the view). With `text`, it holds that text; without, editing starts so the user types it.",
    )
    .category("Insert")
    .synonyms(&["add text", "note", "write", "textfeld"])
    .icon("type"),
    |board, args| {
        let middle = board.view_middle();
        let at = imaginarium_render::kurbo::Point::new(
            args.float("x").unwrap_or(middle.x),
            args.float("y").unwrap_or(middle.y),
        );
        let id = board.insert_text_at(at).map_err(failed("board.insert_text"))?;
        if let Some(text) = args.str("text").filter(|text| !text.is_empty()) {
            board.cancel_pending_edit();
            board
                .transact(&Actor::User, |txn| txn.insert_text(id, 0, text))
                .map_err(failed("board.insert_text"))?;
        }
        Ok(Outcome::Done)
    },
)
.args(ArgsSchema {
    args: &[
        float("x", "Left edge in board points.", -LIMIT, LIMIT, false),
        float("y", "Top edge in board points.", -LIMIT, LIMIT, false),
        ArgSpec {
            name: "text",
            description: "The text it holds (plain text).",
            kind: ArgKind::String {
                max_len: Some(1 << 20),
            },
            required: false,
        },
    ],
});

macro_rules! tool {
    ($name:ident, $id:literal, $title:literal, $description:literal, [$($synonym:literal),*], $icon:literal, $tool:expr) => {
        board_command!(
            /// A tool of the tool bar.
            $name, $id, $title, $description, "Tools", [$($synonym),*], $icon, always,
            |board, _| {
                board.set_tool($tool);
                Ok(Outcome::Done)
            }
        );
    };
}

tool!(
    TOOL_SELECT,
    "tool.select",
    "Select tool",
    "Select and move nodes with the pointer.",
    ["pointer", "arrow"],
    "mouse-pointer-2",
    crate::board::Tool::Select
);
tool!(
    TOOL_HAND,
    "tool.hand",
    "Hand tool",
    "Drag to pan the board.",
    ["pan", "move view", "grab"],
    "hand",
    crate::board::Tool::Hand
);
tool!(
    TOOL_RECTANGLE,
    "tool.rectangle",
    "Rectangle tool",
    "Draw a rectangle by dragging on the board.",
    ["box", "square", "rechteck"],
    "square",
    crate::board::Tool::Shape(crate::board::Preset::Rectangle)
);
tool!(
    TOOL_ELLIPSE,
    "tool.ellipse",
    "Ellipse tool",
    "Draw an ellipse by dragging on the board.",
    ["circle", "oval", "kreis"],
    "circle",
    crate::board::Tool::Shape(crate::board::Preset::Ellipse)
);
tool!(
    TOOL_DIAMOND,
    "tool.diamond",
    "Diamond tool",
    "Draw a diamond (a decision in a flow chart) by dragging on the board.",
    ["rhombus", "decision", "raute"],
    "diamond",
    crate::board::Tool::Shape(crate::board::Preset::Diamond)
);
tool!(
    TOOL_TRIANGLE,
    "tool.triangle",
    "Triangle tool",
    "Draw a triangle by dragging on the board.",
    ["dreieck"],
    "triangle",
    crate::board::Tool::Shape(crate::board::Preset::Triangle)
);
tool!(
    TOOL_LINE,
    "tool.line",
    "Line tool",
    "Draw a straight line by dragging from one end to the other.",
    ["stroke", "linie"],
    "slash",
    crate::board::Tool::Shape(crate::board::Preset::Line)
);
tool!(
    TOOL_TEXT,
    "tool.text",
    "Text tool",
    "Click on the board to place a text box and type.",
    ["type", "write"],
    "type",
    crate::board::Tool::Text
);
tool!(
    TOOL_FRAME,
    "tool.frame",
    "Frame tool",
    "Draw a frame (an area that clips what is in it) by dragging on the board.",
    ["artboard", "section"],
    "frame",
    crate::board::Tool::Frame
);

board_command!(
    /// `board.copy`.
    COPY, "board.copy", "Copy",
    "Copy the selected nodes to the clipboard (with their text for other apps).", "Edit",
    ["kopieren"], "copy", has_selection,
    |board, _| Ok(done(board.copy_to_clipboard()))
);

board_command!(
    /// `board.cut`.
    CUT, "board.cut", "Cut", "Copy the selected nodes to the clipboard and delete them.", "Edit",
    ["ausschneiden"], "scissors", has_selection,
    |board, _| {
        if !board.copy_to_clipboard() {
            return Ok(Outcome::Unchanged);
        }
        board.forget_pastes();
        Ok(done(board.delete_selection().map_err(failed("board.cut"))?))
    }
);

board_command!(
    /// `board.paste`.
    PASTE, "board.paste", "Paste",
    "Paste what the clipboard holds: nodes copied on a board, or text as a text box.", "Edit",
    ["einfügen"], "clipboard", always,
    |board, _| {
        board.request_paste();
        Ok(Outcome::Started)
    }
);

macro_rules! nudge {
    ($name:ident, $id:literal, $title:literal, $description:literal, $dx:expr, $dy:expr) => {
        board_command!(
            /// A nudge of the selection.
            $name, $id, $title, $description, "Arrange", ["nudge", "move"], "move", has_selection,
            |board, _| Ok(done(board.move_selection($dx, $dy).map_err(failed($id))?))
        );
    };
}

nudge!(
    NUDGE_LEFT,
    "board.nudge_left",
    "Nudge left",
    "Move the selection 1 point left.",
    -1.0,
    0.0
);
nudge!(
    NUDGE_RIGHT,
    "board.nudge_right",
    "Nudge right",
    "Move the selection 1 point right.",
    1.0,
    0.0
);
nudge!(
    NUDGE_UP,
    "board.nudge_up",
    "Nudge up",
    "Move the selection 1 point up.",
    0.0,
    -1.0
);
nudge!(
    NUDGE_DOWN,
    "board.nudge_down",
    "Nudge down",
    "Move the selection 1 point down.",
    0.0,
    1.0
);
nudge!(
    NUDGE_LEFT_FAR,
    "board.nudge_left_far",
    "Nudge left far",
    "Move the selection 10 points left.",
    -10.0,
    0.0
);
nudge!(
    NUDGE_RIGHT_FAR,
    "board.nudge_right_far",
    "Nudge right far",
    "Move the selection 10 points right.",
    10.0,
    0.0
);
nudge!(
    NUDGE_UP_FAR,
    "board.nudge_up_far",
    "Nudge up far",
    "Move the selection 10 points up.",
    0.0,
    -10.0
);
nudge!(
    NUDGE_DOWN_FAR,
    "board.nudge_down_far",
    "Nudge down far",
    "Move the selection 10 points down.",
    0.0,
    10.0
);

/// Every command of the board, for the shell's registry and palette.
pub fn board_commands() -> &'static [&'static FnCommand<BoardCanvas>] {
    &BOARD_COMMANDS
}

static BOARD_COMMANDS: [&FnCommand<BoardCanvas>; 62] = [
    &crate::board_settings::SET_SETTING,
    &COPY,
    &CUT,
    &PASTE,
    &INSERT_SHAPE,
    &INSERT_FRAME,
    &INSERT_TEXT,
    &TOOL_SELECT,
    &TOOL_HAND,
    &TOOL_RECTANGLE,
    &TOOL_ELLIPSE,
    &TOOL_DIAMOND,
    &TOOL_TRIANGLE,
    &TOOL_LINE,
    &TOOL_TEXT,
    &TOOL_FRAME,
    &MOVE,
    &SET_BOX,
    &SET_RADIUS,
    &REPLACE_TEXT,
    &UNDO,
    &REDO,
    &DELETE,
    &DUPLICATE,
    &SELECT_ALL,
    &BRING_TO_FRONT,
    &SEND_TO_BACK,
    &BRING_FORWARD,
    &SEND_BACKWARD,
    &GROUP,
    &UNGROUP,
    &LOCK,
    &UNLOCK,
    &ZOOM_IN,
    &ZOOM_OUT,
    &ZOOM_100,
    &ZOOM_TO_FIT,
    &ZOOM_TO_SELECTION,
    &EDIT_TEXT,
    &NUDGE_LEFT,
    &NUDGE_RIGHT,
    &NUDGE_UP,
    &NUDGE_DOWN,
    &NUDGE_LEFT_FAR,
    &NUDGE_RIGHT_FAR,
    &NUDGE_UP_FAR,
    &NUDGE_DOWN_FAR,
    crate::commands_style::STYLE_COMMANDS[0],
    crate::commands_style::STYLE_COMMANDS[1],
    crate::commands_style::STYLE_COMMANDS[2],
    crate::commands_style::STYLE_COMMANDS[3],
    crate::commands_style::STYLE_COMMANDS[4],
    crate::commands_style::OPACITY_COMMANDS[0],
    crate::commands_style::OPACITY_COMMANDS[1],
    crate::commands_style::INSPECTOR_COMMANDS[0],
    crate::commands_style::INSPECTOR_COMMANDS[1],
    crate::commands_style::INSPECTOR_COMMANDS[2],
    crate::commands_style::INSPECTOR_COMMANDS[3],
    crate::commands_style::INSPECTOR_COMMANDS[4],
    crate::commands_style::INSPECTOR_COMMANDS[5],
    crate::commands_style::INSPECTOR_COMMANDS[6],
    crate::commands_style::INSPECTOR_COMMANDS[7],
];

/// Arguments of `text.replace`.
pub(crate) fn replace_args(node: NodeId, at: usize, delete: usize, insert: &str) -> Args {
    let int = |value: usize| ArgValue::Int(i64::try_from(value).unwrap_or(i64::MAX));
    Args::new()
        .with("node", ArgValue::String(node.to_string()))
        .with("at", int(at))
        .with("delete", int(delete))
        .with("insert", ArgValue::String(insert.to_owned()))
}

/// Arguments of an insert: `preset` (a shape, or a frame for `None`) in
/// `area` (board points).
pub(crate) fn insert_args(
    preset: Option<crate::board::Preset>,
    area: imaginarium_render::kurbo::Rect,
) -> Args {
    let args = Args::new()
        .with("x", ArgValue::Float(area.x0))
        .with("y", ArgValue::Float(area.y0))
        .with("w", ArgValue::Float(area.width().max(0.0)))
        .with("h", ArgValue::Float(area.height().max(0.0)));
    match preset {
        Some(preset) => args.with("preset", ArgValue::String(preset.name().into())),
        None => args,
    }
}

/// Arguments of `board.insert_text` at `at`.
pub(crate) fn text_args(at: imaginarium_render::kurbo::Point) -> Args {
    Args::new()
        .with("x", ArgValue::Float(at.x))
        .with("y", ArgValue::Float(at.y))
}

/// Arguments of `shape.set_radius`.
pub(crate) fn radius_args(node: NodeId, radius: f64) -> Args {
    Args::new()
        .with("node", ArgValue::String(node.to_string()))
        .with("radius", ArgValue::Float(radius.max(0.0)))
}

thread_local! {
    /// The registry the board's own gestures and keys run commands through.
    static REGISTRY: RefCell<Option<Registry<BoardCanvas>>> = const { RefCell::new(None) };
}

/// Runs a board command through the registry (availability and argument
/// checks included): the one way gestures and keys change the document.
pub(crate) fn execute(
    board: &mut BoardCanvas,
    id: &str,
    args: &Args,
) -> Result<Outcome, CommandError> {
    // Taken out of the cell while a command runs, so a command that runs
    // another one (an AI action, a composite) doesn't find it borrowed.
    let registry = REGISTRY.with(|registry| registry.borrow_mut().take());
    let registry = registry.unwrap_or_else(|| {
        let mut registry = Registry::new();
        for command in board_commands() {
            // Ids are literals above: they are valid and unique (tested).
            let _ = registry.register(*command);
        }
        registry
    });
    let result = registry.execute(board, id, args);
    REGISTRY.with(|cell| *cell.borrow_mut() = Some(registry));
    result
}

/// Arguments of `board.move`.
pub(crate) fn move_args(dx: f64, dy: f64) -> Args {
    Args::new()
        .with("dx", ArgValue::Float(dx))
        .with("dy", ArgValue::Float(dy))
}

/// Arguments of `board.set_box`; `flip` a line's new one.
pub(crate) fn box_args(node: NodeId, geometry: Geometry, flip: Option<&str>) -> Args {
    let args = match flip {
        Some(flip) => Args::new().with(
            "flip",
            ArgValue::String(if flip.is_empty() { "none" } else { flip }.into()),
        ),
        None => Args::new(),
    };
    args.with("node", ArgValue::String(node.to_string()))
        .with("x", ArgValue::Float(geometry.x))
        .with("y", ArgValue::Float(geometry.y))
        .with("w", ArgValue::Float(geometry.w.max(0.0)))
        .with("h", ArgValue::Float(geometry.h.max(0.0)))
        .with("rotation", ArgValue::Float(geometry.r))
}
