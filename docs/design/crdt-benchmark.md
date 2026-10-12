# CRDT benchmark: Loro vs Yrs vs Automerge (M1.5)

- Status: **Accepted** (2026-10-12). It confirms
  [ADR-0009](../adr/0009-crdt-document-model.md) (Loro).
- Code: [`benchmarks/crdt/`](../../benchmarks/crdt/README.md), a separate
  Cargo workspace. Rerun it with `cargo run --release` there.
- Libraries: Loro 1.16.2, Yrs 0.28.0 (Yjs port), Automerge 0.12.0. These
  are the newest versions the two-week rule allows.

## 1. Question

ADR-0009 chose Loro because it is the only library with a **movable tree**
and **history trimming** (`docs/design/m1-dependencies.md` §3). Before
building the model on it (M1.6), this benchmark checks that the choice
also holds on **performance and memory** for our workloads, and verifies
the features by running them rather than reading the API.

## 2. Method

Each library maps our schema (`file-format.md` §3.2) as closely as it can:

- **Loro**: the `nodes` **movable tree** (fractional indexes on, for
  z-order), node properties in each tree node's meta map, and a rich
  text with `h` (heading level) and `li` marks.
- **Yrs** and **Automerge** have no tree type. They store what we would have
  to build on top of them: a `nodes` map whose node maps carry a parent id
  and an order key.
- Every edit is **one transaction** (Loro `commit`, a Yrs transaction, an
  Automerge change), as in the app, where one command is one transaction.
  Each library sends live edits its idiomatic way:
  - Loro: `export(updates(version before the edit))`;
  - Yrs: the transaction's own update (`encode_update_v2`);
  - Automerge: `save_after(heads)`.

  Yrs uses its more compact v2 encoding.

Workloads, using the canonical fixtures of `imaginarium-test-utils` (the
same ones the PLAN §10 budgets refer to):

| Workload | Content |
|---|---|
| Board | `BoardFixture::LARGE`: 10,000 shapes (rectangles, ellipses, lines, text boxes), each with kind, geometry map, fill and, for text boxes, a text container |
| Document | `DocumentFixture::LARGE`: 300 pages, 2,216 paragraphs, ~911k characters, with heading and list marks |
| Live edits | 1,000 drag steps (board) and 2,000 keystrokes (document, the cursor jumps every 100 keys), each sent to a second peer |
| Long session | 10,000 drag steps, then save and reopen, with and without history |
| Concurrency | Two peers edit offline (500 overlapping moves plus 100 new shapes; 500 keystrokes each at the same spot), then merge both ways |
| Correctness | Convergence of every pair of peers. Concurrent moves of node A into B and B into A. No interleaving of concurrent typing. A functional check of collaboration-aware undo. |

**Measurement:**
- Times are wall-clock medians of 5 runs for whole-document operations. Per-edit rows show the median and p99 over every edit of one run.
- Memory is counted by the global allocator (`peak_alloc`). *Retained* is what the open document holds; *peak* is the high-water mark while opening it.
- Sizes are raw, and compressed with zstd level 3, which is what `.imag` records use.
- Machine: the dev VM (Haswell, 8 vCPUs, shared). Single-thread speed is about half of a current laptop's (STATUS, M1.4), so absolute times are pessimistic, and the comparison between the libraries is what counts.

## 3. Results

