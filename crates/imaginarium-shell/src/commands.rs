// SPDX-License-Identifier: AGPL-3.0-or-later

//! The shell's own commands: what the window does that isn't the board's
//! (MAINTAINERS.md invariant 5). They run on a [`ShellCx`], which only
//! collects what the window should do.

use imaginarium_commands::{
    ArgKind, ArgSpec, ArgsSchema, CommandError, CommandMeta, FnCommand, Outcome,
};

use crate::toolbar::Dock;
use crate::window::Ask;

/// What a shell command asked the window to do.
#[derive(Default)]
pub(crate) struct ShellCx {
    pub(crate) asks: Vec<Ask>,
}

const EDGES: &[&str] = &["left", "top", "right", "bottom", "floating"];

/// `view.dock_toolbar`.
pub(crate) static DOCK_TOOLBAR: FnCommand<ShellCx> = FnCommand::<ShellCx>::new(
    "view.dock_toolbar",
    CommandMeta::new(
        "Move the tool bar",
        "Dock the tool bar to the left, top, right or bottom edge of the window, or let it float where it was last dropped.",
    )
    .category("View")
    .synonyms(&["tool bar position", "toolbar", "tools", "dock", "move tools"])
    .icon("grip-vertical"),
    |cx, args| {
        let edge = match args.str("edge") {
            Some("left") => Dock::Left,
            Some("top") => Dock::Top,
            Some("right") => Dock::Right,
            Some("bottom") => Dock::Bottom,
            Some("floating") => Dock::Floating,
            _ => {
                return Err(CommandError::InvalidArgs {
                    id: "view.dock_toolbar",
                    reason: "`edge` names no edge".into(),
                });
            }
        };
        cx.asks.push(Ask::Dock(edge));
        Ok(Outcome::Done)
    },
)
.args(ArgsSchema {
    args: &[ArgSpec {
        name: "edge",
        description: "Where the tool bar goes: left, top, right, bottom or floating.",
        kind: ArgKind::Enum { variants: EDGES },
        required: true,
    }],
});

/// `view.reset_toolbar`.
pub(crate) static RESET_TOOLBAR: FnCommand<ShellCx> = FnCommand::<ShellCx>::new(
    "view.reset_toolbar",
    CommandMeta::new(
        "Put the tool bar back",
        "Dock the tool bar to the left edge, where it starts (a double-click on its grip does the same).",
    )
    .category("View")
    .synonyms(&["reset tool bar", "toolbar left", "restore tools"])
    .icon("grip-vertical"),
    |cx, _args| {
        cx.asks.push(Ask::Dock(Dock::Left));
        Ok(Outcome::Done)
    },
);

/// The shell's commands, for the window's registry and the palette.
pub(crate) fn shell_commands() -> &'static [&'static FnCommand<ShellCx>] {
    &SHELL_COMMANDS
}

static SHELL_COMMANDS: [&FnCommand<ShellCx>; 2] = [&DOCK_TOOLBAR, &RESET_TOOLBAR];
