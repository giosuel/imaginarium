// SPDX-License-Identifier: AGPL-3.0-or-later

//! The shell's own commands: what the window does that isn't the board's
//! (MAINTAINERS.md invariant 5). They run on a [`ShellCx`], which only
//! collects what the window should do.

use imaginarium_commands::{
    ArgKind, ArgSpec, ArgsSchema, Availability, CommandError, CommandMeta, FnCommand, Outcome,
};

use crate::toolbar::Dock;
use crate::window::Ask;

/// What a shell command asked the window to do.
#[derive(Default)]
pub(crate) struct ShellCx {
    pub(crate) asks: Vec<Ask>,
    /// The window is one of the app's (it can open and close windows).
    pub(crate) in_app: bool,
}

/// Window commands need the app's windows (not a window in a test).
fn in_app(cx: &ShellCx) -> Availability {
    if cx.in_app {
        Availability::Enabled
    } else {
        Availability::Hidden
    }
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

/// `palette.open`.
pub(crate) static OPEN_PALETTE: FnCommand<ShellCx> = FnCommand::<ShellCx>::new(
    "palette.open",
    CommandMeta::new(
        "Search commands and settings",
        "Open the command palette, which finds every command and setting by name.",
    )
    .category("View")
    .synonyms(&["palette", "search", "find command", "command"])
    .icon("search"),
    |cx, _args| {
        cx.asks.push(Ask::OpenPalette);
        Ok(Outcome::Done)
    },
);

/// `view.inspector`.
pub(crate) static TOGGLE_INSPECTOR: FnCommand<ShellCx> = FnCommand::<ShellCx>::new(
    "view.inspector",
    CommandMeta::new(
        "Show or hide the inspector",
        "Open the inspector, the panel on the right with everything about the selection (name, position and size, colors, layer, style), or close it.",
    )
    .category("View")
    .synonyms(&["properties", "inspector", "details", "eigenschaften"])
    .icon("sliders-horizontal"),
    |cx, _args| {
        cx.asks.push(Ask::ToggleInspector);
        Ok(Outcome::Done)
    },
);

/// `view.context_menu`.
pub(crate) static CONTEXT_MENU: FnCommand<ShellCx> = FnCommand::<ShellCx>::new(
    "view.context_menu",
    CommandMeta::new(
        "Open the context menu",
        "Open the menu of the selection's commands (or the board's) at the selection, as a right-click there does.",
    )
    .category("View")
    .synonyms(&["right click", "menu", "kontextmenü"])
    .icon("ellipsis"),
    |cx, _args| {
        cx.asks.push(Ask::ContextMenu);
        Ok(Outcome::Done)
    },
);

/// `view.next_area`.
pub(crate) static NEXT_AREA: FnCommand<ShellCx> = FnCommand::<ShellCx>::new(
    "view.next_area",
    CommandMeta::new(
        "Next area",
        "Move the keyboard to the next area of the window: the board, the selection's toolbar, the tool bar, the panels.",
    )
    .category("View")
    .synonyms(&["focus", "f6", "panels"])
    .icon("chevron-right"),
    |cx, _args| {
        cx.asks.push(Ask::NextArea(true));
        Ok(Outcome::Done)
    },
);

/// `view.previous_area`.
pub(crate) static PREVIOUS_AREA: FnCommand<ShellCx> = FnCommand::<ShellCx>::new(
    "view.previous_area",
    CommandMeta::new(
        "Previous area",
        "Move the keyboard to the previous area of the window (the reverse of Next area).",
    )
    .category("View")
    .synonyms(&["focus back", "shift f6"])
    .icon("chevron-left"),
    |cx, _args| {
        cx.asks.push(Ask::NextArea(false));
        Ok(Outcome::Done)
    },
);

/// `file.new_board`.
pub(crate) static NEW_BOARD: FnCommand<ShellCx> = FnCommand::<ShellCx>::new(
    "file.new_board",
    CommandMeta::new(
        "New board",
        "Open a new, empty board in a window of its own.",
    )
    .category("File")
    .synonyms(&["new", "new window", "blank board", "create board"])
    .icon("plus"),
    |cx, _args| {
        cx.asks.push(Ask::NewBoard);
        Ok(Outcome::Done)
    },
)
.availability(in_app);

/// `window.close`.
pub(crate) static CLOSE_WINDOW: FnCommand<ShellCx> = FnCommand::<ShellCx>::new(
    "window.close",
    CommandMeta::new(
        "Close window",
        "Close this board's window. When it is the last board open, the start window opens.",
    )
    .category("File")
    .synonyms(&["close", "close board"])
    .icon("x"),
    |cx, _args| {
        cx.asks.push(Ask::CloseWindow);
        Ok(Outcome::Done)
    },
)
.availability(in_app);

/// The shell's shortcuts (the board has its own keymap).
pub(crate) fn shell_keymap(
    platform: imaginarium_commands::Platform,
) -> imaginarium_commands::Keymap {
    let mut keymap = imaginarium_commands::Keymap::new(platform);
    // Fixed, valid bindings: this can't fail.
    let _ = keymap.bind(
        "Mod+K",
        imaginarium_commands::CommandId::new("palette.open"),
    );
    let _ = keymap.bind(
        "Shift+F10",
        imaginarium_commands::CommandId::new("view.context_menu"),
    );
    let _ = keymap.bind(
        "Mod+I",
        imaginarium_commands::CommandId::new("view.inspector"),
    );
    let _ = keymap.bind(
        "Mod+N",
        imaginarium_commands::CommandId::new("file.new_board"),
    );
    let _ = keymap.bind(
        "Mod+W",
        imaginarium_commands::CommandId::new("window.close"),
    );
    let _ = keymap.bind("F6", imaginarium_commands::CommandId::new("view.next_area"));
    let _ = keymap.bind(
        "Shift+F6",
        imaginarium_commands::CommandId::new("view.previous_area"),
    );
    keymap
}

/// The shell's commands, for the window's registry and the palette.
pub(crate) fn shell_commands() -> &'static [&'static FnCommand<ShellCx>] {
    &SHELL_COMMANDS
}

static SHELL_COMMANDS: [&FnCommand<ShellCx>; 9] = [
    &NEW_BOARD,
    &CLOSE_WINDOW,
    &TOGGLE_INSPECTOR,
    &CONTEXT_MENU,
    &NEXT_AREA,
    &PREVIOUS_AREA,
    &OPEN_PALETTE,
    &DOCK_TOOLBAR,
    &RESET_TOOLBAR,
];