The complete output of one run (`cargo run --release` in
`benchmarks/crdt`, 14 min, almost all of it Automerge's typing phase). The
load average on the shared VM was 2.8 at the start and 7.9 at the end.

### Features

| Metric | Loro 1.16.2 | Yrs 0.28.0 | Automerge 0.12.0 |
|---|---|---|---|
| Movable tree (no cycles from concurrent moves) | yes | **no** | **no** |
| History trimming | shallow snapshot (`ExportMode::ShallowSnapshot`) | none (deleted content is garbage-collected, history of live items stays) | none (the complete history is always kept) |
| Collaboration-aware undo (undoes only the local change) | yes | yes | not available |

### Board: open and save

| Metric | Loro 1.16.2 | Yrs 0.28.0 | Automerge 0.12.0 |
|---|---|---|---|
| Build 10000 shapes in one transaction | 163.58 ms | 87.57 ms | 4.64 s |
| Memory after building | 36.0 MiB | 35.3 MiB | 4460.4 KiB |
| Save snapshot | 2.34 ms | 12.67 ms | 43.10 ms |
| Snapshot size (raw / zstd) | 1983.4 KiB / 1514.9 KiB | 1170.6 KiB / 506.1 KiB | 532.0 KiB / 518.1 KiB |
| Load snapshot | 87.80 ms | 124.48 ms | 285.33 ms |
| Load snapshot and read every geometry | 105.83 ms | 131.00 ms | 498.03 ms |
| Memory after opening (retained / peak) | 13.9 MiB / 15.6 MiB | 41.9 MiB / 44.2 MiB | 4330.3 KiB / 20.5 MiB |

### Board: live edits

| Metric | Loro 1.16.2 | Yrs 0.28.0 | Automerge 0.12.0 |
|---|---|---|---|
| Local edit including its update (1000 drag steps) | 28.3 µs (p99 66.0 µs) | 7.7 µs (p99 22.2 µs) | 166.4 µs (p99 258.1 µs) |
| … of which the edit | 16.7 µs (p99 42.8 µs) | 7.7 µs (p99 22.2 µs) | 139.0 µs (p99 219.1 µs) |
| … of which encoding the update | 11.5 µs (p99 32.1 µs) | 0.0 µs (p99 0.1 µs) | 25.7 µs (p99 56.1 µs) |
| Update size (median / p99) | 107 B / 107 B | 51 B / 52 B | 123 B / 123 B |
| Apply a remote update | 23.9 µs (p99 54.5 µs) | 8.2 µs (p99 30.4 µs) | 190.8 µs (p99 279.9 µs) |
| Peers converged | yes | yes | yes |

### Board: after a long editing session

| Metric | Loro 1.16.2 | Yrs 0.28.0 | Automerge 0.12.0 |
|---|---|---|---|
| Snapshot size after 10000 drags (raw / zstd) | 2212.3 KiB / 1729.8 KiB | 1272.3 KiB / 563.6 KiB | 820.7 KiB / 802.7 KiB |
| Snapshot without history (raw / zstd) | 1228.7 KiB / 972.1 KiB | not supported | not supported |
| Load it and read every geometry | 119.36 ms | 190.09 ms | 678.23 ms |
| Memory after opening | 13.3 MiB | 47.9 MiB | 6784.8 KiB |

### Board: concurrent editing

| Metric | Loro 1.16.2 | Yrs 0.28.0 | Automerge 0.12.0 |
|---|---|---|---|
| Merge a peer's offline session (500 moves, 100 new shapes) | 56.69 ms (24.3 KiB) | 3.85 ms (19.2 KiB) | 73.14 ms (86.0 KiB) |
| Peers converged | yes | yes | yes |
| No cycle after concurrent moves (A into B, B into A) | yes | **no** | **no** |

### Document: open and save

| Metric | Loro 1.16.2 | Yrs 0.28.0 | Automerge 0.12.0 |
|---|---|---|---|
| Build 2216 paragraphs (911k chars) in one transaction | 12.87 ms | 262.92 ms | 1.54 s |
| Memory after building | 3289.6 KiB | 1674.9 KiB | 1443.8 KiB |
| Save snapshot | 4.33 ms | 2.30 ms | 49.28 ms |
| Snapshot size (raw / zstd) | 893.1 KiB / 630.4 KiB | 899.6 KiB / 223.8 KiB | 216.5 KiB / 216.5 KiB |
| Load snapshot | 1.69 ms | 3.00 ms | 674.15 ms |
| Load snapshot and read the text | 2.65 ms | 3.30 ms | 664.75 ms |
| Memory after opening (retained / peak) | 2746.6 KiB / 10049.0 KiB | 1204.3 KiB / 2558.5 KiB | 1058.8 KiB / 81.1 MiB |

### Document: typing

| Metric | Loro 1.16.2 | Yrs 0.28.0 | Automerge 0.12.0 |
|---|---|---|---|
| Local keystroke including its update (2000 keys) | 16.2 µs (p99 43.6 µs) | 34.4 µs (p99 94.2 µs) | 188.7 µs (p99 398.7 µs) |
| … of which the edit | 3.9 µs (p99 17.0 µs) | 34.4 µs (p99 93.8 µs) | 149.2 µs (p99 329.6 µs) |
| … of which encoding the update | 11.9 µs (p99 34.8 µs) | 0.0 µs (p99 0.1 µs) | 37.6 µs (p99 88.6 µs) |
| Update size (median / p99) | 91 B / 91 B | 29 B / 30 B | 98 B / 98 B |
| Apply a remote update | 18.4 µs (p99 55.4 µs) | 3.0 µs (p99 16.0 µs) | 377.24 ms (p99 542.09 ms) |
| Peers converged | yes | yes | yes |
| Snapshot growth per keystroke (raw / zstd) | 0.5 B / 0.3 B | 1.1 B / 0.2 B | 0.7 B / 0.7 B |
| Snapshot without history (raw / zstd) | 375.4 KiB / 299.0 KiB | not supported | not supported |

### Document: concurrent typing

| Metric | Loro 1.16.2 | Yrs 0.28.0 | Automerge 0.12.0 |
|---|---|---|---|
| Merge a peer's offline typing (500 keys) | 4.72 ms (606 B) | 17.5 µs (529 B) | 399.58 ms (52.1 KiB) |
| Peers converged | yes | yes | yes |
| Both runs stay contiguous (no interleaving) | yes | yes | yes |

## 4. Analysis

### 4.1 Hard requirements

| Requirement (format design §3) | Loro | Yrs | Automerge |
|---|---|---|---|
| Movable tree: concurrent moves never create cycles | ✅ | ❌ | ❌ |
| History trimming (ADR-0016, 90-day default) | ✅ shallow snapshot | ❌ | ❌ |
| Collaboration-aware undo | ✅ | ✅ | ❌ |
| Rich text with marks, no interleaving | ✅ | ✅ | ✅ |

The cycle check isn't theoretical. When peer A moves node 0 into node 1
while peer B moves node 1 into node 0, both Yrs and Automerge converge on a
state where each node is the other's parent. Both nodes are then detached
from the root, so they would disappear from the board on every peer.
Preventing that on Yrs would mean implementing a move CRDT ourselves, the
hard part that Loro already solves.

### 4.2 Budgets (PLAN §10)

| Budget | Loro | Yrs | Automerge |
|---|---|---|---|
| Apply a remote update < 1 ms, including decrypt + verify (~74 µs, M1.4) | ✅ board 24 µs, text 18 µs (p99 ~55 µs) | ✅ 3–8 µs | ✅ board 191 µs; ❌ **377 ms per keystroke** in the 300-page document |
| Open a 300-page document, first page < 500 ms | ✅ 2.7 ms for the CRDT | ✅ 3.3 ms | ❌ 665 ms for the CRDT alone |
| Input to pixels < 1 frame (8 ms) | ✅ edit + update 16–28 µs | ✅ 8–34 µs | ✅ 0.17–0.19 ms |
| Open a 10,000-shape board (no budget yet; for scale) | 106 ms | 131 ms | 498 ms |

### 4.3 Where Loro is weaker

- **Snapshots are about 3× larger** after zstd: 1.5 MiB versus 0.5 MiB for
  the board, and 630 KiB versus 220 KiB for the document. Loro compresses
  its snapshot blocks with LZ4 itself, so zstd only gains about 25 % on
  top. Without history (shallow snapshot), the long-session board shrinks
  from 1.7 MiB to 0.97 MiB.

  For comparison, a 300-page DOCX is a few hundred KiB, and real files are
  dominated by images, which are blobs outside the CRDT. Acceptable, but
  worth tracking.
- **Merging a large offline session** takes 57 ms, against 4 ms for Yrs
  (500 moves plus 100 new shapes). That happens on reconnect, never per
  keystroke, and it runs off the UI thread anyway (architecture invariant
  3).
- **Building 10,000 shapes in one transaction** takes 164 ms, against 88 ms
  for Yrs (a large import or paste).
- **Memory right after building** that board is 36 MiB, but only 14 MiB
  after reopening it. The difference is history caches.

Where Loro is ahead:
- loading (the document in 2.7 ms);
- building text (13 ms versus 263 ms for Yrs);
- local keystrokes in a large text (4 µs versus 34 µs);
- memory of an opened board (14 MiB versus 42 MiB for Yrs).

### 4.4 Yrs and Automerge

- **Yrs** is the fastest at per-edit operations and merges, and writes the
  smallest files. It stays the documented fallback, but it lacks the two
  hard requirements: a movable tree and history trimming.
- A measurement pitfall worth recording: Yrs's catch-up encoding
  (`encode_state_as_update`) repeats the document's whole delete set. On
  the board after 1,000 drags, that made every "update" about 2 KB and
  every remote apply about 0.5 ms. Live edits must use the transaction's
  own update, as this benchmark does.
- **Automerge** uses the least memory (4 MiB for the board). But its remote
  apply goes through a batch path that costs time proportional to the
  document size: 3.7 ms per remote keystroke at 9k characters (the
  `--quick` fixture), and 377 ms at 911k characters. Loading the
  document takes 665 ms, and building the board 4.6 s. It also lacks undo,
  a tree and trimming. It is ruled out.

## 5. Decision

**ADR-0009 is confirmed: Loro (1.16) is the document CRDT.** It is the
only library that meets the hard requirements, and it meets every budget
with a wide margin. Yrs remains the fallback; the `loro-1` codec id in the
format (§3.1) keeps a migration possible.

Follow-ups for M1.6 (`imaginarium-model`):

1. Apply remote updates on the worker pool, batched per frame. Show
   progress for large offline merges (tens of ms).
2. After large imports, measure whether `compact_change_store` or
   `free_history_cache` give back the extra ~20 MiB of history caches.
3. Use shallow snapshots for compaction under the ADR-0016 retention
   policy.
4. Track snapshot sizes on realistic documents. Keep zstd on `SNAP`
   records, since it still saves about 25 % on Loro snapshots.
5. Move this benchmark's Loro workloads into criterion benches of the model
   crate, so CI catches regressions against these numbers.
