# 0004. Persistent document model with operation log

- Status: Accepted
- Date: 2026-10-12

## Context
We need cheap undo, background readers (layout, export, autosave) without
locks, fine-grained change tracking for incremental re-render, and a path to
collaboration later.

## Decision
- Store nodes in an arena keyed by generational `NodeId`. Node and child
  collections use persistent data structures (`imbl`), so taking a snapshot
  is O(1) and versions share structure.
- All mutation goes through small, serializable, **invertible `Op`s** that are
  grouped into transactions. Applying a transaction returns a `ChangeSet`
  (the dirty node ids and the kinds of change).
- Undo and redo apply inverse transactions.
- The op log is persisted for crash recovery and is designed so that a CRDT
  (e.g. Loro or yrs) could later be layered on.

## Consequences
- No ad-hoc `&mut` mutation of the model anywhere. This is enforced by API
  visibility.
- Property tests must guarantee that applying an op and then its inverse
  returns the original model (`apply(op); apply(inverse(op)) == id`).
