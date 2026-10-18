---
title: The app's shell
description: Windows, commands, settings, files and the look, in imaginarium-shell and imaginarium-io-native.
---

`imaginarium-shell` is the app around the board: its windows, their panels
and commands, the settings, files and the look. `imaginarium-app` only
wires it together (`main.rs`). The design notes are
[Shell: the window around the board](/developers/reference/design/shell/),
with an "As built" note under each part.

## Windows

The toolkit runs several windows in one event loop. A **view** asks for
windows (`View::take_requests`: `WindowRequest::Open`, `Close`,
`SetTitle`) and hears the close button (`View::close_requested`, which
may say no: the close dialog). The run ends with the last window.

- `App` holds what the windows share: the fonts (scanned once), the
  settings, the tile workers, the file dialogs, the recent boards, the
  recovery folder. Windows hold it in an `Rc`.
- `BoardWindow` is a board's window (`App::new_board`, `open_board`,
  `board_window`), `StartWindow` the start window, `SettingsWindow`
  Settings (one at a time).
- Work that may wait (a dialog, opening or moving a file) runs on a thread
  of its own and wakes the window that asked (`imaginarium_ui::window_waker`;
  `files::Pending` in the shell). The window takes the result at its next
  frame. The UI thread never waits for the disk (MAINTAINERS invariant 3).

## Commands

Every feature is a command (invariant 5). The board's are
`imaginarium_canvas::commands`; the shell's (`commands.rs`) run on a
`ShellCx`, which only collects what the window should do (`Ask`s) and says
what applies now (`FileState`, `busy`), so the palette, the menus and the
AI see the same availability. Shortcuts come from keymaps: the board's
(`keys.rs`) and the shell's (`shell_keymap`). Menus are built from command
ids (`menu::MAIN`, `SELECTION`, …) with `menu::entries`.

## Settings

Declare a setting next to the code that uses it (a `static Setting<T>`,
with a title and description), add it to the crate's `SETTINGS` list, and
the Settings window shows it with an editor chosen by its kind: a switch,
a segmented control (enums of three or fewer), a dropdown (more, or a
small range with a step), a field. `.per_document()` lets a board have
its own value ([ADR-0022](/developers/reference/adr/0022-settings-a-document-can-override/)),
stored in its file. The section comes from the key's first part (`ui.` is
Appearance, `board.` Board, `files.` Files). Subscribe to changes with
`Registry::subscribe`, and apply them at the next frame where a change
could reenter what is running.

## Files

`imaginarium-io-native` turns `.imag` files into documents and back.

- `Session::open(path)` opens a file to edit it, locked against other
  writers (`flock` on Unix, a `.~name.lock` beside it on Windows), or
  read-only, with the reason in the document's mode: open elsewhere
  (`IN_USE`), not writable (`CANT_WRITE`), a newer format or records this
  version doesn't know.
- `Autosave::start(session, …)` saves on a thread of its own: the board
  window sends each change's updates (`Autosave::update`) once the board
  is still; the thread appends them (spec §9.3), writes a snapshot now and
  then, compacts when most of the file is superseded, and retries a
  failed write. `Autosave::state` is what the pill shows.
- `Autosave::create(path, …)` keeps an untitled board in a file of its own
  for crash recovery, made with its first change; `discard_in_background`
  deletes it when the board closes on purpose. `kept_boards(folder)` finds
  the ones a crashed app left.
- The dialogs are `files::Dialogs` (`SystemDialogs` is `rfd`'s); tests set
  a stand-in with `App::set_dialogs`.

## The look

`imaginarium_ui::appearance` holds the user's choice (`set_appearance`:
the theme, the accent, reduced motion) and follows the system where it
says so: the windows' theme on Windows and macOS, the desktop portal's
`color-scheme` on Linux. `look::follow` applies the settings to it and
keeps it current. `imaginarium_ui::set_interface_scale` sets the interface
size: each window's logical pixels grow by it, and the board divides its
zoom by it, so the board keeps its size.

## Testing it

The shell's tests drive windows without an event loop: they build a
window from `App`, call `draw`, `input`, `wake` and the accessibility
tree, and find controls by their screen-reader names
(`Ui::find_by_label`). Files go to the workspace's `target/test-scratch`,
never `/tmp`. To try the real app on a machine without a desktop, run it
under Xvfb and drive it with `xdotool`.
