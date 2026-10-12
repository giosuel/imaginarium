# 0010. Native `.imag` format: encrypted object pack

- Status: Accepted
- Date: 2026-10-12
- Supersedes: native-format part of 0005

## Decision
`.imag` is an append-only pack of typed, immutable, individually compressed
and encrypted **objects** (snapshot, update, blob, keyring, …) with an index
and trailer at the end of the file. The sync server stores the same objects,
byte for byte. Full design: `docs/design/file-format.md`. The normative spec
and test vectors follow once the design is accepted.

## Why
It is small (binary, zstd, CRDT columnar encoding, deduplicated media), fast
(lazy loading, incremental saves) and end-to-end encrypted by construction.
It is extensible through PNG-style critical and safe-to-copy flags and
namespaced keys, and it is open: protobuf plus a documented byte layout.
