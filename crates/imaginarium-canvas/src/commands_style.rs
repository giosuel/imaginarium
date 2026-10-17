// SPDX-License-Identifier: AGPL-3.0-or-later

//! Style commands on the selection (M3.4, the contextual toolbar): fill,
//! outline color and width, corner radius, text color. Each is one undo
//! step over the selected nodes that take it, as overrides on them
//! (`schema-v1.md` §5.6); locked nodes are left alone. A command whose
//! selection has nothing that takes it is hidden, so the toolbar and the
//! palette don't offer it.

use imaginarium_commands::{
    ArgKind, ArgSpec, ArgsSchema, Availability, CommandError, CommandMeta, FnCommand, Outcome,
};
use imaginarium_model::view::NodeView;
use imaginarium_model::{Actor, NodeId, NodeKind, Value};

use crate::BoardCanvas;

/// What kind of node a style property applies to.
type Takes = fn(&NodeView) -> bool;

fn preset(node: &NodeView) -> Option<String> {
    node.prop("shape")
        .and_then(Value::as_str)
        .map(str::to_owned)
}

/// Shapes other than lines, and frames: they have an inside to fill.
fn takes_fill(node: &NodeView) -> bool {
    match node.kind() {
        NodeKind::Frame => true,
        NodeKind::Shape => preset(node).as_deref() != Some("line"),
        _ => false,
    }
}

/// Shapes and frames: they have an outline.
fn takes_stroke(node: &NodeView) -> bool {
    matches!(node.kind(), NodeKind::Shape | NodeKind::Frame)
}

/// Rectangles without per-corner radii.
fn takes_radius(node: &NodeView) -> bool {
    matches!(node.kind(), NodeKind::Shape)
        && preset(node).is_none_or(|preset| preset == "rect")
        && !node.props().contains_key("radii")
}

/// Text boxes and shapes, which carry a label.
fn takes_text(node: &NodeView) -> bool {
    matches!(node.kind(), NodeKind::Text | NodeKind::Shape)
}

/// The selected, unlocked nodes that take a property.
fn targets(board: &BoardCanvas, takes: Takes) -> Vec<NodeId> {
    let view = board.document().view();
    board.selection().with_untracked(|selection| {
        selection
            .iter()
            .copied()
            .filter(|node| !crate::hit::locked(view, *node))
            .filter(|node| view.node(*node).is_some_and(takes))
            .collect()
    })
}

fn available(board: &BoardCanvas, takes: Takes) -> Availability {
    if targets(board, takes).is_empty() {
        Availability::Hidden
    } else {
        Availability::Enabled
    }
}

/// Sets `prop` to what `value` gives for each target, in one step.
fn set(
    board: &mut BoardCanvas,
    id: &'static str,
    takes: Takes,
    prop: &str,
    value: impl Fn(&NodeView) -> Value,
) -> Result<Outcome, CommandError> {
    let nodes = targets(board, takes);
    let changes: Vec<(NodeId, Value)> = {
        let view = board.document().view();
        nodes
            .iter()
            .filter_map(|node| {
                let node_view = view.node(*node)?;
                let value = value(node_view);
                (view.style_prop(*node, prop) != Some(&value)).then_some((*node, value))
            })
            .collect()
    };
    if changes.is_empty() {
        return Ok(Outcome::Unchanged);
    }
    board
        .transact(&Actor::User, |txn| {
            for (node, value) in &changes {
                txn.set_style_override(*node, prop, value.clone())?;
            }
            Ok(())
        })
        .map_err(|err| CommandError::Failed {
            id,
            message: err.to_string(),
        })?;
    Ok(Outcome::Done)
}

/// A `#rrggbb` or `#rrggbbaa` argument, lowercased as stored.
fn color(args: &imaginarium_commands::Args, id: &'static str) -> Result<String, CommandError> {
    args.str("color")
        .map(str::to_ascii_lowercase)
        .ok_or_else(|| CommandError::InvalidArgs {
            id,
            reason: "`color` is missing".into(),
        })
}

const COLOR: ArgSpec = ArgSpec {
    name: "color",
    description: "`#rrggbb`, or `#rrggbbaa` with its opacity; `#00000000` for none.",
    kind: ArgKind::Color,
    required: true,
};

