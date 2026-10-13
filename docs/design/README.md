# Design documents

Detailed designs for subsystems. Each one states its status (Draft,
Accepted, Implemented). Once implemented, a design must be kept in sync
with the code, or point to the normative spec that replaces it.

| Design | Status | Milestone |
|---|---|---|
| [`file-format.md`](file-format.md): `.imag` container, objects, CRDT schema, encryption | Accepted | M1 |
| [`m1-dependencies.md`](m1-dependencies.md): crypto, protobuf and CRDT crate review for M1 | Accepted | M1 |
| [`crdt-benchmark.md`](crdt-benchmark.md): Loro vs Yrs vs Automerge on our workloads (confirms ADR-0009) | Accepted | M1.5 |
| [`sync-protocol.md`](sync-protocol.md): client ↔ Hub protocol, live frames, presence, snapshots/GC | Draft (normative in M8) | M1 → M8 |
| [`m2-ui-stack.md`](m2-ui-stack.md): UI and render crate versions, Masonry and Vello choices, how the M2 gate is measured | Accepted | M2.0 |
| [`m2-gate.md`](m2-gate.md): running the gate app, the manual checklist, the results | Draft (results in M2.7) | M2.3 |
| [`ui-toolkit.md`](ui-toolkit.md): shell, widget tree, Taffy layout, paint caching, input and focus, signals, accessibility | Draft (accepted with the M2 gate) | M2.4 |
| [`rendering.md`](rendering.md): display lists, tiles and levels, invalidation, compositor, backends | Implemented (accepted with the M2 gate) | M2.2, M2.5 |
| `text-engine.md`: rich text, editing, IME, pagination | To be written | M5 |
| `ai.md`: capabilities, providers, tool bridge, jobs, privacy | To be written | M6 |
| `hub.md`: accounts, OPAQUE, roles, storage, admin | To be written | M8 |
