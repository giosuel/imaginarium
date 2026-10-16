# 0020. The open document lives on the UI thread

- Status: Proposed
- Date: 2026-10-16

## Context

M3 makes the board editable. Every edit is a command, which runs a
`Document::transact` (invariant 2). The model is used from one thread at a
time, and its `DocumentView` (persistent `imbl` maps) can be read from any
thread. Two places are possible for the `Document`:

1. **The UI thread**, with the slow work (opening, saving, import, export,
   big pastes, sync) on workers.
2. **A model thread** of its own, which the UI sends commands to and gets
   views and `ChangeSet`s back from.

What edits cost (M1 benchmarks, this VM): a drag step on a 10,000-node
board 29 µs, a keystroke in a 300-page document 59 µs, both with the view
update and the `ChangeSet`. A frame has 8 ms at 120 Hz. The canvas already
paints tile scenes on the UI thread and rasterizes them on workers
(ADR-0019). Opening a 10,000-node board takes 339 ms (M1: Loro import and
building the view), and saving writes, compresses and maybe encrypts
records. A `Document` is `Send` (checked 2026-10-16), so it can be built on
one thread and used on another.

## Decision

1. **The open document lives on the UI thread.** Commands run their
   transaction there and return; the `ChangeSet` updates the canvas and the
   panels in the same frame. Undo groups (a drag, a typing burst) are open
   and closed there.
2. **Slow work runs elsewhere and hands results back:**
   - **opening** builds the `Document` (decrypt, import, view) on a worker
     and moves it to the window when it's ready; the window shows the
     board's thumbnail until then;
   - **saving** (`UPDT` records, M3.6) takes the update bytes since the
     last write (`export_updates_since`, cheap) on the UI thread, and
     compresses, encrypts and writes them on a file worker;
   - **imports and big pastes** are parsed and converted on a worker into
     a list of node edits, which one transaction on the UI thread applies;
   - **remote updates** (M8) are decrypted on the sync worker; the UI
     thread applies them in batches that fit a frame (`apply_remote`).
3. **Budgets** (PLAN §10): a command's transaction stays under 1 ms on a
   10,000-node board. M3.2's benchmarks measure it on the real canvas
   (drag latency, a `ChangeSet` to tiles); if it doesn't hold, we revisit.

## Consequences

- No message passing, no copies of the view, no second source of truth:
  a command reads the document it changes, the palette's availability
  checks read the same view synchronously, and undo is immediate.
- Anything that can take long must stay off the transaction: no IO, no
  compression, no image decoding, no AI call inside `transact`. Reviews
  check this; the profiling spans (`tracing`) show slow commands.
- A burst of remote changes could take a frame; batching keeps frames
  short at the cost of collaborators' changes arriving a frame later.
- Views are cheap to clone, so workers (export, thumbnails, search, AI
  context) read a snapshot without blocking the UI.

## Alternatives considered

- **A model thread.** Keeps the UI thread free of model work, but every
  command becomes a round trip (a frame of latency or a blocking wait),
  availability checks need a copy of the view, and two threads share the
  undo state. The measurements show the model work is far below a frame,
  so the cost buys nothing yet.
- **A document behind a lock**, shared by threads. Lock waits on the UI
  thread are what invariant 3 forbids, and the CRDT's ordering would
  depend on who takes the lock first.