/// `style.set_fill`.
pub static SET_FILL: FnCommand<BoardCanvas> = FnCommand::<BoardCanvas>::new(
    "style.set_fill",
    CommandMeta::new(
        "Fill color",
        "Fill the selected shapes and frames with a color (#rrggbb, or #rrggbbaa with opacity; #00000000 for no fill).",
    )
    .category("Style")
    .synonyms(&["background", "fill", "color", "farbe", "füllung"])
    .icon("paint-bucket"),
    |board, args| {
        let fill = color(args, "style.set_fill")?;
        set(board, "style.set_fill", takes_fill, "fill", |_| {
            Value::String(fill.as_str().into())
        })
    },
)
.availability(|board| available(board, takes_fill))
.args(ArgsSchema { args: &[COLOR] });

/// `style.set_stroke`.
pub static SET_STROKE: FnCommand<BoardCanvas> = FnCommand::<BoardCanvas>::new(
    "style.set_stroke",
    CommandMeta::new(
        "Outline color",
        "Color the outline of the selected shapes and frames (#rrggbb, or #rrggbbaa; #00000000 for no outline).",
    )
    .category("Style")
    .synonyms(&["stroke", "border", "line color", "rand", "kontur"])
    .icon("paintbrush"),
    |board, args| {
        let stroke = color(args, "style.set_stroke")?;
        set(board, "style.set_stroke", takes_stroke, "stroke", |_| {
            Value::String(stroke.as_str().into())
        })
    },
)
.availability(|board| available(board, takes_stroke))
.args(ArgsSchema { args: &[COLOR] });

/// `style.set_stroke_width`.
pub static SET_STROKE_WIDTH: FnCommand<BoardCanvas> = FnCommand::<BoardCanvas>::new(
    "style.set_stroke_width",
    CommandMeta::new(
        "Outline width",
        "Set how wide the outline of the selected shapes and frames is, in points (0 for none).",
    )
    .category("Style")
    .synonyms(&[
        "stroke width",
        "border width",
        "line width",
        "thickness",
        "strichstärke",
    ])
    .icon("list"),
    |board, args| {
        let width = args.float("width").unwrap_or(1.5);
        set(
            board,
            "style.set_stroke_width",
            takes_stroke,
            "stroke_w",
            |_| Value::Number(width),
        )
    },
)
.availability(|board| available(board, takes_stroke))
.args(ArgsSchema {
    args: &[ArgSpec {
        name: "width",
        description: "The outline width in points.",
        kind: ArgKind::Float {
            min: Some(0.0),
            max: Some(64.0),
        },
        required: true,
    }],
});

/// `style.set_radius`.
pub static SET_SELECTION_RADIUS: FnCommand<BoardCanvas> = FnCommand::<BoardCanvas>::new(
    "style.set_radius",
    CommandMeta::new(
        "Set corner radius",
        "Round the corners of the selected rectangles, in points: 0 for sharp corners, up to half a rectangle's shorter side.",
    )
    .category("Style")
    .synonyms(&["round corners", "rounded", "radius", "abrunden"])
    .icon("square"),
    |board, args| {
        let radius = args.float("radius").unwrap_or(0.0).max(0.0);
        set(board, "style.set_radius", takes_radius, "radius", |node| {
            let size = node.geometry().unwrap_or_default();
            Value::Number(radius.min((size.w.min(size.h) / 2.0).max(0.0)))
        })
    },
)
.availability(|board| available(board, takes_radius))
.args(ArgsSchema {
    args: &[ArgSpec {
        name: "radius",
        description: "The corner radius in points.",
        kind: ArgKind::Float {
            min: Some(0.0),
            max: Some(1.0e7),
        },
        required: true,
    }],
});

/// `style.set_text_color`.
pub static SET_TEXT_COLOR: FnCommand<BoardCanvas> = FnCommand::<BoardCanvas>::new(
    "style.set_text_color",
    CommandMeta::new(
        "Text color",
        "Color the text of the selected text boxes and shapes' labels (#rrggbb).",
    )
    .category("Style")
    .synonyms(&["font color", "text colour", "schriftfarbe"])
    .icon("type"),
    |board, args| {
        let text = color(args, "style.set_text_color")?;
        set(board, "style.set_text_color", takes_text, "color", |_| {
            Value::String(text.as_str().into())
        })
    },
)
.availability(|board| available(board, takes_text))
.args(ArgsSchema { args: &[COLOR] });

/// The style commands, for [`board_commands`](crate::commands::board_commands).
pub(crate) const STYLE_COMMANDS: [&FnCommand<BoardCanvas>; 5] = [
    &SET_FILL,
    &SET_STROKE,
    &SET_STROKE_WIDTH,
    &SET_SELECTION_RADIUS,
    &SET_TEXT_COLOR,
];
