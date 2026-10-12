# 0009. CRDT (Loro) as document source of truth + materialized view model

- Status: Accepted. **Confirmed by the M1.5 benchmark** (2026-10-12,
  [`docs/design/crdt-benchmark.md`](../design/crdt-benchmark.md)).
- Date: 2026-10-12
- Supersedes: 0004

## Context
Real-time multi-user editing through a server that cannot read content
(ADR-0011) requires clients to merge edits on their own, without a central
authority.

## Decision
- The persisted, synced document is a **Loro CRDT**: a movable tree for nodes,
  rich text for text flows, and maps for properties. The schema is in
  `docs/design/file-format.md` §3.
- The app renders from a **materialized view model**: persistent `imbl`
  structures that are updated incrementally from Loro diff events. This
  keeps the guarantees of ADR-0004 (O(1) snapshots, lock-free readers,
  `ChangeSet` → fine-grained signals).
- All edits are commands that produce one CRDT transaction. **Undo** uses
  Loro's collaboration-aware undo manager, so it undoes only your own changes.
- The AI agent and add-ons write through the same command/transaction path,
  each with its own peer id. Their edits can therefore be attributed and
  undone separately.
- Benchmark against Yrs and Automerge on our workloads in M1 before
  freezing this choice.

## Consequences
- No migration is needed when collaboration ships.
- Holding both the CRDT and the view model costs memory. This is measured in M1.
