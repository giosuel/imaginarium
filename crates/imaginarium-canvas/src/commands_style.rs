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

fn preset(node: &NodeView) -> Option<&str> {
    node.prop("shape").and_then(Value::as_str)
}

/// Shapes other than lines, and frames: they have an inside to fill.
fn takes_fill(node: &NodeView) -> bool {
    match node.kind() {
        NodeKind::Frame => true,
        NodeKind::Shape => preset(node) != Some("line"),
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

/// The first selected, unlocked node that takes a property (stops
/// there: availability runs every frame for the contextual toolbar).
fn first(board: &BoardCanvas, takes: Takes) -> Option<NodeId> {
    let view = board.document().view();
    board.selection().with_untracked(|selection| {
        selection
            .iter()
            .copied()
            .find(|node| view.node(*node).is_some_and(takes) && !crate::hit::locked(view, *node))
    })
}

fn available(board: &BoardCanvas, takes: Takes) -> Availability {
    if first(board, takes).is_some() {
        Availability::Enabled
    } else {
        Availability::Hidden
    }
}

/// What a panel shows a style control for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StyleTarget {
    /// Fill (shapes but lines, frames).
    Fill,
    /// Outline (shapes, frames).
    Stroke,
    /// Corner radius (rectangles).
    Radius,
    /// Text color and size (text boxes, shapes' labels).
    Text,
}

/// The first selected node a style control applies to, whose look the
/// control shows.
pub fn first_target(board: &BoardCanvas, target: StyleTarget) -> Option<NodeId> {
    first(
        board,
        match target {
            StyleTarget::Fill => takes_fill,
            StyleTarget::Stroke => takes_stroke,
            StyleTarget::Radius => takes_radius,
            StyleTarget::Text => takes_text,
        },
    )
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
                (!same(view.style_prop(*node, prop), &value)).then_some((*node, value))
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

/// Whether a stored value is `value` already (colors in any case: older
/// files store `#E0E7FF`).
fn same(stored: Option<&Value>, value: &Value) -> bool {
    match (stored, value) {
        (Some(Value::String(stored)), Value::String(value)) => stored.eq_ignore_ascii_case(value),
        (stored, value) => stored == Some(value),
    }
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

/// The color `prop` of `node` with its alpha at `opacity` (0 to 1), or
/// `None` when it has no color there (no fill).
fn with_alpha(board: &BoardCanvas, node: &NodeView, prop: &str, opacity: f64) -> Option<Value> {
    let view = board.document().view();
    let stored = view.style_prop(node.id(), prop).and_then(Value::as_str);
    // An outline without a color of its own is the palette's ink.
    let stored = stored.or((prop == "stroke").then_some("#1e293b"))?;
    let color = crate::style::parse_color(stored)?;
    let rgba = color.to_rgba8();
    if prop == "fill" && rgba.a == 0 && stored.len() == 9 {
        // "No fill" stays none: there is nothing to make see-through.
        return None;
    }
    let alpha = (opacity.clamp(0.0, 1.0) * 255.0).round() as u8;
    Some(Value::String(
        format!("#{:02x}{:02x}{:02x}{alpha:02x}", rgba.r, rgba.g, rgba.b).into(),
    ))
}

/// Sets the alpha of each target's own color `prop`, in one step.
fn set_alpha(
    board: &mut BoardCanvas,
    id: &'static str,
    takes: Takes,
    prop: &str,
    opacity: f64,
) -> Result<Outcome, CommandError> {
    let nodes = targets(board, takes);
    let changes: Vec<(NodeId, Value)> = {
        let view = board.document().view();
        nodes
            .iter()
            .filter_map(|node| {
                let value = with_alpha(board, view.node(*node)?, prop, opacity)?;
                (!same(view.style_prop(*node, prop), &value)).then_some((*node, value))
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
        .map_err(failed(id))?;
    Ok(Outcome::Done)
}

const COLOR_OPACITY: ArgSpec = ArgSpec {
    name: "opacity",
    description: "From 0 (see-through) to 1 (solid).",
    kind: ArgKind::Float {
        min: Some(0.0),
        max: Some(1.0),
    },
    required: true,
};

/// `style.set_fill_opacity`.
pub static SET_FILL_OPACITY: FnCommand<BoardCanvas> = FnCommand::<BoardCanvas>::new(
    "style.set_fill_opacity",
    CommandMeta::new(
        "Fill opacity",
        "Make the fill of each selected shape and frame see-through (0) to solid (1), keeping each one's own color.",
    )
    .category("Style")
    .synonyms(&["fill transparency", "fill alpha"])
    .icon("paint-bucket"),
    |board, args| {
        let opacity = args.float("opacity").unwrap_or(1.0);
        set_alpha(board, "style.set_fill_opacity", takes_fill, "fill", opacity)
    },
)
.availability(|board| available(board, takes_fill))
.args(ArgsSchema { args: &[COLOR_OPACITY] });

/// `style.set_stroke_opacity`.
pub static SET_STROKE_OPACITY: FnCommand<BoardCanvas> = FnCommand::<BoardCanvas>::new(
    "style.set_stroke_opacity",
    CommandMeta::new(
        "Outline opacity",
        "Make the outline of each selected shape and frame see-through (0) to solid (1), keeping each one's own color.",
    )
    .category("Style")
    .synonyms(&["stroke transparency", "outline alpha"])
    .icon("paintbrush"),
    |board, args| {
        let opacity = args.float("opacity").unwrap_or(1.0);
        set_alpha(board, "style.set_stroke_opacity", takes_stroke, "stroke", opacity)
    },
)
.availability(|board| available(board, takes_stroke))
.args(ArgsSchema { args: &[COLOR_OPACITY] });

/// The color opacity commands.
pub(crate) const OPACITY_COMMANDS: [&FnCommand<BoardCanvas>; 2] =
    [&SET_FILL_OPACITY, &SET_STROKE_OPACITY];
/// Every node: opacity, hiding, names and styles apply to all kinds.
fn takes_any(_node: &NodeView) -> bool {
    true
}

/// Shapes: their type can change.
fn takes_preset(node: &NodeView) -> bool {
    matches!(node.kind(), NodeKind::Shape)
}

/// The nodes with overrides of their own.
fn takes_reset(node: &NodeView) -> bool {
    !node.style_overrides().is_empty()
}

fn failed(id: &'static str) -> impl Fn(imaginarium_model::ModelError) -> CommandError {
    move |err| CommandError::Failed {
        id,
        message: err.to_string(),
    }
}

/// `style.set_opacity`.
pub static SET_OPACITY: FnCommand<BoardCanvas> = FnCommand::<BoardCanvas>::new(
    "style.set_opacity",
    CommandMeta::new(
        "Opacity",
        "Set how opaque the selected nodes are, from 0 (invisible) to 1 (solid).",
    )
    .category("Style")
    .synonyms(&["transparency", "alpha", "see-through", "deckkraft"])
    .icon("eye"),
    |board, args| {
        let opacity = args.float("opacity").unwrap_or(1.0).clamp(0.0, 1.0);
        set(board, "style.set_opacity", takes_any, "opacity", |_| {
            Value::Number(opacity)
        })
    },
)
.availability(|board| available(board, takes_any))
.args(ArgsSchema {
    args: &[ArgSpec {
        name: "opacity",
        description: "From 0 (invisible) to 1 (solid).",
        kind: ArgKind::Float {
            min: Some(0.0),
            max: Some(1.0),
        },
        required: true,
    }],
});

/// `style.set_text_size`.
pub static SET_TEXT_SIZE: FnCommand<BoardCanvas> = FnCommand::<BoardCanvas>::new(
    "style.set_text_size",
    CommandMeta::new(
        "Text size",
        "Set the size of the selected text boxes' and shapes' text, in points.",
    )
    .category("Style")
    .synonyms(&["font size", "bigger text", "smaller text", "schriftgröße"])
    .icon("type"),
    |board, args| {
        let size = args.float("size").unwrap_or(16.0).clamp(4.0, 400.0);
        set(board, "style.set_text_size", takes_text, "size", |_| {
            Value::Number(size)
        })
    },
)
.availability(|board| available(board, takes_text))
.args(ArgsSchema {
    args: &[ArgSpec {
        name: "size",
        description: "The text size in points.",
        kind: ArgKind::Float {
            min: Some(4.0),
            max: Some(400.0),
        },
        required: true,
    }],
});

/// `style.reset`.
pub static RESET_STYLE: FnCommand<BoardCanvas> = FnCommand::<BoardCanvas>::new(
    "style.reset",
    CommandMeta::new(
        "Reset to style",
        "Remove the values changed on the selected nodes themselves, so they look as their named style (or the defaults) says.",
    )
    .category("Style")
    .synonyms(&["clear formatting", "reset style", "default look"])
    .icon("rotate-ccw"),
    |board, _args| {
        let nodes = targets(board, takes_reset);
        if nodes.is_empty() {
            return Ok(Outcome::Unchanged);
        }
        let props: Vec<(NodeId, Vec<String>)> = {
            let view = board.document().view();
            nodes
                .iter()
                .filter_map(|node| {
                    let keys = view.node(*node)?.style_overrides().iter().map(|(key, _)| key.to_owned()).collect();
                    Some((*node, keys))
                })
                .collect()
        };
        board
            .transact(&Actor::User, |txn| {
                for (node, keys) in &props {
                    for key in keys {
                        txn.set_style_override(*node, key, Value::Null)?;
                    }
                }
                Ok(())
            })
            .map_err(failed("style.reset"))?;
        Ok(Outcome::Done)
    },
)
.availability(|board| available(board, takes_reset));

/// `style.apply`.
pub static APPLY_STYLE: FnCommand<BoardCanvas> = FnCommand::<BoardCanvas>::new(
    "style.apply",
    CommandMeta::new(
        "Apply style",
        "Give the selected nodes the board's named style `style` (its id), or no named style with an empty id; their own changes stay.",
    )
    .category("Style")
    .synonyms(&["named style", "use style", "formatvorlage"])
    .icon("paintbrush"),
    |board, args| {
        let style = args.str("style").unwrap_or("");
        let style = if style.is_empty() {
            None
        } else {
            let id = board
                .document()
                .view()
                .style_ids()
                .find(|id| id.as_str() == style)
                .cloned();
            Some(id.ok_or_else(|| CommandError::InvalidArgs {
                id: "style.apply",
                reason: "no such style on this board".into(),
            })?)
        };
        let nodes = targets(board, takes_any);
        if nodes.is_empty() {
            return Ok(Outcome::Unchanged);
        }
        board
            .transact(&Actor::User, |txn| {
                for node in &nodes {
                    txn.set_style(*node, style.as_ref())?;
                }
                Ok(())
            })
            .map_err(failed("style.apply"))?;
        Ok(Outcome::Done)
    },
)
.availability(|board| {
    if board.document().view().style_ids().next().is_none() {
        Availability::Hidden
    } else {
        available(board, takes_any)
    }
})
.args(ArgsSchema {
    args: &[ArgSpec {
        name: "style",
        description: "The style's id; empty for none.",
        kind: ArgKind::String { max_len: Some(64) },
        required: true,
    }],
});

/// `board.rename`.
pub static RENAME: FnCommand<BoardCanvas> = FnCommand::<BoardCanvas>::new(
    "board.rename",
    CommandMeta::new(
        "Rename",
        "Give the one selected node a name (frames show theirs above them); an empty name removes it.",
    )
    .category("Edit")
    .synonyms(&["name", "label", "title", "umbenennen"])
    .icon("type"),
    |board, args| {
        let name = args.str("name").unwrap_or("").trim().to_owned();
        let [node] = targets(board, takes_any)[..] else {
            return Err(CommandError::InvalidArgs {
                id: "board.rename",
                reason: "select one node".into(),
            });
        };
        let value = if name.is_empty() {
            Value::Null
        } else {
            Value::String(name.as_str().into())
        };
        if board.document().view().node(node).and_then(|node| node.name()) == Some(name.as_str()) {
            return Ok(Outcome::Unchanged);
        }
        board
            .transact(&Actor::User, |txn| txn.set_prop(node, "name", value))
            .map_err(failed("board.rename"))?;
        Ok(Outcome::Done)
    },
)
.availability(|board| {
    if targets(board, takes_any).len() == 1 {
        Availability::Enabled
    } else {
        Availability::Hidden
    }
})
.args(ArgsSchema {
    args: &[ArgSpec {
        name: "name",
        description: "The new name; empty to remove it.",
        kind: ArgKind::String { max_len: Some(256) },
        required: true,
    }],
});

/// `board.set_hidden`.
pub static SET_HIDDEN: FnCommand<BoardCanvas> = FnCommand::<BoardCanvas>::new(
    "board.set_hidden",
    CommandMeta::new(
        "Hide or show",
        "Hide the selected nodes (`hidden` true: they stay on the board but aren't drawn or clicked), or show them again (false; with no hidden node selected, every hidden node).",
    )
    .category("Arrange")
    .synonyms(&["hide", "show", "visibility", "ausblenden"])
    .icon("eye-off"),
    |board, args| {
        let hidden = args.bool("hidden").unwrap_or(true);
        let mut nodes = targets(board, takes_any);
        let changed: Vec<NodeId> = {
            let view = board.document().view();
            // A hidden node leaves the selection, so showing again works
            // on the board's hidden nodes when none is selected.
            if !hidden && !nodes.iter().any(|node| view.node(*node).is_some_and(NodeView::hidden)) {
                nodes = view
                    .walk()
                    .into_iter()
                    .filter(|node| view.node(*node).is_some_and(NodeView::hidden))
                    .filter(|node| !crate::hit::locked(view, *node))
                    .collect();
            }
            nodes
                .into_iter()
                .filter(|node| view.node(*node).is_some_and(|node| node.hidden() != hidden))
                .collect()
        };
        if changed.is_empty() {
            return Ok(Outcome::Unchanged);
        }
        board
            .transact(&Actor::User, |txn| {
                for node in &changed {
                    let value = if hidden { Value::Bool(true) } else { Value::Null };
                    txn.set_prop(*node, "hidden", value)?;
                }
                Ok(())
            })
            .map_err(failed("board.set_hidden"))?;
        Ok(Outcome::Done)
    },
)
.availability(|board| {
    let view = board.document().view();
    let any_hidden = || view.walk().into_iter().any(|node| view.node(node).is_some_and(NodeView::hidden));
    if !targets(board, takes_any).is_empty() || any_hidden() {
        Availability::Enabled
    } else {
        Availability::Hidden
    }
})
.args(ArgsSchema {
    args: &[ArgSpec {
        name: "hidden",
        description: "True to hide, false to show.",
        kind: ArgKind::Bool,
        required: true,
    }],
});

const PRESETS: &[&str] = &["rect", "ellipse", "diamond", "triangle", "line"];

/// `shape.set_preset`.
pub static SET_PRESET: FnCommand<BoardCanvas> = FnCommand::<BoardCanvas>::new(
    "shape.set_preset",
    CommandMeta::new(
        "Change shape",
        "Turn the selected shapes into another shape (rect, ellipse, diamond, triangle or line), keeping their box, text and look.",
    )
    .category("Style")
    .synonyms(&["shape type", "convert shape", "form ändern"])
    .icon("diamond"),
    |board, args| {
        let preset = args
            .str("preset")
            .and_then(crate::board::Preset::parse)
            .ok_or_else(|| CommandError::InvalidArgs {
                id: "shape.set_preset",
                reason: "`preset` names no shape".into(),
            })?;
        let nodes = targets(board, takes_preset);
        let changed: Vec<NodeId> = {
            let view = board.document().view();
            nodes
                .into_iter()
                .filter(|node| {
                    view.node(*node)
                        .is_some_and(|node| preset_of(node) != preset.name())
                })
                .collect()
        };
        if changed.is_empty() {
            return Ok(Outcome::Unchanged);
        }
        board
            .transact(&Actor::User, |txn| {
                for node in &changed {
                    txn.set_prop(*node, "shape", Value::String(preset.name().into()))?;
                }
                Ok(())
            })
            .map_err(failed("shape.set_preset"))?;
        Ok(Outcome::Done)
    },
)
.availability(|board| available(board, takes_preset))
.args(ArgsSchema {
    args: &[ArgSpec {
        name: "preset",
        description: "The shape: rect, ellipse, diamond, triangle or line.",
        kind: ArgKind::Enum { variants: PRESETS },
        required: true,
    }],
});

/// A shape's preset name (a rectangle without one).
pub fn preset_of(node: &NodeView) -> String {
    preset(node).unwrap_or("rect").to_owned()
}

/// The inspector's commands, for [`board_commands`](crate::commands::board_commands).
pub(crate) const INSPECTOR_COMMANDS: [&FnCommand<BoardCanvas>; 7] = [
    &SET_OPACITY,
    &SET_TEXT_SIZE,
    &RESET_STYLE,
    &APPLY_STYLE,
    &RENAME,
    &SET_HIDDEN,
    &SET_PRESET,
];

/// The style commands, for [`board_commands`](crate::commands::board_commands).
pub(crate) const STYLE_COMMANDS: [&FnCommand<BoardCanvas>; 5] = [
    &SET_FILL,
    &SET_STROKE,
    &SET_STROKE_WIDTH,
    &SET_SELECTION_RADIUS,
    &SET_TEXT_COLOR,
];
