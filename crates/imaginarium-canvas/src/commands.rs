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
        let insert = args.str("insert").unwrap_or("").to_owned();
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
                    txn.insert_text(node, at, &insert)?;
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

/// Every command of the board, for the shell's registry and palette.
pub fn board_commands() -> [&'static FnCommand<BoardCanvas>; 4] {
    [&MOVE, &SET_BOX, &SET_RADIUS, &REPLACE_TEXT]
}

/// Arguments of `text.replace`.
pub(crate) fn replace_args(node: NodeId, at: usize, delete: usize, insert: &str) -> Args {
    let int = |value: usize| ArgValue::Int(i64::try_from(value).unwrap_or(i64::MAX));
    Args::new()
        .with("node", ArgValue::String(node.to_string()))
        .with("at", int(at))
        .with("delete", int(delete))
        .with("insert", ArgValue::String(insert.to_owned()))
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
            let _ = registry.register(command);
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
