# 0022. Settings a document can override

- Status: Accepted (owner, 2026-10-17)
- Date: 2026-10-17

## Context

ADR-0014 gives every setting one **scope**: app, user, workspace,
document or hub. The shell design (`shell.md` §5) has settings with two at
once: `board.background` is "user and board". It is the user's choice for
every board, and one board can still have its own (the Settings mockup's
**For me** and **This board** tabs). A setting with `Scope::Document`
would only exist per document, with no value of the user's to fall back
on.

## Decision

1. **A setting can be declared `.per_document()`** (`SettingMeta::
   per_document`): its value is the user's (or the app's, by its scope),
   and a document may have **its own value**, which wins in that document.
   Lists and secrets can't be (registration refuses them).
2. **The document's own values are in `meta.settings`** (schema-v1.md §4),
   a mergeable map of setting key → plain value in the setting's stored
   form. An additive schema change: readers keep entries they don't know
   and ignore values they can't use, falling back to their own.
3. **Changing a document's value is a command** of the document's editor
   (`board.set_setting` for boards), so it is one undo step, syncs with
   the document, and goes through the one mutation path (MAINTAINERS.md
   invariant 2). The settings registry doesn't store document values: the
   editor reads them from its document.
4. **`Scope::Document` stays** for settings that exist only per document
   (none yet).

## Consequences

- Settings shows For me and This board where a section has such settings
  and a document window is open; This board edits the window used last.
- Each editor lists the settings it takes (`BOARD_SETTINGS`); a test
  keeps that list equal to the registry's per-document settings.
- A document can only carry values its readers understand to the same
  degree as any other data: an older reader keeps and ignores them.
