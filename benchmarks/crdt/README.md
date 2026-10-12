# CRDT benchmark (M1.5)

Compares **Loro**, **Yrs** and **Automerge** on Imaginarium's workloads,
to confirm or supersede [ADR-0009](../../docs/adr/0009-crdt-document-model.md).
The results and the decision are in
[`docs/design/crdt-benchmark.md`](../../docs/design/crdt-benchmark.md).

This is a separate Cargo workspace, so the product build never compiles
three CRDT libraries. CI doesn't build it.

```sh
cd benchmarks/crdt
cargo run --release -- --quick                 # small fixtures, a few seconds
cargo run --release -- --out results.md        # full run (the published numbers)
cargo run --release -- --engine loro --runs 3  # one engine only
```

## What it measures

The fixtures are the canonical ones from `imaginarium-test-utils`
(`BoardFixture::LARGE`, 10,000 shapes; `DocumentFixture::LARGE`, 300
pages), so the numbers are comparable with the PLAN §10 budgets.

| Workload | What it stands for |
|---|---|
| Board: open and save | Build 10,000 shapes in one transaction, save, load, and read every node's geometry (what building the view model does on open). Memory is counted by the global allocator. |
| Board: live edits | 1,000 drag steps, one transaction each, exported as an update and applied on a second peer. The remote apply has to fit the budget "apply a remote update < 1 ms including decrypt + verify". |
| Board: long session | 10,000 drags, then save and reopen, with and without history. |
| Board: concurrent | Two peers move overlapping shapes offline and add shapes, then merge. Then two peers move node A into B and B into A at the same time: does the hierarchy stay a tree? |
| Document: open and save | A 300-page text flow (~800k characters) with heading and list marks. |
| Document: typing | 2,000 keystrokes, one transaction each, sent to a second peer. |
| Document: concurrent typing | Two peers type at the same position offline. After the merge, each run must stay contiguous. |
| Features | Movable tree, history trimming, and a functional check of collaboration-aware undo. |

## Mapping the schema

Each engine maps the schema of `docs/design/file-format.md` §3.2 as closely
as it can:

- **Loro**: the `nodes` movable tree, with fractional indexes for z-order;
  node properties in the tree node's meta map.
- **Yrs** and **Automerge** have no tree type. They store what we would have
  to build ourselves: a `nodes` map whose node maps carry a parent id (`p`)
  and an order key (`z`). That layer can't prevent cycles from concurrent
  moves. The benchmark demonstrates this.
- Text: one rich text with `h` (heading level) and `li` marks. Yrs uses the
  v2 update encoding, its more compact one.
- Every edit is its own transaction (Loro `commit`, a Yrs transaction,
  an Automerge change), as in the app, where one command is one transaction.

## Dependencies

Versions are pinned exactly and follow the two-week rule (MAINTAINERS.md),
including the transitive ones in this workspace's `Cargo.lock`.
