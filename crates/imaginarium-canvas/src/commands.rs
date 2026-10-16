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
        if geometry == current {
            return Ok(Outcome::Unchanged);
        }
        board
            .transact(&Actor::User, |txn| txn.set_geometry(node, geometry))
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
        let radius = args.float("radius").unwrap_or(0.0);
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

/// Every command of the board, for the shell's registry and palette.
pub fn board_commands() -> [&'static FnCommand<BoardCanvas>; 3] {
    [&MOVE, &SET_BOX, &SET_RADIUS]
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
    REGISTRY.with(|registry| {
        let mut registry = registry.borrow_mut();
        let registry = registry.get_or_insert_with(|| {
            let mut registry = Registry::new();
            for command in board_commands() {
                // Ids are literals above: they are valid and unique (tested).
                let _ = registry.register(command);
            }
            registry
        });
        registry.execute(board, id, args)
    })
}

/// Arguments of `board.move`.
pub(crate) fn move_args(dx: f64, dy: f64) -> Args {
    Args::new()
        .with("dx", ArgValue::Float(dx))
        .with("dy", ArgValue::Float(dy))
}

/// Arguments of `board.set_box`.
pub(crate) fn box_args(node: NodeId, geometry: Geometry) -> Args {
    Args::new()
        .with("node", ArgValue::String(node.to_string()))
        .with("x", ArgValue::Float(geometry.x))
        .with("y", ArgValue::Float(geometry.y))
        .with("w", ArgValue::Float(geometry.w.max(0.0)))
        .with("h", ArgValue::Float(geometry.h.max(0.0)))
        .with("rotation", ArgValue::Float(geometry.r))
}
