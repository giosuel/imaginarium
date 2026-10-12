# `.imag` test vectors

Reproducible files for checking an implementation against
[`../spec-v1.md`](../spec-v1.md) (§13). All of them use the document id
`6d696e69-6d61-6c40-8976-6563746f7273`, and object ids made of a single
repeated byte (`0x01…01`, `0x02…02`, …), so they are easy to read in a hex
dump. Contents are opaque placeholder bytes, because these vectors test
the container, not the CRDT codec.

The reference implementation's test
(`crates/imaginarium-format/tests/vectors.rs`) checks that it writes these
files **byte for byte** and reads them with the results below.

| File | Content | Expected when reading |
|---|---|---|
| `01-minimal.imag` | Header, `META` (`DocumentMeta`: format 1.0, `blob_id_key` = 32 × `0xB1`), `SNAP` (stream main, codec `loro-1`), `INDX`, trailer | No recovery. Entries: `META` at offset 36, `SNAP`. Snapshot content `opaque snapshot bytes`. |
| `02-two-commits.imag` | Vector 1, then one incremental save: `UPDT` `0x03…`, new `INDX`, new trailer | Its first 427 bytes equal vector 1 (append-only). Three entries. Main head = `SNAP 0x02…` + update `0x03…`. |
| `03a-torn-append.imag` | Vector 2, plus a complete uncommitted `UPDT 0x04…` and the first half of `UPDT 0x05…` | Recovery `PreviousCommit` with 1 adopted record. Main head has updates `0x03…`, `0x04…`. |
| `03b-corrupt-trailer.imag` | Vector 1 with one flipped byte in the trailer's CRC | Recovery `ForwardScan` with 2 records. |
| `03c-no-index.imag` | Header and three records (`META`, `SNAP`, `UPDT`), no index or trailer | Recovery `ForwardScan` with 3 records. |
| `04-unknown-records.imag` | `META`, extension `0x4001` (`SAFE_TO_COPY`), extension `0x4002` (no flags) | Compaction keeps `0x4001` byte-for-byte, drops `0x4002`, and reports it. |

## Layout of `01-minimal.imag` (427 bytes)

| Offset | Length | Content |
|---|---|---|
| 0 | 36 | Header: magic `89 49 4D 47 0D 0A 1A 0A`, major `01 00`, minor `00 00`, flags `00 00 00 00`, doc id, 4 reserved zero bytes |
| 36 | 116 | `META` record: rtype `01`, rflags `03` (CRITICAL, SAFE_TO_COPY), hlen 36, header, plen 72, payload, CRC |
| 152 | 75 | `SNAP` record: rtype `03`, rflags `01` (CRITICAL), hlen 46, header, plen 21, payload, CRC |
| 227 | 176 | `INDX` record: rtype varint `FF 7F` (0x3FFF), rflags `01`, index payload |
| 403 | 24 | Trailer: index offset 227, length 176, CRC, `IMGEND\0\0` |

## Still to come

Vectors 5–10 (encrypted records, segmented blobs, keyrings, signatures, ACL
chain) are added with `imaginarium-crypto` in M1.4.

## Regenerating

Only when the spec changes deliberately:

```sh
IMAGINARIUM_UPDATE_VECTORS=1 cargo test -p imaginarium-format --test vectors
```

Commit the changed files together with the spec change that caused them.
