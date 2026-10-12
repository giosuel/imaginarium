# `.imag` format specification

| File | Content | Status |
|---|---|---|
| [`spec-v1.md`](spec-v1.md) | Normative specification, version 1 | Draft 1 (normative from `v0.0.2-m1`) |
| [`proto/imag_v1.proto`](proto/imag_v1.proto) | Normative protobuf schema of all structured records | Draft 1 |
| `schema-v1.md` | Logical document schema on the CRDT (containers, keys, node kinds) | M1.6 |
| [`test-vectors/`](test-vectors/README.md) | Reproducible vectors: container vectors 1–4 (M1.3); encryption vectors follow in M1.4 | Partial |

The rationale behind the format is in
[`../design/file-format.md`](../design/file-format.md). Where they differ,
**the spec wins**. Changes to a published spec version are additive only
(spec §10).
